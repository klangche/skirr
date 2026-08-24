//! Skirr Windows backend - PnP/SetupAPI enumeration, USB IOCTL speeds, hotplug monitoring.
//!
//! Phase status: 2.1 enumeration implemented (SetupAPI native + PowerShell
//! fallback). Topology (2.2), speeds (2.3) and hotplug (2.4) still return
//! `BackendError::Unsupported`.
//!
//! All `windows`-crate usage lives behind `#[cfg(windows)]`; the parsing and
//! normalization layers compile everywhere so tests run on any host.

pub mod backend;
pub mod hwid;
mod native;

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
    fn unsupported_off_windows_or_pre_phase_methods_error_cleanly() {
        let b = create_backend();
        let topo = b.get_topology();
        assert!(topo.is_err());
        assert!(topo.unwrap_err().to_string().contains("2.2"));

        let speeds = b.get_speeds(uuid::Uuid::new_v4());
        assert!(speeds.is_err());

        // Off-Windows hosts must get Unsupported rather than panics.
        if !cfg!(windows) {
            let err = b.enumerate_devices().unwrap_err();
            assert!(err.to_string().contains("not running on Windows"));
            assert!(b.platform_info().is_err());
        }
    }
}
