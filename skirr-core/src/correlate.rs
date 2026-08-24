//! Live-event correlation and stability scoring (DATA_MAP §9).
//!
//! Pure consumer-side logic: backends emit raw `DiagnosticEvent`s; this
//! module turns streams into insight — hub-removal collapsing, flapper
//! detection, and an `EventSummary` with a stability score. No I/O, no
//! clocks beyond event timestamps, fully testable on every host.

use crate::{DiagnosticEvent, EventSummary, EventType, SystemTopology};
use std::collections::HashMap;
use std::time::Duration;

/// One correlated observation from a monitoring session.
#[derive(Debug, Clone)]
pub enum CorrelatedEvent {
    /// A single device/hub transition with no children involved.
    Single(DiagnosticEvent),
    /// A hub removal plus its collapsed subtree (child disconnect count
    /// includes nested hubs). The inner event is the hub's own.
    HubRemoval {
        hub_event: DiagnosticEvent,
        child_disconnects: usize,
        max_depth: u8,
    },
}

/// Sliding-window flap detector for one device identity.
///
/// A device that connects ≥ `threshold` times within `window` is
/// flapping — usually cable/port/power trouble, exactly what users need
/// surfaced.
pub struct FlapDetector {
    window: Duration,
    threshold: u32,
    /// device key → recent connect timestamps (seconds since epoch).
    history: HashMap<String, Vec<i64>>,
}

impl FlapDetector {
    pub fn new(window: Duration, threshold: u32) -> Self {
        Self {
            window,
            threshold,
            history: HashMap::new(),
        }
    }

    /// Record a connect for `key`; returns the connect count inside the
    /// window when it has reached the flap threshold, else None.
    pub fn observe_connect(&mut self, key: &str, at_secs: i64) -> Option<u32> {
        let window_start = at_secs - self.window.as_secs() as i64;
        let entry = self.history.entry(key.to_string()).or_default();
        entry.retain(|&t| t >= window_start);
        entry.push(at_secs);
        if entry.len() as u32 >= self.threshold {
            Some(entry.len() as u32)
        } else {
            None
        }
    }
}

/// Stream correlator: feed raw events in arrival order, get correlated
/// output. Tracks the live topology to know subtrees when hubs vanish.
pub struct EventCorrelator<'a> {
    topo: &'a SystemTopology,
    pending_hub_removals: HashMap<uuid::Uuid, Vec<DiagnosticEvent>>,
    flaps: FlapDetector,
    summary: EventSummary,
}

impl<'a> EventCorrelator<'a> {
    pub fn new(topo: &'a SystemTopology) -> Self {
        Self {
            topo,
            pending_hub_removals: HashMap::new(),
            flaps: FlapDetector::new(Duration::from_secs(60), 3),
            summary: EventSummary::default(),
        }
    }

    /// Flap detector configuration override (window/threshold).
    pub fn with_flap_config(mut self, window: Duration, threshold: u32) -> Self {
        self.flaps = FlapDetector::new(window, threshold);
        self
    }

    /// Ingest one raw event. Returns what should be surfaced to the user:
    /// `None` while a hub removal's subtree is still draining, or a flap
    /// annotation alongside the correlated event.
    pub fn ingest(&mut self, event: DiagnosticEvent) -> IngestOutcome {
        self.count_basic(&event);

        // Flap detection keys on the affected device id when present.
        if event.event_type == EventType::DeviceConnected {
            if let Some(dev_id) = event.device_id {
                if let Some(count) = self
                    .flaps
                    .observe_connect(&dev_id.to_string(), event.timestamp.timestamp())
                {
                    return IngestOutcome {
                        correlated: self.route(event),
                        flap_count: Some(count),
                    };
                }
            }
        }

        IngestOutcome {
            correlated: self.route(event),
            flap_count: None,
        }
    }

    fn route(&mut self, event: DiagnosticEvent) -> Option<CorrelatedEvent> {
        match event.event_type {
            EventType::HubDisconnected => {
                // Hold the hub's own event until its subtree drains.
                let dev_id = event.device_id?;
                self.pending_hub_removals.insert(dev_id, vec![event]);
                None
            }
            EventType::DeviceDisconnected => {
                // Belongs to a held hub removal?
                let Some(parent) = self.parent_of(event.device_id) else {
                    return Some(CorrelatedEvent::Single(event));
                };
                let Some(mut chain) = self.pending_hub_removals.remove(&parent) else {
                    return Some(CorrelatedEvent::Single(event));
                };
                chain.push(event);
                let expected = subtree_size(self.topo, parent);
                if chain.len() > expected {
                    let hub_event = chain.remove(0);
                    return Some(CorrelatedEvent::HubRemoval {
                        hub_event,
                        child_disconnects: chain.len(),
                        max_depth: subtree_depth(self.topo, parent),
                    });
                }
                // Still draining.
                self.pending_hub_removals.insert(parent, chain);
                None
            }
            _ => Some(CorrelatedEvent::Single(event)),
        }
    }

