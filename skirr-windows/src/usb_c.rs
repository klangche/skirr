//! USB-C / Type-C detection for Windows (DATA_MAP §5, ★★ — sparse outside
//! OEM drivers).
//!
//! Sources:
//! - UCM/UCSI PnP nodes (`UCM-UCSI`, `Microsoft UsbCcMux`, `Type-C` in
//!   hardware/friendly names) prove a Type-C *port stack* is present
//! - Billboard-class devices (class 0x11) prove an active Type-C partner
//!
//! Per-port alt modes and PD contracts are effectively driver-only on
//! Windows → reported as Unknown with reason, never guessed.

use crate::native::RawDeviceInfo;
use skirr_core::{ConnectorOrientation, UsbCCurrentMode, UsbCInfo, UsbCPortType};

/// USB device class code for Billboard devices.
pub const BILLBOARD_CLASS: u8 = 0x11;

/// Markers that a PnP node belongs to the USB-C connector manager stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UcmMarker {
    /// `UCM-UCSI` ACPI/PnP devices
    Ucsi,
    /// `Microsoft UsbCcMux` connector mux nodes
    CcMux,
}

/// Classify one PnP identifier string (hardware id, friendly name, or
/// description). None when unrelated.
pub fn classify_pnp_node(text: &str) -> Option<UcmMarker> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("ucm-ucsi") || lower.contains("ucmucsi") {
        Some(UcmMarker::Ucsi)
    } else if lower.contains("usbccmux") || lower.contains("usb c cmux") {
        Some(UcmMarker::CcMux)
    } else {
        None
    }
}

/// Is this raw record a Billboard partner device? Windows doesn't give us
/// the numeric class byte here — match the Class_11 compatible ID or any
/// "billboard" marker in ids/names.
pub fn is_billboard(raw: &RawDeviceInfo) -> bool {
    let hay = [
        raw.hardware_ids.join(" "),
        raw.description.clone().unwrap_or_default(),
        raw.friendly_name.clone().unwrap_or_default(),
        raw.class_name.clone().unwrap_or_default(),
    ]
    .join(" ")
    .to_ascii_lowercase();
    hay.contains("class_11") || hay.contains("billboard")
}

/// Conservative info for a confirmed Billboard partner.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(class: u8, hwids: &[&str]) -> RawDeviceInfo {
        RawDeviceInfo {
            instance_id: r"USB\VID_1234&PID_5678\test".into(),
            hardware_ids: hwids.iter().map(|s| s.to_string()).collect(),
            manufacturer: None,
            description: None,
            friendly_name: None,
            class_name: if class == 0x11 {
                Some("Billboard".into())
            } else {
                None
            },
            status: Some("OK".into()),
            parent: None,
        }
    }

    #[test]
    fn pnp_markers_classify() {
        assert_eq!(classify_pnp_node(r"ACPI\UCM-UCSI\0"), Some(UcmMarker::Ucsi));
        assert_eq!(
            classify_pnp_node("Microsoft UsbCcMux Device"),
            Some(UcmMarker::CcMux)
        );
        assert_eq!(classify_pnp_node("USB Mass Storage"), None);
    }

    #[test]
    fn billboards_detected_by_class_or_hwid() {
        assert!(is_billboard(&raw(
            0x00,
            &[r"USB\Class_11", r"USB\VID_2109&PID_0817"]
        )));
        assert!(is_billboard(&raw(0x11, &["USB\\VID_1234&PID_5678"])));
        assert!(!is_billboard(&raw(0x00, &[r"USB\VID_0781"])));
    }

    #[test]
    fn billboard_info_is_conservatively_unknown() {
        let info = billboard_info();
        assert_eq!(info.port_type, UsbCPortType::Unknown);
        assert_eq!(info.current_mode, UsbCCurrentMode::Unknown);
        assert!(!info.pd_supported);
        assert_eq!(
            info.connector_orientation,
            Some(ConnectorOrientation::Unknown)
        );
    }
}
