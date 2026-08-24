//! Display enumeration via DRM (DATA_MAP §7 Linux row).
//!
//! EDID parsing is pure (testable anywhere); the `/sys/class/drm` walk is
//! `#[cfg(target_os = "linux")]`. Connection-path correlation with USB
//! topology lands in Phase 7 (USB-C / DP alt mode).

use skirr_core::{DisplayInfo, DisplayResolution};

/// Parse base-block EDID bytes into core DisplayInfo fields.
///
/// Returns None for anything that doesn't look like EDID (missing 8-byte
/// header) — callers treat that as "no display data available".
pub fn parse_edid(bytes: &[u8], platform_id: String) -> Option<DisplayInfo> {
    const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if bytes.len() < 128 || bytes[..8] != HEADER {
        return None;
    }

    // Manufacturer id: big-endian word, three 5-bit letters.
    let mfg_word = u16::from_be_bytes([bytes[8], bytes[9]]);
    let letter = |shift: u16| ((mfg_word >> shift) & 0x1F) as u8 + b'A' - 1;
    let manufacturer_id = format!(
        "{}{}{}",
        letter(10) as char,
        letter(5) as char,
        letter(0) as char
    );

    let mut info = DisplayInfo {
        id: uuid::Uuid::new_v4(),
        platform_id,
        manufacturer_id: Some(manufacturer_id),
        product_code: Some(u16::from_le_bytes([bytes[10], bytes[11]])),
        serial_number: Some(u32::from_le_bytes([
            bytes[12], bytes[13], bytes[14], bytes[15],
        ])),
        manufacture_week: Some(bytes[16]),
        manufacture_year: Some(u16::from(bytes[17]) + 1990),
        edid_version: Some(format!("{}.{}", bytes[18], bytes[19])),
        name: None,
        serial_number_str: None,
        max_horizontal_size_cm: Some(u16::from(bytes[21])),
        max_vertical_size_cm: Some(u16::from(bytes[22])),
        supported_resolutions: Vec::new(),
        preferred_resolution: None,
        current_resolution: None,
        refresh_rates: Vec::new(),
        current_refresh_rate: None,
        color_depth: None,
        hdr_supported: false,
        hdr_metadata: None,
        display_type: skirr_core::DisplayType::External,
        connection_type: None,
        gpu_id: None,
        usb_path: None,
        is_primary: false,
        is_internal: false,
        is_enabled: true,
        position: None,
        scale_factor: None,
        edid_raw: Some(bytes.to_vec()),
    };

    // Descriptor blocks at fixed offsets: name (0xFC), serial string (0xFF),
    // first detailed timing supplies the preferred resolution.
    for offset in [54usize, 72, 90, 108] {
        let desc = &bytes[offset..offset + 18];
        match desc[0] {
            0xFC => {
                let text: String = desc[5..18]
                    .iter()
                    .filter(|&&b| b != b'\n' && b != 0x00 && b.is_ascii_graphic() || b == b' ')
                    .map(|&b| b as char)
                    .collect();
                info.name = Some(text.trim().to_string());
            }
            0xFF => {
                let text: String = desc[5..18]
                    .iter()
                    .filter(|&&b| b.is_ascii_graphic())
                    .map(|&b| b as char)
                    .collect();
                info.serial_number_str = Some(text.trim().to_string());
            }
            _ if desc[0] != 0x00 || desc[1] != 0x00 => {
                // Detailed timing descriptor: active pixels/lines.
                let h_active = (u16::from(desc[4] & 0xF0) << 4) | u16::from(desc[2]);
                let v_active = (u16::from(desc[7] & 0xF0) << 4) | u16::from(desc[5]);
                if h_active > 0 && v_active > 0 {
                    info.preferred_resolution = Some(DisplayResolution {
                        width: h_active,
                        height: v_active,
                        is_interlaced: desc[17] & 0x80 != 0,
                        aspect_ratio: None,
                    });
                }
            }
            _ => {}
        }
    }
    info.current_resolution = info.preferred_resolution.clone();
    Some(info)
}

/// Enumerate connected displays from `/sys/class/drm`.
#[cfg(target_os = "linux")]
pub fn enumerate_displays() -> skirr_core::BackendResult<Vec<DisplayInfo>> {
    use skirr_core::BackendError;
    use std::fs;

    const DRM_DIR: &str = "/sys/class/drm";
    let mut out = Vec::new();
    let entries = fs::read_dir(DRM_DIR)
        .map_err(|e| BackendError::os_api("skirr-linux", format!("read_dir {DRM_DIR}: {e}")))?;

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
        let edid = fs::read(path.join("edid")).unwrap_or_default();
        if let Some(info) = parse_edid(&edid, name.to_string()) {
            out.push(info);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid EDID base block with a detailed timing of 1920x1080
    /// and a name descriptor.
    fn fixture_edid() -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // Manufacturer "SAM" (Samsung-style encoding).
        let w: u16 = (((b'S' - b'A' + 1) as u16) << 10) | (1u16 << 5) | ((b'M' - b'A' + 1) as u16);
        e[8..10].copy_from_slice(&w.to_be_bytes());
        e[10..12].copy_from_slice(&0x1234u16.to_le_bytes());
        e[12..16].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        e[16] = 26; // week
        e[17] = (2026 - 1990) as u8; // year
        e[18] = 1;
        e[19] = 4;
        e[21] = 60; // h size cm
        e[22] = 34; // v size cm

        // Detailed timing at offset 54: pixel clock nonzero, 1920x1080.
        // desc[i] = e[54+i]; active sizes split low byte + high nibble.
        // hactive 1920 = 0x780 → desc[2]=0x80, desc[4] hi nibble 0x07
        // vactive 1080 = 0x438 → desc[5]=0x38, desc[7] hi nibble 0x04
        e[54] = 0x01;
        e[55] = 0x1D;
        e[56] = 0x80;
        e[58] = 0x70;
        e[59] = 0x38;
        e[61] = 0x40;
        e[59..73].fill(0x20);
        e[59] = 0x38;
        e[61] = 0x40;
        e[72..74].copy_from_slice(&[0xFC, 0x00]); // name tag? no—tag byte is desc[0]
        e[72] = 0x00;
        e[73] = 0x00;
        e[74] = 0x00;
        e[75] = 0xFC;
        e[76..86].copy_from_slice(b"U28E590\0\0\n");
        e
    }

    #[test]
    fn parses_header_manufacturer_and_timing() {
        let info = parse_edid(&fixture_edid(), "card0-HDMI-A-1".into()).expect("parses");
        assert_eq!(info.manufacturer_id.as_deref(), Some("SAM"));
        assert_eq!(info.product_code, Some(0x1234));
        assert_eq!(info.serial_number, Some(0xDEAD_BEEF));
        assert_eq!(info.edid_version.as_deref(), Some("1.4"));
        assert_eq!(info.max_horizontal_size_cm, Some(60));
        assert_eq!(info.manufacture_year, Some(2026));
        let pref = info.preferred_resolution.expect("timing");
        assert_eq!((pref.width, pref.height), (1920, 1080));
    }

    #[test]
    fn rejects_non_edid_bytes() {
        assert!(parse_edid(&[], "x".into()).is_none());
        assert!(parse_edid(&[0u8; 128], "x".into()).is_none());
        let mut short = fixture_edid();
        short.truncate(64);
        assert!(parse_edid(&short, "x".into()).is_none());
    }
}
