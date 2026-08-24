//! Hotplug monitoring for Windows.
//!
//! Polling-diff design (Shoko-proven, headless-friendly): periodically
//! re-enumerate, diff instance-ID sets against the previous snapshot, and
//! emit normalized `DiagnosticEvent`s. Devices whose identity (VID/PID +
//! serial) reappears under a different instance/port are reported as
//! re-enumerations rather than remove+add pairs.
//!
//! The diff engine is pure and unit-tested off-Windows; the polling session
//! lives behind `#[cfg(windows)]`.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::hwid::{extract_serial_from_instance, parse_hardware_id};
use crate::native::RawDeviceInfo;
use chrono::{DateTime, Utc};
use skirr_core::{BackendError, BackendResult, DiagnosticEvent, EventSeverity, EventType};
use std::collections::HashMap;
#[cfg(windows)]
use std::time::Duration;
use uuid::Uuid;

/// Comparable snapshot entry derived from one enumeration record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    pub instance: String,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Instance-path serial segment when present.
    pub serial: Option<String>,
    pub port: Option<u8>,
    pub is_hub: bool,
}

impl Fingerprint {
    pub(crate) fn from_raw(raw: &RawDeviceInfo) -> Self {
        let upper = raw.instance_id.to_ascii_uppercase();
        let last_segment = raw.instance_id.rsplit('\\').next().unwrap_or_default();

        // Prefer explicit USB hardware IDs; fall back to the instance path.
        let (vendor_id, product_id) = raw
            .hardware_ids
            .iter()
            .map(|h| parse_hardware_id(h))
            .find(|id| id.vid.is_some() || id.pid.is_some())
            .map(|id| (id.vid.unwrap_or(0), id.pid.unwrap_or(0)))
            .unwrap_or((0, 0));

        let is_hub = upper.contains("ROOT_HUB")
            || [
                raw.class_name.as_deref(),
                raw.description.as_deref(),
                raw.friendly_name.as_deref(),
            ]
            .iter()
            .flatten()
            .any(|s| s.to_ascii_lowercase().contains("hub"));

        Self {
            instance: raw.instance_id.clone(),
            vendor_id,
            product_id,
            serial: extract_serial_from_instance(last_segment),
            port: crate::topology::extract_port(&raw.instance_id),
            is_hub,
        }
    }

    /// Identity across ports/re-enumeration: same silicon + serial.
    fn same_identity(&self, other: &Self) -> bool {
        self.vendor_id == other.vendor_id
            && self.product_id == other.product_id
            && match (&self.serial, &other.serial) {
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                _ => false,
            }
    }
}

/// Pure diff between two enumeration snapshots.
///
/// Emits, in order: re-enumerations (paired removals/additions of identical
/// silicon), disconnections, connections. Hubs get the dedicated event kinds.
pub(crate) fn diff_snapshots(
    previous: &[Fingerprint],
    current: &[Fingerprint],
    now: DateTime<Utc>,
) -> Vec<DiagnosticEvent> {
    let cur_instances: std::collections::HashSet<&str> =
        current.iter().map(|f| f.instance.as_str()).collect();

    // Removals = previously present, now gone. Consumed as we pair them up
    // with arrivals of identical silicon (→ re-enumeration instead).
    let mut removed: Vec<&Fingerprint> = previous
        .iter()
        .filter(|f| !cur_instances.contains(f.instance.as_str()))
        .collect();

    let mut events: Vec<DiagnosticEvent> = Vec::new();
    let mut reenumerated: std::collections::HashSet<String> = Default::default();

    for cur in current {
        if cur_instances.is_empty() {
            break;
        }
        let known_before = previous.iter().any(|f| f.instance == cur.instance);
        if known_before {
            continue;
        }
        if let Some(idx) = removed.iter().position(|old| old.same_identity(cur)) {
            let old = removed.swap_remove(idx);
            reenumerated.insert(cur.instance.clone());
            events.push(event(
                EventType::DeviceReEnumerated,
                cur,
                now,
                format!(
                    "re-enumerated {}:{} ({} → {})",
                    hex4(cur.vendor_id),
                    hex4(cur.product_id),
                    old.instance,
                    cur.instance
                ),
            ));
        }
    }

    for old in &removed {
        events.push(event(
            if old.is_hub {
                EventType::HubDisconnected
            } else {
                EventType::DeviceDisconnected
            },
            old,
            now,
            format!("removed {}", old.instance),
        ));
    }

    for cur in current {
        let known_before = previous.iter().any(|f| f.instance == cur.instance);
        if known_before || reenumerated.contains(&cur.instance) {
            continue;
        }
        events.push(event(
            if cur.is_hub {
                EventType::HubConnected
            } else {
                EventType::DeviceConnected
            },
            cur,
            now,
            format!("arrived {}", cur.instance),
        ));
    }

    events
}

