//! skirr-linux backend: sysfs enumeration + topology + speeds + hotplug.

use crate::speeds;
#[allow(unused_imports)]
use crate::{drm, hotplug, native, topology};
use skirr_core::{
    BackendError, BackendResult, HotplugBackend, PlatformInfo, SpeedReport, SystemTopology,
    UsbBackend, UsbDevice,
};
use std::collections::HashMap;

pub struct SkirrLinuxBackend;

impl SkirrLinuxBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SkirrLinuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl UsbBackend for SkirrLinuxBackend {
    fn name(&self) -> &'static str {
        "skirr-linux"
    }

    fn platform_info(&self) -> BackendResult<PlatformInfo> {
        platform_info_impl()
    }

    fn enumerate_devices(&self) -> BackendResult<Vec<UsbDevice>> {
        #[cfg(target_os = "linux")]
        {
            Ok(native::enumerate()?
                .iter()
                .filter(|raw| !raw.is_root_hub())
                .map(topology::build_usb_device)
                .collect())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BackendError::unsupported(
                self.name(),
                "enumeration requires Linux",
            ))
        }
    }

    fn get_topology(&self) -> BackendResult<SystemTopology> {
        #[cfg(target_os = "linux")]
        {
            let raws = native::enumerate()?;
            let mut topo = topology::build(&raws, &pci_by_bus(), chrono::Utc::now());
            topo.platform_info = platform_info_impl()?;
            if let Ok(displays) = drm::enumerate_displays() {
                topo.displays = displays;
            }
            attach_type_c_info(&mut topo);
            skirr_core::docks::annotate_docks(&mut topo);
            // Best-effort like displays: no TB bus → empty list.
            topo.thunderbolt_routers = crate::thunderbolt::collect();
            Ok(topo)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BackendError::unsupported(
                self.name(),
                "topology requires Linux",
            ))
        }
    }

    fn get_speeds(&self, device_id: uuid::Uuid) -> BackendResult<SpeedReport> {
        let topo = self.get_topology()?;
        let dev = topo
            .devices
            .iter()
            .find(|d| d.id == device_id)
            .ok_or_else(|| {
                BackendError::unsupported("skirr-linux", format!("device {device_id} not present"))
            })?
            .clone();
        let (severity, _detail) = speeds::bottleneck_for(&dev)
            .unwrap_or((skirr_core::BottleneckSeverity::Minor, String::new()));
        Ok(SpeedReport {
            device_id,
            max_supported: dev.max_supported_speed,
            current_link: dev.current_link_speed,
            bottleneck: Some(skirr_core::SpeedBottleneck {
                device_id,
                max_speed: dev.max_supported_speed,
                current_speed: dev.current_link_speed,
                severity,
            }),
        })
    }

    fn monitor(&self) -> BackendResult<Box<dyn HotplugBackend + '_>> {
        Ok(Box::new(crate::monitor::PollMonitor::new()))
    }
}

