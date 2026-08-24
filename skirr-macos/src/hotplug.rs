//! Hotplug monitoring for macOS.
//!
//! Polling-diff design (mirrors skirr-windows/src/hotplug.rs; headless-friendly):
//! periodically re-enumerate, diff instance-ID sets, emit normalized
//! `DiagnosticEvent`s. Devices whose silicon reappears under a new instance
//! pair into re-enumerations. macOS identity nuance: instances are
//! deterministic (`USB\VID_x&PID_y\<location>`), so a device that re-enumerates
//! on the SAME port is indistinguishable from steady state — pairing fires on
//! serial match across different locations instead.
//!
//! Future enhancement (intentionally not implemented): IOServiceAddMatchingNotification
//! push events require CFRunLoop ownership, which fights the CLI.

use crate::native::RawDeviceInfo;
use chrono::{DateTime, Utc};
use skirr_core::{BackendError, BackendResult, DiagnosticEvent, EventSeverity, EventType};
use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::time::Duration;
use uuid::Uuid;

/// Comparable snapshot entry derived from one enumeration record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    pub instance: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: Option<String>,
    pub location_id: u32,
    /// Parent instance string when the device sits under another device.
    pub parent_instance: Option<String>,
    pub is_hub: bool,
}

impl Fingerprint {
    pub(crate) fn from_raw(raw: &RawDeviceInfo) -> Self {
        Self {
            instance: raw.instance_id.clone(),
            vendor_id: raw.vendor_id,
            product_id: raw.product_id,
            serial: raw.serial_number.clone(),
            location_id: raw.location_id,
            parent_instance: raw.parent.clone(),
            is_hub: raw.device_class == 0x09,
        }
    }

    /// Identity across locations: same silicon plus either the same serial or
    /// (when serials are absent) the same physical port location.
    fn same_identity(&self, other: &Self) -> bool {
        if self.vendor_id != other.vendor_id || self.product_id != other.product_id {
            return false;
        }
        match (&self.serial, &other.serial) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => self.location_id != 0 && self.location_id == other.location_id,
        }
    }

    fn immediate_port(&self, by_instance: &HashMap<&str, u32>) -> Option<u8> {
        let parent_loc = self
            .parent_instance
            .as_deref()
            .and_then(|p| by_instance.get(p).copied());
        crate::topology::port_from_location(self.location_id, parent_loc)
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

    // Removals = previously present, now gone; consumed when paired with an
    // arrival of identical silicon (→ re-enumeration instead).
    let mut removed: Vec<Fingerprint> = previous
        .iter()
        .filter(|f| !cur_instances.contains(f.instance.as_str()))
        .cloned()
        .collect();

    let loc_by_instance: HashMap<&str, u32> = previous
        .iter()
        .chain(current.iter())
        .map(|f| (f.instance.as_str(), f.location_id))
        .collect();

    let mut events: Vec<DiagnosticEvent> = Vec::new();
    let mut reenumerated: std::collections::HashSet<String> = Default::default();

    for cur in current {
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
                cur.immediate_port(&loc_by_instance),
                now,
                format!(
                    "re-enumerated {:04X}:{:04X} ({} → {})",
                    cur.vendor_id, cur.product_id, old.instance, cur.instance
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
            None,
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
            cur.immediate_port(&loc_by_instance),
            now,
            format!("arrived {}", cur.instance),
        ));
    }

    events
}

fn event(
    kind: EventType,
    fp: &Fingerprint,
    port: Option<u8>,
    now: DateTime<Utc>,
    details: String,
) -> DiagnosticEvent {
    DiagnosticEvent {
        id: Uuid::new_v4(),
        timestamp: now,
        event_type: kind,
        device_id: None,
        hub_id: None,
        port_number: port,
        details,
        severity: EventSeverity::Info,
        metadata: HashMap::from([("instance_id".to_string(), fp.instance.clone())]),
    }
}

