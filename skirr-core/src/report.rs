//! Canonical JSON report envelope (Phase 8.2).
//!
//! One typed wrapper guarantees a stable, versioned schema for `skirr report`
//! output. Every section serializes even when empty (empty arrays, never
//! absent keys), so consumers can pin against `schema_version` and index
//! safely. Thunderbolt/USB4 sections are reserved until Phase 11 lands the
//! collectors; the topology carries per-device USB-C info today.

use crate::{DiagnosticResult, PlatformInfo, SystemTopology};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Current schema version of the JSON report.
pub const SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkirrReport {
    pub schema_version: String,
    pub tool: ToolInfo,
    pub generated: DateTime<Utc>,
    pub profile_name: String,
    pub platform: PlatformInfo,
    pub topology: SystemTopology,
    pub diagnosis: DiagnosticResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub version: String,
}

impl SkirrReport {
    pub fn new(
        profile_name: impl Into<String>,
        platform: PlatformInfo,
        topology: SystemTopology,
        diagnosis: DiagnosticResult,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            tool: ToolInfo {
                name: "skirr".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            generated: Utc::now(),
            profile_name: profile_name.into(),
            platform,
            topology,
            diagnosis,
        }
    }

    /// Pretty-printed JSON (the default human-diffable form).
    pub fn to_pretty_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Minified single-line JSON (`--compact`).
    pub fn to_compact_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConnectionStatus, EventSummary, Fact, FactCategory, HostController,
        HostControllerCapabilities, RuleEvaluation, UsbClass, Verdict,
    };

    fn empty_platform_info() -> PlatformInfo {
        PlatformInfo {
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

    fn test_device(vid: u16, pid: u16, product: &str, class: UsbClass) -> crate::UsbDevice {
        let mut d = crate::UsbDevice::new(vid, pid);
        d.product = Some(product.into());
        d.device_class = class;
        d.is_hub = class == UsbClass::Hub;
        d.connection_status = ConnectionStatus::Connected;
        d
    }

    fn sample_report() -> SkirrReport {
        let hub = test_device(0x2109, 0x0817, "Dock", UsbClass::Hub);
        let platform_info = empty_platform_info();
        let topo = SystemTopology {
            timestamp: Utc::now(),
            host_controllers: vec![HostController {
                id: uuid::Uuid::new_v4(),
                platform_id: "xhci-0".into(),
                name: "Test xHCI".into(),
                vendor_id: None,
                device_id: None,
                revision: None,
                usb_version: crate::UsbSpeed::SuperSpeed,
                root_hub_ids: vec![],
                port_count: 4,
                is_xhci: true,
                pci_address: None,
                driver_version: None,
                capabilities: HostControllerCapabilities::default(),
            }],
            root_hubs: vec![],
            devices: vec![hub],
            hubs: vec![],
            displays: vec![],
            thunderbolt_routers: vec![],
            type_c_ports: vec![],
            events: vec![],
            platform_info,
        };
        let diagnosis = DiagnosticResult {
            timestamp: Utc::now(),
            profile_version: "1.0".into(),
            overall_verdict: Verdict::Pass,
            facts: vec![Fact {
                id: "f".into(),
                category: FactCategory::Platform,
                description: "os".into(),
                value: serde_json::json!("macOS"),
                source: "test".into(),
                confidence: 100,
            }],
            rules_applied: vec![RuleEvaluation {
                rule_id: "R01".into(),
                rule_description: "d".into(),
                rule_category: FactCategory::Speed,
                expected: serde_json::json!("5000"),
                actual: serde_json::json!("480"),
                verdict: Verdict::Warning,
                explanation: "e".into(),
            }],
            bottlenecks: vec![],
            topology_issues: vec![],
            display_issues: vec![],
            usb_c_issues: vec![],
            power_issues: vec![],
            event_summary: EventSummary::default(),
            recommendations: vec![],
        };
        SkirrReport::new(
            "Skirr Standard Profile v1.0",
            topo.platform_info.clone(),
            topo,
            diagnosis,
        )
    }

    #[test]
    fn round_trip_preserves_all_sections() {
        let report = sample_report();
        let json = report.to_pretty_json().expect("pretty");
        let back: SkirrReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert_eq!(back.tool.name, "skirr");
        assert_eq!(back.topology.devices.len(), 1);
        assert_eq!(back.diagnosis.rules_applied.len(), 1);
        // Empty sections must serialize as [], never be absent.
        assert!(json.contains("\"displays\": []"));
        assert!(json.contains("\"events\": []"));
        assert!(json.contains("\"usb_c_issues\": []"));
    }

    #[test]
    fn compact_is_single_line_and_equivalent() {
        let report = sample_report();
        let compact = report.to_compact_json().expect("compact");
        assert!(!compact.contains('\n'), "no newlines: {compact}");
        let back: SkirrReport = serde_json::from_str(&compact).expect("deserialize compact");
        assert_eq!(back.generated, report.generated);
        assert_eq!(back.diagnosis.overall_verdict, Verdict::Pass);
    }

    #[test]
    fn schema_version_is_semverish_string() {
        let json = sample_report().to_compact_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["schema_version"], "1.0");
        assert!(v["topology"]["devices"].is_array());
        assert!(v["diagnosis"]["overall_verdict"].is_string());
    }
}
