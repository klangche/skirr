//! Rule engine - facts from the normalized model, evaluated against a profile.
//!
//! Pipeline: `SystemTopology` -> `Vec<Fact>` -> `Vec<RuleEvaluation>`
//! -> `DiagnosticResult` with an overall PASS/WARNING/FAIL verdict and a
//! Shoko-style FACT/RULE/VERDICT text report.

use crate::model::{
    BottleneckSeverity, DiagnosticResult, Fact, FactCategory, RuleEvaluation, SpeedBottleneck,
    SystemTopology, TopologyIssue, TopologyIssueType, UsbClass, UsbDevice, Verdict,
};
use crate::profile::{PlatformKey, Profile};
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

/// Evaluates a topology against a profile.
pub struct RuleEngine {
    pub profile: Profile,
}

impl RuleEngine {
    /// Engine with a custom profile.
    pub fn new(profile: Profile) -> Self {
        Self { profile }
    }

    /// Engine with the built-in Skirr Standard Profile v1.0.
    pub fn standard() -> Self {
        Self::new(Profile::standard_v1())
    }

    /// Run all rules against a topology snapshot.
    ///
    /// Chain metrics (hops/tiers/external hubs) are recomputed here from the
    /// parent/child links rather than trusting per-device cached counters,
    /// so stub or partially-filled backends still evaluate correctly.
    pub fn evaluate(&self, topology: &SystemTopology) -> DiagnosticResult {
        let facts = collect_facts(topology);
        let platform = PlatformKey::detect(
            &topology.platform_info.os,
            &topology.platform_info.architecture,
        );
        let limits = platform.and_then(|p| self.profile.limits_for(p));

        let metrics = compute_chain_metrics(topology);

        let mut rules_applied: Vec<RuleEvaluation> = Vec::new();
        let mut topology_issues: Vec<TopologyIssue> = Vec::new();

        match limits {
            Some(lim) => {
                let structural = [
                    ("max_hops", metrics.deepest_hops, lim.max_hops),
                    ("max_tiers", metrics.deepest_tiers, lim.max_tiers),
                    ("max_hubs", metrics.worst_path_external_hubs, lim.max_hubs),
                ];
                for (rule_id, actual, expected) in structural {
                    let verdict = verdict_at_or_under(actual, expected);
                    if matches!(verdict, Verdict::Fail | Verdict::Warning) {
                        let issue_type = match rule_id {
                            "max_hops" => TopologyIssueType::TooManyHops,
                            "max_tiers" => TopologyIssueType::TooManyTiers,
                            _ => TopologyIssueType::TooManyHubs,
                        };
                        for device in &metrics.deepest_path {
                            topology_issues.push(TopologyIssue {
                                device_id: device.id,
                                issue_type,
                                description: format!(
                                    "{rule_id} exceeded: {} > limit {}",
                                    actual, expected
                                ),
                                severity: verdict,
                                current_value: actual,
                                max_allowed: expected,
                            });
                        }
                    }
                    rules_applied.push(RuleEvaluation {
                        rule_id: rule_id.to_string(),
                        rule_description: structural_rule_text(rule_id),
                        rule_category: FactCategory::Topology,
                        expected: json!(expected),
                        actual: json!(actual),
                        verdict,
                        explanation: format!(
                            "worst path has {actual} (limit {expected}); verdict {}",
                            VerdictText(verdict)
                        ),
                    });
                }
            }
            None => {
                rules_applied.push(RuleEvaluation {
                    rule_id: "platform_limits".to_string(),
                    rule_description: "Structural limits for this platform are not defined"
                        .to_string(),
                    rule_category: FactCategory::Topology,
                    expected: json!("known platform"),
                    actual: json!(format!(
                        "{} / {}",
                        topology.platform_info.os, topology.platform_info.architecture
                    )),
                    verdict: Verdict::Unknown,
                    explanation: "no StabilityLimits entry; add one to the profile".to_string(),
                });
            }
        }

        // Speed bottlenecks: USB3+ capable device stuck on a slower link.
        let mut bottlenecks: Vec<SpeedBottleneck> = Vec::new();
        for device in &topology.devices {
            if let Some(b) = device.speed_bottleneck() {
                bottlenecks.push(b.clone());
                rules_applied.push(RuleEvaluation {
                    rule_id: format!("speed_bottleneck[{}]", short_id(device.id)),
                    rule_description: "Device linked below its capability".to_string(),
                    rule_category: FactCategory::Speed,
                    expected: json!(device.max_supported_speed.mbps()),
                    actual: json!(device.current_link_speed.mbps()),
                    verdict: severity_verdict(b.severity),
                    explanation: format!(
                        "{} links at {} but supports {} ({:?})",
                        display_name(device),
                        device.current_link_speed.marketing_name(),
                        device.max_supported_speed.marketing_name(),
                        b.severity
                    ),
                });
            }
        }

        // Orphaned devices: parent pointer does not resolve.
        let known: std::collections::HashSet<Uuid> =
            topology.devices.iter().map(|d| d.id).collect();
        let orphans: Vec<&UsbDevice> = topology
            .devices
            .iter()
            .filter(|d| d.parent_id.is_some_and(|p| !known.contains(&p)))
            .collect();
        for orphan in &orphans {
            rules_applied.push(RuleEvaluation {
                rule_id: format!("orphaned_device[{}]", short_id(orphan.id)),
                rule_description: "Device parent is missing from enumeration".to_string(),
                rule_category: FactCategory::Device,
                expected: json!("resolvable parent"),
                actual: json!(orphan.parent_id.map(short_id_string).unwrap_or_default()),
                verdict: Verdict::Warning,
                explanation: format!("{} has no resolvable parent hub", display_name(orphan)),
            });
        }

        let overall = worst_verdict(rules_applied.iter().map(|r| r.verdict));
        let mut recommendations = build_recommendations(
            &rules_applied,
            &bottlenecks,
            &metrics.deepest_path,
            limits.map(|l| l.max_hubs),
        );

        if overall == Verdict::Pass {
            recommendations.push("No stability risks detected under this profile.".to_string());
        }

        DiagnosticResult {
            timestamp: Utc::now(),
            profile_version: self.profile.version.clone(),
            overall_verdict: overall,
            facts,
            rules_applied,
            bottlenecks,
            topology_issues,
            display_issues: Vec::new(),
            usb_c_issues: Vec::new(),
            power_issues: Vec::new(),
            event_summary: Default::default(),
            recommendations,
        }
    }
}

