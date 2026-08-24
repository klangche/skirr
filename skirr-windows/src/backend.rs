//! `UsbBackend` implementation for Windows.

use crate::hwid::{extract_serial_from_instance, parse_hardware_id, usb_class_from_windows_name};
use crate::native::RawDeviceInfo;
use skirr_core::{
    BackendError, BackendResult, ConnectionStatus, PlatformInfo, SpeedReport, SystemTopology,
    UsbBackend, UsbClass, UsbDevice,
};
use uuid::Uuid;

/// Windows USB backend (SetupAPI native, PowerShell fallback).
#[derive(Debug, Default)]
pub struct SkirrWindowsBackend;

impl SkirrWindowsBackend {
    pub fn new() -> Self {
        Self
    }
}

impl UsbBackend for SkirrWindowsBackend {
    fn name(&self) -> &'static str {
        "skirr-windows"
    }

    fn platform_info(&self) -> BackendResult<PlatformInfo> {
        platform_info_impl()
    }

    fn enumerate_devices(&self) -> BackendResult<Vec<UsbDevice>> {
        enumerate_devices_impl()
    }

    fn get_topology(&self) -> BackendResult<SystemTopology> {
        Err(BackendError::unsupported(
            self.name(),
            "topology construction lands in Phase 2.2",
        ))
    }

    fn get_speeds(&self, _device_id: Uuid) -> BackendResult<SpeedReport> {
        Err(BackendError::unsupported(
            self.name(),
            "speed detection lands in Phase 2.3",
        ))
    }

    fn monitor(&self) -> BackendResult<Box<dyn skirr_core::HotplugBackend + '_>> {
        Err(BackendError::unsupported(
            self.name(),
            "hotplug monitoring lands in Phase 2.4",
        ))
    }
}

#[cfg(windows)]
fn is_admin_now() -> bool {
    crate::native::win::is_elevated()
}

#[cfg(not(windows))]
fn is_admin_now() -> bool {
    false
}

fn platform_info_impl() -> BackendResult<PlatformInfo> {
    if !cfg!(windows) {
        return Err(BackendError::unsupported(
            "skirr-windows",
            "not running on Windows",
        ));
    }
    Ok(PlatformInfo {
        os: "Windows".to_string(),
        os_version: "unknown".to_string(),
        kernel_version: None,
        architecture: std::env::consts::ARCH.to_string(),
        hostname: None,
        username: None,
        is_admin: is_admin_now(),
        is_virtual_machine: false,
        boot_time: None,
    })
}

fn enumerate_devices_impl() -> BackendResult<Vec<UsbDevice>> {
    #[cfg(windows)]
    {
        let raw = crate::native::win::enumerate()?;
        Ok(raw.iter().map(build_usb_device).collect())
    }
    #[cfg(not(windows))]
    {
        Err(BackendError::unsupported(
            "skirr-windows",
            "not running on Windows",
        ))
    }
}

/// Normalize one OS record into the core model.
///
/// Used on Windows builds by both collection paths and exercised in tests on
/// every platform; hence the allowance when `cfg(windows)` is false.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn build_usb_device(raw: &RawDeviceInfo) -> UsbDevice {
    // Prefer explicit hardware IDs, fall back to parsing the instance path.
    let hwid = raw
        .primary_hwid()
        .map(parse_hardware_id)
        .unwrap_or_default();
    let from_instance = parse_hardware_id(&raw.instance_id);
    let vid = hwid.vid.or(from_instance.vid).unwrap_or(0);
    let pid = hwid.pid.or(from_instance.pid).unwrap_or(0);

    // Composite devices expose per-interface instances (`&MI_xx`); strip to
    // the parent hardware ID portion so interfaces of one device share VID/PID.
    let composite = raw.instance_id.contains("&MI_");

    let class_name = raw.class_name.clone().unwrap_or_else(|| {
        raw.primary_hwid()
            .and_then(|h| h.split('\\').next().map(str::to_string))
            .unwrap_or_default()
    });

    let mut device = UsbDevice::new(vid, pid);
    device.platform_id = raw.instance_id.clone();
    device.manufacturer = raw.manufacturer.clone();
    device.product = raw.display_name().map(str::to_string);
    device.serial_number = extract_serial_from_instance(instance_serial_segment(&raw.instance_id));
    device.device_class = usb_class_from_windows_name(&class_name);
    device.is_hub = device.device_class == UsbClass::Hub || is_hub_name(&raw.instance_id);
    if composite {
        device.properties.insert("composite".into(), "true".into());
    }
    if let Some(class) = &raw.class_name {
        device
            .properties
            .insert("windows_class".into(), class.clone());
    }
    if let Some(status) = &raw.status {
        device.properties.insert("status".into(), status.clone());
    }
    device.connection_status = ConnectionStatus::Connected;
    device
}

