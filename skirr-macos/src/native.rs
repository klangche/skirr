//! macOS raw device collection: IORegistry via IOKit, `system_profiler` fallback.
//!
//! Pure parsing/normalization compiles everywhere so it stays unit-testable;
//! the native collectors live behind `#[cfg(target_os = "macos")]`.
//! Instance-ID scheme mirrors Windows shape (`USB\VID_x&PID_y\<location>`) so
//! downstream topology/rule code stays platform-agnostic; `<location>` is the
//! hex `locationID` (macOS devices often have no serial string).

use serde::Deserialize;
use skirr_core::BackendError;

// ---------------------------------------------------------------------------
// Raw record
// ---------------------------------------------------------------------------

/// One USB device as observed by macOS sources, pre-normalization.
#[derive(Debug, Clone, Default)]
pub struct RawDeviceInfo {
    pub instance_id: String,
    /// e.g. `USB\VID_05AC&PID_12A8` (informational; numeric fields are primary).
    pub hardware_ids: Vec<String>,
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product_name: Option<String>,
    pub serial_number: Option<String>,
    pub device_class: u8,
    pub device_sub_class: u8,
    pub device_protocol: u8,
    pub bcd_usb: u16,
    pub location_id: u32,
    /// Parent's instance ID when the parent is itself a USB device
    /// (hub chain); `None` under a host controller.
    pub parent: Option<String>,
    /// IORegistry `Speed` property (negotiated; IOKit `USBDeviceSpeed` code).
    pub speed_code: Option<u32>,
    /// system_profiler `"speed"` string parsed to Mb/s — ADVERTISED max,
    /// never negotiated (DATA_MAP §4).
    pub advertised_mbps: Option<u32>,
}

/// Stable per-plug identity: VID/PID plus location ID.
pub(crate) fn make_instance(vendor_id: u16, product_id: u16, location_id: u32) -> String {
    format!(r"USB\VID_{vendor_id:04X}&PID_{product_id:04X}\{location_id:#010x}")
}

/// One connected display as read from `IODisplayConnect` (DATA_MAP §7):
/// registry service name, vendor/product codes, raw EDID bytes when the
/// panel exposes them. Decoding to `DisplayInfo` happens in the backend.
#[derive(Debug, Clone)]
pub(crate) struct DisplayRecord {
    pub service_name: String,
    pub vendor_id: Option<u32>,
    pub product_id: Option<u32>,
    pub edid_raw: Vec<u8>,
}

/// Build the hardware-id string matching our instance scheme.
pub(crate) fn make_hwid(vendor_id: u16, product_id: u16) -> String {
    format!(r"USB\VID_{vendor_id:04X}&PID_{product_id:04X}")
}

// ---------------------------------------------------------------------------
// system_profiler parsing (pure)
// ---------------------------------------------------------------------------

/// Parse one `location_id` value as emitted by system_profiler:
/// `"0x14500000 / 3"` or plain `"0x14500000"`.
pub(crate) fn parse_location_id(raw: &str) -> Option<u32> {
    let token = raw.split('/').next()?.trim();
    u32::from_str_radix(token.trim_start_prefix("0x"), 16).ok()
}

trait TrimStartPrefix {
    fn trim_start_prefix(&self, prefix: &str) -> &str;
}
impl TrimStartPrefix for str {
    fn trim_start_prefix(&self, prefix: &str) -> &str {
        self.strip_prefix(prefix).unwrap_or(self)
    }
}

#[derive(Debug, Deserialize)]
struct SpNode {
    #[serde(rename = "_name")]
    name: Option<String>,
    #[serde(rename = "_items", default)]
    items: Vec<SpNode>,
    vendor_id: Option<String>,
    product_id: Option<String>,
    location_id: Option<String>,
    serial_num: Option<String>,
    manufacturer: Option<String>,
    bcd_device: Option<String>,
    /// "Up to 480 Mb/s" — advertised ceiling, not negotiated.
    speed: Option<String>,
}

impl SpNode {
    /// Numeric VID/PID when both parse; hub-only nodes may lack them.
    fn ids(&self) -> Option<(u16, u16)> {
        let vid =
            u16::from_str_radix(self.vendor_id.as_deref()?.trim_start_prefix("0x"), 16).ok()?;
        let pid =
            u16::from_str_radix(self.product_id.as_deref()?.trim_start_prefix("0x"), 16).ok()?;
        Some((vid, pid))
    }
}

