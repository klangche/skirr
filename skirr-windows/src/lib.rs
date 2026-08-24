//! Skirr Windows backend - PnP/SetupAPI enumeration, USB IOCTL speeds, hotplug monitoring.
//!
//! Implemented in Phase 2. This stub keeps the workspace building on all platforms.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkirrWindowsError {
    #[error("windows backend is not implemented yet")]
    NotImplemented,
}

pub type Result<T> = std::result::Result<T, SkirrWindowsError>;

/// Version marker for the Windows backend.
pub const BACKEND_NAME: &str = "skirr-windows";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_stable() {
        assert_eq!(BACKEND_NAME, "skirr-windows");
    }
}