fn structural_rule_text(rule_id: &str) -> String {
    match rule_id {
        "max_hops" => "Deepest chain hop count within platform limit".to_string(),
        "max_tiers" => "Deepest chain tier count within platform limit".to_string(),
        "max_hubs" => "External hubs on worst path within platform limit".to_string(),
        other => other.to_string(),
    }
}

/// PASS when under limit, WARNING exactly at limit, FAIL above it.
fn verdict_at_or_under(actual: u8, limit: u8) -> Verdict {
    if actual > limit {
        Verdict::Fail
    } else if actual == limit {
        Verdict::Warning
    } else {
        Verdict::Pass
    }
}

fn severity_verdict(severity: BottleneckSeverity) -> Verdict {
    match severity {
        BottleneckSeverity::Minor | BottleneckSeverity::Major => Verdict::Warning,
        BottleneckSeverity::Critical => Verdict::Fail,
    }
}

fn worst_verdict(verdicts: impl Iterator<Item = Verdict>) -> Verdict {
    verdicts.fold(Verdict::Pass, |acc, v| match (acc, v) {
        (Verdict::Fail, _) | (_, Verdict::Fail) => Verdict::Fail,
        (Verdict::Unknown, _) | (_, Verdict::Unknown) => Verdict::Unknown,
        (Verdict::Warning, _) | (_, Verdict::Warning) => Verdict::Warning,
        _ => Verdict::Pass,
    })
}

/// Aggregated chain metrics derived from parent links.
#[derive(Debug, Default)]
struct ChainMetrics {
    deepest_hops: u8,
    deepest_tiers: u8,
    worst_path_external_hubs: u8,
    /// Devices along the worst (deepest) path, root-first.
    deepest_path: Vec<UsbDevice>,
}

fn compute_chain_metrics(topology: &SystemTopology) -> ChainMetrics {
    let by_id: HashMap<Uuid, &UsbDevice> = topology.devices.iter().map(|d| (d.id, d)).collect();

    let mut best = ChainMetrics::default();

    for device in &topology.devices {
        // Walk ancestors up to the root.
        let mut hops: u8 = 0;
        let mut external_hubs: u8 = 0;
        let mut path: Vec<UsbDevice> = Vec::new();
        path.push(device.clone());

        let mut cursor = device.parent_id;
        while let Some(pid) = cursor {
            if let Some(parent_dev) = by_id.get(&pid) {
                let next_cursor = parent_dev.parent_id;
                let parent = (*parent_dev).clone();
                if parent.device_class == UsbClass::Hub || parent.is_hub {
                    hops += 1;
                    if !parent.is_internal {
                        external_hubs += 1;
                    }
                }
                path.push(parent);
                cursor = next_cursor;
            } else {
                break;
            }
        }

        let tiers = hops.saturating_add(1);
        if best.deepest_path.is_empty() || hops > best.deepest_hops {
            path.reverse(); // root-first
            best = ChainMetrics {
                deepest_hops: hops,
                deepest_tiers: tiers,
                worst_path_external_hubs: external_hubs,
                deepest_path: path,
            };
        }
    }

    best
}

