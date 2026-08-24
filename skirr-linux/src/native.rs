//! sysfs enumeration for Linux USB devices.
//!
//! Pure parsing/mapping lives here (testable on any host); the actual
//! `/sys/bus/usb/devices` walk is `#[cfg(target_os = "linux")]`.
//!
//! sysfs naming: device dir names encode the physical path —
//! `usbN` = root hub of bus N, `N-M` = device at bus N port M,
//! `N-M.P` = device behind downstream hubs. Interfaces carry a colon
//! (`N-M:C.I`) and are skipped.

use skirr_core::{UsbClass, UsbSpeed};

/// One USB device (or root hub) as read from sysfs attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct RawDeviceInfo {
    /// sysfs directory name, e.g. `"usb3"`, `"3-2"`, `"3-2.1.4"`
    pub name: String,
    pub busnum: u8,
    pub devnum: u32,
    pub vendor_id: u16,
    pub product_id: u16,
    pub device_class: u8,
    pub device_subclass: u8,
    pub device_protocol: u8,
    /// `bcdUSB` attribute, e.g. `" 3.10"`
    pub bcd_usb: Option<String>,
    pub serial: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub maxchild: u16,
    /// sysfs `removable` == "removable"
    pub removable: bool,
    /// sysfs `speed` in Mbps (1.5 / 12 / 480 / 5000 / …)
    pub speed_mbps: Option<u32>,
}

impl RawDeviceInfo {
    pub fn is_root_hub(&self) -> bool {
        is_root_hub_name(&self.name)
    }

    /// Windows-shaped instance id, deterministic per location:
    /// `USB\VID_2109&PID_0817\3-2.1`
    pub fn make_instance(&self) -> String {
        format!(
            "USB\\VID_{:04X}&PID_{:04X}\\{}",
            self.vendor_id, self.product_id, self.name
        )
    }

    pub fn make_hwid(&self) -> String {
        format!(
            "USB\\VID_{:04X}&PID_{:04X}\\REV_{:04X}",
            self.vendor_id,
            self.product_id,
            parse_bcd_hex(self.bcd_usb.as_deref()).unwrap_or(0)
        )
    }
}

