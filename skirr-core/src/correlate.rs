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

// ---------------------------------------------------------------------------
// Post-hoc session analysis (Phase 10.4): root causes, periodicity, display
// correlation, and a support-exportable timeline.
// ---------------------------------------------------------------------------

/// Where a disruption most plausibly originated.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RootCauseAttribution {
    pub affected_device: Option<uuid::Uuid>,
    /// "upstream hub X dropped its children" vs "device-local trouble".
    pub is_upstream: bool,
    pub upstream_hub_id: Option<uuid::Uuid>,
    pub description: String,
}

const ROOT_CAUSE_WINDOW_SECS: i64 = 3;

/// Attribute each re-enumeration/disconnect to either an upstream hub event
/// within ±3 s (cable/port/power trouble above the device) or device-local.
pub fn attribute_root_causes(
    topo: &SystemTopology,
    events: &[DiagnosticEvent],
) -> Vec<RootCauseAttribution> {
    let by_id: HashMap<uuid::Uuid, &crate::UsbDevice> =
        topo.devices.iter().map(|d| (d.id, d)).collect();
    let mut out = Vec::new();

    for event in events.iter().filter(|e| {
        matches!(
            e.event_type,
            EventType::DeviceReEnumerated | EventType::DeviceDisconnected | EventType::SpeedChanged
        )
    }) {
        let Some(dev_id) = event.device_id else {
            continue;
        };
        let ts = event.timestamp.timestamp();

        // Walk the parent chain; first ancestor hub with its own disruptive
        // event inside the window claims the cause (nearest wins).
        let mut cursor = by_id.get(&dev_id).and_then(|d| d.parent_id);
        let mut culprit = None;
        while let Some(pid) = cursor {
            let upstream_hit = events.iter().any(|e| {
                e.device_id == Some(pid)
                    && matches!(
                        e.event_type,
                        EventType::DeviceDisconnected
                            | EventType::HubDisconnected
                            | EventType::DeviceReEnumerated
                    )
                    && (e.timestamp.timestamp() - ts).abs() <= ROOT_CAUSE_WINDOW_SECS
            });
            if upstream_hit {
                culprit = Some(pid);
                break;
            }
            cursor = by_id.get(&pid).and_then(|d| d.parent_id);
        }

        out.push(if let Some(hub_id) = culprit {
            RootCauseAttribution {
                affected_device: Some(dev_id),
                is_upstream: true,
                upstream_hub_id: Some(hub_id),
                description: format!(
                    "{:?} of {} attributed to upstream hub {} (event within {ROOT_CAUSE_WINDOW_SECS}s)",
                    event.event_type, dev_id, hub_id
                ),
            }
        } else {
            RootCauseAttribution {
                affected_device: Some(dev_id),
                is_upstream: false,
                upstream_hub_id: None,
                description: format!("{:?} of {dev_id}: no upstream event — likely device/cable-local", event.event_type),
            }
        });
    }
    out
}

/// A device dropping at regular intervals — the classic failing-cable or
/// power-save-timer signature.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PeriodicPattern {
    pub device_id: uuid::Uuid,
    pub interval_mean_secs: f64,
    pub interval_stddev_secs: f64,
    pub samples: usize,
}

/// Detect periodic disconnects per device. `max_cv` caps the coefficient of
/// variation (stddev/mean); 0.25 keeps only clock-like regularity.
pub fn detect_periodic_drops(events: &[DiagnosticEvent], max_cv: f64) -> Vec<PeriodicPattern> {
    let mut drops_by_device: HashMap<uuid::Uuid, Vec<i64>> = HashMap::new();
    for e in events
        .iter()
        .filter(|e| e.event_type == EventType::DeviceDisconnected)
    {
        if let Some(id) = e.device_id {
            drops_by_device
                .entry(id)
                .or_default()
                .push(e.timestamp.timestamp());
        }
    }

    let mut patterns = Vec::new();
    for (device_id, mut times) in drops_by_device {
        times.sort_unstable();
        let intervals: Vec<f64> = times
            .windows(2)
            .map(|w| (w[1] - w[0]) as f64)
            .filter(|d| *d > 0.0)
            .collect();
        if intervals.len() < 3 {
            continue; // need ≥4 drops before "periodic" means anything
        }
        let mean = intervals.iter().sum::<f64>() / intervals.len() as f64;
        if mean <= 0.0 {
            continue;
        }
        let variance =
            intervals.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / intervals.len() as f64;
        let stddev = variance.sqrt();
        if stddev / mean <= max_cv {
            patterns.push(PeriodicPattern {
                device_id,
                interval_mean_secs: mean,
                interval_stddev_secs: stddev,
                samples: intervals.len() + 1,
            });
        }
    }
    patterns
}

