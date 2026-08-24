//! Skirr Linux backend — sysfs enumeration, topology, speeds, hotplug.
//!
//! Pure parsing logic (sysfs names/attrs, EDID bytes) is testable on any
//! host; the `/sys` walks are `#[cfg(target_os = "linux")]`.
//!
//! Phase 6.1–6.5 complete. udev netlink push events and rusb descriptor
//! walks are documented future enhancements; polling-diff covers the MVP.

pub mod backend;
pub mod drm;
pub mod hotplug;
mod monitor;
pub mod native;
pub mod speeds;
pub mod thunderbolt;
pub mod topology;
pub mod usb_c;

pub use backend::{create_backend, SkirrLinuxBackend};

/// Version marker for the Linux backend.
pub const BACKEND_NAME: &str = "skirr-linux";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_stable() {
        assert_eq!(BACKEND_NAME, "skirr-linux");
    }
}