/// True for names like `usb3` (root hubs), false for `3-2`, `3-2.1`,
/// and interface dirs.
pub fn is_root_hub_name(name: &str) -> bool {
    name.strip_prefix("usb")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Parent sysfs name: `3-2.1.4` → `3-2.1`; `3-2` → `usb3`; `usb3` → None.
pub fn parent_name(name: &str) -> Option<String> {
    if is_root_hub_name(name) {
        return None;
    }
    let (bus, path) = name.split_once('-')?;
    match path.rsplit_once('.') {
        Some((parent_path, _)) => Some(format!("{bus}-{parent_path}")),
        None => Some(format!("usb{bus}")),
    }
}

/// Immediate downstream port number: `3-2.1.4` → 4; `3-2` → 2.
pub fn immediate_port(name: &str) -> Option<u8> {
    if is_root_hub_name(name) {
        return None;
    }
    let (_, path) = name.split_once('-')?;
    let last = path.split('.').next_back();
    last.and_then(|seg| seg.parse().ok())
}

/// Hop count to the root hub = number of dot segments in the devpath:
/// `3-2` → 0 hops (direct attachment), `3-2.1.4` → 2 hops.
pub fn hop_count(name: &str) -> usize {
    if is_root_hub_name(name) {
        return 0;
    }
    name.split_once('-')
        .map(|(_, path)| path.matches('.').count())
        .unwrap_or(0)
}

fn parse_bcd_hex(value: Option<&str>) -> Option<u16> {
    u16::from_str_radix(value?.trim().replace('.', "").as_str(), 16).ok()
}

/// Parse an unsigned sysfs attribute (`idVendor`, `maxchild`, …).
pub fn parse_uint(body: &str) -> Option<u32> {
    body.trim().parse().ok()
}

/// Parse hex attributes written without prefix (`idVendor` = `"2109"`).
pub fn parse_hex(body: &str) -> Option<u16> {
    u16::from_str_radix(body.trim(), 16).ok()
}

/// Parse the `speed` file: fractional Mbps allowed ("1.5"), rounded down.
pub fn parse_speed_mbps(body: &str) -> Option<u32> {
    body.trim().parse::<f64>().ok().map(|v| v.floor() as u32)
}

/// Map negotiated Mbps to UsbSpeed (DATA_MAP §4 Linux row).
pub fn map_speed_code(mbps: u32) -> UsbSpeed {
    match mbps {
        0..=2 => UsbSpeed::LowSpeed,
        3..=100 => UsbSpeed::FullSpeed,
        101..=4000 => UsbSpeed::HighSpeed,
        4001..=9000 => UsbSpeed::SuperSpeed,
        9001..=19000 => UsbSpeed::SuperSpeedPlus10,
        _ => UsbSpeed::SuperSpeedPlus20,
    }
}

/// Floor capability implied by the USB spec version (`bcdUSB`): a " 3.10"
/// device is *at least* SuperSpeed+10 capable.
pub fn derive_min_speed(bcd_usb: Option<&str>) -> UsbSpeed {
    match bcd_usb.unwrap_or("").trim() {
        v if v.starts_with("3.2") => UsbSpeed::SuperSpeedPlus20,
        v if v.starts_with("3.1") => UsbSpeed::SuperSpeedPlus10,
        v if v.starts_with("3.") => UsbSpeed::SuperSpeed,
        v if v.starts_with("2.") => UsbSpeed::HighSpeed,
        _ => UsbSpeed::FullSpeed,
    }
}

#[allow(dead_code)]
fn class_from_u8(value: u8) -> UsbClass {
    UsbClass::from_u8(value)
}

/// Read one device's attributes from its sysfs directory.
#[cfg(target_os = "linux")]
pub fn collect_entry(dir: &std::path::Path) -> std::io::Result<Option<RawDeviceInfo>> {
    use std::fs;

    let name = match dir.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_string(),
        None => return Ok(None),
    };
    // Interface directories never carry idVendor.
    if name.contains(':') || !dir.join("idVendor").exists() {
        return Ok(None);
    }
    let read = |file: &str| fs::read_to_string(dir.join(file)).ok();

    let busnum = read("busnum").as_deref().and_then(parse_uint).unwrap_or(0) as u8;
    let raw = RawDeviceInfo {
        name,
        busnum,
        devnum: read("devnum").as_deref().and_then(parse_uint).unwrap_or(0),
        vendor_id: read("idVendor").as_deref().and_then(parse_hex).unwrap_or(0),
        product_id: read("idProduct")
            .as_deref()
            .and_then(parse_hex)
            .unwrap_or(0),
        device_class: read("bDeviceClass")
            .as_deref()
            .and_then(parse_hex)
            .map(|v| u8::try_from(v).unwrap_or(0))
            .unwrap_or(0),
        device_subclass: read("bDeviceSubClass")
            .as_deref()
            .and_then(parse_hex)
            .map(|v| u8::try_from(v).unwrap_or(0))
            .unwrap_or(0),
        device_protocol: read("bDeviceProtocol")
            .as_deref()
            .and_then(parse_hex)
            .map(|v| u8::try_from(v).unwrap_or(0))
            .unwrap_or(0),
        bcd_usb: read("bcdUSB"),
        serial: read("serial")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        manufacturer: read("manufacturer")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        product: read("product")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        maxchild: read("maxchild")
            .as_deref()
            .and_then(parse_uint)
            .unwrap_or(0) as u16,
        removable: read("removable").is_some_and(|r| r.trim() == "removable"),
        speed_mbps: read("speed").as_deref().and_then(parse_speed_mbps),
    };
    Ok(Some(raw))
}

