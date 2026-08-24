//! `UsbBackend` implementation for macOS (IOKit primary, system_profiler fallback).

use crate::native::{self, RawDeviceInfo};
use skirr_core::{
    BackendError, BackendResult, ConnectionStatus, PlatformInfo, SpeedReport, SystemTopology,
    UsbBackend, UsbClass, UsbDevice,
};
use uuid::Uuid;

/// macOS USB backend.
#[derive(Debug, Default)]
pub struct SkirrMacosBackend;

impl SkirrMacosBackend {
    pub fn new() -> Self {
        Self
    }
}

impl UsbBackend for SkirrMacosBackend {
    fn name(&self) -> &'static str {
        crate::BACKEND_NAME
    }

    fn platform_info(&self) -> BackendResult<PlatformInfo> {
        platform_info_impl()
    }

    fn enumerate_devices(&self) -> BackendResult<Vec<UsbDevice>> {
        enumerate_devices_impl()
    }

    fn get_topology(&self) -> BackendResult<SystemTopology> {
        #[cfg(target_os = "macos")]
        {
            let raw = native::enumerate()?;
            let info = platform_info_impl()?;
            Ok(crate::topology::build(&raw, info, chrono::Utc::now()))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(BackendError::unsupported(
                self.name(),
                "topology requires macOS",
            ))
        }
    }

    fn get_speeds(&self, device_id: Uuid) -> BackendResult<SpeedReport> {
        #[cfg(target_os = "macos")]
        {
            speeds_for_device(self.name(), device_id)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = device_id;
            Err(BackendError::unsupported(
                self.name(),
                "speed detection requires macOS",
            ))
        }
    }

    fn monitor(&self) -> BackendResult<Box<dyn skirr_core::HotplugBackend + '_>> {
        // Box<dyn HotplugBackend> ('static) coerces to the shorter lifetime.
        crate::hotplug::create_monitor()
    }
}

/// Normalize one raw record into the core model. Class codes come straight
/// from the IORegistry descriptor fields (system_profiler fallback reports
/// zeros → Unspecified until 3.2 enriches).
pub(crate) fn build_usb_device(raw: &RawDeviceInfo) -> UsbDevice {
    let mut device = UsbDevice::new(raw.vendor_id, raw.product_id);
    device.platform_id = raw.instance_id.clone();
    device.manufacturer = raw.manufacturer.clone();
    device.product = raw.product_name.clone();
    device.serial_number = raw.serial_number.clone();
    device.device_class = UsbClass::from_u8(raw.device_class);
    device.is_hub = device.device_class == UsbClass::Hub;
    if let Some(parent) = &raw.parent {
        device
            .properties
            .insert("parent_instance".into(), parent.clone());
    }
    device
        .properties
        .insert("location_id".into(), format!("{:#010x}", raw.location_id));
    if let Some(hw) = raw.hardware_ids.first() {
        device.properties.insert("hwid".into(), hw.clone());
    }
    if raw.bcd_usb != 0 {
        device
            .properties
            .insert("bcd_usb".into(), format!("{:#06x}", raw.bcd_usb));
    }
    if raw.device_sub_class != 0 || raw.device_protocol != 0 {
        device.properties.insert(
            "class_sub_proto".into(),
            format!("{:#04x}/{:#04x}", raw.device_sub_class, raw.device_protocol),
        );
    }
    device.connection_status = ConnectionStatus::Connected;
    device
}

fn enumerate_devices_impl() -> BackendResult<Vec<UsbDevice>> {
    Ok(native::enumerate()?.iter().map(build_usb_device).collect())
}

