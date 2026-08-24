//! Hotplug monitoring via polling-diff of sysfs snapshots.
//!
//! Same headless-friendly design as the Windows/macOS backends (udev
//! netlink monitoring is the future push-based enhancement; it needs
//! libudev and a socket loop, deliberately not pulled in yet).
//!
//! Identity nuance: sysfs names are deterministic per physical location
//! (`USB\VID_…\3-2.1`), so same-port re-enumeration is invisible by
//! construction — identical to macOS. Serial-matched moves across
//! locations pair into `DeviceReEnumerated`; serialless devices moving
//! ports surface as remove+add.

use crate::native::{self, RawDeviceInfo};
use skirr_core::{DiagnosticEvent, EventSeverity, EventType};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fingerprint {
    /// Deterministic instance id (`USB\VID_…&PID_…\<sysfs name>`)
    pub instance: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: Option<String>,
    /// sysfs location name (`3-2.1`), stable per port
    pub location: String,
    pub is_hub: bool,
}

impl Fingerprint {
    pub fn from_raw(raw: &RawDeviceInfo) -> Self {
        Self {
            instance: raw.make_instance(),
            vendor_id: raw.vendor_id,
            product_id: raw.product_id,
            serial: raw.serial.clone(),
            location: raw.name.clone(),
            is_hub: raw.device_class == 9,
        }
    }

    /// Same silicon? Serial match wins; serialless devices fall back to
    /// location identity.
    pub fn same_identity(&self, other: &Fingerprint) -> bool {
        match (&self.serial, &other.serial) {
            (Some(a), Some(b)) => a == b && self.vendor_id == other.vendor_id,
            _ => {
                self.instance == other.instance
                    || (self.vendor_id == other.vendor_id
                        && self.product_id == other.product_id
                        && self.location == other.location)
            }
        }
    }
}

/// Diff two snapshots into normalized diagnostic events.
///
/// Ordering mirrors the Windows backend: re-enum pairing first, then
/// disconnects, then connects. Hubs get dedicated event kinds.
pub fn diff_snapshots(
    before: &[Fingerprint],
    after: &[Fingerprint],
    timestamp: chrono::DateTime<chrono::Utc>,
) -> Vec<DiagnosticEvent> {
    let mut events = Vec::new();
    let after_by_fp: HashMap<&Fingerprint, &Fingerprint> = after.iter().map(|f| (f, f)).collect();

    let mut consumed_after: HashMap<&Fingerprint, ()> = HashMap::new();
    let mut unmatched_before = Vec::new();

    for old in before {
        match after.iter().find(|new| new.same_identity(old)) {
            Some(new) => {
                consumed_after.insert(new, ());
                if old.location != new.location {
                    events.push(make_event(
                        EventType::DeviceReEnumerated,
                        new,
                        timestamp,
                        format!("moved from {} to {}", old.location, new.location),
                    ));
                }
            }
            None => unmatched_before.push(old),
        }
    }

    for old in unmatched_before {
        let kind = if old.is_hub {
            EventType::HubDisconnected
        } else {
            EventType::DeviceDisconnected
        };
        events.push(make_event(
            kind,
            old,
            timestamp,
            format!("removed at {}", old.location),
        ));
    }

    for new in after {
        // Same-location presence already covered by steady state.
        if !consumed_after.contains_key(&after_by_fp[new]) {
            let existed_before = before.iter().any(|old| old.same_identity(new));
            if !existed_before {
                let kind = if new.is_hub {
                    EventType::HubConnected
                } else {
                    EventType::DeviceConnected
                };
                events.push(make_event(
                    kind,
                    new,
                    timestamp,
                    format!("attached at {}", new.location),
                ));
            }
        }
    }

    events
}

fn make_event(
    event_type: EventType,
    fp: &Fingerprint,
    timestamp: chrono::DateTime<chrono::Utc>,
    details: String,
) -> DiagnosticEvent {
    DiagnosticEvent {
        id: uuid::Uuid::new_v4(),
        timestamp,
        device_id: None,
        hub_id: if fp.is_hub {
            Some(uuid::Uuid::nil())
        } else {
            None
        },
        event_type,
        port_number: native::immediate_port(&fp.location),
        details,
        severity: EventSeverity::Info,
        metadata: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(location: &str, vid: u16, pid: u16, serial: Option<&str>, hub: bool) -> Fingerprint {
        Fingerprint {
            instance: format!(r"USB\VID_{vid:04X}&PID_{pid:04X}\{location}"),
            vendor_id: vid,
            product_id: pid,
            serial: serial.map(String::from),
            location: location.into(),
            is_hub: hub,
        }
    }

    #[test]
    fn connect_and_disconnect() {
        let t = chrono::Utc::now();
        let stick = fp("3-1", 0x0781, 0x5583, Some("AA01"), false);
        let events = diff_snapshots(&[], std::slice::from_ref(&stick), t);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::DeviceConnected);
        assert_eq!(events[0].port_number, Some(1));

        let events = diff_snapshots(&[stick], &[], t);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::DeviceDisconnected);
    }

    #[test]
    fn steady_state_is_silent() {
        let t = chrono::Utc::now();
        let dev = fp("3-2.1", 0x2109, 0x0817, None, true);
        assert!(
            diff_snapshots(std::slice::from_ref(&dev), std::slice::from_ref(&dev), t).is_empty()
        );
    }

    #[test]
    fn serial_match_pairs_moves_across_ports() {
        let t = chrono::Utc::now();
        let before = [fp("3-1", 0x0781, 0x5583, Some("SN1"), false)];
        let after = [fp("3-4.2", 0x0781, 0x5583, Some("SN1"), false)];
        let events = diff_snapshots(&before, &after, t);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::DeviceReEnumerated);
    }

    #[test]
    fn serialless_move_is_remove_plus_add() {
        let t = chrono::Utc::now();
        let before = [fp("3-1", 0x046D, 0xC52B, None, false)];
        let after = [fp("3-2", 0x046D, 0xC52B, None, false)];
        let events = diff_snapshots(&before, &after, t);

        let kinds: Vec<_> = events.iter().map(|e| e.event_type).collect();
        assert!(kinds.contains(&EventType::DeviceDisconnected));
        assert!(kinds.contains(&EventType::DeviceConnected));
    }

    #[test]
    fn hub_events_use_hub_kinds() {
        let t = chrono::Utc::now();
        let hub = fp("3-2", 0x2109, 0x0817, None, true);
        let events = diff_snapshots(&[], &[hub], t);
        assert_eq!(events[0].event_type, EventType::HubConnected);
    }
}
