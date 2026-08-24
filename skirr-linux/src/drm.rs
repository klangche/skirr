//! Display enumeration via DRM (DATA_MAP §7 Linux row).
//!
//! EDID parsing lives in `skirr_core::edid` (shared with macOS/Windows);
//! this module adds the `/sys/class/drm` walk, cfg(linux).
//! Connection-path correlation with USB topology is handled by
//! `correlate::displays_to_usb` in the backend.

#[cfg(target_os = "linux")]
use skirr_core::DisplayInfo;

/// Enumerate connected displays from `/sys/class/drm`.
#[cfg(target_os = "linux")]
pub fn enumerate_displays() -> skirr_core::BackendResult<Vec<DisplayInfo>> {
    use skirr_core::{edid, BackendError};
    use std::fs;

    const DRM_DIR: &str = "/sys/class/drm";
    let mut out = Vec::new();
    let entries = fs::read_dir(DRM_DIR).map_err(|e| {
        BackendError::os_api(crate::BACKEND_NAME, format!("read_dir {DRM_DIR}: {e}"))
    })?;

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Only connectors with per-connector dirs carry status/edid.
        if !name.contains('-') || !path.join("status").exists() {
            continue;
        }
        let status = fs::read_to_string(path.join("status")).unwrap_or_default();
        if status.trim() != "connected" {
            continue;
        }
        let raw_edid = fs::read(path.join("edid")).unwrap_or_default();
        if let Some(mut info) = edid::parse_edid(&raw_edid, name.to_string()) {
            info.hdr_supported = edid::detect_hdr_support(&raw_edid);
            out.push(info);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use skirr_core::edid;

    #[test]
    fn core_edid_parser_is_the_single_source() {
        // Guard that the shared parser still handles our Linux fixture
        // shape; full EDID coverage lives in skirr-core.
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        let info = edid::parse_edid(&e, "test".into()).expect("header accepted");
        // All-zero manufacturer word decodes to '@' padding — real EDIDs
        // always carry valid letters (covered in core's fixture).
        assert_eq!(info.manufacturer_id, Some("@@@".to_string()));
        assert!(info.preferred_resolution.is_none());
    }
}