fn build_recommendations(
    rules: &[RuleEvaluation],
    bottlenecks: &[SpeedBottleneck],
    deepest_path: &[UsbDevice],
    max_hubs: Option<u8>,
) -> Vec<String> {
    let mut recs = Vec::new();

    if let Some(limit) = max_hubs {
        let failed_hub_rule = rules.iter().any(|r| {
            r.rule_id == "max_hubs" && matches!(r.verdict, Verdict::Fail | Verdict::Warning)
        });
        if failed_hub_rule {
            recs.push(format!(
                "Chain exceeds the safe external-hub budget ({}); remove at least one powered hub between host and end device.",
                limit
            ));
        }
    }

    for b in bottlenecks {
        if b.severity != BottleneckSeverity::Minor {
            recs.push(format!(
                "Device at {:?} runs at {} but supports {}; connect it directly to a faster port or replace the cable/hub in between.",
                b.device_id,
                b.current_speed.marketing_name(),
                b.max_speed.marketing_name()
            ));
        }
    }

    if rules
        .iter()
        .any(|r| r.rule_id.starts_with("orphaned_device"))
    {
        recs.push(
            "One or more devices lost their parent hub; replug the affected branch and rescan."
                .to_string(),
        );
    }

    let _ = deepest_path;
    recs
}

fn display_name(device: &UsbDevice) -> String {
    let product = device.product.as_deref().unwrap_or("unknown product");
    format!(
        "{product} [{:04X}:{:04X}]",
        device.vendor_id, device.product_id
    )
}

fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_string()
}

fn short_id_string(id: Uuid) -> String {
    short_id(id)
}

/// Extract normalized facts from a topology snapshot.
///
/// Facts carry `confidence` 100 for direct measurements, lower where the
/// value is heuristic-derived (see docs/DATA_MAP.md confidence scale).
pub fn collect_facts(topology: &SystemTopology) -> Vec<Fact> {
    let mut facts = Vec::new();

    facts.push(Fact {
        id: "platform.os".to_string(),
        category: FactCategory::Platform,
        description: "Host operating system family".to_string(),
        value: json!({
            "os": topology.platform_info.os,
            "version": topology.platform_info.os_version,
            "arch": topology.platform_info.architecture,
        }),
        source: "skirr-core".to_string(),
        confidence: 100,
    });

    let total = topology.devices.len();
    let hubs_total = topology.devices.iter().filter(|d| d.is_hub).count();
    let hubs_external = topology
        .devices
        .iter()
        .filter(|d| d.is_hub && !d.is_internal)
        .count();
    facts.push(Fact {
        id: "devices.count".to_string(),
        category: FactCategory::Device,
        description: "Present USB device counts".to_string(),
        value: json!({
            "total": total,
            "hubs_total": hubs_total,
            "hubs_external": hubs_external,
        }),
        source: "skirr-core".to_string(),
        confidence: 100,
    });

    let metrics = compute_chain_metrics(topology);
    facts.push(Fact {
        id: "chain.deepest".to_string(),
        category: FactCategory::Topology,
        description: "Deepest chain metrics (recomputed from parent links)".to_string(),
        value: json!({
            "hops": metrics.deepest_hops,
            "tiers": metrics.deepest_tiers,
            "external_hubs_on_worst_path": metrics.worst_path_external_hubs,
        }),
        source: "skirr-core".to_string(),
        confidence: 90,
    });

    facts.push(Fact {
        id: "controllers.count".to_string(),
        category: FactCategory::HostController,
        description: "USB host controllers present".to_string(),
        value: json!(topology.host_controllers.len()),
        source: "skirr-core".to_string(),
        confidence: 100,
    });

    facts.push(Fact {
        id: "displays.count".to_string(),
        category: FactCategory::Display,
        description: "Displays reported in snapshot".to_string(),
        value: json!(topology.displays.len()),
        source: "skirr-core".to_string(),
        confidence: 80,
    });

    facts
}