fn hex4(v: u16) -> String {
    format!("{v:04X}")
}

fn event(
    kind: EventType,
    fp: &Fingerprint,
    now: DateTime<Utc>,
    details: String,
) -> DiagnosticEvent {
    DiagnosticEvent {
        id: Uuid::new_v4(),
        timestamp: now,
        event_type: kind,
        device_id: None,
        hub_id: None,
        port_number: fp.port,
        details,
        severity: EventSeverity::Info,
        metadata: HashMap::from([("instance_id".to_string(), fp.instance.clone())]),
    }
}

// ---------------------------------------------------------------------------
// Polling session (Windows only)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use super::*;
    use crate::native::win::enumerate;
    use skirr_core::BackendError;
    use std::collections::VecDeque;

    const POLL_INTERVAL: Duration = Duration::from_millis(250);

    /// Headless hotplug monitor: periodic re-enumeration + pure diffing.
    pub(super) struct PollMonitor {
        previous: Vec<Fingerprint>,
        pending: VecDeque<DiagnosticEvent>,
        active: bool,
    }

    impl PollMonitor {
        pub(super) fn new() -> Result<Self, BackendError> {
            Ok(Self {
                previous: Vec::new(),
                pending: VecDeque::new(),
                active: false,
            })
        }
    }

    impl skirr_core::HotplugBackend for PollMonitor {
        fn start_monitoring(&mut self) -> BackendResult<()> {
            self.previous = enumerate()?.iter().map(Fingerprint::from_raw).collect();
            self.active = true;
            Ok(())
        }

        fn poll_event(&mut self, timeout: Duration) -> BackendResult<Option<DiagnosticEvent>> {
            if !self.active {
                return Err(BackendError::NotMonitoring {
                    backend: "skirr-windows",
                });
            }
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if let Some(ev) = self.pending.pop_front() {
                    return Ok(Some(ev));
                }
                if std::time::Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(POLL_INTERVAL.min(Duration::from_millis(50)));

                let current = match enumerate() {
                    Ok(devices) => devices
                        .iter()
                        .map(Fingerprint::from_raw)
                        .collect::<Vec<_>>(),
                    Err(_) => continue, // transient failure; retry next tick
                };
                let fresh = diff_snapshots(&self.previous, &current, Utc::now());
                self.previous = current;
                self.pending.extend(fresh);
            }
        }

        fn stop_monitoring(&mut self) -> BackendResult<()> {
            self.active = false;
            self.pending.clear();
            Ok(())
        }

        fn is_active(&self) -> bool {
            self.active
        }
    }
}

#[cfg(windows)]
pub(crate) fn create_monitor() -> BackendResult<Box<dyn skirr_core::HotplugBackend>> {
    Ok(Box::new(win::PollMonitor::new()?))
}

#[cfg(not(windows))]
pub(crate) fn create_monitor() -> BackendResult<Box<dyn skirr_core::HotplugBackend>> {
    Err(BackendError::unsupported(
        "skirr-windows",
        "hotplug monitoring requires Windows",
    ))
}

