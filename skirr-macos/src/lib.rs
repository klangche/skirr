//! Skirr macOS backend - IOKit/IORegistry enumeration, speed detection, hotplug monitoring.
//!
//! Implemented in Phase 3. This stub keeps the workspace building on all platforms.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkirrMacOsError {
    #[error("macos backend is not implemented yet")]
    NotImplemented,
}

pub type Result<T> = std::result::Result<T, SkirrMacOsError>;

/// Version marker for the macOS backend.
pub const BACKEND_NAME: &str = "skirr-macos";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_stable() {
        assert_eq!(BACKEND_NAME, "skirr-macos");
    }
}
