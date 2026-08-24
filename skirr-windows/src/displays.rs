//! Display enumeration for the Windows backend (DATA_MAP §7).
//!
//! Present monitors come from SetupAPI (`GUID_DEVCLASS_MONITOR`); EDID
//! bytes are read from the driver key's `Device Parameters\EDID` registry
//! value. Decoding is the shared `skirr_core::edid` parser. The
//! DIGCF_PRESENT filter keeps disconnected ghosts out — stale Enum
//! entries are ignored, never reported.

#[cfg(not(windows))]
use skirr_core::{edid, BackendError, BackendResult, DisplayInfo, SystemTopology};
#[cfg(windows)]
use skirr_core::{edid, BackendResult, DisplayInfo, SystemTopology};

/// One present display as read from SetupAPI + registry.
/// Public so the pure decode path is exercised by tests on every host.
#[derive(Debug, Clone)]
pub struct DisplayRecord {
    pub instance_id: String,
    pub edid_raw: Vec<u8>,
}

/// Collect connected displays and attach them to the topology.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn attach(topo: &mut SystemTopology) -> BackendResult<()> {
    topo.displays = collect()?;
    Ok(())
}

#[cfg(windows)]
fn collect() -> BackendResult<Vec<DisplayInfo>> {
    let records = crate::native::win::display_records()?;
    Ok(records.iter().filter_map(record_to_display).collect())
}

/// Decode one record; public for cross-platform test coverage.
pub fn record_to_display(record: &DisplayRecord) -> Option<DisplayInfo> {
    let mut info = edid::parse_edid(&record.edid_raw, record.instance_id.clone())?;
    info.hdr_supported = edid::detect_hdr_support(&record.edid_raw);
    Some(info)
}

/// Non-windows build: no SetupAPI/registry, nothing honest to report.
#[cfg(not(windows))]
fn collect() -> BackendResult<Vec<DisplayInfo>> {
    Err(BackendError::unsupported(
        crate::BACKEND_NAME,
        "display enumeration requires Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edid_record_decodes_with_hdr_flag() {
        let mut raw = fixture_edid();
        raw.extend_from_slice(&hdr_extension());
        let record = DisplayRecord {
            instance_id: r"DISPLAY\SAM0E16\5&2a4b&0&UID256".into(),
            edid_raw: raw,
        };
        let info = record_to_display(&record).expect("decodes");
        assert!(info.platform_id.contains("SAM"));
        assert!(info.preferred_resolution.is_some());
        assert!(info.hdr_supported);
    }

    #[test]
    fn garbage_edid_is_skipped() {
        let record = DisplayRecord {
            instance_id: "DISPLAY\\X\\1".into(),
            edid_raw: vec![0u8; 128],
        };
        assert!(record_to_display(&record).is_none());
    }

    fn fixture_edid() -> Vec<u8> {
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
