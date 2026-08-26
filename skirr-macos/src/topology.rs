//! macOS topology construction from raw device records.
//!
//! Pure graph logic - no OS calls - so the builder is unit-testable on every
//! platform. Semantics match `skirr-core`'s rule engine: hops = ancestor hub
//! count, tiers = hops + 1.
//!
//! macOS deviation (documented per planner): modern Apple Silicon machines do
//! not expose explicit root-hub device records, so each bus gets a synthesized
//! `HostController` + `RootHub` pair. Buses are keyed by the root nibble of
//! the locationID (each controller's root ports occupy distinct top-level
//! nibble domains).

use crate::backend::build_usb_device;
use crate::native::RawDeviceInfo;
use chrono::{DateTime, Utc};
use skirr_core::{
    HostController, HubInfo, HubPowerSource, HubTTType, PlatformInfo, RootHub, SystemTopology,
    UsbDevice, UsbSpeed,
};
use std::collections::HashMap;
use uuid::Uuid;

/// Immediate port of a device within its parent hub.
///
/// locationID encodes the path as nibbles from the most significant side,
/// with the FIRST nibble identifying the controller/bus domain (not a port):
/// `0x14230000` = bus 1, then ports 4 → 2 → 3 down the chain. The child's
/// immediate port is the first non-zero nibble position where the parent's
/// nibble is zero. Devices attached straight to a controller therefore take
/// their root-hub port from the first port-level nibble.
///
/// On Apple Silicon the bus nibble sits at position 6 (not 7 as on Intel),
/// so we detect the bus nibble dynamically instead of hard-coding 6.
pub(crate) fn port_from_location(child: u32, parent: Option<u32>) -> Option<u8> {
    let nibble_at = |loc: u32, pos: u8| ((loc >> (pos * 4)) & 0xF) as u8;
    let depth_limit = match parent {
        Some(p) if p != 0 => (0..8).find(|&pos| nibble_at(p, pos) == 0)?,
        _ => {
            // Find the bus nibble (highest non-zero nibble) and start one
            // position below it to skip past the bus domain.
            let bus_pos = (0..8).rev().find(|&pos| nibble_at(child, pos) != 0)?;
            bus_pos.saturating_sub(1)
        }
    };
    // First non-zero nibble at or after the parent's depth.
    (depth_limit..8)
        .map(|pos| nibble_at(child, pos))
        .find(|&n| n != 0)
}

fn root_nibble(location_id: u32) -> u8 {
    ((location_id >> 28) & 0xF) as u8
}