/// Enumerate every present USB device + root hub via `/sys/bus/usb/devices`.
#[cfg(target_os = "linux")]
pub fn enumerate() -> skirr_core::BackendResult<Vec<RawDeviceInfo>> {
    use std::fs;

    const DEVICES_DIR: &str = "/sys/bus/usb/devices";
    let mut out = Vec::new();
    let entries = fs::read_dir(DEVICES_DIR).map_err(|e| {
        skirr_core::BackendError::os_api("skirr-linux", format!("read_dir {DEVICES_DIR}: {e}"))
    })?;
    for entry in entries {
        let entry = entry
            .map_err(|e| skirr_core::BackendError::os_api("skirr-linux", format!("entry: {e}")))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match collect_entry(&path) {
            Ok(Some(raw)) => out.push(raw),
            Ok(None) => {}
            Err(e) => {
                return Err(skirr_core::BackendError::os_api(
                    "skirr-linux",
                    format!("collect {}: {e}", path.display()),
                ))
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_links_follow_sysfs_naming() {
        assert_eq!(parent_name("3-2.1.4").as_deref(), Some("3-2.1"));
        assert_eq!(parent_name("3-2.1").as_deref(), Some("3-2"));
        assert_eq!(parent_name("3-2").as_deref(), Some("usb3"));
        assert_eq!(parent_name("usb3"), None);
    }

    #[test]
    fn root_hub_detection() {
        assert!(is_root_hub_name("usb1"));
        assert!(!is_root_hub_name("usb"));
        assert!(!is_root_hub_name("1-0:1.0"));
        assert!(!is_root_hub_name("1-1"));
    }

    #[test]
    fn ports_hops_and_tiers_derive_from_names() {
        assert_eq!(immediate_port("3-2.1.4"), Some(4));
        assert_eq!(immediate_port("3-12"), Some(12));
        assert_eq!(hop_count("3-2"), 0);
        assert_eq!(hop_count("3-2.1.4"), 2);
    }

    #[test]
    fn attribute_parsers_handle_sysfs_quirks() {
        assert_eq!(parse_hex("2109\n"), Some(0x2109));
        assert_eq!(parse_uint("480\n"), Some(480));
        assert_eq!(parse_speed_mbps(" 1.5 "), Some(1));
        assert_eq!(parse_speed_mbps("5000"), Some(5000));
    }

    #[test]
    fn speed_mapping_covers_all_tiers() {
        assert_eq!(map_speed_code(1), UsbSpeed::LowSpeed);
        assert_eq!(map_speed_code(12), UsbSpeed::FullSpeed);
        assert_eq!(map_speed_code(480), UsbSpeed::HighSpeed);
        assert_eq!(map_speed_code(5000), UsbSpeed::SuperSpeed);
        assert_eq!(map_speed_code(10000), UsbSpeed::SuperSpeedPlus10);
        assert_eq!(map_speed_code(20000), UsbSpeed::SuperSpeedPlus20);
    }

    #[test]
    fn version_implies_capability_floor() {
        assert_eq!(derive_min_speed(Some(" 3.10")), UsbSpeed::SuperSpeedPlus10);
        assert_eq!(derive_min_speed(Some(" 2.00")), UsbSpeed::HighSpeed);
        assert_eq!(derive_min_speed(None), UsbSpeed::FullSpeed);
    }

    #[test]
    fn instance_ids_are_deterministic_per_location() {
        let mut raw = RawDeviceInfo {
            name: "3-2.1".into(),
            busnum: 3,
            devnum: 42,
            vendor_id: 0x2109,
            product_id: 0x0817,
            device_class: 9,
            device_subclass: 0,
            device_protocol: 2,
            bcd_usb: Some(" 3.00".into()),
            serial: None,
            manufacturer: Some("VIA Labs".into()),
            product: Some("USB 3.0 Hub".into()),
            maxchild: 4,
            removable: true,
            speed_mbps: Some(5000),
        };
        assert_eq!(raw.make_instance(), r"USB\VID_2109&PID_0817\3-2.1");
        assert_eq!(raw.make_instance(), raw.make_instance());
        assert!(raw.make_hwid().starts_with(r"USB\VID_2109&PID_0817"));

        raw.devnum = 7;
        // devnum changes on re-enumeration; identity must not depend on it.
        assert_eq!(raw.make_instance(), r"USB\VID_2109&PID_0817\3-2.1");
        assert_eq!(class_from_u8(9), UsbClass::Hub);
    }
}
