//! Dock/hub identification from vendor ids (DATA_MAP §5/§9).
//!
//! Curated silicon-family table — VID-level only. Full VID/PID dock
//! catalogs rot fast; silicon vendors don't. Role claims stay strictly
//! evidence-based: DisplayLink ⇒ video, hub-class + known family ⇒ dock
//! hub, everything else ⇒ unclaimed component. Nothing here guesses.

use crate::{SystemTopology, UsbClass};
use uuid::Uuid;

/// Well-known USB hub/dock silicon vendors. Values are stable marketing
/// names suitable for reports.
pub fn hub_family(vendor_id: u16) -> Option<&'static str> {
    match vendor_id {
        0x17E9 => Some("DisplayLink"),
        0x0BDA => Some("Realtek"),
        0x2109 => Some("VIA Labs"),
        0x05E3 => Some("Genesys Logic"),
        0x174C => Some("ASMedia"),
        0x0451 => Some("Texas Instruments"),
        0x04B4 => Some("Cypress"),
        0x1A40 => Some("Terminus"),
        _ => None,
    }
}

/// What we can honestly claim a component does inside a dock/hub stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockRole {
    /// Hub-class device from a known hub-silicon family.
    Hub,
    /// DisplayLink-family device — virtual video adapter.
    VideoAdapter,
    /// Known family but no further claim.
    Component,
}

/// Classify one device against the family table.
pub fn dock_role(vendor_id: u16, device_class: UsbClass, is_hub: bool) -> Option<DockRole> {
    let family = hub_family(vendor_id)?;
    let _ = family;
    match device_class {
        UsbClass::Hub if is_hub => Some(DockRole::Hub),
        _ if vendor_id == 0x17E9 => Some(DockRole::VideoAdapter),
        _ => Some(DockRole::Component),
    }
}

/// Stamp `properties` on every device the table recognizes and mark the
/// root-most recognized external hub as the dock anchor. Purely additive;
/// unrecognized devices are left untouched (absence = no claim).
///
/// Properties written:
/// - `dock_family`   — silicon family name
/// - `dock_role`     — "hub" | "video" | "component"
/// - `dock_anchor`   — set on the topmost external recognized hub
///   ("true"); its subtree inherits nothing — tier/port fields already
///   describe the physical layout.
pub fn annotate_docks(topo: &mut SystemTopology) {
    use crate::ConnectionStatus;

    // Find anchor candidate: lowest-tier external recognized hub.
    let mut anchor: Option<Uuid> = None;
    let mut anchor_tier = u8::MAX;
    for dev in &topo.devices {
        if !dev.is_hub
            || dev.is_internal
            || dev.connection_status != ConnectionStatus::Connected
            || dev.tier == 0
        {
            continue;
        }
        if matches!(
            dock_role(dev.vendor_id, dev.device_class, dev.is_hub),
            Some(DockRole::Hub)
        ) && dev.tier < anchor_tier
        {
            anchor_tier = dev.tier;
            anchor = Some(dev.id);
        }
    }

    for dev in &mut topo.devices {
        let Some(role) = dock_role(dev.vendor_id, dev.device_class, dev.is_hub) else {
            continue;
        };
        dev.properties.insert(
            "dock_family".into(),
            hub_family(dev.vendor_id).unwrap().into(),
        );
        dev.properties.insert(
            "dock_role".into(),
            match role {
                DockRole::Hub => "hub",
                DockRole::VideoAdapter => "video",
                DockRole::Component => "component",
            }
            .into(),
        );
        if anchor == Some(dev.id) {
            dev.properties.insert("dock_anchor".into(), "true".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlatformInfo, UsbDevice};

    fn empty_platform_info() -> PlatformInfo {
        PlatformInfo {
            os: "test".into(),
            os_version: String::new(),
            kernel_version: None,
            architecture: "x86_64".into(),
            hostname: None,
            username: None,
            is_admin: false,
            is_virtual_machine: false,
            boot_time: None,
        }
    }

    fn device(vid: u16, class: UsbClass, is_hub: bool) -> UsbDevice {
        let mut d = UsbDevice::new(vid, 0x1234);
        d.device_class = class;
        d.is_hub = is_hub;
        d
    }

    #[test]
    fn families_resolve() {
        assert_eq!(hub_family(0x17E9), Some("DisplayLink"));
        assert_eq!(hub_family(0x2109), Some("VIA Labs"));
        assert_eq!(hub_family(0x05E3), Some("Genesys Logic"));
        assert_eq!(hub_family(0x05AC), None, "Apple is not dock silicon");
    }

    #[test]
    fn roles_follow_evidence() {
        // DisplayLink video adapter regardless of class.
        assert_eq!(
            dock_role(0x17E9, UsbClass::VendorSpecific, false),
            Some(DockRole::VideoAdapter)
        );
        // Known hub silicon + hub class → dock hub.
        assert_eq!(dock_role(0x2109, UsbClass::Hub, true), Some(DockRole::Hub));
        // Unknown vendor → no claim at all.
        assert_eq!(dock_role(0x1234, UsbClass::Hub, true), None);
    }

    fn fixture_topo() -> SystemTopology {
        let mut topo = SystemTopology {
            timestamp: chrono::Utc::now(),
            host_controllers: Vec::new(),
            root_hubs: Vec::new(),
            devices: Vec::new(),
            hubs: Vec::new(),
            displays: Vec::new(),
            events: Vec::new(),
            platform_info: empty_platform_info(),
        };
        // Tier-0 controller, tier-1 root hub (internal), tier-2 VIA dock
        // hub, tier-3 devices: DisplayLink adapter + mass storage.
        let mut ctrl = device(0x8086, UsbClass::Hub, true);
        ctrl.is_internal = true;
        ctrl.tier = 0;
        let mut root = device(0x05AC, UsbClass::Hub, true);
        root.is_internal = true;
        root.tier = 1;
        let mut dock_hub = device(0x2109, UsbClass::Hub, true);
        dock_hub.tier = 2;
        let mut dl = device(0x17E9, UsbClass::VendorSpecific, false);
        dl.tier = 3;
        let mut stick = device(0x0781, UsbClass::MassStorage, false);
        stick.tier = 3;
        topo.devices = vec![ctrl, root, dock_hub, dl, stick];
        topo
    }

    #[test]
    fn annotation_stamps_families_and_anchor() {
        let mut topo = fixture_topo();
        annotate_docks(&mut topo);

        let by_vid = |v: u16| topo.devices.iter().find(|d| d.vendor_id == v).unwrap();
        let dock_hub = by_vid(0x2109);
        assert_eq!(dock_hub.properties.get("dock_family").unwrap(), "VIA Labs");
        assert_eq!(dock_hub.properties.get("dock_role").unwrap(), "hub");
        assert_eq!(dock_hub.properties.get("dock_anchor").unwrap(), "true");

        let dl = by_vid(0x17E9);
        assert_eq!(dl.properties.get("dock_role").unwrap(), "video");

        // Unrecognized devices untouched.
        let stick = by_vid(0x0781);
        assert!(!stick.properties.contains_key("dock_family"));

        // Internal root hub must never be the anchor even when its
        // vendor is recognized.
        let root = topo.devices.iter().find(|d| d.vendor_id == 0x05AC).unwrap();
        assert!(!root.properties.contains_key("dock_anchor"));
    }

    #[test]
    fn empty_topology_is_safe() {
        let mut topo = fixture_topo();
        topo.devices.clear();
        annotate_docks(&mut topo);
        assert!(topo.devices.is_empty());
    }
}
