//! Skirr macOS backend - IOKit/IORegistry enumeration, speed detection, hotplug monitoring.
//!
//! Phase status: 3.1 enumeration + 3.2 topology implemented (IOKit primary,
//! system_profiler fallback). Speeds (3.3) and hotplug (3.4) still return
//! `BackendError::Unsupported`.
//!
//! All IOKit usage lives behind `#[cfg(target_os = "macos")]`; the parsing and
//! normalization layers compile everywhere so tests run on any host.

mod backend;
mod native;
mod topology;

/// Version marker for the macOS backend.
pub const BACKEND_NAME: &str = "skirr-macos";

/// Construct the platform backend instance.
pub fn create_backend() -> backend::SkirrMacosBackend {
    backend::SkirrMacosBackend::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_stable() {
        assert_eq!(BACKEND_NAME, "skirr-macos");
    }
}