/// Flatten a parsed `system_profiler SPUSBDataType -json` document into raw
/// records with parent links resolved through the tree structure.
pub(crate) fn parse_system_profiler_json(json: &str) -> Result<Vec<RawDeviceInfo>, BackendError> {
    #[derive(Deserialize)]
    struct SpRoot {
        #[serde(rename = "SPUSBDataType", default)]
        usb: Vec<SpNode>,
    }

    let root: SpRoot = serde_json::from_str(json).map_err(|e| {
        BackendError::os_api(
            crate::BACKEND_NAME,
            format!("system_profiler json parse: {e}"),
        )
    })?;

    let mut records = Vec::new();
    walk_sp_nodes(&root.usb, None, &mut records);
    Ok(records)
}

fn walk_sp_nodes(nodes: &[SpNode], parent: Option<&str>, out: &mut Vec<RawDeviceInfo>) {
    for node in nodes {
        let Some((vid, pid)) = node.ids() else {
            // No IDs → not addressable as a device; still descend for children.
            walk_sp_nodes(&node.items, None, out);
            continue;
        };
        let location = node
            .location_id
            .as_deref()
            .and_then(parse_location_id)
            .unwrap_or(0);
        let instance = make_instance(vid, pid, location);
        out.push(RawDeviceInfo {
            instance_id: instance.clone(),
            hardware_ids: vec![make_hwid(vid, pid)],
            vendor_id: vid,
            product_id: pid,
            manufacturer: node.manufacturer.clone(),
            product_name: node.name.clone(),
            serial_number: node.serial_num.clone(),
            device_class: 0, // system_profiler does not expose class codes
            device_sub_class: 0,
            device_protocol: 0,
            bcd_usb: parse_bcd_device(node.bcd_device.as_deref()),
            location_id: location,
            parent: parent.map(str::to_string),
            speed_code: None,
            advertised_mbps: node
                .speed
                .as_deref()
                .and_then(crate::speeds::parse_advertised_mbps),
        });
        walk_sp_nodes(&node.items, Some(instance.as_str()), out);
    }
}

/// `"8.21"` → `0x0821`bcd-style; unparseable → 0.
pub(crate) fn parse_bcd_device(value: Option<&str>) -> u16 {
    let Some(value) = value else { return 0 };
    let mut parts = value.split('.');
    let major: u16 = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    let minor: u16 = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    (major << 8) | (minor & 0xFF)
}