// ---------------------------------------------------------------------------
// Polling session (macOS only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod darwin {
    use super::*;
    use crate::native;

    const POLL_INTERVAL: Duration = Duration::from_millis(250);

    /// Headless hotplug monitor: periodic re-enumeration + pure diffing.
    pub(super) struct PollMonitor {
        previous: Vec<Fingerprint>,
        pending: VecDeque<DiagnosticEvent>,
        active: bool,
    }

    use std::collections::VecDeque;

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
            self.previous = native::enumerate()?
                .iter()
                .map(Fingerprint::from_raw)
                .collect();
            self.active = true;
            Ok(())
        }

        fn poll_event(&mut self, timeout: Duration) -> BackendResult<Option<DiagnosticEvent>> {
            if !self.active {
                return Err(BackendError::NotMonitoring {
                    backend: crate::BACKEND_NAME,
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

                let current = match native::enumerate() {
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

/// Public entry used by the backend; falls back to an error off-macOS.
#[cfg(target_os = "macos")]
pub(crate) fn create_monitor() -> BackendResult<Box<dyn skirr_core::HotplugBackend>> {
    Ok(Box::new(darwin::PollMonitor::new()?))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn create_monitor() -> BackendResult<Box<dyn skirr_core::HotplugBackend>> {
    Err(BackendError::unsupported(
        crate::BACKEND_NAME,
        "hotplug monitoring requires macOS",
    ))
}

// ---------------------------------------------------------------------------
// Tests (pure logic, run on every host)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(vid: u16, pid: u16, location: u32, serial: Option<&str>, class: u8) -> Fingerprint {
        Fingerprint {
            instance: crate::native::make_instance(vid, pid, location),
            vendor_id: vid,
            product_id: pid,
            serial: serial.map(str::to_string),
            location_id: location,
            parent_instance: None,
            is_hub: class == 0x09,
        }
    }

    #[test]
    fn empty_diff_emits_nothing() {
        let snap = vec![fp(0x1111, 0x2222, 0x14500000, Some("SER"), 0x08)];
        assert!(diff_snapshots(&snap, &snap, Utc::now()).is_empty());
    }

    #[test]
    fn plain_arrival_and_removal() {
        let before = vec![fp(0x1111, 0x2222, 0x14500000, Some("A"), 0x08)];
        let after = vec![fp(0x3333, 0x4444, 0x14200000, Some("B"), 0x08)];
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
    fn moved_device_with_serial_is_reenumeration() {
        // Same silicon + serial, replugged into a different port.
        let before = vec![fp(0x0781, 0x5583, 0x14100000, Some("AA01"), 0x08)];
        let after = vec![fp(0x0781, 0x5583, 0x14300000, Some("AA01"), 0x08)];
        let events = diff_snapshots(&before, &after, Utc::now());

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::DeviceReEnumerated);
    }

    #[test]
    fn serialless_device_moving_ports_is_not_paired() {
        // No serial → location identity differs → remove+add, not re-enum.
        let before = vec![fp(0x1111, 0x2222, 0x14500000, None, 0x08)];
        let after = vec![fp(0x1111, 0x2222, 0x14600000, None, 0x08)];
        let events = diff_snapshots(&before, &after, Utc::now());

        assert_eq!(events.len(), 2);
        assert!(!events
            .iter()
            .any(|e| e.event_type == EventType::DeviceReEnumerated));
    }

    #[test]
    fn hubs_use_dedicated_event_kinds() {
        let before = vec![fp(0x2109, 0x0817, 0x14200000, Some("HUB"), 0x09)];
        let after = vec![fp(0x0583, 0x9846, 0x14200000, Some("HUB"), 0x09)];
        let events = diff_snapshots(&before, &after, Utc::now());

        // Same serial+location but DIFFERENT vid/pid → plain hub swap.
        assert!(events
            .iter()
            .any(|e| e.event_type == EventType::HubDisconnected));
        assert!(events
            .iter()
            .any(|e| e.event_type == EventType::HubConnected));
    }

    #[test]
    fn fingerprint_carries_identity_fields() {
        let raw = RawDeviceInfo {
            instance_id: crate::native::make_instance(0x05AC, 0x12A8, 0x14500000),
            vendor_id: 0x05AC,
            product_id: 0x12A8,
            serial_number: Some("CAFEBABE".into()),
            location_id: 0x14500000,
            parent: Some("USB\\VID_2109&PID_0817\\0x14200000".into()),
            device_class: 0x00,
            ..Default::default()
        };
        let f = Fingerprint::from_raw(&raw);
        assert_eq!(f.vendor_id, 0x05AC);
        assert_eq!(f.serial.as_deref(), Some("CAFEBABE"));
        assert_eq!(
            f.parent_instance.as_deref(),
            Some("USB\\VID_2109&PID_0817\\0x14200000")
        );
        assert!(!f.is_hub);

        let mut hub_raw = raw.clone();
        hub_raw.device_class = 0x09;
        assert!(Fingerprint::from_raw(&hub_raw).is_hub);
    }
}
