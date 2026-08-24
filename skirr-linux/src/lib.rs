//! Skirr Linux backend - sysfs/libusb/udev enumeration and hotplug monitoring.
//!
//! Implemented in Phase 6. This stub keeps the workspace building on all platforms.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkirrLinuxError {
    #[error("linux backend is not implemented yet")]
    NotImplemented,
}

pub type Result<T> = std::result::Result<T, SkirrLinuxError>;

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
