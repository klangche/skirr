//! Topology construction from sysfs raw devices.
//!
//! Linux exposes no explicit controller records in the USB device tree, so
//! each bus synthesizes a HostController + RootHub pair (same documented
//! deviation as macOS). PCI addresses are best-effort enrichment on Linux.

use crate::native::{self, RawDeviceInfo};
use skirr_core::{
    ConnectionStatus, HostController, PlatformInfo, RootHub, SystemTopology, UsbDevice, UsbSpeed,
};
use std::collections::HashMap;

fn empty_platform_info() -> PlatformInfo {
    PlatformInfo {
        os: "Linux".into(),
        os_version: "unknown".into(),
        kernel_version: None,
        architecture: std::env::consts::ARCH.into(),
        hostname: None,
        username: None,
        is_admin: false,
        is_virtual_machine: false,
        boot_time: None,
    }
}

/// Build the full topology. `pci_by_bus` maps bus numbers to PCI addresses
/// (injected so tests can exercise enrichment without /sys).
pub fn build(
    raws: &[RawDeviceInfo],
    pci_by_bus: &HashMap<u8, String>,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> SystemTopology {
    let mut topo = SystemTopology {
        timestamp,
        host_controllers: Vec::new(),
        root_hubs: Vec::new(),
        devices: Vec::new(),
        hubs: Vec::new(),
        displays: Vec::new(),
        events: Vec::new(),
        platform_info: empty_platform_info(),
    };

    // Controllers + root hubs, one pair per bus.
    let mut buses: Vec<u8> = raws.iter().map(|r| r.busnum).collect();
    buses.sort_unstable();
    buses.dedup();
    let mut root_hub_ids: HashMap<String, uuid::Uuid> = HashMap::new();
    for bus in &buses {
        let hc_id = uuid::Uuid::new_v4();
        let rh_id = uuid::Uuid::new_v4();
        let pci = pci_by_bus.get(bus);
        topo.host_controllers.push(HostController {
            id: hc_id,
            platform_id: pci.cloned().unwrap_or_else(|| format!("USB_BUS_{bus}")),
            name: format!("USB Bus {bus}"),
            vendor_id: None,
            device_id: None,
            revision: None,
            usb_version: UsbSpeed::Unknown,
            root_hub_ids: vec![rh_id],
            port_count: 0,
            is_xhci: false,
            pci_address: pci.cloned(),
            driver_version: None,
            capabilities: Default::default(),
        });
        topo.root_hubs.push(RootHub {
            id: rh_id,
            platform_id: format!("usb{bus}"),
            host_controller_id: hc_id,
            port_count: 0,
            hub_speed: UsbSpeed::Unknown,
            is_integrated: true,
            children_ids: Vec::new(),
        });
        root_hub_ids.insert(format!("usb{bus}"), rh_id);
    }

    // Real devices.
    for raw in raws.iter().filter(|r| !r.is_root_hub()) {
        let rh_id = root_hub_ids.get(&format!("usb{}", raw.busnum)).copied();
        let parent_sysfs = native::parent_name(&raw.name);
        let is_root_child = parent_sysfs
            .as_deref()
            .is_some_and(native::is_root_hub_name);

        let mut dev = build_usb_device(raw);
        if let Some(rh) = rh_id {
            dev.root_hub_id = Some(rh);
        }
        if is_root_child {
            dev.tier = 1;
            dev.hop_count = 0;
            if let Some(rh_id) = rh_id {
                if let Some(rh) = topo.root_hubs.iter_mut().find(|r| r.id == rh_id) {
                    rh.children_ids.push(dev.id);
                }
                // Direct attachments read their port from the first path
                // segment (`3-12` → port 12).
                dev.port_number = raw.name.split_once('-').and_then(|(_, p)| p.parse().ok());
            }
        } else {
            dev.hop_count = native::hop_count(&raw.name) as u8;
            dev.tier = dev.hop_count + 1;
            dev.port_number = native::immediate_port(&raw.name);
        }
        topo.devices.push(dev);
    }

    // Parent/child links between real devices.
    let id_by_name: HashMap<String, uuid::Uuid> = topo
        .devices
        .iter()
        .map(|d| (d.platform_id.clone(), d.id))
        .collect();
    for raw in raws.iter().filter(|r| !r.is_root_hub()) {
        let parent_name = match native::parent_name(&raw.name) {
            Some(p) => p,
            None => continue,
        };
        if native::is_root_hub_name(&parent_name) {
            continue;
        }
        // Parent's instance id needs its own VID/PID from its raw record.
        let Some(parent_raw) = raws.iter().find(|r| r.name == parent_name) else {
            continue;
        };
        if let (Some(child_id), Some(parent_id)) = (
            id_by_name.get(&raw.make_instance()),
            id_by_name.get(&parent_raw.make_instance()),
        ) {
            if let Some(child) = topo.devices.iter_mut().find(|d| d.id == *child_id) {
                child.parent_id = Some(*parent_id);
            }
            if let Some(parent_dev) = topo.devices.iter_mut().find(|d| d.id == *parent_id) {
                parent_dev.children_ids.push(*child_id);
            }
        }
    }

    // Hub aggregates: root-hub port counts come from maxchild of usbN.
    for rh in &mut topo.root_hubs {
        rh.port_count = raws
            .iter()
            .find(|r| r.name == rh.platform_id)
            .map(|r| r.maxchild)
            .unwrap_or(0) as u8;
    }
    topo.hubs = topo.devices.iter().filter(|d| d.is_hub).cloned().collect();
    for hub in &mut topo.hubs {
        let maxchild = raws
            .iter()
            .find(|r| r.make_instance() == hub.platform_id)
            .map(|r| r.maxchild)
            .unwrap_or(0) as u8;
        hub.properties
            .insert("maxchild".into(), maxchild.to_string());
    }
    topo
}

/// Numeric-driven conversion — mirrors the Windows/macOS shape so rule
/// layers stay platform-agnostic.
pub fn build_usb_device(raw: &RawDeviceInfo) -> UsbDevice {
    let mut dev = UsbDevice::new(raw.vendor_id, raw.product_id);
    dev.platform_id = raw.make_instance();
    dev.device_class = skirr_core::UsbClass::from_u8(raw.device_class);
    dev.is_hub = raw.device_class == 9;
    dev.manufacturer = raw.manufacturer.clone();
    dev.product = raw.product.clone();
    dev.serial_number = raw.serial.clone();
    dev.connection_status = ConnectionStatus::Connected;
    // sysfs `removable` is the Linux internal-device heuristic (DATA_MAP §9).
    dev.is_internal = !raw.removable;
    dev.current_link_speed = raw
        .speed_mbps
        .map(native::map_speed_code)
        .unwrap_or(UsbSpeed::Unknown);
    dev.properties.insert("sysfs_name".into(), raw.name.clone());
    dev.properties.insert("hwid".into(), raw.make_hwid());
    if let Some(bcd) = &raw.bcd_usb {
        dev.properties
            .insert("bcdUSB".into(), bcd.trim().to_string());
    }
    dev
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(
        name: &str,
        vid: u16,
        pid: u16,
        class: u8,
        speed: Option<u32>,
        maxchild: u16,
    ) -> RawDeviceInfo {
        RawDeviceInfo {
            name: name.into(),
            busnum: name
                .split('-')
                .next()
                .and_then(|b| {
                    b.strip_prefix("usb")
                        .map_or(b.parse().ok(), |n| n.parse().ok())
                })
                .unwrap_or(0),
            devnum: 0,
            vendor_id: vid,
            product_id: pid,
            device_class: class,
            device_subclass: 0,
            device_protocol: 0,
            bcd_usb: None,
            serial: None,
            manufacturer: None,
            product: None,
            maxchild,
            removable: true,
            speed_mbps: speed,
        }
    }

    /// usb3 (4 ports) → hub 3-2 → leaf 3-2.3, plus direct 3-1.
    fn fixture() -> Vec<RawDeviceInfo> {
        vec![
            raw("usb3", 0x1D6B, 0x0003, 9, Some(5000), 4),
            raw("3-2", 0x2109, 0x0817, 9, Some(5000), 4),
            raw("3-2.3", 0x0781, 0x5583, 0x00, Some(480), 0),
            raw("3-1", 0x046D, 0xC52B, 0x00, Some(12), 0),
        ]
    }

    #[test]
    fn builds_controllers_root_hubs_and_chains() {
        let topo = build(&fixture(), &HashMap::new(), chrono::Utc::now());

        assert_eq!(topo.host_controllers.len(), 1);
        assert_eq!(topo.root_hubs.len(), 1);
        assert_eq!(topo.root_hubs[0].port_count, 4);
        assert_eq!(topo.devices.len(), 3);

        let hub = topo
            .devices
            .iter()
            .find(|d| d.platform_id.ends_with("\\3-2"))
            .expect("external hub");
        let leaf = topo
            .devices
            .iter()
            .find(|d| d.platform_id.ends_with("\\3-2.3"))
            .expect("leaf");
        assert_eq!(leaf.hop_count, 1);
        assert_eq!(leaf.tier, 2);
        assert_eq!(leaf.port_number, Some(3));
        assert_eq!(leaf.parent_id, Some(hub.id));
        let direct = topo
            .devices
            .iter()
            .find(|d| d.platform_id.ends_with("\\3-1"))
            .expect("direct");
        assert_eq!(hub.children_ids, vec![leaf.id]);
        assert_eq!(topo.root_hubs[0].children_ids, vec![hub.id, direct.id]);
        assert_eq!(topo.root_hubs[0].children_ids, vec![hub.id, direct.id]);

        let direct = topo
            .devices
            .iter()
            .find(|d| d.platform_id.ends_with("\\3-1"))
            .expect("direct");
        assert_eq!(direct.tier, 1);
        assert_eq!(direct.port_number, Some(1));
    }
    #[test]
    fn empty_input_builds_empty_topology() {
        let topo = build(&[], &HashMap::new(), chrono::Utc::now());
        assert!(topo.host_controllers.is_empty());
        assert!(topo.devices.is_empty());
    }

    #[test]
    fn pci_enrichment_names_controller() {
        let mut pci = HashMap::new();
        pci.insert(3u8, "0000:00:14.0".to_string());
        let topo = build(&fixture(), &pci, chrono::Utc::now());
        assert_eq!(topo.host_controllers[0].platform_id, "0000:00:14.0");
        assert_eq!(
            topo.host_controllers[0].pci_address.as_deref(),
            Some("0000:00:14.0")
        );
    }
}