// ---------------------------------------------------------------------------
// Tests (pure logic, run on every host)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(
        instance: &str,
        vid: u16,
        pid: u16,
        serial: Option<&str>,
        port: Option<u8>,
    ) -> Fingerprint {
        Fingerprint {
            instance: instance.to_string(),
            vendor_id: vid,
            product_id: pid,
            serial: serial.map(str::to_string),
            port,
            is_hub: false,
        }
    }

    fn hub_fp(instance: &str) -> Fingerprint {
        let mut f = fp(instance, 0x8086, 0xA36D, None, None);
        f.is_hub = true;
        f
    }

    #[test]
    fn empty_diff_emits_nothing() {
        let snap = vec![fp(r"USB\VID_1&PID_2\SER", 0x1, 0x2, Some("SER"), Some(1))];
        assert!(diff_snapshots(&snap, &snap, Utc::now()).is_empty());
    }

    #[test]
    fn plain_arrival_and_removal() {
        let before = vec![fp(
            r"USB\VID_1111&PID_2222\SER",
            0x1111,
            0x2222,
            Some("SER"),
            Some(1),
        )];
        let after = vec![fp(
            r"USB\VID_3333&PID_4444\OTHER",
            0x3333,
            0x4444,
            Some("OTHER"),
            Some(3),
        )];
        let events = diff_snapshots(&before, &after, Utc::now());

        assert_eq!(events.len(), 2);
        assert!(events
            .iter()
            .any(|e| e.event_type == EventType::DeviceDisconnected));
        assert!(events
            .iter()
            .any(|e| e.event_type == EventType::DeviceConnected));
        assert!(events
            .iter()
            .all(|e| e.metadata.contains_key("instance_id")));
    }

    #[test]
    fn moved_device_is_reenumeration_not_remove_add() {
        let before = vec![fp(
            r"USB\VID_1111&PID_2222\SER",
            0x1111,
            0x2222,
            Some("SER"),
            Some(1),
        )];
        let after = vec![fp(
            r"USB\VID_1111&PID_2222\6&X&0005",
            0x1111,
            0x2222,
            Some("SER"),
            Some(5),
        )];
        let events = diff_snapshots(&before, &after, Utc::now());

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::DeviceReEnumerated);
        assert_eq!(events[0].port_number, Some(5));
    }

    #[test]
    fn different_silicon_same_port_is_not_reenumeration() {
        let before = vec![fp(
            r"USB\VID_1111&PID_2222\SERA",
            0x1111,
            0x2222,
            Some("SERA"),
            Some(1),
        )];
        let after = vec![fp(
            r"USB\VID_3333&PID_4444\SERB",
            0x3333,
            0x4444,
            Some("SERB"),
            Some(1),
        )];
        let events = diff_snapshots(&before, &after, Utc::now());

        assert_eq!(events.len(), 2);
        assert!(!events
            .iter()
            .any(|e| e.event_type == EventType::DeviceReEnumerated));
    }

    #[test]
    fn hubs_use_dedicated_event_kinds() {
        let before = vec![hub_fp(r"USB\ROOT_HUB30\A")];
        let after = vec![hub_fp(r"USB\ROOT_HUB30\B")];
        let events = diff_snapshots(&before, &after, Utc::now());

        assert!(events
            .iter()
            .any(|e| e.event_type == EventType::HubDisconnected));
        assert!(events
            .iter()
            .any(|e| e.event_type == EventType::HubConnected));
    }

    #[test]
    fn fingerprint_reads_vid_pid_from_hardware_ids() {
        let raw = RawDeviceInfo {
            instance_id: r"USB\VID_05AC&PID_12A8\0001".into(),
            hardware_ids: vec![r"USB\VID_05AC&PID_12A8\0001".into()],
            manufacturer: None,
            description: None,
            friendly_name: Some("Apple Mobile".into()),
            class_name: None,
            status: Some("OK".into()),
            parent: None,
        };
        let f = Fingerprint::from_raw(&raw);
        assert_eq!(f.vendor_id, 0x05AC);
        assert_eq!(f.product_id, 0x12A8);
        assert_eq!(f.serial.as_deref(), Some("0001"));
        assert!(!f.is_hub);
    }

    #[test]
    fn fingerprint_flags_root_hubs_as_hubs() {
        let raw = RawDeviceInfo {
            instance_id: r"USB\ROOT_HUB30\4&38A99F6&0".into(),
            hardware_ids: vec![r"USB\ROOT_HUB30&VID_8086&PID_A36D".into()],
            manufacturer: None,
            description: None,
            friendly_name: None,
            class_name: Some("USB".into()),
            status: Some("OK".into()),
            parent: None,
        };
        assert!(Fingerprint::from_raw(&raw).is_hub);
    }
}