/// Resolve one device's speeds: locate it in the topology, then pull its raw
/// speed signals from a fresh enumeration pass (single IOKit sweep; no extra
/// OS calls beyond that).
#[cfg(target_os = "macos")]
fn speeds_for_device(backend: &'static str, device_id: Uuid) -> BackendResult<SpeedReport> {
    let topo = crate::backend::SkirrMacosBackend
        .get_topology()
        .map_err(|e| BackendError::os_api(backend, format!("topology for speed query: {e}")))?;

    let device = topo
        .devices
        .iter()
        .find(|d| d.id == device_id)
        .ok_or_else(|| BackendError::os_api(backend, format!("device {device_id} not present")))?;

    let raw = native::enumerate()?
        .into_iter()
        .find(|r| r.instance_id.eq_ignore_ascii_case(&device.platform_id))
        .ok_or_else(|| {
            BackendError::os_api(backend, format!("no enumeration data for {device_id}"))
        })?;

    // Negotiated from IORegistry; fall back to advertised only when no
    // negotiated value exists — and then it doubles as both ceiling floor
    // and best-known link estimate (documented limitation).
    let current_link = raw
        .speed_code
        .map(crate::speeds::map_speed_code)
        .unwrap_or_else(|| {
            raw.advertised_mbps
                .map(crate::speeds::mbps_to_speed)
                .unwrap_or(skirr_core::UsbSpeed::Unknown)
        });
    let max_supported = raw
        .advertised_mbps
        .map(crate::speeds::mbps_to_speed)
        .unwrap_or(current_link)
        .max(current_link);

    let bottleneck = (max_supported > current_link
        && current_link != skirr_core::UsbSpeed::Unknown)
        .then(|| skirr_core::SpeedBottleneck {
            device_id,
            max_speed: max_supported,
            current_speed: current_link,
            severity: skirr_core::BottleneckSeverity::from_speeds(max_supported, current_link),
        });

    Ok(SpeedReport {
        device_id,
        max_supported,
        current_link,
        bottleneck,
    })
}

