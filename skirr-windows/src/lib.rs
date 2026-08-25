//! Skirr Windows backend - PnP/SetupAPI enumeration, topology, USB IOCTL speeds, hotplug monitoring.
//!
//! Phase status: 2.1 enumeration + 2.2 topology implemented (SetupAPI native,
//! PowerShell fallback). Speeds (2.3) and hotplug (2.4) still return
//! `BackendError::Unsupported`.
//!
//! All `windows`-crate usage lives behind `#[cfg(windows)]`; the parsing and
//! normalization layers compile everywhere so tests run on any host.

pub mod backend;
pub mod displays;
mod hotplug;
pub mod hwid;
mod native;
mod speeds;
mod topology;
pub mod usb_c;

pub use backend::SkirrWindowsBackend;
pub use hwid::{parse_hardware_id, parse_instance_path, HardwareId, InstancePathInfo};

/// Backend identifier.
pub const BACKEND_NAME: &str = "skirr-windows";

/// Construct the platform backend as a trait object.
pub fn create_backend() -> Box<dyn skirr_core::UsbBackend> {
    Box::new(SkirrWindowsBackend::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_stable() {
        assert_eq!(BACKEND_NAME, "skirr-windows");
        assert_eq!(create_backend().name(), "skirr-windows");
    }

    #[test]
    fn unsupported_methods_error_cleanly() {
        let b = create_backend();
        let topo = b.get_topology();
        let speeds = b.get_speeds(uuid::Uuid::new_v4());

        if cfg!(windows) {
            // On real Windows the topology call may succeed (CI runners
            // have SetupAPI access) or fail — both are acceptable.
            let _ = topo;
            // Speeds may also succeed on real Windows.
            let _ = speeds;
        } else {
            // Off-Windows must get Unsupported rather than panics.
            assert!(topo.is_err());
            assert!(topo.unwrap_err().to_string().contains("Windows"));
            assert!(speeds.is_err());
            assert!(b.platform_info().is_err());
            assert!(b.enumerate_devices().is_err());
        }
    }
}