// ---------------------------------------------------------------------------
// Native collectors (macOS only): IOKit primary, system_profiler fallback
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod iokit {
    use super::*;
    use std::ffi::{c_char, c_void, CStr, CString};

    type MachPort = u32;
    type IoObject = u32;
    type Kern = i32;

    const KERN_SUCCESS: Kern = 0;
    const K_IO_MAIN_PORT_DEFAULT: MachPort = 0;
    const K_CF_NUMBER_SINT32: isize = 3;
    const K_CF_NUMBER_SINT64: isize = 4;
    const K_CF_STRING_UTF8: u32 = 0x0800_0100;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingServices(
            port: MachPort,
            matching: *mut c_void,
            existing: *mut IoObject,
        ) -> Kern;
        fn IOIteratorNext(iterator: IoObject) -> IoObject;
        fn IOObjectRelease(object: IoObject) -> Kern;
        fn IORegistryEntryCreateCFProperties(
            entry: IoObject,
            properties: *mut *mut c_void,
            allocator: *mut c_void,
            options: u32,
        ) -> Kern;
        fn IORegistryEntryGetParentEntry(
            entry: IoObject,
            plane: *const c_char,
            parent: *mut IoObject,
        ) -> Kern;
        fn IORegistryEntryGetName(entry: IoObject, name: *mut c_char) -> Kern;
        fn CFStringCreateWithCString(
            alloc: *mut c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> *mut c_void;
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFGetTypeID(cf: *const c_void) -> usize;
        fn CFNumberGetValue(number: *const c_void, the_type: isize, value: *mut c_void) -> bool;
        fn CFDataGetLength(data: *const c_void) -> isize;
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
        fn CFStringGetCString(
            s: *const c_void,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> bool;
        fn CFRelease(cf: *const c_void);
    }

    struct Dict(*mut c_void);

    impl Dict {
        fn get(&self, key: &str) -> *const c_void {
            let Ok(ckey) = CString::new(key) else {
                return std::ptr::null();
            };
            let cfkey = unsafe {
                CFStringCreateWithCString(std::ptr::null_mut(), ckey.as_ptr(), K_CF_STRING_UTF8)
            };
            if cfkey.is_null() {
                return std::ptr::null();
            }
            let value = unsafe { CFDictionaryGetValue(self.0, cfkey) };
            unsafe { CFRelease(cfkey) };
            value
        }

        fn number<T: TryFrom<i64>>(&self, key: &str) -> Option<T> {
            let v = self.get(key);
            if v.is_null() || unsafe { CFGetTypeID(v) } != cf_number_typeid() {
                return None;
            }
            // Try 64-bit first, fall back to 32-bit storage.
            let mut wide: i64 = 0;
            if unsafe {
                CFNumberGetValue(v, K_CF_NUMBER_SINT64, &mut wide as *mut i64 as *mut c_void)
            } {
                return T::try_from(wide).ok();
            }
            let mut narrow: i32 = 0;
            unsafe {
                CFNumberGetValue(
                    v,
                    K_CF_NUMBER_SINT32,
                    &mut narrow as *mut i32 as *mut c_void,
                )
            }
            .then(|| T::try_from(i64::from(narrow)).ok())
            .flatten()
        }

        fn string(&self, key: &str) -> Option<String> {
            let v = self.get(key);
            if v.is_null() || unsafe { CFGetTypeID(v) } != cf_string_typeid() {
                return None;
            }
            let mut buf = [0u8; 512];
            let ok = unsafe {
                CFStringGetCString(
                    v,
                    buf.as_mut_ptr().cast::<c_char>(),
                    buf.len() as isize,
                    K_CF_STRING_UTF8,
                )
            };
            if !ok {
                return None;
            }
            Some(
                unsafe { CStr::from_ptr(buf.as_ptr().cast::<c_char>()) }
                    .to_string_lossy()
                    .into_owned(),
            )
        }

        /// Raw bytes property (e.g. `IODisplayEDID`).
        fn data(&self, key: &str) -> Option<Vec<u8>> {
            let v = self.get(key);
            if v.is_null() {
                return None;
            }
            let len = unsafe { CFDataGetLength(v) };
            if len <= 0 {
                return None;
            }
            let ptr = unsafe { CFDataGetBytePtr(v) };
            if ptr.is_null() {
                return None;
            }
            Some(unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec())
        }
    }

    extern "C" {
        #[link_name = "CFNumberGetTypeID"]
        fn cf_number_typeid_impl() -> usize;
        #[link_name = "CFStringGetTypeID"]
        fn cf_string_typeid_impl() -> usize;
    }
    fn cf_number_typeid() -> usize {
        unsafe { cf_number_typeid_impl() }
    }
    fn cf_string_typeid() -> usize {
        unsafe { cf_string_typeid_impl() }
    }

    /// Enumerate present `IOUSBDevice` instances straight from the IORegistry.
    pub(super) fn ioreg_enumerate() -> Result<Vec<RawDeviceInfo>, BackendError> {
        let name = CString::new("IOUSBDevice")
            .map_err(|e| BackendError::os_api(crate::BACKEND_NAME, format!("internal: {e}")))?;
        let mut iterator: IoObject = 0;

        let kr = unsafe {
            IOServiceGetMatchingServices(
                K_IO_MAIN_PORT_DEFAULT,
                IOServiceMatching(name.as_ptr()),
                &mut iterator,
            )
        };
        if kr != KERN_SUCCESS {
            return Err(BackendError::os_api(
                crate::BACKEND_NAME,
                format!("IOServiceGetMatchingServices kern={kr}"),
            ));
        }

        let mut records = Vec::new();
        loop {
            let entry = unsafe { IOIteratorNext(iterator) };
            if entry == 0 {
                break;
            }
            if let Some(record) = collect_entry(entry) {
                records.push(record);
            }
            unsafe { IOObjectRelease(entry) };
        }
        unsafe { IOObjectRelease(iterator) };
        Ok(records)
    }

    /// Read one registry entry into a raw record; `None` when it lacks USB ids.
    fn collect_entry(entry: IoObject) -> Option<RawDeviceInfo> {
        let mut props: *mut c_void = std::ptr::null_mut();
        let kr = unsafe {
            IORegistryEntryCreateCFProperties(entry, &mut props, std::ptr::null_mut(), 0)
        };
        if kr != KERN_SUCCESS || props.is_null() {
            return None;
        }
        let dict = Dict(props);

        let vendor_id = dict
            .number::<i64>("idVendor")
            .and_then(|v| u16::try_from(v).ok());
        let product_id = dict
            .number::<i64>("idProduct")
            .and_then(|v| u16::try_from(v).ok());
        let location_id = dict.number::<u32>("locationID").unwrap_or(0);
        let result = match (vendor_id, product_id) {
            (Some(vid), Some(pid)) => {
                let parent_instance = parent_usb_instance(entry);
                Some(RawDeviceInfo {
                    instance_id: make_instance(vid, pid, location_id),
                    hardware_ids: vec![make_hwid(vid, pid)],
                    vendor_id: vid,
                    product_id: pid,
                    manufacturer: dict.string("USB Vendor Name"),
                    product_name: dict.string("USB Product Name"),
                    serial_number: dict.string("USB Serial Number"),
                    device_class: dict.number::<i64>("bDeviceClass").unwrap_or(0) as u8,
                    device_sub_class: dict.number::<i64>("bDeviceSubClass").unwrap_or(0) as u8,
                    device_protocol: dict.number::<i64>("bDeviceProtocol").unwrap_or(0) as u8,
                    bcd_usb: dict
                        .number::<i64>("bcdUSB")
                        .and_then(|v| u16::try_from(v).ok())
                        .unwrap_or(0),
                    location_id,
                    parent: parent_instance,
                    speed_code: dict
                        .number::<i64>("Speed")
                        .and_then(|v| u32::try_from(v).ok()),
                    advertised_mbps: None,
                })
            }
            _ => None,
        };
        unsafe { CFRelease(props) };
        result
    }

    /// The PnP-parent analogue: nearest ancestor that is itself a USB device.
    /// Controllers return `None`, mirroring the Windows PCI-root behaviour.
    fn parent_usb_instance(entry: IoObject) -> Option<String> {
        let plane = CString::new("IOService").ok()?;
        let mut parent: IoObject = 0;
        let kr = unsafe { IORegistryEntryGetParentEntry(entry, plane.as_ptr(), &mut parent) };
        if kr != KERN_SUCCESS || parent == 0 {
            return None;
        }
        let mut props: *mut c_void = std::ptr::null_mut();
        let ok = unsafe {
            IORegistryEntryCreateCFProperties(parent, &mut props, std::ptr::null_mut(), 0)
        } == KERN_SUCCESS
            && !props.is_null();
        let result = if !ok {
            None
        } else {
            let dict = Dict(props);
            let vid = dict
                .number::<i64>("idVendor")
                .and_then(|v| u16::try_from(v).ok());
            let pid = dict
                .number::<i64>("idProduct")
                .and_then(|v| u16::try_from(v).ok());
            let loc = dict.number::<u32>("locationID").unwrap_or(0);
            match (vid, pid) {
                (Some(v), Some(p)) => Some(make_instance(v, p, loc)),
                _ => None,
            }
        };
        if !props.is_null() {
            unsafe { CFRelease(props) };
        }
        unsafe { IOObjectRelease(parent) };
        result
    }

    /// Names of present Type-C-related services (`AppleTypeCCRU` family).
    /// Empty result = this Mac exposes nothing — the honest "not exposed"
    /// outcome per DATA_MAP §5.
    pub(super) fn typec_service_names() -> Result<Vec<String>, BackendError> {
        const CANDIDATES: [&str; 2] = ["AppleTypeCCRU", "AppleUSBXHCIPortTypeC"];
        let mut found = Vec::new();
        for service in CANDIDATES {
            let name = CString::new(service)
                .map_err(|e| BackendError::os_api(crate::BACKEND_NAME, format!("internal: {e}")))?;
            let mut iterator: IoObject = 0;
            let kr = unsafe {
                IOServiceGetMatchingServices(
                    K_IO_MAIN_PORT_DEFAULT,
                    IOServiceMatching(name.as_ptr()),
                    &mut iterator,
                )
            };
            if kr != KERN_SUCCESS {
                continue;
            }
            loop {
                let entry = unsafe { IOIteratorNext(iterator) };
                if entry == 0 {
                    break;
                }
                let mut buf = [0i8; 128];
                let gk = unsafe { IORegistryEntryGetName(entry, buf.as_mut_ptr()) };
                if gk == KERN_SUCCESS {
                    let cstr = unsafe { CStr::from_ptr(buf.as_ptr()) };
                    if let Ok(s) = cstr.to_str() {
                        found.push(s.to_string());
                    }
                }
                unsafe { IOObjectRelease(entry) };
            }
            unsafe { IOObjectRelease(iterator) };
        }
        Ok(found)
    }

    /// Walk `IODisplayConnect` services collecting EDID + id codes.
    pub(super) fn display_records() -> Result<Vec<DisplayRecord>, BackendError> {
        let name = CString::new("IODisplayConnect")
            .map_err(|e| BackendError::os_api(crate::BACKEND_NAME, format!("internal: {e}")))?;
        let mut iterator: IoObject = 0;
        let kr = unsafe {
            IOServiceGetMatchingServices(
                K_IO_MAIN_PORT_DEFAULT,
                IOServiceMatching(name.as_ptr()),
                &mut iterator,
            )
        };
        if kr != KERN_SUCCESS {
            return Err(BackendError::os_api(
                crate::BACKEND_NAME,
                format!("IOServiceGetMatchingServices(IODisplayConnect) kern={kr}"),
            ));
        }

        let mut out = Vec::new();
        loop {
            let entry = unsafe { IOIteratorNext(iterator) };
            if entry == 0 {
                break;
            }
            if let Some(record) = collect_display(entry) {
                out.push(record);
            }
            unsafe { IOObjectRelease(entry) };
        }
        unsafe { IOObjectRelease(iterator) };
        Ok(out)
    }

    fn collect_display(entry: IoObject) -> Option<DisplayRecord> {
        let mut props: *mut c_void = std::ptr::null_mut();
        let kr = unsafe {
            IORegistryEntryCreateCFProperties(entry, &mut props, std::ptr::null_mut(), 0)
        };
        if kr != KERN_SUCCESS || props.is_null() {
            return None;
        }
        let dict = Dict(props);
        // A display with neither EDID nor id codes carries nothing usable.
        let edid_raw = dict.data("IODisplayEDID").unwrap_or_default();
        let vendor_id = dict
            .number::<i64>("DisplayVendorID")
            .and_then(|v| u32::try_from(v).ok());
        let product_id = dict
            .number::<i64>("DisplayProductID")
            .and_then(|v| u32::try_from(v).ok());
        let result = if edid_raw.is_empty() && vendor_id.is_none() && product_id.is_none() {
            None
        } else {
            let mut buf = [0i8; 128];
            let service_name =
                if unsafe { IORegistryEntryGetName(entry, buf.as_mut_ptr()) } == KERN_SUCCESS {
                    unsafe { CStr::from_ptr(buf.as_ptr()) }
                        .to_string_lossy()
                        .into_owned()
                } else {
                    String::new()
                };
            Some(DisplayRecord {
                service_name,
                vendor_id,
                product_id,
                edid_raw,
            })
        };
        unsafe { CFRelease(props) };
        result
    }
}

#[cfg(target_os = "macos")]
fn system_profiler_enumerate() -> Result<Vec<RawDeviceInfo>, BackendError> {
    let output = std::process::Command::new("system_profiler")
        .args(["SPUSBDataType", "-json"])
        .output()
        .map_err(|e| {
            BackendError::os_api(crate::BACKEND_NAME, format!("spawn system_profiler: {e}"))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_system_profiler_json(&stdout)
}

/// Names of present Type-C-related IORegistry services, if any.
#[cfg(target_os = "macos")]
pub(crate) fn typec_services() -> Result<Vec<String>, BackendError> {
    iokit::typec_service_names()
}

/// Connected displays via `IODisplayConnect` IORegistry services.
/// Returns raw EDID bytes plus vendor/product codes per display; EDID
/// decoding is shared (`skirr_core::edid`). Empty result = nothing
/// exposed — the honest outcome, never an error on healthy systems.
#[cfg(target_os = "macos")]
pub(crate) fn displays() -> Result<Vec<DisplayRecord>, BackendError> {
    iokit::display_records()
}

/// DATA_MAP §11 chain: IOKit primary, system_profiler fallback.
#[cfg(target_os = "macos")]
pub(crate) fn enumerate() -> Result<Vec<RawDeviceInfo>, BackendError> {
    match iokit::ioreg_enumerate() {
        Ok(devices) if !devices.is_empty() => Ok(devices),
        _ => system_profiler_enumerate(),
    }
}

/// Thunderbolt/USB4 fabric via `system_profiler SPThunderboltDataType`.
/// Best-effort: callers treat errors as "no routers" (Phase 10.1).
#[cfg(target_os = "macos")]
pub(crate) fn thunderbolt_routers() -> Result<Vec<skirr_core::ThunderboltRouter>, BackendError> {
    let output = std::process::Command::new("system_profiler")
        .args(["SPThunderboltDataType", "-json"])
        .output()
        .map_err(|e| {
            BackendError::os_api(crate::BACKEND_NAME, format!("spawn system_profiler: {e}"))
        })?;
    if !output.status.success() {
        return Err(BackendError::os_api(
            crate::BACKEND_NAME,
            format!(
                "system_profiler SPThunderboltDataType exited {:?}",
                output.status
            ),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    skirr_core::thunderbolt::parse_system_profiler_thunderbolt(&stdout)
        .map_err(|e| BackendError::os_api(crate::BACKEND_NAME, e))
}

/// Pure-JSON variant for tests and off-macOS compilation of the parser path.
#[cfg(not(target_os = "macos"))]
pub(crate) fn thunderbolt_routers() -> Result<Vec<skirr_core::ThunderboltRouter>, BackendError> {
    Err(BackendError::unsupported(
        crate::BACKEND_NAME,
        "thunderbolt enumeration requires macOS",
    ))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn enumerate() -> Result<Vec<RawDeviceInfo>, BackendError> {
    Err(BackendError::unsupported(
        crate::BACKEND_NAME,
        "enumeration requires macOS",
    ))
}

// ---------------------------------------------------------------------------
// Tests (pure logic, run on every host)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "SPUSBDataType": [
        {"_name": "USB31Bus", "_items": [
          {"_name": "Storage", "vendor_id": "0x0781", "product_id": "0x5583",
           "location_id": "0x14500000 / 3", "serial_num": "AA01",
           "manufacturer": "SanDisk", "bcd_device": "1.00"},
          {"_name": "Hub Thing", "vendor_id": "0x2109", "product_id": "0x0817",
           "location_id": "0x14200000", "_items": [
             {"_name": "Keyboard", "vendor_id": "0x05ac", "product_id": "0x024f",
              "location_id": "0x14230000", "serial_num": "KBD"}
           ]}
        ]}
      ]
    }"#;

    #[test]
    fn location_ids_parse_with_port_suffix() {
        assert_eq!(parse_location_id("0x14500000 / 3"), Some(0x14500000));
        assert_eq!(parse_location_id("0x14500000"), Some(0x14500000));
        assert_eq!(parse_location_id("junk"), None);
    }

    #[test]
    fn profiler_tree_flattens_with_parent_links() {
        let records = parse_system_profiler_json(SAMPLE).expect("parse");
        assert_eq!(records.len(), 3);

        let storage = records
            .iter()
            .find(|r| r.manufacturer.as_deref() == Some("SanDisk"))
            .expect("storage");
        assert_eq!(storage.instance_id, r"USB\VID_0781&PID_5583\0x14500000");
        assert_eq!(storage.location_id, 0x14500000);
        assert!(storage.parent.is_none()); // top level under controller

        let keyboard = records
            .iter()
            .find(|r| r.serial_number.as_deref() == Some("KBD"))
            .expect("keyboard");
        assert_eq!(
            keyboard.parent.as_deref(),
            Some(r"USB\VID_2109&PID_0817\0x14200000")
        );
    }

    #[test]
    fn malformed_json_maps_to_os_api_error() {
        let err = parse_system_profiler_json("{not json").expect_err("must fail");
        assert!(err.to_string().contains("system_profiler"));
    }

    #[test]
    fn bcd_device_converts_to_bcd() {
        assert_eq!(parse_bcd_device(Some("2.10")), 0x020A);
        assert_eq!(parse_bcd_device(Some("1.00")), 0x0100);
        assert_eq!(parse_bcd_device(None), 0);
    }

    #[test]
    fn instance_format_is_stable() {
        assert_eq!(
            make_instance(0x05AC, 0x12A8, 0x14500000),
            r"USB\VID_05AC&PID_12A8\0x14500000"
        );
    }
}