/// Build a full topology snapshot from one enumeration pass.
pub(crate) fn build(
    raw_devices: &[RawDeviceInfo],
    platform_info: PlatformInfo,
    timestamp: DateTime<Utc>,
) -> SystemTopology {
    let mut devices: Vec<UsbDevice> = raw_devices.iter().map(build_usb_device).collect();
    let meta: HashMap<String, (&RawDeviceInfo, Uuid)> = raw_devices
        .iter()
        .zip(devices.iter())
        .map(|(r, d)| (r.instance_id.clone(), (r, d.id)))
        .collect();

    // Link parents/children where both sides are present.
    for dev in &mut devices {
        let raw = raw_devices
            .iter()
            .find(|r| r.instance_id == dev.platform_id);
        if let Some(parent_instance) = raw.and_then(|r| r.parent.as_deref()) {
            if let Some((_, pid)) = meta.get(parent_instance) {
                dev.parent_id = Some(*pid);
            }
        }
    }
    let mut children: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for dev in &devices {
        if let Some(pid) = dev.parent_id {
            children.entry(pid).or_default().push(dev.id);
        }
    }
    for dev in &mut devices {
        if let Some(kids) = children.get(&dev.id) {
            dev.children_ids = kids.clone();
        }
    }

    // Populate hub_info for devices that are hubs.
    // Strategy: IOKit port-count > known chip database > max child port number > default (4).
    fn known_hub_port_count(vid: u16, pid: u16) -> Option<u8> {
        match (vid, pid) {
            // Terminus Technology FE1.1s — 4-port USB2 hub
            (0x1A40, 0x0101) => Some(4),
            (0x1A40, 0x0201) => Some(4),
            // Terminus Technology FE2.1 — 4-port USB2 hub
            (0x1A40, 0x0801) => Some(4),
            // Realtek RTS5411 — 4-port USB3 hub
            (0x0BDA, 0x0411) => Some(4),
            // Realtek RTS5413 — 4-port USB3 hub
            (0x0BDA, 0x5413) => Some(4),
            // Realtek RTS54 (generic) — 4-port USB2 hub side
            (0x0BDA, 0x5411) => Some(4),
            // Genesys Logic GL3523 — 4-port USB3 hub
            (0x05E3, 0x0620) => Some(4),
            // Genesys Logic GL3521 — 4-port USB3 hub
            (0x05E3, 0x0610) => Some(4),
            // Genesys Logic GL850/852 — 4-port USB2 hub
            (0x05E3, 0x0608) => Some(4),
            // VIA Labs VL810 — 4-port USB3 hub
            (0x2109, 0x3431) => Some(4),
            // VIA Labs VL811 — 4-port USB3 hub
            (0x2109, 0x3432) => Some(4),
            // VIA Labs VL812 — 4-port USB3 hub
            (0x2109, 0x0812) => Some(4),
            // Microchip USB2514 — 4-port USB2 hub
            (0x0424, 0x2514) => Some(4),
            // Microchip USB2517 — 7-port USB2 hub
            (0x0424, 0x2517) => Some(7),
            // Cypress CY7C65632 — 4-port USB2 hub
            (0x04B4, 0x6572) => Some(4),
            _ => None,
        }
    }

    // Build a lookup of device_id -> UsbDevice for the fallback pass.
    // Compute hub port counts in two passes to avoid borrow conflicts.
    let devices_by_id: HashMap<Uuid, &UsbDevice> = devices.iter().map(|d| (d.id, d)).collect();
    let hub_port_counts: Vec<(Uuid, u8)> = devices
        .iter()
        .filter(|d| d.is_hub && d.hub_info.is_none())
        .filter_map(|dev| {
            let raw = raw_devices
                .iter()
                .find(|r| r.instance_id == dev.platform_id);
            let port_count = raw
                .and_then(|r| r.hub_port_count)
                .filter(|&n| n > 0)
                .or_else(|| known_hub_port_count(dev.vendor_id, dev.product_id))
                .or_else(|| {
                    children.get(&dev.id).map(|kids| {
                        kids.iter()
                            .filter_map(|kid_id| devices_by_id.get(kid_id))
                            .filter_map(|kid| kid.port_number)
                            .max()
                            .unwrap_or(4)
                    })
                })?;
            Some((dev.id, port_count))
        })
        .collect();

    for dev in &mut devices {
        if let Some(&port_count) = hub_port_counts
            .iter()
            .find(|(id, _)| *id == dev.id)
            .map(|(_, n)| n)
        {
            dev.hub_info = Some(HubInfo {
                port_count,
                is_powered: false,
                power_source: HubPowerSource::Unknown,
                supports_mtt: false,
                tt_count: 0,
                tt_type: HubTTType::Unknown,
                hub_speed: dev.max_supported_speed,
                ports: Vec::new(),
                dock_ports: Vec::new(),
            });
        }
    }

    // Populate dock_ports for hubs that are part of a dock/composite product.
    // Build dock_ports dynamically from USB hub downstream ports.
    // Must be two-pass to avoid borrow conflicts between immutable lookup and mutable devices.
    {
        let devices_by_id: HashMap<Uuid, &UsbDevice> = devices.iter().map(|d| (d.id, d)).collect();
        let mut dock_ports_map: HashMap<Uuid, Vec<skirr_core::DockPort>> = HashMap::new();

        for dev in &devices {
            if !dev.is_hub {
                continue;
            }
            let info = match &dev.hub_info {
                Some(i) => i,
                None => continue,
            };
            let mut dock_ports: Vec<skirr_core::DockPort> = Vec::new();
            let mut port_idx: u8 = 1;

            if let Some(kids) = children.get(&dev.id) {
                let mut sorted_kids: Vec<&UsbDevice> = kids
                    .iter()
                    .filter_map(|kid_id| devices_by_id.get(kid_id).copied())
                    .collect();
                sorted_kids.sort_by_key(|k| k.port_number.unwrap_or(0));

                for kid in &sorted_kids {
                    if let Some(pn) = kid.port_number {
                        let port_type = if kid.vendor_id == 0x0BDA && kid.product_id == 0x8153 {
                            skirr_core::DockPortType::Ethernet
                        } else {
                            match kid.device_class {
                                skirr_core::UsbClass::Hub => skirr_core::DockPortType::UsbC,
                                skirr_core::UsbClass::MassStorage => {
                                    skirr_core::DockPortType::SdCard
                                }
                                skirr_core::UsbClass::Audio => {
                                    skirr_core::DockPortType::AudioJack35
                                }
                                _ => skirr_core::DockPortType::UsbA,
                            }
                        };
                        dock_ports.push(skirr_core::DockPort {
                            index: port_idx,
                            port_type,
                            usb_hub_port: Some(pn),
                            connected_device_id: Some(kid.id),
                            label: port_type.label().to_string(),
                        });
                        port_idx += 1;
                    }
                }

                for pn in 1..=info.port_count {
                    if !kids.iter().any(|kid_id| {
                        devices_by_id
                            .get(kid_id)
                            .map(|k| k.port_number == Some(pn))
                            .unwrap_or(false)
                    }) {
                        dock_ports.push(skirr_core::DockPort {
                            index: port_idx,
                            port_type: skirr_core::DockPortType::UsbA,
                            usb_hub_port: Some(pn),
                            connected_device_id: None,
                            label: "free".to_string(),
                        });
                        port_idx += 1;
                    }
                }
            }

            if !dock_ports.is_empty() {
                dock_ports_map.insert(dev.id, dock_ports);
            }
        }

        for dev in &mut devices {
            if let Some(dock_ports) = dock_ports_map.remove(&dev.id) {
                if let Some(ref mut info) = dev.hub_info {
                    info.dock_ports = dock_ports;
                }
            }
        }
    }

    // Bus domains keyed by the root nibble of the locationID. Records without
    // a usable location collapse into bus 0 so nothing is dropped silently.
    let bus_of = |raw: &RawDeviceInfo| {
        if raw.location_id == 0 {
            0
        } else {
            root_nibble(raw.location_id)
        }
    };
    let mut buses: Vec<u8> = raw_devices.iter().map(bus_of).collect();
    buses.sort();
    buses.dedup();

    // Synthesized controllers + root hubs, one pair per bus.
    let mut controller_by_bus: HashMap<u8, Uuid> = HashMap::new();
    let mut root_hub_by_bus: HashMap<u8, RootHub> = HashMap::new();
    for &bus in &buses {
        let cid = Uuid::new_v4();
        controller_by_bus.insert(bus, cid);
        // Port count is filled from tier-1 attachments below (root-port
        // devices always carry ≥2 non-zero nibbles: bus + port).
        root_hub_by_bus.insert(
            bus,
            RootHub {
                id: Uuid::new_v4(),
                platform_id: format!(r"ROOT_HUB\BUS_{bus}"),
                host_controller_id: cid,
                port_count: 0,
                hub_speed: UsbSpeed::SuperSpeed,
                is_integrated: true,
                children_ids: Vec::new(),
            },
        );
    }

    let by_id: HashMap<Uuid, &UsbDevice> = devices.iter().map(|d| (d.id, d)).collect();
    let location_by_id: HashMap<Uuid, u32> = raw_devices
        .iter()
        .zip(devices.iter())
        .filter(|(r, _)| r.location_id != 0)
        .map(|(r, d)| (d.id, r.location_id))
        .collect();

    // Chain metrics + attribution per device.
    let computed: Vec<UsbDevice> = devices
        .iter()
        .map(|dev| {
            let mut d = (*dev).clone();
            let raw = raw_devices.iter().find(|r| r.instance_id == d.platform_id);
            let bus = raw.map(bus_of).unwrap_or(0);

            let mut hops: u8 = 0;
            let mut cursor = d.parent_id;
            while let Some(cid) = cursor {
                match by_id.get(&cid) {
                    Some(node) => {
                        if node.is_hub || node.device_class == skirr_core::UsbClass::Hub {
                            hops += 1;
                        }
                        cursor = node.parent_id;
                    }
                    None => break,
                }
            }

            d.hop_count = hops;
            d.tier = hops.saturating_add(1);
            d.topology_depth = d.tier;
            let parent_loc = d.parent_id.and_then(|p| location_by_id.get(&p).copied());
            d.port_number = location_by_id
                .get(&d.id)
                .and_then(|&loc| port_from_location(loc, parent_loc));

            // Attribution: nearest real hub ancestor's root hub is the same
            // synthesized one this bus owns; every record belongs to its bus.
            d.root_hub_id = root_hub_by_bus.get(&bus).map(|rh| rh.id);
            d.host_controller_id = controller_by_bus.get(&bus).copied();
            d
        })
        .collect();

    // Fill synthesized root-hub children (direct root-port attachments).
    let mut rh_children: HashMap<u8, Vec<Uuid>> = HashMap::new();
    for dev in &computed {
        if dev.tier == 1 {
            if let Some(raw) = raw_devices
                .iter()
                .find(|r| r.instance_id == dev.platform_id)
            {
                rh_children.entry(bus_of(raw)).or_default().push(dev.id);
            }
        }
    }
    let root_hubs: Vec<RootHub> = root_hub_by_bus
        .into_values()
        .map(|mut rh| {
            if let Some(kids) = rh_children.get(&parse_bus(&rh.platform_id)) {
                rh.children_ids = kids.clone();
                if kids.len() > usize::from(rh.port_count) {
                    rh.port_count = kids.len() as u8;
                }
            }
            rh
        })
        .collect();

    let host_controllers: Vec<HostController> = controller_by_bus
        .iter()
        .map(|(&bus, &cid)| HostController {
            id: cid,
            platform_id: format!("USB_BUS_{bus}"),
            name: "Apple USB Host Controller".to_string(),
            vendor_id: Some(0x106B),
            device_id: None,
            revision: None,
            usb_version: UsbSpeed::SuperSpeed,
            root_hub_ids: root_hubs
                .iter()
                .filter(|r| r.host_controller_id == cid)
                .map(|r| r.id)
                .collect(),
            port_count: root_hubs
                .iter()
                .filter(|r| r.host_controller_id == cid)
                .map(|r| r.port_count)
                .sum(),
            is_xhci: true,
            pci_address: None,
            driver_version: None,
            capabilities: Default::default(),
        })
        .collect();
    let mut host_controllers = host_controllers;
    host_controllers.sort_by(|a, b| a.platform_id.cmp(&b.platform_id));

    let hubs = computed.iter().filter(|d| d.is_hub).cloned().collect();

    SystemTopology {
        timestamp,
        host_controllers,
        root_hubs,
        devices: computed,
        hubs,
        displays: Vec::new(),
        thunderbolt_routers: Vec::new(),
        type_c_ports: Vec::new(),
        events: Vec::new(),
        platform_info,
    }
}