/// Render a Shoko-style FACT/RULE/VERDICT text report.
pub fn format_report(result: &DiagnosticResult) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let _ = writeln!(out, "==== Skirr Diagnostic Report ====");
    let _ = writeln!(
        out,
        "profile : {} v{}",
        result.profile_version, result.profile_version
    );
    let _ = writeln!(out, "--- FACT ---");
    for f in &result.facts {
        let _ = writeln!(
            out,
            "FACT   | {:?} | {} = {}",
            f.category, f.description, f.value
        );
    }
    let _ = writeln!(out, "--- RULE ---");
    for r in &result.rules_applied {
        let _ = writeln!(
            out,
            "RULE   | {} | {} | expected={} actual={} :: {}",
            r.rule_id,
            VerdictText(r.verdict),
            r.expected,
            r.actual,
            r.explanation
        );
    }
    let _ = writeln!(out, "--- VERDICT ---");
    let _ = writeln!(
        out,
        "VERDICT| OVERALL | {}",
        VerdictText(result.overall_verdict)
    );
    if !result.recommendations.is_empty() {
        let _ = writeln!(out, "--- RECOMMENDATIONS ---");
        for rec in &result.recommendations {
            let _ = writeln!(out, "* {}", rec);
        }
    }
    out
}

/// Upper-case verdict label for report lines.
struct VerdictText(Verdict);

