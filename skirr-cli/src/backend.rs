//! Platform backend dispatch: pick the backend matching the build target.
//! Target-gated dependencies mean each binary only carries its own platform's
//! backend; unsupported hosts get a clean error.

use skirr_core::{BackendResult, UsbBackend};

/// Construct the backend for the current platform.
pub fn create_backend() -> BackendResult<Box<dyn UsbBackend>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(skirr_macos::create_backend()))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(skirr_windows::create_backend())
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(skirr_linux::create_backend()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        use skirr_core::BackendError;
        Err(BackendError::unsupported(
            "skirr-cli",
            "no backend available for this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_dispatch_matches_host() {
        let result = create_backend();
        if cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        )) {
            let backend = result.expect("host platform has a backend");
            let expected = if cfg!(target_os = "macos") {
                "skirr-macos"
            } else if cfg!(target_os = "windows") {
                "skirr-windows"
            } else {
                "skirr-linux"
            };
            assert_eq!(backend.name(), expected);
        } else {
            assert!(result.is_err());
        }
    }
}
