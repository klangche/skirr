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

// ---------------------------------------------------------------------------
// Display bandwidth planning (Phase 10.3)
// ---------------------------------------------------------------------------

/// Estimated video payload of one display, used for multi-display planning.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DisplayRequirement {
    pub display_platform_id: String,
    pub name: String,
    /// Estimated payload in Mb/s including blanking and encoding overhead.
    pub required_mbps: u32,
    /// Nearest upstream physical hub, if the display hangs under one.
    pub upstream_hub_label: Option<String>,
    /// The estimate alone exceeds that hub's uplink capacity.
    pub exceeds_upstream_uplink: bool,
}

/// Multi-display bandwidth plan: per-display estimates plus the combined
/// load. Estimates only — real tunneling shares lane capacity with USB/PCIe,
/// which no OS API exposes; this flags the gross oversubscription cases.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MultiDisplayPlan {
    pub requirements: Vec<DisplayRequirement>,
    pub total_required_mbps: u32,
}

impl MultiDisplayPlan {
    pub fn is_empty(&self) -> bool {
        self.requirements.is_empty()
    }
}

/// Rough video payload: pixels × refresh × bits-per-pixel × 1.25 (blanking +
/// 8b/10b). Good enough to spot "4K60 through a 480 Mb/s hub" nonsense.
pub fn estimate_display_mbps(width: u32, height: u32, refresh_hz: f32, bits_per_pixel: u32) -> u32 {
    ((width as f64 * height as f64 * refresh_hz as f64 * bits_per_pixel as f64 * 1.25)
        / 1_000_000.0) as u32
}

/// Plan display payloads against the hub chains they hang from.
pub fn plan_display_bandwidth(topo: &SystemTopology) -> MultiDisplayPlan {
    let by_id: std::collections::HashMap<Uuid, &UsbDevice> =
        topo.devices.iter().map(|d| (d.id, d)).collect();
    let mut requirements = Vec::new();

    for display in &topo.displays {
        let Some(res) = display.current_resolution.as_ref() else {
            continue;
        };
        // HDR panels commonly run 30 bpp (10-bit RGB).
        let bpp = if display.hdr_supported { 30 } else { 24 };
        let required_mbps = estimate_display_mbps(
            res.width as u32,
            res.height as u32,
            display.current_refresh_rate.unwrap_or(60) as f32,
            bpp,
        );
        // Locate the USB device the display hangs from: `usb_path` (GPU
        // driver-provided device chain) wins, platform-id match is the
        // fallback for backends that mirror display IDs onto devices.
        let mut cursor = display
            .usb_path
            .as_ref()
            .and_then(|path| path.last())
            .and_then(|tip| by_id.get(tip).copied())
            .or_else(|| {
                topo.devices
                    .iter()
                    .find(|d| d.platform_id == display.platform_id)
            });
        let mut upstream_hub_label = None;
        let mut exceeds = false;
        while let Some(dev) = cursor {
            if dev.is_hub || dev.hub_info.is_some() {
                upstream_hub_label = Some(hub_label(dev));
                let uplink = dev.current_link_speed.mbps() as u32;
                exceeds = uplink > 0 && required_mbps > uplink;
                break;
            }
            cursor = dev.parent_id.and_then(|pid| by_id.get(&pid)).copied();
        }
        requirements.push(DisplayRequirement {
            display_platform_id: display.platform_id.clone(),
            name: display.name.clone().unwrap_or_else(|| "Display".into()),
            required_mbps,
            upstream_hub_label,
            exceeds_upstream_uplink: exceeds,
        });
    }
    requirements.sort_by(|a, b| b.required_mbps.cmp(&a.required_mbps));
    MultiDisplayPlan {
        total_required_mbps: requirements.iter().map(|r| r.required_mbps).sum(),
        requirements,
    }
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
            type_c_ports: Vec::new(),
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

    #[test]
    fn display_estimate_matches_hand_math() {
        // 4K60 24bpp: 3840*2160*60*24*1.25 / 1e6 ≈ 14.9 Gb/s.
        let est = estimate_display_mbps(3840, 2160, 60.0, 24);
        assert!((14_900..15_000).contains(&est), "got {est}");
        // 1080p60 with overhead is ~3.7 Gb/s.
        let fhd = estimate_display_mbps(1920, 1080, 60.0, 24);
        assert!((3_700..3_800).contains(&fhd), "got {fhd}");
    }

    #[test]
    fn display_plan_flags_display_behind_slow_hub() {
        use crate::model::{DisplayInfo, DisplayResolution};

        let mut topo = topo(Vec::new());
        let mut hub = device(None, UsbSpeed::HighSpeed); // 480 Mb/s uplink
        hub.is_hub = true;
        topo.devices.push(hub.clone());

        let display = DisplayInfo {
            id: uuid::Uuid::new_v4(),
            platform_id: "DISPLAY\\TEST1".into(),
            manufacturer_id: None,
            product_code: None,
            serial_number: None,
            manufacture_week: None,
            manufacture_year: None,
            edid_version: None,
            name: Some("Studio Panel".into()),
            serial_number_str: None,
            max_horizontal_size_cm: None,
            max_vertical_size_cm: None,
            supported_resolutions: Vec::new(),
            preferred_resolution: None,
            current_resolution: Some(DisplayResolution {
                width: 1920,
                height: 1080,
                aspect_ratio: None,
                is_interlaced: false,
            }),
            refresh_rates: vec![60],
            current_refresh_rate: Some(60),
            color_depth: None,
            hdr_supported: false,
            hdr_metadata: None,
            display_type: crate::DisplayType::Unknown,
            connection_type: None,
            gpu_id: None,
            usb_path: Some(vec![topo.devices[0].id]),
            is_primary: false,
            is_internal: false,
            is_enabled: true,
            position: None,
            scale_factor: None,
            edid_raw: None,
        };
        topo.displays.push(display);

        let plan = plan_display_bandwidth(&topo);
        assert_eq!(plan.requirements.len(), 1);
        let req = &plan.requirements[0];
        assert_eq!(req.name, "Studio Panel");
        assert!(req.exceeds_upstream_uplink, "~3.1 Gb/s cannot fit 480 Mb/s");
        assert_eq!(req.upstream_hub_label.as_deref(), Some("2109:0817"));
        assert_eq!(plan.total_required_mbps, req.required_mbps);
    }
}