impl std::fmt::Display for VerdictText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self.0 {
            Verdict::Pass => "PASS",
            Verdict::Warning => "WARNING",
            Verdict::Fail => "FAIL",
            Verdict::Unknown => "UNKNOWN",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ConnectionStatus, EventSummary, HostController, PlatformInfo, RootHub, UsbSpeed,
    };

    fn topo(platform_os: &str, arch: &str, devices: Vec<UsbDevice>) -> SystemTopology {
        SystemTopology {
            timestamp: Utc::now(),
            host_controllers: vec![HostController {
                id: Uuid::new_v4(),
                platform_id: "PCI\\VEN_8086".into(),
                name: "xHC".into(),
                vendor_id: Some(0x8086),
                device_id: None,
                revision: None,
                usb_version: UsbSpeed::SuperSpeed,
                root_hub_ids: vec![],
                port_count: 4,
                is_xhci: true,
                pci_address: None,
                driver_version: None,
                capabilities: Default::default(),
            }],
            root_hubs: vec![RootHub {
                id: Uuid::new_v4(),
                platform_id: "usb-xhci-0".into(),
                host_controller_id: Uuid::new_v4(),
                port_count: 4,
                hub_speed: UsbSpeed::SuperSpeed,
                is_integrated: true,
                children_ids: vec![],
            }],
            devices,
            hubs: vec![],
            displays: vec![],
            thunderbolt_routers: vec![],
            events: vec![],
            platform_info: PlatformInfo {
                os: platform_os.into(),
                os_version: "1.0".into(),
                kernel_version: None,
                architecture: arch.into(),
                hostname: None,
                username: None,
                is_admin: false,
                is_virtual_machine: false,
                boot_time: None,
            },
        }
    }

    fn device(name: &str, is_hub: bool) -> UsbDevice {
        let mut d = UsbDevice::new(0x1234, 0x5678);
        d.product = Some(name.to_string());
        d.is_hub = is_hub;
        d.device_class = if is_hub {
            UsbClass::Hub
        } else {
            UsbClass::MassStorage
        };
        d.connection_status = ConnectionStatus::Connected;
        d
    }

    fn link(parent: &mut UsbDevice, child: &mut UsbDevice) {
        parent.children_ids.push(child.id);
        child.parent_id = Some(parent.id);
    }

    #[test]
    fn simple_direct_devices_pass() {
        let d1 = device("keyboard", false);
        let t = topo("macOS", "aarch64", vec![d1]);
        let result = RuleEngine::standard().evaluate(&t);
        assert_eq!(result.overall_verdict, Verdict::Pass);
        assert!(result.topology_issues.is_empty());
    }

    #[test]
    fn apple_silicon_limits_apply() {
        // Chain: internal hub -> external hub -> stick.
        let mut internal = device("Internal TB Hub", true);
        internal.is_internal = true;
        let mut ext = device("Dock Hub", true);
        let mut stick = device("SSD", false);
        link(&mut internal, &mut ext);
        link(&mut ext, &mut stick);

        let t = topo("macOS", "aarch64", vec![internal, ext, stick]);
        let result = RuleEngine::standard().evaluate(&t);

        // stick: hops=2 (both hubs), tiers=3, external hubs=1 -> within 6/6/4.
        assert_eq!(result.overall_verdict, Verdict::Pass);
        let hops = result
            .rules_applied
            .iter()
            .find(|r| r.rule_id == "max_hops")
            .unwrap();
        assert_eq!(hops.actual, json!(2));
        assert_eq!(hops.expected, json!(6));
    }

    #[test]
    fn exceeding_hub_budget_fails_with_issue() {
        // Windows x86: 5 external hubs allowed; build a 7-hub chain.
        let mut devices: Vec<UsbDevice> = Vec::new();
        let root_hub = device("hub-0", true);
        devices.push(root_hub.clone());
        let mut prev = root_hub;
        for i in 1..7 {
            let mut hub = device(&format!("hub-{i}"), true);
            link(&mut prev, &mut hub);
            devices.push(hub.clone());
            prev = hub;
        }
        let mut leaf = device("leaf", false);
        link(&mut prev, &mut leaf);
        devices.push(leaf);

        let t = topo("Windows", "AMD64", devices);
        let result = RuleEngine::standard().evaluate(&t);

        assert_eq!(result.overall_verdict, Verdict::Fail);
        assert!(result
            .rules_applied
            .iter()
            .any(|r| r.rule_id == "max_hubs" && r.verdict == Verdict::Fail));
        assert!(result
            .topology_issues
            .iter()
            .any(|i| i.issue_type == TopologyIssueType::TooManyHubs));
        assert!(result.recommendations.iter().any(|r| r.contains("hub")));
    }

    #[test]
    fn bottleneck_detected() {
        // 20x gap -> Critical -> FAIL
        let mut d = device("fast ssd", false);
        d.max_supported_speed = UsbSpeed::SuperSpeedPlus10;
        d.current_link_speed = UsbSpeed::HighSpeed;
        let t = topo("Linux", "x86_64", vec![d]);
        let result = RuleEngine::standard().evaluate(&t);
        assert_eq!(result.overall_verdict, Verdict::Fail);

        // 2x gap -> Major -> WARNING
        let mut mild = device("mild", false);
        mild.max_supported_speed = UsbSpeed::SuperSpeedPlus20;
        mild.current_link_speed = UsbSpeed::SuperSpeedPlus10;
        let t3 = topo("Linux", "x86_64", vec![mild]);
        let result3 = RuleEngine::standard().evaluate(&t3);
        assert_eq!(result3.overall_verdict, Verdict::Warning);
    }

    #[test]
    fn unknown_platform_yields_unknown_overall() {
        let d = device("thing", false);
        let t = topo("Haiku", "riscv64", vec![d]);
        let result = RuleEngine::standard().evaluate(&t);
        assert_eq!(result.overall_verdict, Verdict::Unknown);
        assert!(result
            .rules_applied
            .iter()
            .any(|r| r.rule_id == "platform_limits"));
    }

    #[test]
    fn orphaned_child_warns() {
        let mut parent = device("gone-hub", true);
        let mut child = device("child", false);
        link(&mut parent, &mut child);
        // Drop the parent from the snapshot -> orphaned child.
        let t = topo("Windows", "x86_64", vec![child]);
        let result = RuleEngine::standard().evaluate(&t);
        assert_eq!(result.overall_verdict, Verdict::Warning);
        assert!(result
            .rules_applied
            .iter()
            .any(|r| r.rule_id.starts_with("orphaned_device")));
    }

    #[test]
    fn report_contains_fact_rule_verdict_lines() {
        let d = device("keyboard", false);
        let t = topo("macOS", "arm64", vec![d]);
        let result = RuleEngine::standard().evaluate(&t);
        let report = format_report(&result);
        assert!(report.contains("--- FACT ---"));
        assert!(report.contains("--- RULE ---"));
        assert!(report.contains("VERDICT| OVERALL | PASS"));
    }

    #[test]
    fn facts_capture_platform_and_counts() {
        let mut internal_hub = device("int", true);
        internal_hub.is_internal = true;
        let mut ext_hub = device("ext", true);
        let mut cam = device("cam", false);
        link(&mut ext_hub, &mut cam);
        let t = topo("macOS", "aarch64", vec![internal_hub, ext_hub, cam]);

        let facts = collect_facts(&t);
        let counts = facts.iter().find(|f| f.id == "devices.count").unwrap();
        assert_eq!(counts.value["total"], json!(3));
        assert_eq!(counts.value["hubs_external"], json!(1));

        let chain = facts.iter().find(|f| f.id == "chain.deepest").unwrap();
        assert_eq!(chain.value["hops"], json!(1));
    }

    #[test]
    fn event_summary_defaults_present() {
        let summary = EventSummary::default();
        assert_eq!(summary.total_events, 0);
    }
}
