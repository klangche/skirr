//! Windows hardware-ID and instance-path parsing.
//!
//! Pure string logic, no OS calls - unit-tested on every platform.

use skirr_core::UsbClass;

/// IDs parsed from a Windows hardware ID string
/// (`USB\VID_0B05&PID_1ACE&MI_00`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HardwareId {
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub interface: Option<u8>,
}

impl HardwareId {
    pub fn is_complete(&self) -> bool {
        self.vid.is_some() && self.pid.is_some()
    }
}

/// Parse VID/PID/interface from one hardware ID line.
pub fn parse_hardware_id(hwid: &str) -> HardwareId {
    let upper = hwid.to_ascii_uppercase();
    HardwareId {
        vid: hex_field(&upper, "VID_"),
        pid: hex_field(&upper, "PID_"),
        interface: decimal_field(&upper, "&MI_").map(|v| v as u8),
    }
}

fn hex_field(s: &str, key: &str) -> Option<u16> {
    let start = s.find(key)? + key.len();
    let slice = s.get(start..start + 4)?;
    u16::from_str_radix(slice, 16).ok()
}

fn decimal_field(s: &str, key: &str) -> Option<u16> {
    let start = s.find(key)? + key.len();
    let rest = s.get(start..)?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Heuristic decomposition of a PnP instance path segment
/// (ported from Shoko's `_parse_devpath`).
///
/// `HubHi&HubLo&Flags&Port` shapes encode hub + port; serial-shaped instances
/// are root-level; `6&`/`5&` prefixes are generated (no serial) instances.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstancePathInfo {
    pub hub_id: String,
    pub port: Option<u16>,
    pub is_composite_interface: bool,
    /// 0 = root/serial instance, 1 = generated, 2 = explicit hub+port path
    pub depth_guess: u8,
}

pub fn parse_instance_path(instance: &str) -> InstancePathInfo {
    let parts: Vec<&str> = instance.split('&').collect();
    if parts.len() >= 4
        && parts[..2]
            .iter()
            .all(|p| p.len() == 8 && p.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return InstancePathInfo {
            hub_id: format!("{}&{}", parts[0], parts[1]),
            port: parts[3].parse().ok(),
            is_composite_interface: false,
            depth_guess: 2,
        };
    }
    InstancePathInfo {
        hub_id: String::new(),
        port: None,
        is_composite_interface: false,
        depth_guess: generated_instance_depth(instance),
    }
}

fn generated_instance_depth(instance: &str) -> u8 {
    if instance.starts_with("6&") || instance.starts_with("5&") {
        1
    } else {
        0
    }
}

/// Extract a serial number from an instance ID when Windows embedded one;
/// returns `None` for generated (`6&...`) or hub/port paths.
pub fn extract_serial_from_instance(instance: &str) -> Option<String> {
    match parse_instance_path(instance) {
        info if info.depth_guess == 0 && !instance.is_empty() => Some(instance.to_string()),
        _ => None,
    }
}

/// Map Windows class/device names onto the normalized UsbClass enum.
pub fn usb_class_from_windows_name(name: &str) -> UsbClass {
    let lower = name.to_ascii_lowercase();
    if lower.contains("usbstor") || lower.contains("mass storage") {
        UsbClass::MassStorage
    } else if lower.contains("hid") {
        UsbClass::HID
    } else if lower == "usb" || lower.contains("hub") {
        UsbClass::Hub
    } else if lower.contains("audio") || lower.contains("media") {
        UsbClass::Audio
    } else if lower.contains("printer") {
        UsbClass::Printer
    } else if lower.contains("image") || lower.contains("camera") || lower.contains("scanner") {
        UsbClass::Image
    } else if lower.contains("video") {
        UsbClass::Video
    } else if lower.contains("bluetooth") || lower.contains("wireless") {
        UsbClass::WirelessController
    } else if lower.contains("net") || lower.contains("cdc") || lower.contains("modem") {
        UsbClass::Communications
    } else if lower.contains("billboard") {
        UsbClass::Billboard
    } else if lower.contains("smartcard") {
        UsbClass::SmartCard
    } else if lower.contains("vendor") {
        UsbClass::VendorSpecific
    } else {
        UsbClass::Unspecified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_vid_pid() {
        let id = parse_hardware_id(r"USB\VID_0B05&PID_1ACE");
        assert_eq!(id.vid, Some(0x0B05));
        assert_eq!(id.pid, Some(0x1ACE));
        assert_eq!(id.interface, None);
        assert!(id.is_complete());
    }

    #[test]
    fn parses_composite_interface() {
        let id = parse_hardware_id(r"USB\VID_046d&pid_c52b&MI_03");
        assert_eq!(id.vid, Some(0x046D));
        assert_eq!(id.pid, Some(0xC52B));
        assert_eq!(id.interface, Some(3));
    }

    #[test]
    fn missing_fields_are_none() {
        assert!(!parse_hardware_id(r"ACPI\PNP0501").is_complete());
        assert_eq!(parse_hardware_id("").vid, None);
    }

    #[test]
    fn hub_port_instance_path() {
        // `6&`-prefixed generated instances do NOT match the full hub shape
        // (first segment too short) -> classified as generated, no hub id.
        let info = parse_instance_path("6&1A89C1F2&0&0000");
        assert_eq!(info.hub_id, "");
        assert_eq!(info.port, None);
        assert_eq!(info.depth_guess, 1);
    }

    #[test]
    fn true_hub_port_path_detected() {
        let info = parse_instance_path("00000001&00000002&0003");
        // 4 segments expected for full shape; this has 3 -> falls back.
        assert_ne!(info.depth_guess, 2);

        let full = parse_instance_path("12345678&9ABCDEF0&0&0012");
        assert_eq!(full.hub_id, "12345678&9ABCDEF0");
        assert_eq!(full.port, Some(12)); // decimal, as in PnP instance paths
        assert_eq!(full.depth_guess, 2);
    }

    #[test]
    fn serial_instances_extract_cleanly() {
        assert_eq!(
            extract_serial_from_instance("T6MPKRD00HWM5"),
            Some("T6MPKRD00HWM5".to_string())
        );
        assert_eq!(extract_serial_from_instance("6&1A89C1F2&0&0000"), None);
    }

    #[test]
    fn class_name_mapping() {
        assert_eq!(usb_class_from_windows_name("USB"), UsbClass::Hub);
        assert_eq!(
            usb_class_from_windows_name("USBSTOR\\Disk"),
            UsbClass::MassStorage
        );
        assert_eq!(usb_class_from_windows_name("HIDClass"), UsbClass::HID);
        assert_eq!(usb_class_from_windows_name("Media"), UsbClass::Audio);
        assert_eq!(
            usb_class_from_windows_name("SomethingNew"),
            UsbClass::Unspecified
        );
    }
}
