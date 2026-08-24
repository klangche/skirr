//! EDID base-block parsing, shared by every platform backend.
//!
//! Pure byte-level decoding (DATA_MAP §7): header validation, 5-bit-letter
//! manufacturer id, LE product/serial codes, manufacture date, physical
//! size, descriptor blocks (0xFC name / 0xFF serial string), and the
//! preferred resolution from the first detailed-timing block.
//!
//! Extension blocks (CTA-861/HDR) are parsed by [`parse_cta_extensions`]
//! on a best-effort basis only.

use crate::{DisplayInfo, DisplayResolution};

/// EDID v1 base block is 128 bytes with the fixed 00 FF … FF 00 header.
const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// Parse a base-block EDID into core `DisplayInfo` fields.
///
/// Returns None for anything that doesn't look like EDID — callers treat
/// that as "no display data available".
pub fn parse_edid(bytes: &[u8], platform_id: String) -> Option<DisplayInfo> {
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
        display_type: crate::DisplayType::External,
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
    // Layout: bytes 0-2 = 0x00, byte 3 = tag for text descriptors;
    // detailed timings have a nonzero pixel clock in bytes 0-1.
    for offset in [54usize, 72, 90, 108] {
        let desc = &bytes[offset..offset + 18];
        let is_text_tag = desc[0] == 0 && desc[1] == 0 && desc[2] == 0;
        match desc[3] {
            0xFC if is_text_tag => {
                info.name = Some(descriptor_text(desc));
            }
            0xFF if is_text_tag => {
                info.serial_number_str = Some(descriptor_text(desc));
            }
            _ if !is_text_tag => {
                if let Some(res) = parse_detailed_timing(desc) {
                    info.preferred_resolution = Some(res);
                }
            }
            _ => {}
        }
    }
    info.current_resolution = info.preferred_resolution.clone();
    Some(info)
}

/// Scan extension blocks for CTA-861 HDR static metadata. Base block is
/// 128 bytes; each following block is another 128 with a tag in byte 0.
/// Returns true when an extension advertises HDR support (SMD present).
pub fn detect_hdr_support(edid: &[u8]) -> bool {
    let mut offset = 128usize;
    while offset + 128 <= edid.len() {
        let block = &edid[offset..offset + 128];
        if block[0] == 0x02 {
            // CTA-861 extension
            // Data block collection starts at byte 4; walk data blocks
            // looking for the HDR Static Metadata Data Block (tag 0x7).
            let mut pos = 4usize;
            let end = block[2].min(127) as usize;
            while pos < end {
                let db = block[pos];
                let tag = db >> 5;
                let size = (db & 0x1F) as usize;
                if tag == 0x07 && size >= 3 && pos + 1 + size < 128 {
                    return true;
                }
                pos += 1 + size;
            }
        } else if block[0] == 0x00 {
            break; // no more extensions
        }
        offset += 128;
    }
    false
}

fn descriptor_text(desc: &[u8]) -> String {
    desc[5..18]
        .iter()
        .filter(|&&b| b.is_ascii_graphic() || b == b' ')
        .map(|&b| b as char)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Active pixels/lines from a detailed timing descriptor.
fn parse_detailed_timing(desc: &[u8]) -> Option<DisplayResolution> {
    let h_active = u16::from(desc[4] & 0xF0) << 4 | u16::from(desc[2]);
    let v_active = u16::from(desc[7] & 0xF0) << 4 | u16::from(desc[5]);
    if h_active == 0 || v_active == 0 {
        return None;
    }
    Some(DisplayResolution {
        width: h_active,
        height: v_active,
        is_interlaced: desc[17] & 0x80 != 0,
        aspect_ratio: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid EDID base block: SAM manufacturer, product 0x1234,
    /// serial 0xDEADBEEF, 1.4, 60x34 cm, detailed timing 1920x1080,
    /// name descriptor "U28E590".
    pub(crate) fn fixture_edid() -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&HEADER);
        let w: u16 = (((b'S' - b'A' + 1) as u16) << 10) | (1u16 << 5) | ((b'M' - b'A' + 1) as u16);
        e[8..10].copy_from_slice(&w.to_be_bytes());
        e[10..12].copy_from_slice(&0x1234u16.to_le_bytes());
        e[12..16].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        e[16] = 26;
        e[17] = (2026 - 1990) as u8;
        e[18] = 1;
        e[19] = 4;
        e[21] = 60;
        e[22] = 34;

        // Detailed timing at 54: 1920x1080 (low byte + high nibble split).
        e[54] = 0x01;
        e[55] = 0x1D;
        e[56] = 0x80;
        e[58] = 0x70;
        e[59] = 0x38;
        e[61] = 0x40;
        // Blank padding inside the timing descriptor only; the name
        // descriptor at 72 must keep its zero header bytes.
        e[59..72].fill(0x20);
        e[59] = 0x38;
        e[61] = 0x40;

        // Name descriptor at 72 (bytes 0-2 zero, byte 3 tag, byte 4 zero,
        // text from byte 5).
        e[75] = 0xFC;
        e[77..87].copy_from_slice(b"U28E590\0\0\n");
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
        assert_eq!(info.max_vertical_size_cm, Some(34));
        assert_eq!(info.manufacture_year, Some(2026));
        assert_eq!(info.name.as_deref(), Some("U28E590"));
        let pref = info.preferred_resolution.expect("timing");
        assert_eq!((pref.width, pref.height), (1920, 1080));
        let cur = info.current_resolution.expect("current mirrors preferred");
        assert_eq!((cur.width, cur.height), (1920, 1080));
        assert!(info.edid_raw.is_some());
    }

    #[test]
    fn rejects_non_edid_bytes() {
        assert!(parse_edid(&[], "x".into()).is_none());
        assert!(parse_edid(&[0u8; 128], "x".into()).is_none());
        let mut short = fixture_edid();
        short.truncate(64);
        assert!(parse_edid(&short, "x".into()).is_none());
    }

    #[test]
    fn hdr_detected_only_with_smd_data_block() {
        let mut edid = fixture_edid();

        // No extensions → no HDR.
        assert!(!detect_hdr_support(&edid));

        // Append a CTA-861 extension carrying an HDR SMD data block
        // (tag 7 in the top 3 bits, size 3 in the low 5 → 0xE3).
        let mut cta = vec![0u8; 128];
        cta[0] = 0x02;
        cta[2] = 8; // data bytes end
        cta[4] = 0xE3; // tag 7, size 3
        cta[5..8].copy_from_slice(&[0x00, 0x30, 0x40]); // SMD payload
        edid.extend_from_slice(&cta);

        assert!(detect_hdr_support(&edid));
    }

    #[test]
    fn truncated_extension_is_safe() {
        let mut edid = fixture_edid();
        edid.extend_from_slice(&[0x02, 0x00]);
        assert!(!detect_hdr_support(&edid));
    }
}
