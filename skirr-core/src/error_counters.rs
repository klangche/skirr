//! Device-level error counters and link diagnostics (Phase 11.3).
//!
//! Primary source: our own `DiagnosticEvent` stream (re-enumerations,
//! speed changes, errors, warnings are all observed by the monitor).
//! Secondary (Linux only): dwc3 debugfs LTSSM state — only readable
//! when running as root with debugfs mounted; silently skipped
//! otherwise.

use crate::{DiagnosticEvent, EventType, UsbDevice};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Per-device error/event statistics aggregated from the monitoring session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceErrorStats {
    pub device_id: uuid::Uuid,
    pub device_name: String,
    /// Number of Error-severity events attributed to this device.
    pub error_events: u32,
    /// Number of re-enumerations observed.
    pub re_enumerations: u32,
    /// Number of reconnects (DeviceConnected after disconnect).
    pub reconnects: u32,
    /// Link error count (Linux debugfs when available; None elsewhere).
    pub link_errors: Option<u64>,
    /// CRC error count (Linux debugfs when available; None elsewhere).
    pub crc_errors: Option<u64>,
    /// LTSSM state (Linux dwc3 debugfs when present).
    pub ltssm_state: Option<String>,
}

/// Aggregate error stats for all devices observed in a session.
pub fn summarize_device_errors(
    devices: &[UsbDevice],
    events: &[DiagnosticEvent],
) -> Vec<DeviceErrorStats> {
    let mut by_id: HashMap<uuid::Uuid, DeviceErrorStats> = devices
        .iter()
        .map(|d| {
            (
                d.id,
                DeviceErrorStats {
                    device_id: d.id,
                    device_name: d.product.clone().unwrap_or_else(|| d.platform_id.clone()),
                    error_events: 0,
                    re_enumerations: 0,
                    reconnects: 0,
                    link_errors: None,
                    crc_errors: None,
                    ltssm_state: None,
                },
            )
        })
        .collect();

    for event in events {
        let Some(dev_id) = event.device_id else {
            continue;
        };
        if let Some(stats) = by_id.get_mut(&dev_id) {
            match event.event_type {
                EventType::Error => stats.error_events += 1,
                EventType::DeviceReEnumerated => stats.re_enumerations += 1,
                EventType::DeviceConnected => stats.reconnects += 1,
                _ => {}
            }
        }
    }

    let mut out: Vec<DeviceErrorStats> = by_id.into_values().collect();
    out.sort_by(|a, b| {
        b.error_events
            .cmp(&a.error_events)
            .then(b.re_enumerations.cmp(&a.re_enumerations))
            .then(b.reconnects.cmp(&a.reconnects))
    });
    out
}

/// Attempt to read the dwc3 LTSSM state for a USB device from Linux debugfs.
/// Returns None on non-Linux platforms or when the file is absent/unreadable.
pub fn read_dwc3_ltssm(device_name: &str) -> Option<String> {
    // dwc3 debugfs exposes per-device LTSSM state at:
    //   /sys/kernel/debug/usb/dwc3/<device>/ltssm
    // Requires root + debugfs; silently skip if missing.
    #[cfg(target_os = "linux")]
    {
        let path = format!("/sys/kernel/debug/usb/dwc3/{device_name}/ltssm");
        std::fs::read_to_string(&path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device_name;
        None
    }
}

/// Read link/CRC error counters from Linux debugfs where exposed (xHCI
/// or dwc3). Returns (link_errors, crc_errors) or both None when absent.
pub fn read_debugfs_counters(_device_name: &str) -> (Option<u64>, Option<u64>) {
    // xHCI exposes port-level link error counters under:
    //   /sys/bus/usb/devices/<dev>/link_errors
    //   /sys/bus/usb/devices/<dev>/crc_errors
    // Not universally available; requires debugfs.
    #[cfg(target_os = "linux")]
    {
        let link_path = format!("/sys/bus/usb/devices/{_device_name}/link_errors");
        let crc_path = format!("/sys/bus/usb/devices/{_device_name}/crc_errors");
        let link = std::fs::read_to_string(&link_path)
            .ok()
            .and_then(|s| s.trim().parse().ok());
        let crc = std::fs::read_to_string(&crc_path)
            .ok()
            .and_then(|s| s.trim().parse().ok());
        (link, crc)
    }
    #[cfg(not(target_os = "linux"))]
    {
        (None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventSeverity, UsbDevice};
    use std::collections::HashMap;
    use uuid::Uuid;

    fn make_dev(id: Uuid) -> UsbDevice {
        UsbDevice {
            id,
            ..UsbDevice::new(0x1234, 0x5678)
        }
    }

    fn make_event(ty: EventType, dev_id: Uuid, severity: EventSeverity) -> DiagnosticEvent {
        DiagnosticEvent {
            id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            event_type: ty,
            device_id: Some(dev_id),
            hub_id: None,
            port_number: None,
            details: format!("{ty:?}"),
            severity,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn counts_errors_and_re_enumerations() {
        let dev_id = Uuid::new_v4();
        let devices = vec![make_dev(dev_id)];
        let events = vec![
            make_event(EventType::Error, dev_id, EventSeverity::Error),
            make_event(EventType::Error, dev_id, EventSeverity::Error),
            make_event(
                EventType::DeviceReEnumerated,
                dev_id,
                EventSeverity::Warning,
            ),
            make_event(EventType::DeviceConnected, dev_id, EventSeverity::Info),
        ];
        let stats = summarize_device_errors(&devices, &events);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].error_events, 2);
        assert_eq!(stats[0].re_enumerations, 1);
        assert_eq!(stats[0].reconnects, 1);
    }

    #[test]
    fn unmatched_device_events_are_ignored() {
        let dev_id = Uuid::new_v4();
        let unknown = Uuid::new_v4();
        let devices = vec![make_dev(dev_id)];
        let events = vec![make_event(EventType::Error, unknown, EventSeverity::Error)];
        let stats = summarize_device_errors(&devices, &events);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].error_events, 0, "unknown device event not counted");
    }

    #[test]
    fn stats_sorted_by_severity() {
        let clean = Uuid::new_v4();
        let dirty = Uuid::new_v4();
        let devices = vec![make_dev(clean), make_dev(dirty)];
        let events = vec![
            make_event(EventType::Error, dirty, EventSeverity::Error),
            make_event(EventType::DeviceReEnumerated, dirty, EventSeverity::Warning),
            make_event(EventType::Error, dirty, EventSeverity::Error),
        ];
        let stats = summarize_device_errors(&devices, &events);
        assert_eq!(stats[0].device_id, dirty, "most errors first");
        assert_eq!(stats[0].error_events, 2);
        assert_eq!(stats[1].error_events, 0);
    }
}
