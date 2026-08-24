//! Skirr macOS backend - IOKit/IORegistry enumeration, topology, speeds, hotplug monitoring.
//!
//! Phase status: 3.1-3.4 complete — enumeration, topology, speeds and
//! hotplug monitoring all implemented.
//!
//! All IOKit usage lives behind `#[cfg(target_os = "macos")]`; the parsing and
//! normalization layers compile everywhere so tests run on any host.

mod backend;
pub mod displays;
mod hotplug;
mod native;
mod speeds;
mod topology;
pub mod usb_c;

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
