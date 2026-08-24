//! Windows topology construction from raw device records.
//!
//! Pure graph logic - no OS calls - so the builder is unit-testable on every
//! platform. Semantics match `skirr-core`'s rule engine: hops = ancestor hub
//! count, tiers = hops + 1.

use crate::backend::build_usb_device;
use crate::hwid::parse_instance_path;
use crate::native::RawDeviceInfo;
use chrono::{DateTime, Utc};
use skirr_core::{HostController, PlatformInfo, RootHub, SystemTopology, UsbDevice, UsbSpeed};
use std::collections::HashMap;
use uuid::Uuid;

/// Build a full topology snapshot from one enumeration pass.
///
/// Devices whose PnP parent sits outside the enumerated USB-class set are
/// treated as attached to host controllers; instances containing `ROOT_HUB`
/// become `RootHub` records grouped under those controllers.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn build(
    raw_devices: &[RawDeviceInfo],
    platform_info: PlatformInfo,
    timestamp: DateTime<Utc>,
) -> SystemTopology {
    let is_root_hub = |instance: &str| instance.to_ascii_uppercase().contains("ROOT_HUB");

    // Normalize + keep parent strings indexed by device UUID.
    let mut devices: Vec<UsbDevice> = raw_devices.iter().map(build_usb_device).collect();
    let parent_string: HashMap<Uuid, String> = raw_devices
        .iter()
        .filter_map(|r| {
            let instance = r.instance_id.as_str();
            devices
                .iter()
                .find(|d| d.platform_id == instance)
                .and_then(|d| r.parent.clone().map(|p| (d.id, p)))
        })
        .collect();

    let id_by_instance: HashMap<String, Uuid> = devices
        .iter()
        .map(|d| (d.platform_id.clone(), d.id))
        .collect();

    // Link parents/children where both sides are present.
    for dev in &mut devices {
        if let Some(parent) = parent_string.get(&dev.id) {
            if let Some(pid) = id_by_instance.get(parent) {
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

    // Controllers keyed by the PnP instance of whatever the root hubs hang off.
    let mut controller_keys: Vec<String> = devices
        .iter()
        .filter(|d| is_root_hub(&d.platform_id))
        .filter_map(|d| parent_string.get(&d.id).cloned())
        .collect();
    controller_keys.sort();
    controller_keys.dedup();
    let controller_by_platform: HashMap<String, Uuid> = controller_keys
        .into_iter()
        .map(|k| (k.clone(), Uuid::new_v4()))
        .collect();
    let controller_uuid_for = |platform: &str| controller_by_platform.get(platform).copied();

    let by_id: HashMap<Uuid, &UsbDevice> = devices.iter().map(|d| (d.id, d)).collect();

    // Compute chain metrics + root/controller attribution per device.
    let computed: Vec<UsbDevice> = devices
        .iter()
        .map(|dev| {
            let mut d = (*dev).clone();
            let self_is_root = is_root_hub(&d.platform_id);

            let mut hops: u8 = 0;
            let mut cursor = d.parent_id;
            let mut path_root_hub: Option<Uuid> = None;
            while let Some(cid) = cursor {
                let Some(node) = by_id.get(&cid) else { break };
                if node.is_hub || node.device_class == skirr_core::UsbClass::Hub {
                    hops += 1;
                }
                if is_root_hub(&node.platform_id) {
                    path_root_hub = Some(node.id);
                }
                cursor = node.parent_id;
            }

            d.hop_count = hops;
            d.tier = hops.saturating_add(1);
            d.topology_depth = d.tier;
            d.port_number = extract_port(&d.platform_id);

            if self_is_root {
                d.root_hub_id = Some(d.id);
                d.host_controller_id = parent_string
                    .get(&d.id)
                    .and_then(|p| controller_uuid_for(p));
            } else if let Some(rh) = path_root_hub {
                d.root_hub_id = Some(rh);
                d.host_controller_id = by_id.get(&rh).and_then(|rh_dev| {
                    parent_string
                        .get(&rh_dev.id)
                        .and_then(|p| controller_uuid_for(p))
                });
            }
            d
        })
        .collect();
    let devices = computed;

    // RootHub records.
    let root_hubs: Vec<RootHub> = devices
        .iter()
        .filter(|d| is_root_hub(&d.platform_id))
        .map(|rh| RootHub {
            id: rh.id,
            platform_id: rh.platform_id.clone(),
            host_controller_id: rh.host_controller_id.unwrap_or_else(Uuid::new_v4),
            port_count: rh.children_ids.len() as u8,
            hub_speed: rh.current_link_speed,
            is_integrated: true,
            children_ids: rh.children_ids.clone(),
        })
        .collect();

    let mut host_controllers: Vec<HostController> = controller_by_platform
        .iter()
        .map(|(instance, id)| HostController {
            id: *id,
            platform_id: instance.clone(),
            name: "USB Host Controller".to_string(),
            vendor_id: None,
            device_id: None,
            revision: None,
            usb_version: UsbSpeed::Unknown,
            root_hub_ids: root_hubs
                .iter()
                .filter(|r| r.host_controller_id == *id)
                .map(|r| r.id)
                .collect(),
            port_count: root_hubs
                .iter()
                .filter(|r| r.host_controller_id == *id)
                .map(|r| r.port_count)
                .sum(),
            is_xhci: instance.to_ascii_uppercase().contains("_XHC"),
            pci_address: None,
            driver_version: None,
            capabilities: Default::default(),
        })
        .collect();
    host_controllers.sort_by(|a, b| a.platform_id.cmp(&b.platform_id));

    let hubs = devices.iter().filter(|d| d.is_hub).cloned().collect();

    SystemTopology {
        timestamp,
        host_controllers,
        root_hubs,
        devices,
        hubs,
        displays: Vec::new(),
        events: Vec::new(),
        platform_info,
    }
}

/// Port number from an instance ID: explicit hub+port shape first, then the
/// trailing segment of generated (`6&...`) paths. Serial-shaped instances
/// have no embedded port and yield `None`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn extract_port(instance_id: &str) -> Option<u8> {
    let last = instance_id.rsplit('\\').next()?;
    let info = parse_instance_path(last);
    if let Some(port) = info.port {
        return u8::try_from(port).ok();
    }
    if info.depth_guess == 1 {
        return last.rsplit('&').next()?.parse().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::RawDeviceInfo;
    use skirr_core::PlatformInfo;

    fn test_platform_info() -> PlatformInfo {
        PlatformInfo {
            os: "Windows".into(),
            os_version: "test".into(),
            kernel_version: None,
            architecture: "x86_64".into(),
            hostname: None,
            username: None,
            is_admin: false,
            is_virtual_machine: false,
            boot_time: None,
        }
    }

    fn raw(instance: &str, parent: Option<&str>) -> RawDeviceInfo {
        RawDeviceInfo {
            instance_id: instance.to_string(),
            hardware_ids: vec![format!(
                "USB\\VID_{}&PID_{}",
                &pseudo_hash_hex(instance)[..4],
                &pseudo_hash_hex(instance)[4..8]
            )],
            manufacturer: Some("TestVendor".into()),
            description: None,
            friendly_name: Some(format!("Dev {}", instance)),
            class_name: if instance.to_uppercase().contains("ROOT_HUB") || instance.contains("HUB")
            {
                Some("USB".into())
            } else {
                Some("USBSTOR".into())
            },
            status: Some("OK".into()),
            parent: parent.map(str::to_string),
        }
    }

    /// Stable pseudo-hash so VID/PID look valid per fixture name.
    fn pseudo_hash_hex(s: &str) -> String {
        let mut h: u32 = 0x811C_9DC5;
        for b in s.bytes() {
            h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
        format!("{h:08X}")
    }

    const CONTROLLER: &str = r"PCI\VEN_8086&DEV_A36D\3&11583659&0&A0";
    const ROOT_HUB: &str = r"USB\ROOT_HUB30\4&38A99F6&0";

    #[test]
    fn links_full_chain_and_computes_metrics() {
        let raws = vec![
            raw(CONTROLLER, None),
            raw(ROOT_HUB, Some(CONTROLLER)),
            raw(r"USB\VID_1111&PID_2222\HUBDEV", Some(ROOT_HUB)),
            raw(r"USB\VID_3333&PID_4444\6&2D6F9AB&0&0003", Some(ROOT_HUB)),
        ];
        let topo = build(&raws, test_platform_info(), Utc::now());

        assert_eq!(topo.host_controllers.len(), 1);
        assert_eq!(topo.root_hubs.len(), 1);
        assert_eq!(topo.devices.len(), 4);

        let by_pid = |p: &str| {
            topo.devices
                .iter()
                .find(|d| d.platform_id == p)
                .unwrap_or_else(|| panic!("missing {p}"))
        };
        let hub_dev = by_pid(ROOT_HUB);
        assert!(hub_dev
            .platform_id
            .to_ascii_uppercase()
            .contains("ROOT_HUB"));
        assert_eq!(hub_dev.hop_count, 0);
        assert_eq!(hub_dev.tier, 1);
        assert_eq!(hub_dev.root_hub_id, Some(hub_dev.id));
        assert!(topo.host_controllers[0].root_hub_ids.contains(&hub_dev.id));

        let mid = by_pid(r"USB\VID_1111&PID_2222\HUBDEV");
        assert_eq!(mid.parent_id, Some(hub_dev.id));
        assert_eq!(mid.hop_count, 1);
        assert_eq!(mid.root_hub_id, Some(hub_dev.id));

        let leaf = by_pid(r"USB\VID_3333&PID_4444\6&2D6F9AB&0&0003");
        assert_eq!(leaf.hop_count, 1);
        assert_eq!(leaf.port_number, Some(3));

        let rh_record = &topo.root_hubs[0];
        assert_eq!(rh_record.port_count, 2);
        assert_eq!(rh_record.host_controller_id, topo.host_controllers[0].id);
    }

    #[test]
    fn three_level_chain_counts_hops() {
        let raws = vec![
            raw(CONTROLLER, None),
            raw(ROOT_HUB, Some(CONTROLLER)),
            raw(r"USB\VID_AA&PID_BB\EXT_HUB", Some(ROOT_HUB)),
            raw(
                r"USB\VID_CC&PID_DD\LEAF",
                Some(r"USB\VID_AA&PID_BB\EXT_HUB"),
            ),
        ];
        let topo = build(&raws, test_platform_info(), Utc::now());
        let leaf = topo
            .devices
            .iter()
            .find(|d| d.platform_id.ends_with("LEAF"))
            .unwrap();
        assert_eq!(leaf.hop_count, 2); // root hub + ext hub
        assert_eq!(leaf.tier, 3);
        assert_eq!(leaf.root_hub_id, Some(topo.root_hubs[0].id));
        assert!(leaf.host_controller_id.is_some());
    }

    #[test]
    fn unknown_parent_stays_unlinked_without_panic() {
        let raws = vec![raw(r"USB\VID_EE&PID_FF\LONELY", Some(r"PCI\SOMEWHERE"))];
        let topo = build(&raws, test_platform_info(), Utc::now());
        let lonely = &topo.devices[0];
        assert_eq!(lonely.parent_id, None);
        assert_eq!(lonely.hop_count, 0);
    }
}