fn parse_bus(platform_id: &str) -> u8 {
    platform_id
        .rsplit('_')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::RawDeviceInfo;
    use skirr_core::PlatformInfo;

    fn test_platform_info() -> PlatformInfo {
        PlatformInfo {
            os: "macOS".into(),
            os_version: "test".into(),
            kernel_version: None,
            architecture: "arm64".into(),
            hostname: None,
            username: None,
            is_admin: false,
            is_virtual_machine: false,
            boot_time: None,
        }
    }

    fn raw(vid: u16, pid: u16, location: u32, parent: Option<&str>, class: u8) -> RawDeviceInfo {
        RawDeviceInfo {
            instance_id: crate::native::make_instance(vid, pid, location),
            hardware_ids: vec![crate::native::make_hwid(vid, pid)],
            vendor_id: vid,
            product_id: pid,
            manufacturer: Some("TestVendor".into()),
            product_name: Some(format!("Dev {vid:04X}")),
            serial_number: Some("SER".into()),
            device_class: class,
            ..Default::default()
        }
        .with_location(location)
        .with_parent(parent)
    }

    impl RawDeviceInfo {
        fn with_location(mut self, loc: u32) -> Self {
            self.location_id = loc;
            self
        }
        fn with_parent(mut self, parent: Option<&str>) -> Self {
            self.parent = parent.map(str::to_string);
            self
        }
    }

    const HUB_CLASS: u8 = 0x09;
    const MSC_CLASS: u8 = 0x08;

    #[test]
    fn port_decoding_walks_nibbles() {
        // Direct attachment: bus nibble (1) skipped, root-hub port = 4.
        assert_eq!(port_from_location(0x14500000, None), Some(4));
        assert_eq!(port_from_location(0x14230000, Some(0x14200000)), Some(3));
        assert_eq!(port_from_location(0x14230400, Some(0x14230000)), Some(4));
        assert_eq!(port_from_location(0x00000000, None), None);
    }

    #[test]
    fn links_full_chain_and_computes_metrics() {
        let hub = raw(0x2109, 0x0817, 0x14200000, None, HUB_CLASS);
        let leaf = raw(
            0x0781,
            0x5583,
            0x14230000,
            Some(hub.instance_id.as_str()),
            MSC_CLASS,
        );

        let topo = build(
            &[hub.clone(), leaf.clone()],
            test_platform_info(),
            Utc::now(),
        );

        assert_eq!(topo.host_controllers.len(), 1);
        assert_eq!(topo.root_hubs.len(), 1);

        let hub_dev = topo
            .devices
            .iter()
            .find(|d| d.vendor_id == 0x2109)
            .expect("hub");
        assert_eq!(hub_dev.hop_count, 0); // directly on root port
        assert_eq!(hub_dev.tier, 1);
        assert_eq!(hub_dev.port_number, Some(4)); // 0x14**2**0000 → bus 1, port 4

        let leaf_dev = topo
            .devices
            .iter()
            .find(|d| d.vendor_id == 0x0781)
            .expect("leaf");
        assert_eq!(leaf_dev.parent_id, Some(hub_dev.id));
        assert_eq!(leaf_dev.hop_count, 1);
        assert_eq!(leaf_dev.tier, 2);
        assert_eq!(leaf_dev.port_number, Some(3)); // under hub: 0x142**3**0000

        // Synthesized root hub owns the tier-1 attachment.
        let rh = &topo.root_hubs[0];
        assert_eq!(rh.port_count, 1);
        assert_eq!(rh.host_controller_id, topo.host_controllers[0].id);
        assert!(rh.children_ids.contains(&hub_dev.id));
        assert_eq!(hub_dev.root_hub_id, Some(rh.id));
        assert_eq!(
            leaf_dev.root_hub_id, hub_dev.root_hub_id,
            "leaf shares the bus root hub"
        );
    }

    #[test]
    fn separate_root_nibbles_form_separate_buses() {
        let a = raw(0x1111, 0x2222, 0x14100000, None, MSC_CLASS);
        let b = raw(0x3333, 0x4444, 0x02100000, None, MSC_CLASS);
        let topo = build(&[a, b], test_platform_info(), Utc::now());

        assert_eq!(topo.host_controllers.len(), 2);
        assert_eq!(topo.root_hubs.len(), 2);
        let hubs_a: Vec<_> = topo
            .devices
            .iter()
            .filter(|d| d.vendor_id == 0x1111)
            .collect();
        let hubs_b: Vec<_> = topo
            .devices
            .iter()
            .filter(|d| d.vendor_id == 0x3333)
            .collect();
        assert_ne!(
            hubs_a[0].host_controller_id, hubs_b[0].host_controller_id,
            "distinct root nibbles must not merge into one bus"
        );
    }

    #[test]
    fn unknown_parent_stays_unlinked_without_panic() {
        let lonely = raw(
            0xEEEE,
            0xFFFF,
            0x14500000,
            Some(r"USB\GHOST\0x99999999"),
            MSC_CLASS,
        );
        let topo = build(&[lonely], test_platform_info(), Utc::now());
        let dev = &topo.devices[0];
        assert_eq!(dev.parent_id, None);
        assert_eq!(dev.hop_count, 0);
        assert!(dev.host_controller_id.is_some());
    }

    #[test]
    fn empty_enumeration_still_builds_valid_skeleton() {
        let topo = build(&[], test_platform_info(), Utc::now());
        assert!(topo.devices.is_empty());
        assert!(topo.host_controllers.is_empty());
        assert!(topo.root_hubs.is_empty());
    }
}