/// A USB transition and a display transition that happened close together —
/// usually one dock/TB chain carrying both.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DisplayCorrelation {
    pub usb_timestamp: chrono::DateTime<chrono::Utc>,
    pub usb_event_type: EventType,
    pub display_timestamp: chrono::DateTime<chrono::Utc>,
    pub display_event_type: EventType,
    pub delta_ms: i64,
    pub description: String,
}

/// Pair USB connect/disconnect events with display events within `window`.
/// Greedy nearest-match, each USB event consumed once.
pub fn correlate_display_events(
    events: &[DiagnosticEvent],
    window_secs: i64,
) -> Vec<DisplayCorrelation> {
    let usb_kinds = [
        EventType::DeviceDisconnected,
        EventType::HubDisconnected,
        EventType::DeviceConnected,
        EventType::HubConnected,
    ];
    let display_kinds = [
        EventType::DisplayDisconnected,
        EventType::DisplayConnected,
        EventType::DisplayModeChanged,
    ];

    let usb_events: Vec<&DiagnosticEvent> = events
        .iter()
        .filter(|e| usb_kinds.contains(&e.event_type))
        .collect();
    let mut used = vec![false; usb_events.len()];
    let mut out = Vec::new();

    for display_event in events
        .iter()
        .filter(|e| display_kinds.contains(&e.event_type))
    {
        let dt = display_event.timestamp.timestamp();
        let best = usb_events
            .iter()
            .enumerate()
            .filter(|(i, e)| !used[*i] && (e.timestamp.timestamp() - dt).abs() <= window_secs)
            .min_by_key(|(_, e)| (e.timestamp.timestamp() - dt).abs());
        if let Some((i, usb)) = best {
            used[i] = true;
            let delta_ms = (usb.timestamp.timestamp_millis()
                - display_event.timestamp.timestamp_millis())
            .abs();
            out.push(DisplayCorrelation {
                usb_timestamp: usb.timestamp,
                usb_event_type: usb.event_type,
                display_timestamp: display_event.timestamp,
                display_event_type: display_event.event_type,
                delta_ms,
                description: format!(
                    "{:?} on USB coincided with {:?} on displays ({delta_ms} ms apart) — shared chain?",
                    usb.event_type, display_event.event_type
                ),
            });
        }
    }
    out.sort_by_key(|c| c.display_timestamp);
    out
}

/// One support-facing timeline line.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TimelineEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub kind: String,
    pub description: String,
}

impl TimelineEntry {
    pub fn from_single(event: &DiagnosticEvent) -> Self {
        TimelineEntry {
            timestamp: event.timestamp,
            kind: format!("{:?}", event.event_type),
            description: event.details.clone(),
        }
    }

    pub fn from_hub_removal(
        hub_event: &DiagnosticEvent,
        child_disconnects: usize,
        max_depth: u8,
    ) -> Self {
        TimelineEntry {
            timestamp: hub_event.timestamp,
            kind: "HubRemoval".into(),
            description: format!(
                "{} ({} device(s), depth {max_depth})",
                hub_event.details, child_disconnects
            ),
        }
    }
}

/// Everything worth handing to support after a monitoring session.
#[derive(Debug, Default, serde::Serialize)]
pub struct TimelineReport {
    pub generated: chrono::DateTime<chrono::Utc>,
    pub duration_secs: u64,
    pub summary: EventSummary,
    pub entries: Vec<TimelineEntry>,
    pub periodic_patterns: Vec<PeriodicPattern>,
    pub root_causes: Vec<RootCauseAttribution>,
    pub display_correlations: Vec<DisplayCorrelation>,
}

