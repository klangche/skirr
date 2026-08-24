//! Hub bandwidth allocation analysis (Phase 10.1).
//!
//! Heuristic accounting: a hub's usable uplink capacity is compared against
//! the sum of its directly-attached children's negotiated link speeds. This
//! cannot see active traffic (no OS API exposes per-device utilization), but
//! it reliably flags oversubscription — the "everything through one cable"
//! scenario Skirr exists to diagnose.

use crate::model::{BottleneckSeverity, SystemTopology, UsbDevice};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HubBandwidth {
    pub hub_id: Uuid,
    pub hub_label: String,
    /// The hub's own negotiated uplink speed in Mb/s.
    pub uplink_mbps: u32,
    /// Sum of children's negotiated link speeds in Mb/s.
    pub used_downstream_mbps: u32,
    /// used / uplink, percent. None when the uplink speed is unknown.
    pub utilization_pct: Option<f32>,
}

impl HubBandwidth {
    /// Thresholds for oversubscription flagging: ≥95 % is critical (traffic
    /// will stall), ≥70 % is major (no headroom left).
    pub fn severity(&self) -> BottleneckSeverity {
        match self.utilization_pct {
            Some(pct) if pct >= 95.0 => BottleneckSeverity::Critical,
            Some(pct) if pct >= 70.0 => BottleneckSeverity::Major,
            _ => BottleneckSeverity::Minor,
        }
    }
}

/// Analyze every physical hub's bandwidth budget. Hubs with unknown uplink or
/// no children are skipped (nothing meaningful to report). Results are sorted
/// by utilization, most-saturated first.
pub fn analyze_bandwidth(topo: &SystemTopology) -> Vec<HubBandwidth> {
    let mut children_of: std::collections::HashMap<Uuid, Vec<&UsbDevice>> =
        std::collections::HashMap::new();
    for dev in &topo.devices {
        if let Some(pid) = dev.parent_id {
            children_of.entry(pid).or_default().push(dev);
        }
    }

    let mut report = Vec::new();
    for hub in &topo.devices {
        let kids = match children_of.get(&hub.id) {
            Some(k) if !k.is_empty() => k,
            _ => continue,
        };
        let uplink_mbps = hub.current_link_speed.mbps() as u32;
        if uplink_mbps == 0 {
            continue; // unknown hub link → no meaningful ratio
        }
        let used_downstream_mbps: u32 = kids
            .iter()
            .map(|d| d.current_link_speed.mbps() as u32)
            .sum();
        let utilization_pct = Some((used_downstream_mbps as f32 / uplink_mbps as f32) * 100.0);
        report.push(HubBandwidth {
            hub_id: hub.id,
            hub_label: hub_label(hub),
            uplink_mbps,
            used_downstream_mbps,
            utilization_pct,
        });
    }
    report.sort_by(|a, b| {
        b.utilization_pct
            .unwrap_or(0.0)
            .total_cmp(&a.utilization_pct.unwrap_or(0.0))
            .then_with(|| a.hub_label.cmp(&b.hub_label))
    });
    report
}

fn hub_label(hub: &UsbDevice) -> String {
    hub.product
        .clone()
        .unwrap_or_else(|| format!("{:04x}:{:04x}", hub.vendor_id, hub.product_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PlatformInfo, UsbSpeed};

    fn topo(devices: Vec<UsbDevice>) -> SystemTopology {
        SystemTopology {
            timestamp: chrono::Utc::now(),
            host_controllers: Vec::new(),
            root_hubs: Vec::new(),
            devices,
            hubs: Vec::new(),
            displays: Vec::new(),
            thunderbolt_routers: Vec::new(),
            events: Vec::new(),
            platform_info: PlatformInfo {
                os: "test".into(),
                os_version: String::new(),
                kernel_version: None,
                architecture: "arm64".into(),
                hostname: None,
                username: None,
                is_admin: false,
                is_virtual_machine: false,
                boot_time: None,
            },
        }
    }

    fn device(parent: Option<Uuid>, speed: UsbSpeed) -> UsbDevice {
        let mut d = UsbDevice::new(0x2109, 0x0817);
        d.parent_id = parent;
        d.current_link_speed = speed;
        d.max_supported_speed = speed;
        d
    }

    #[test]
    fn oversubscribed_hub_flags_critical() {
        let uplink_hub = Uuid::new_v4();
        let hub_id = Uuid::new_v4();
        // USB 2.0 hub at 480 Mb/s feeding three SuperSpeed+ devices.
        let topo = topo(vec![
            device(None, UsbSpeed::HighSpeed),
            {
                let mut hub = device(Some(uplink_hub), UsbSpeed::HighSpeed);
                hub.id = hub_id;
                hub.is_hub = true;
                hub
            },
            device(Some(hub_id), UsbSpeed::SuperSpeedPlus20),
            device(Some(hub_id), UsbSpeed::SuperSpeedPlus20),
            device(Some(hub_id), UsbSpeed::SuperSpeed),
        ]);

        let report = analyze_bandwidth(&topo);
        assert_eq!(report.len(), 1);
        let hbw = &report[0];
        assert_eq!(hbw.uplink_mbps, 480);
        assert_eq!(hbw.used_downstream_mbps, 20_000 + 20_000 + 5_000);
        assert!(hbw.utilization_pct.unwrap() > 900.0);
        assert_eq!(hbw.severity(), BottleneckSeverity::Critical);
    }

    #[test]
    fn healthy_hub_stays_minor_and_unknowns_are_skipped() {
        let unknown_hub = Uuid::new_v4();
        let mut unknown_hub_dev = device(None, UsbSpeed::Unknown);
        unknown_hub_dev.id = unknown_hub;

        let healthy_hub = Uuid::new_v4();
        let mut healthy_hub_dev = device(None, UsbSpeed::SuperSpeed);
        healthy_hub_dev.id = healthy_hub;

        let topo = topo(vec![
            unknown_hub_dev,
            device(Some(unknown_hub), UsbSpeed::HighSpeed),
            device(Some(healthy_hub), UsbSpeed::HighSpeed),
            device(Some(healthy_hub), UsbSpeed::FullSpeed),
            healthy_hub_dev,
        ]);

        let report = analyze_bandwidth(&topo);
        assert_eq!(
            report.len(),
            1,
            "unknown-uplink hub is skipped; empty hubs report nothing"
        );
        let hbw = &report[0];
        assert_eq!(hbw.hub_id, healthy_hub);
        assert_eq!(hbw.uplink_mbps, 5_000);
        assert_eq!(hbw.used_downstream_mbps, 480 + 12);
        assert!(hbw.utilization_pct.unwrap() < 15.0);
        assert_eq!(hbw.severity(), BottleneckSeverity::Minor);
    }

    #[test]
    fn severity_thresholds_hold() {
        let mk = |pct: f32| HubBandwidth {
            hub_id: Uuid::new_v4(),
            hub_label: "t".into(),
            uplink_mbps: 100,
            used_downstream_mbps: pct as u32,
            utilization_pct: Some(pct),
        };
        assert_eq!(mk(96.0).severity(), BottleneckSeverity::Critical);
        assert_eq!(mk(75.0).severity(), BottleneckSeverity::Major);
        assert_eq!(mk(10.0).severity(), BottleneckSeverity::Minor);
        assert_eq!(
            HubBandwidth {
                hub_id: Uuid::new_v4(),
                hub_label: "t".into(),
                uplink_mbps: 0,
                used_downstream_mbps: 5,
                utilization_pct: None,
            }
            .severity(),
            BottleneckSeverity::Minor,
            "unknown utilization never alarms"
        );
    }
}