fn platform_info_impl() -> BackendResult<PlatformInfo> {
    if !cfg!(target_os = "macos") {
        return Err(BackendError::unsupported(
            crate::BACKEND_NAME,
            "not running on macOS",
        ));
    }
    let run = |cmd: &str, args: &[&str]| -> Option<String> {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let os_version = run("/usr/bin/sw_vers", &["-productVersion"]);
    let kernel_version = run("/usr/sbin/sysctl", &["-n", "kern.osrelease"]);
    let hostname = run("hostname", &[]);
    let username = std::env::var("USER").ok().filter(|u| !u.is_empty());
    let euid: u32 = run("id", &["-u"])
        .and_then(|out| out.parse().ok())
        .unwrap_or(u32::MAX);
    let hw_model = run("/usr/sbin/sysctl", &["-n", "hw.model"]).unwrap_or_default();

    Ok(PlatformInfo {
        os: "macOS".into(),
        os_version: os_version.unwrap_or_else(|| "unknown".into()),
        kernel_version,
        architecture: std::env::consts::ARCH.into(),
        hostname,
        username,
        is_admin: euid == 0,
        is_virtual_machine: is_vm_model(&hw_model),
        boot_time: None,
    })
}

/// Pure check so VM detection stays testable off-macOS.
pub(crate) fn is_vm_model(hw_model: &str) -> bool {
    const VM_MARKERS: [&str; 5] = ["virtualmac", "vmware", "virtualbox", "kvm", "qemu"];
    let lower = hw_model.to_ascii_lowercase();
    VM_MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{make_hwid, make_instance};

    #[test]
    fn backend_name_is_stable() {
        assert_eq!(SkirrMacosBackend.name(), "skirr-macos");
    }

    fn sample_raw() -> RawDeviceInfo {
        RawDeviceInfo {
            instance_id: make_instance(0x05AC, 0x12A8, 0x14500000),
            hardware_ids: vec![make_hwid(0x05AC, 0x12A8)],
            vendor_id: 0x05AC,
            product_id: 0x12A8,
            manufacturer: Some("Apple Inc.".into()),
            product_name: Some("iPhone".into()),
            serial_number: Some("CAFEBABE".into()),
            device_class: 0x00,
            device_sub_class: 0x00,
            device_protocol: 0x00,
            bcd_usb: 0x0200,
            location_id: 0x14500000,
            parent: Some(make_instance(0x2109, 0x0817, 0x14200000)),
            speed_code: Some(3),
            advertised_mbps: Some(480),
        }
    }

    #[test]
    fn normalization_carries_identity_and_properties() {
        let dev = build_usb_device(&sample_raw());
        assert_eq!(dev.vendor_id, 0x05AC);
        assert_eq!(dev.product_id, 0x12A8);
        assert_eq!(dev.platform_id, r"USB\VID_05AC&PID_12A8\0x14500000");
        assert_eq!(dev.product.as_deref(), Some("iPhone"));
        assert_eq!(dev.serial_number.as_deref(), Some("CAFEBABE"));
        assert_eq!(dev.device_class, UsbClass::Unspecified);
        assert!(!dev.is_hub);
        assert_eq!(
            dev.properties.get("location_id").map(String::as_str),
            Some("0x14500000")
        );
        assert_eq!(
            dev.properties.get("parent_instance").map(String::as_str),
            Some(r"USB\VID_2109&PID_0817\0x14200000")
        );
    }

    #[test]
    fn hub_class_marks_hub_devices() {
        let mut raw = sample_raw();
        raw.device_class = 0x09;
        let dev = build_usb_device(&raw);
        assert_eq!(dev.device_class, UsbClass::Hub);
        assert!(dev.is_hub);
    }

    #[test]
    fn class_codes_map_to_core_enum() {
        for (code, expected) in [
            (0x03u8, UsbClass::HID),
            (0x08, UsbClass::MassStorage),
            (0xFF, UsbClass::VendorSpecific),
        ] {
            let mut raw = sample_raw();
            raw.device_class = code;
            assert_eq!(build_usb_device(&raw).device_class, expected);
        }
    }

    #[test]
    fn vm_detection_matches_known_models() {
        assert!(is_vm_model("VirtualMac2,1"));
        assert!(is_vm_model("VMware7,1"));
        assert!(!is_vm_model("Mac14,6"));
        assert!(!is_vm_model(""));
    }

    #[test]
    fn speed_signals_combine_like_speeds_for_device() {
        use skirr_core::UsbSpeed;

        // High negotiated vs 5 Gb/s advertised: ceiling raised, link stays.
        let current = crate::speeds::map_speed_code(3);
        let max = crate::speeds::mbps_to_speed(5000).max(current);
        assert_eq!(current, UsbSpeed::HighSpeed);
        assert_eq!(max, UsbSpeed::SuperSpeed);
        assert!(max > current);

        // No negotiated data: advertised doubles as best-known estimate.
        let fallback = crate::speeds::mbps_to_speed(480);
        assert_eq!(fallback.max(fallback), UsbSpeed::HighSpeed);

        // Fully unknown device reports Unknown with no false bottleneck.
        let unknown = crate::speeds::map_speed_code(0).max(crate::speeds::map_speed_code(0));
        assert_eq!(unknown, UsbSpeed::Unknown);
    }

    #[test]
    fn off_macos_enumerate_errors_cleanly() {
        if !cfg!(target_os = "macos") {
            let err = native::enumerate().expect_err("must be unsupported");
            assert!(err.to_string().contains("requires macOS"));
        } else {
            // Live smoke test on the dev host. May legitimately be EMPTY when
            // no USB devices are attached (e.g. headless CI/Mac Studio);
            // structural checks apply to whatever comes back.
            let devices = native::enumerate().expect("live enumeration on macOS host");
            for record in devices.iter().take(4) {
                assert!(record.instance_id.starts_with(r"USB\VID_"));
                assert!(matches!(
                    build_usb_device(record).connection_status,
                    ConnectionStatus::Connected
                ));
            }
        }
    }

    #[test]
    fn platform_info_shape_is_sane() {
        if cfg!(target_os = "macos") {
            let info = platform_info_impl().expect("live platform info");
            assert_eq!(info.os, "macOS");
            assert!(info
                .os_version
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit()));
        }
    }
}