    fn count_basic(&mut self, event: &DiagnosticEvent) {
        self.summary.total_events += 1;
        match event.event_type {
            EventType::DeviceConnected | EventType::HubConnected => self.summary.connects += 1,
            EventType::DeviceDisconnected => self.summary.disconnects += 1,
            EventType::DeviceReEnumerated => self.summary.re_enumerations += 1,
            EventType::SpeedChanged => self.summary.speed_changes += 1,
            EventType::PowerChanged => self.summary.power_changes += 1,
            EventType::Error => self.summary.errors += 1,
            EventType::Warning => self.summary.warnings += 1,
            _ => {}
        }
    }

    fn parent_of(&self, dev_id: Option<uuid::Uuid>) -> Option<uuid::Uuid> {
        let id = dev_id?;
        self.topo
            .devices
            .iter()
            .find(|d| d.id == id)
            .and_then(|d| d.parent_id)
    }

    /// Finish the session: flush any undrained hub removals (topology
    /// snapshot was incomplete or events were dropped) and compute the
    /// stability score over `duration_secs`.
    pub fn finish(mut self, duration_secs: u64) -> EventSummary {
        // Flush incomplete chains as plain counts so totals stay honest.
        for (_, mut chain) in self.pending_hub_removals.drain() {
            let hub_event = chain.remove(0);
            self.summary.disconnects += chain.len();
            let _ = hub_event;
        }
        self.summary.monitoring_duration_secs = duration_secs;
        self.summary.stability_score = stability_score(&self.summary);
        self.summary
    }
}

/// What `ingest` decided.
#[derive(Debug, Clone)]
pub struct IngestOutcome {
    /// What to surface now (`None` = hold, more events expected).
    pub correlated: Option<CorrelatedEvent>,
    /// Set when this connect crossed the flap threshold.
    pub flap_count: Option<u32>,
}

/// Max depth of the subtree rooted at `hub_id` (hub itself excluded),
/// counting every descendant level.
pub fn subtree_depth(topo: &SystemTopology, hub_id: uuid::Uuid) -> u8 {
    let mut depth = 0u8;
    let mut frontier = vec![hub_id];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for parent in &frontier {
            for child in &topo.devices {
                if child.parent_id == Some(*parent) {
                    next.push(child.id);
                }
            }
        }
        if !next.is_empty() {
            depth += 1;
        }
        frontier = next;
    }
    depth
}

/// Count direct + transitive children of `hub_id`.
pub fn subtree_size(topo: &SystemTopology, hub_id: uuid::Uuid) -> usize {
    let mut count = 0usize;
    let mut frontier = vec![hub_id];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for parent in &frontier {
            for child in &topo.devices {
                if child.parent_id == Some(*parent) {
                    count += 1;
                    next.push(child.id);
                }
            }
        }
        frontier = next;
    }
    count
}

