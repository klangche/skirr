//! USB-C / Type-C detection for macOS (DATA_MAP §5, ★★ — undocumented,
//! model-dependent).
//!
//! Sources:
//! - `AppleTypeCCRU`-family IORegistry services expose orientation/current
//!   on some Apple Silicon Macs
//! - Billboard-class devices (class 0x11) prove an active Type-C partner
//!
//! Where nothing is exposed the answer is honest absence: no `usb_c_info`
//! on the device and a reason string in its properties.

use skirr_core::{ConnectorOrientation, UsbCCurrentMode, UsbCInfo, UsbCPortType};

/// Which flavor of Type-C-related IORegistry service this name is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TypeCKind {
    /// `AppleTypeCCRU` / `AppleTypeCCRU-mst` — PD controller
    Cru,
    /// XHCI port nodes that mention Type-C
    XhciPort,
}

/// Classify an IORegistry service name; None when unrelated.
pub fn classify_registry_name(name: &str) -> Option<TypeCKind> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("typeccru") {
        Some(TypeCKind::Cru)
    } else if lower.contains("xhci") && lower.contains("typec") {
        Some(TypeCKind::XhciPort)
    } else {
        None
    }
}

/// USB device class code for Billboard devices.
pub const BILLBOARD_CLASS: u8 = 0x11;

/// Info we can state for a Billboard partner: it IS a Type-C connection,
/// but current/alt-mode detail stays Unknown.
pub fn billboard_info() -> UsbCInfo {
    UsbCInfo {
        port_type: UsbCPortType::Unknown,
        current_mode: UsbCCurrentMode::Unknown,
        pd_supported: false,
        pd_revision: None,
        alt_modes: Vec::new(),
        cable_info: None,
        connector_orientation: Some(ConnectorOrientation::Unknown),
        port_index: None,
        partner_info: None,
    }
}

/// Read orientation/current from CRU-style properties (keys vary by macOS
/// version — everything optional).
#[cfg(target_os = "macos")]
pub fn info_from_props(orientation: Option<&str>, current_ma: Option<i64>) -> UsbCInfo {
    let mut info = billboard_info();
    info.connector_orientation = Some(match orientation {
        Some(o) if o.eq_ignore_ascii_case("flipped") || o.eq_ignore_ascii_case("reverse") => {
            ConnectorOrientation::Flipped
        }
        Some(o) if o.eq_ignore_ascii_case("normal") => ConnectorOrientation::Normal,
        _ => ConnectorOrientation::Unknown,
    });
    info.current_mode = match current_ma {
        Some(ma) if ma >= 3000 => UsbCCurrentMode::TypeCCurrent3_0A,
        Some(ma) if ma >= 1500 => UsbCCurrentMode::TypeCCurrent1_5A,
        _ => UsbCCurrentMode::DefaultUsb,
    };
    info
}

/// Enumerate Type-C controller presence. Returns classified names found in
/// the IORegistry (empty on Intel Macs without CRU services — that's the
/// honest "not exposed" outcome).
#[cfg(target_os = "macos")]
pub fn detect_cru_presence() -> skirr_core::BackendResult<Vec<String>> {
    crate::native::typec_services()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_names_classify() {
        assert_eq!(
            classify_registry_name("AppleTypeCCRU"),
            Some(TypeCKind::Cru)
        );
        assert_eq!(
            classify_registry_name("AppleTypeCCRU-mst"),
            Some(TypeCKind::Cru)
        );
        assert_eq!(
            classify_registry_name("AppleUSB20XHCIPortTypeC"),
            Some(TypeCKind::XhciPort)
        );
        assert_eq!(classify_registry_name("IOUSBHostDevice"), None);
    }

    #[test]
    fn billboard_info_is_conservatively_unknown() {
        let info = billboard_info();
        assert_eq!(info.port_type, UsbCPortType::Unknown);
        assert_eq!(info.current_mode, UsbCCurrentMode::Unknown);
        assert!(!info.pd_supported);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cru_props_map_to_current_and_orientation() {
        let flipped = info_from_props(Some("Flipped"), Some(3000));
        assert_eq!(
            flipped.connector_orientation,
            Some(ConnectorOrientation::Flipped)
        );
        assert_eq!(flipped.current_mode, UsbCCurrentMode::TypeCCurrent3_0A);

        let normal = info_from_props(Some("normal"), Some(1500));
        assert_eq!(
            normal.connector_orientation,
            Some(ConnectorOrientation::Normal)
        );
        assert_eq!(normal.current_mode, UsbCCurrentMode::TypeCCurrent1_5A);

        let unknown = info_from_props(None, None);
        assert_eq!(unknown.current_mode, UsbCCurrentMode::DefaultUsb);
    }
}