/// Best-effort PCI address per bus from the root-hub symlink target.
#[cfg(target_os = "linux")]
#[allow(dead_code)] // used by get_topology on linux; clippy sees only non-linux cfg
fn pci_by_bus() -> HashMap<u8, String> {
    use std::fs;
    let mut out = HashMap::new();
    if let Ok(entries) = fs::read_dir("/sys/bus/usb/devices") {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(String::from) else {
                continue;
            };
            if !native::is_root_hub_name(&name) {
                continue;
            }
            let bus = name.strip_prefix("usb").and_then(|b| b.parse().ok());
            let Some(bus) = bus else { continue };
            // Symlink target contains .../pci0000:00/0000:00:14.0/usb3
            if let Ok(target) = fs::read_link(entry.path()) {
                let target_str = target.to_string_lossy().into_owned();
                let segs: Vec<&str> = target_str.split('/').collect();
                if let Some(pos) = segs.iter().position(|s| s.starts_with("usb")) {
                    if pos >= 1 {
                        let pci = segs[pos - 1];
                        if pci.contains(":") && pci.split(':').count() == 3 {
                            out.insert(bus, pci.to_string());
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)] // symmetric stub; real one is cfg-gated
fn pci_by_bus() -> HashMap<u8, String> {
    HashMap::new()
}

#[cfg(target_os = "linux")]
fn platform_info_impl() -> BackendResult<PlatformInfo> {
    use std::fs;

    let read = |path: &str| fs::read_to_string(path).ok();

    let os_pretty = read("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("PRETTY_NAME="))
        .map(|l| {
            l.trim_start_matches("PRETTY_NAME=")
                .trim_matches('"')
                .to_string()
        })
        .unwrap_or_else(|| "Linux".into());

    let kernel = read("/proc/sys/kernel/osrelease").map(|s| s.trim().to_string());

    let hostname = read("/proc/sys/kernel/hostname").map(|s| s.trim().to_string());

    let uid_zero = read("/proc/self/status").is_some_and(|status| {
        status.lines().any(|l| {
            l.starts_with("Uid:") && l.split_whitespace().nth(1).is_some_and(|uid| uid == "0")
        })
    });

    let dmi_product = read("/sys/class/dmi/id/product_name").unwrap_or_default();
    let is_vm = ["VirtualBox", "qemu", "KVM", "VMware", "Parallels"]
        .iter()
        .any(|marker| dmi_product.contains(marker));

    Ok(PlatformInfo {
        os: "Linux".into(),
        os_version: os_pretty,
        kernel_version: kernel,
        architecture: std::env::consts::ARCH.into(),
        hostname,
        username: std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("LOGNAME").ok()),
        is_admin: uid_zero,
        is_virtual_machine: is_vm,
        boot_time: read("/proc/stat")
            .and_then(|stat| {
                stat.lines()
                    .find(|l| l.starts_with("btime "))
                    .and_then(|l| l.split_whitespace().nth(1)?.parse::<i64>().ok())
            })
            .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0)),
    })
}

#[cfg(not(target_os = "linux"))]
fn platform_info_impl() -> BackendResult<PlatformInfo> {
    Err(BackendError::unsupported(
        "skirr-linux",
        "native sysfs access requires Linux",
    ))
}

/// Public constructor mirroring the other backends.
pub fn create_backend() -> SkirrLinuxBackend {
    SkirrLinuxBackend::new()
}

/// Correlate `/sys/class/typec` ports onto devices by syspath ancestry and
/// stamp `usb_c_info`. Purely additive: devices with no Type-C evidence are
/// left untouched (absence = no claim, per DATA_MAP §5 philosophy).
#[cfg(target_os = "linux")]
fn attach_type_c_info(topo: &mut SystemTopology) {
    use crate::usb_c;
    use std::fs;

    let Ok(typec_ports) = usb_c::enumerate_typec_ports() else {
        return;
    };
    if typec_ports.is_empty() {
        return;
    }

    for dev in &mut topo.devices {
        // The device's sysfs dir name is its location (`3-2.1`); find the
        // canonical path under /sys/devices to test ancestry against.
        let syspath = fs::canonicalize(format!(
            "/sys/bus/usb/devices/{}",
            dev.properties
                .get("sysfs_name")
                .map(String::as_str)
                .unwrap_or_default()
        ));
        let Ok(usb_path) = syspath else { continue };
        let usb_path = usb_path.to_string_lossy().to_string();

        for (attrs, port_path) in &typec_ports {
            if port_path.is_empty() || !usb_c::usb_path_under_typec_port(&usb_path, port_path) {
                continue;
            }
            let mut info = usb_c::build_port_info(attrs);
            // Partner alt modes when present.
            if let Ok(entries) = fs::read_dir(format!("/sys/class/typec/{}/partner", attrs.name)) {
                let names: Vec<String> = entries
                    .flatten()
                    .filter_map(|e| e.file_name().to_str().map(String::from))
                    .collect();
                info.alt_modes = usb_c::alt_modes_from_partner(&names);
            }
            info.pd_supported = matches!(info.current_mode, skirr_core::UsbCCurrentMode::UsbPd);
            dev.usb_c_info = Some(info);
            break;
        }
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
fn attach_type_c_info(_topo: &mut SystemTopology) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_stable() {
        assert_eq!(SkirrLinuxBackend::new().name(), "skirr-linux");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_access_is_cleanly_unsupported_off_linux() {
        assert!(platform_info_impl().is_err());
    }
}