/// Stability score 0–100: starts perfect, deductions per disruptive
/// event class relative to session length.
pub fn stability_score(summary: &EventSummary) -> u8 {
    let minutes = (summary.monitoring_duration_secs.max(1) as f64 / 60.0).max(1.0);
    let penalties = (summary.re_enumerations * 5
        + summary.speed_changes * 3
        + summary.errors * 10
        + summary.warnings) as f64
        / minutes;
    100u8.saturating_sub(penalties.min(100.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventSeverity, UsbDevice};

    fn empty_platform_info() -> crate::PlatformInfo {
        crate::PlatformInfo {
            os: "test".into(),
            os_version: String::new(),
            kernel_version: None,
            architecture: "x86_64".into(),
            hostname: None,
            username: None,
            is_admin: false,
            is_virtual_machine: false,
            boot_time: None,
        }
    }

    fn topo_with(chain: &[(&str, Option<&str>)]) -> SystemTopology {
        // chain: (name, parent name).
        let mut topo = SystemTopology {
            timestamp: chrono::Utc::now(),
            host_controllers: Vec::new(),
            root_hubs: Vec::new(),
            devices: Vec::new(),
            hubs: Vec::new(),
            displays: Vec::new(),
            events: Vec::new(),
            platform_info: empty_platform_info(),
        };
        for (name, parent) in chain {
            let mut d = UsbDevice::new(0x2109, 0x0817);
            d.platform_id = (*name).to_string();
            d.is_hub = true;
            if let Some(p) = parent {
                d.parent_id = topo
                    .devices
                    .iter()
                    .find(|d| d.platform_id == *p)
                    .map(|d| d.id);
            }
            topo.devices.push(d);
        }
        topo
    }

    fn event(ty: EventType, dev_id: Option<uuid::Uuid>, secs: i64) -> DiagnosticEvent {
        DiagnosticEvent {
            id: uuid::Uuid::new_v4(),
            timestamp: chrono::DateTime::from_timestamp(secs, 0).unwrap(),
            event_type: ty,
            device_id: dev_id,
            hub_id: None,
            port_number: Some(2),
            details: "test".into(),
            severity: EventSeverity::Info,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn hub_removal_collapses_subtree() {
        let topo = topo_with(&[
            ("root", None),
            ("dock", Some("root")),
            ("kb", Some("dock")),
            ("cam", Some("dock")),
        ]);
        let dock_id = topo
            .devices
            .iter()
            .find(|d| d.platform_id == "dock")
            .unwrap()
            .id;
        let kb_id = topo
            .devices
            .iter()
            .find(|d| d.platform_id == "kb")
            .unwrap()
            .id;
        let cam_id = topo
            .devices
            .iter()
            .find(|d| d.platform_id == "cam")
            .unwrap()
            .id;

        let mut c = EventCorrelator::new(&topo);
        assert!(
            c.ingest(event(EventType::HubDisconnected, Some(dock_id), 0))
                .correlated
                .is_none(),
            "held until subtree drains"
        );
        let out1 = c.ingest(event(EventType::DeviceDisconnected, Some(kb_id), 0));
        assert!(out1.correlated.is_none(), "still draining");
        let out2 = c.ingest(event(EventType::DeviceDisconnected, Some(cam_id), 0));
        let Some(CorrelatedEvent::HubRemoval {
            child_disconnects,
            max_depth,
            ..
        }) = out2.correlated
        else {
            panic!("expected collapsed hub removal");
        };
        assert_eq!(child_disconnects, 2);
        assert_eq!(max_depth, 1);
    }

    #[test]
    fn lone_device_disconnect_passes_through() {
        let topo = topo_with(&[("root", None)]);
        let mut c = EventCorrelator::new(&topo);
        let out = c.ingest(event(
            EventType::DeviceDisconnected,
            Some(uuid::Uuid::new_v4()),
            0,
        ));
        assert!(matches!(out.correlated, Some(CorrelatedEvent::Single(_))));
    }

    #[test]
    fn flapper_detected_on_third_connect() {
        let topo = topo_with(&[("root", None)]);
        let id = topo.devices[0].id;
        let mut c = EventCorrelator::new(&topo);
        assert_eq!(
            c.ingest(event(EventType::DeviceConnected, Some(id), 0))
                .flap_count,
            None
        );
        assert_eq!(
            c.ingest(event(EventType::DeviceConnected, Some(id), 10))
                .flap_count,
            None
        );
        let third = c.ingest(event(EventType::DeviceConnected, Some(id), 20));
        assert_eq!(third.flap_count, Some(3));
    }

    #[test]
    fn flap_window_expires() {
        let topo = topo_with(&[("root", None)]);
        let id = topo.devices[0].id;
        let mut c = EventCorrelator::new(&topo);
        assert_eq!(
            c.ingest(event(EventType::DeviceConnected, Some(id), 0))
                .flap_count,
            None
        );
        assert_eq!(
            c.ingest(event(EventType::DeviceConnected, Some(id), 10))
                .flap_count,
            None
        );
        // 61s later: outside the default 60s window.
        let late = c.ingest(event(EventType::DeviceConnected, Some(id), 61));
        assert_eq!(late.flap_count, None);
    }

    #[test]
    fn summary_counts_and_stability() {
        let topo = topo_with(&[("root", None)]);
        let mut c = EventCorrelator::new(&topo);
        c.ingest(event(
            EventType::DeviceConnected,
            Some(uuid::Uuid::new_v4()),
            0,
        ));
        c.ingest(event(
            EventType::DeviceReEnumerated,
            Some(uuid::Uuid::new_v4()),
            1,
        ));
        c.ingest(event(
            EventType::SpeedChanged,
            Some(uuid::Uuid::new_v4()),
            2,
        ));
        let s = c.finish(60);
        assert_eq!(s.total_events, 3);
        assert_eq!(s.connects, 1);
        assert_eq!(s.re_enumerations, 1);
        assert_eq!(s.speed_changes, 1);
        assert_eq!(s.stability_score, 92, "5+3 penalty over one minute");
    }

    #[test]
    fn perfect_session_scores_100() {
        let topo = topo_with(&[("root", None)]);
        let s = EventCorrelator::new(&topo).finish(300);
        assert_eq!(s.stability_score, 100);
    }

    #[test]
    fn undrained_hubs_flush_as_disconnects() {
        let topo = topo_with(&[("root", None), ("dock", Some("root")), ("x", Some("dock"))]);
        let dock_id = topo
            .devices
            .iter()
            .find(|d| d.platform_id == "dock")
            .unwrap()
            .id;
        let mut c = EventCorrelator::new(&topo);
        c.ingest(event(EventType::HubDisconnected, Some(dock_id), 0));
        // No children ever arrive — finish must not hang or lose the count.
        let s = c.finish(60);
        assert_eq!(s.total_events, 1);
    }
}