impl TimelineReport {
    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
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
            thunderbolt_routers: Vec::new(),
            type_c_ports: Vec::new(),
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
    fn root_causes_point_at_upstream_hub_when_it_dropped() {
        let topo = topo_with(&[("dock", None), ("ssd", Some("dock"))]);
        let dock = &topo.devices[0];
        let ssd = &topo.devices[1];

        let events = vec![
            event(EventType::DeviceDisconnected, Some(dock.id), 100),
            event(EventType::DeviceDisconnected, Some(ssd.id), 101),
            event(EventType::DeviceReEnumerated, Some(dock.id), 104),
            event(EventType::DeviceReEnumerated, Some(ssd.id), 105),
        ];
        let causes = attribute_root_causes(&topo, &events);
        assert_eq!(causes.len(), 4);
        // The SSD's re-enum is attributed to the dock (1s after dock's).
        let ssd_cause = causes
            .iter()
            .find(|c| c.affected_device == Some(ssd.id) && c.is_upstream)
            .expect("ssd attributed upstream");
        assert_eq!(ssd_cause.upstream_hub_id, Some(dock.id));
        // But the dock's own re-enum has no ancestor → device-local.
        let dock_local = causes.iter().find(|c| !c.is_upstream).expect("local cause");
        assert_eq!(dock_local.affected_device, Some(dock.id));
    }

    #[test]
    fn periodic_drops_detected_only_for_regular_intervals() {
        let topo = topo_with(&[("flapper", None)]);
        let dev = topo.devices[0].id;
        // Drops every 45s ±0 — clock-like.
        let regular: Vec<DiagnosticEvent> = (0..5)
            .map(|i| event(EventType::DeviceDisconnected, Some(dev), 1000 + i * 45))
            .collect();
        let patterns = detect_periodic_drops(&regular, 0.25);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].device_id, dev);
        assert!((patterns[0].interval_mean_secs - 45.0).abs() < 0.001);
        assert!(patterns[0].interval_stddev_secs < 1.0);

        // Irregular drops don't qualify.
        let irregular = vec![
            event(EventType::DeviceDisconnected, Some(dev), 100),
            event(EventType::DeviceDisconnected, Some(dev), 200),
            event(EventType::DeviceDisconnected, Some(dev), 500),
            event(EventType::DeviceDisconnected, Some(dev), 900),
        ];
        assert!(detect_periodic_drops(&irregular, 0.25).is_empty());
    }

    #[test]
    fn display_events_pair_with_nearby_usb_events() {
        let events = vec![
            event(EventType::HubDisconnected, None, 100),
            event(EventType::DisplayDisconnected, None, 102), // 2s later
            event(EventType::DisplayConnected, None, 600),
        ];
        let pairs = correlate_display_events(&events, 5);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].usb_event_type, EventType::HubDisconnected);
        assert_eq!(pairs[0].display_event_type, EventType::DisplayDisconnected);
        assert_eq!(pairs[0].delta_ms, 2000);

        // Outside the window nothing pairs.
        assert!(correlate_display_events(&events, 1).is_empty());
    }

    #[test]
    fn timeline_report_serializes_with_all_sections() {
        let report = TimelineReport {
            generated: chrono::Utc::now(),
            duration_secs: 60,
            summary: EventSummary::default(),
            entries: vec![TimelineEntry {
                timestamp: chrono::Utc::now(),
                kind: "DeviceConnected".into(),
                description: "FlashDrive".into(),
            }],
            periodic_patterns: Vec::new(),
            root_causes: Vec::new(),
            display_correlations: Vec::new(),
        };
        let json = report.to_pretty_json().expect("serializes");
        for key in [
            "\"generated\"",
            "\"summary\"",
            "\"entries\"",
            "\"periodic_patterns\"",
            "\"root_causes\"",
            "\"display_correlations\"",
        ] {
            assert!(json.contains(key), "missing {key}");
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
