//! Display enumeration for the macOS backend (DATA_MAP §7).
//!
//! `IODisplayConnect` records come from `native::displays()`; decoding is
//! the shared `skirr_core::edid` parser so all platforms agree on field
//! semantics. Displays with no EDID still appear (vendor/product codes
//! only) — honest partial data beats absence.

use crate::native::{self, DisplayRecord};
use skirr_core::{edid, BackendResult, DisplayInfo, DisplayType, SystemTopology};

/// Collect connected displays and attach them to the topology.
pub(crate) fn attach(topo: &mut SystemTopology) -> BackendResult<()> {
    topo.displays = collect()?;
    Ok(())
}

fn collect() -> BackendResult<Vec<DisplayInfo>> {
    let records = native::displays()?;
    Ok(records.iter().filter_map(record_to_display).collect())
}

/// Decode one record; `None` when even vendor/product are absent.
fn record_to_display(record: &DisplayRecord) -> Option<DisplayInfo> {
    if let Some(mut info) = edid::parse_edid(&record.edid_raw, record.service_name.clone()) {
        info.hdr_supported = edid::detect_hdr_support(&record.edid_raw);
        return Some(info);
    }
    // No usable EDID: fall back to registry id codes, folding VID/PID into
    // the platform id so the identification data isn't lost.
    let vid = record.vendor_id?;
    let pid = record.product_id?;
    Some(DisplayInfo {
        id: uuid::Uuid::new_v4(),
        platform_id: format!("{} [VID {vid:04x} PID {pid:04x}]", record.service_name),
        manufacturer_id: None,
        product_code: u16::try_from(pid).ok(),
        serial_number: None,
        manufacture_week: None,
        manufacture_year: None,
        edid_version: None,
        name: None,
        serial_number_str: None,
        max_horizontal_size_cm: None,
        max_vertical_size_cm: None,
        supported_resolutions: Vec::new(),
        preferred_resolution: None,
        current_resolution: None,
        refresh_rates: Vec::new(),
        current_refresh_rate: None,
        color_depth: None,
        hdr_supported: false,
        hdr_metadata: None,
        display_type: DisplayType::External,
        connection_type: None,
        gpu_id: None,
        usb_path: None,
        is_primary: false,
        is_internal: false,
        is_enabled: true,
        position: None,
        scale_factor: None,
        edid_raw: if record.edid_raw.is_empty() {
            None
        } else {
            Some(record.edid_raw.clone())
        },
    })
}

/// Non-macos build: no IOKit, nothing honest to report.
#[cfg(not(target_os = "macos"))]
pub(crate) fn attach(_topo: &mut SystemTopology) -> BackendResult<()> {
    Err(BackendError::unsupported(
        crate::BACKEND_NAME,
        "display enumeration requires macOS",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edid_record_decodes_with_hdr_flag() {
        let mut raw = super_test_edid();
        raw.extend_from_slice(&hdr_extension());
        let record = DisplayRecord {
            service_name: "Display0".into(),
            vendor_id: Some(0x0424),
            product_id: Some(0x1234),
            edid_raw: raw,
        };
        let info = record_to_display(&record).expect("decodes");
        assert_eq!(info.platform_id, "Display0");
        assert!(info.preferred_resolution.is_some());
        assert!(info.hdr_supported);
    }

    #[test]
    fn bare_ids_still_yield_minimal_info() {
        let record = DisplayRecord {
            service_name: "AppleInternal".into(),
            vendor_id: Some(0x610),
            product_id: Some(0xA031),
            edid_raw: Vec::new(),
        };
        let info = record_to_display(&record).expect("minimal info");
        assert_eq!(info.product_code, Some(0xA031));
        assert!(info.preferred_resolution.is_none());
    }

    #[test]
    fn empty_record_is_skipped() {
        let record = DisplayRecord {
            service_name: "Empty".into(),
            vendor_id: None,
            product_id: None,
            edid_raw: Vec::new(),
        };
        assert!(record_to_display(&record).is_none());
    }

    fn super_test_edid() -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        let w: u16 = (((b'S' - b'A' + 1) as u16) << 10) | (1u16 << 5) | ((b'M' - b'A' + 1) as u16);
        e[8..10].copy_from_slice(&w.to_be_bytes());
        e[54] = 0x01;
        e[55] = 0x1D;
        e[56] = 0x80;
        e[58] = 0x70;
        e[59] = 0x38;
        e[61] = 0x40;
        e
    }

    fn hdr_extension() -> Vec<u8> {
        let mut cta = vec![0u8; 128];
        cta[0] = 0x02;
        cta[2] = 8;
        cta[4] = 0xE3; // HDR SMD: tag 7, size 3
        cta
    }
}