/// Instance IDs look like `USB\VID_x&PID_y\<serial-or-generated>`; the last
/// segment carries serial/hub info.
#[allow(dead_code)]
fn instance_serial_segment(instance_id: &str) -> &str {
    instance_id.rsplit('\\').next().unwrap_or("")
}

#[allow(dead_code)]
fn is_hub_name(instance_id: &str) -> bool {
    let lower = instance_id.to_ascii_lowercase();
    lower.contains("hub") && lower.starts_with("usb\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{parse_powershell_json, split_multi_sz};
    use skirr_core::UsbClass;

    fn raw(instance_id: &str, hwids: &[&str]) -> RawDeviceInfo {
        RawDeviceInfo {
            instance_id: instance_id.to_string(),
            hardware_ids: hwids.iter().map(|s| s.to_string()).collect(),
            manufacturer: Some("ASUSTeK".to_string()),
            description: Some("USB Composite Device".to_string()),
            friendly_name: None,
            class_name: Some("USB".to_string()),
            status: Some("OK".to_string()),
        }
    }

    #[test]
    fn builds_device_from_setupapi_shape() {
        let r = raw(
            r"USB\VID_0B05&PID_1ACE\T6MPKRD00HWM5",
            &[r"USB\VID_0B05&PID_1ACE"],
        );
        let d = build_usb_device(&r);

        assert_eq!(d.vendor_id, 0x0B05);
        assert_eq!(d.product_id, 0x1ACE);
        assert_eq!(d.platform_id, r"USB\VID_0B05&PID_1ACE\T6MPKRD00HWM5");
        assert_eq!(d.serial_number.as_deref(), Some("T6MPKRD00HWM5"));
        assert_eq!(d.manufacturer.as_deref(), Some("ASUSTeK"));
        assert_eq!(d.connection_status, skirr_core::ConnectionStatus::Connected);
    }

    #[test]
    fn hub_detection_via_class_and_name() {
        let hub = raw(
            "USB\\VID_2109&PID_0817\\5&21B0E3D&0&3",
            &["USB\\VID_2109&PID_0817"],
        );
        let d = build_usb_device(&hub);
        // class "USB" maps to Hub bucket in our mapping; also name contains hub? no.
        assert_eq!(d.device_class, UsbClass::Hub);

        let named = RawDeviceInfo {
            friendly_name: Some("Generic USB Hub".into()),
            ..raw("USB\\VID_0000&PID_0000\\X", &[])
        };
        let d2 = build_usb_device(&named);
        assert!(d2.is_hub);
    }

    #[test]
    fn composite_marks_property_and_still_resolves_ids() {
        let r = raw(
            r"USB\VID_046D&PID_C52B&MI_03\6&2D6F9AB&0&0003",
            &[r"USB\VID_046D&PID_C52B&MI_03"],
        );
        let d = build_usb_device(&r);
        assert_eq!(d.vendor_id, 0x046D);
        assert_eq!(d.product_id, 0xC52B);
        assert_eq!(
            d.properties.get("composite").map(String::as_str),
            Some("true")
        );
        assert_eq!(d.serial_number, None); // generated instance, no serial
    }

    #[test]
    fn powershell_single_object_parses() {
        let json = r#"{"InstanceId":"USB\\VID_A&PID_B\\S1","FriendlyName":"Foo Cam","Manufacturer":"Acme","Class":"Camera","Status":"OK"}"#;
        let devices = parse_powershell_json(json).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].instance_id, r"USB\VID_A&PID_B\S1");
        assert_eq!(devices[0].friendly_name.as_deref(), Some("Foo Cam"));
        assert_eq!(devices[0].display_name(), Some("Foo Cam"));
    }

    #[test]
    fn powershell_array_and_empty_parse() {
        let arr = parse_powershell_json(
            r#"[{"InstanceId":"USB\\1"},{"InstanceId":"USB\\2","Status":"Error"}]"#,
        )
        .unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1].status.as_deref(), Some("Error"));

        assert!(parse_powershell_json("").unwrap().is_empty());
        assert!(parse_powershell_json("null").unwrap().is_empty());
        assert!(parse_powershell_json("{}").is_err());
    }

    #[test]
    fn multi_sz_splits() {
        let buf: Vec<u16> = "USB\\VID_X\0USB\\VID_Y\0\0".encode_utf16().collect();
        assert_eq!(split_multi_sz(&buf), vec!["USB\\VID_X", "USB\\VID_Y"]);
        assert!(split_multi_sz(&[0, 0]).is_empty());
    }
}
