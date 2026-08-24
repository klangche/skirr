//! Output rendering for display commands. Pure functions over the core model
//! (data in → String out) so they stay unit-testable without a backend.
//!
//! Colors come from `colored`, which auto-disables under NO_COLOR and when
//! stdout is not a TTY.

use colored::Colorize;
use skirr_core::{BottleneckSeverity, DiagnosticResult, SystemTopology, UsbDevice, Verdict};
use std::collections::HashMap;
use tabled::settings::Style;
use tabled::{Table, Tabled};

fn speed_label(speed: skirr_core::UsbSpeed) -> &'static str {
    match speed {
        skirr_core::UsbSpeed::Unknown => "unknown",
        skirr_core::UsbSpeed::LowSpeed => "1.5M",
        skirr_core::UsbSpeed::FullSpeed => "12M",
        skirr_core::UsbSpeed::HighSpeed => "480M",
        skirr_core::UsbSpeed::SuperSpeed => "5G",
        skirr_core::UsbSpeed::SuperSpeedPlus10 => "10G",
        skirr_core::UsbSpeed::SuperSpeedPlus20 => "20G",
        skirr_core::UsbSpeed::USB4Gen2x2 => "USB4 G2x2",
        skirr_core::UsbSpeed::USB4Gen3x2 => "USB4 G3x2",
        skirr_core::UsbSpeed::USB4Gen4x2 => "USB4 G4x2",
    }
}

fn verdict_tag(verdict: Verdict) -> colored::ColoredString {
    match verdict {
        Verdict::Pass => "PASS".green(),
        Verdict::Warning => "WARNING".yellow(),
        Verdict::Fail => "FAIL".red().bold(),
        Verdict::Unknown => "UNKNOWN".dimmed(),
    }
}

fn severity_label(severity: BottleneckSeverity) -> colored::ColoredString {
    match severity {
        BottleneckSeverity::Minor => "minor".cyan(),
        BottleneckSeverity::Major => "major".yellow(),
        BottleneckSeverity::Critical => "critical".red().bold(),
    }
}

#[derive(Tabled)]
struct DeviceRow {
    #[tabled(rename = "DEVICE")]
    device: String,
    #[tabled(rename = "VID")]
    vid: String,
    #[tabled(rename = "PID")]
    pid: String,
    #[tabled(rename = "CLASS")]
    class: String,
    #[tabled(rename = "LINK")]
    link: &'static str,
    #[tabled(rename = "PORT")]
    port: String,
    #[tabled(rename = "NAME")]
    name: String,
}

/// Flat device table used by `scan` and `usb`.
pub fn devices_table(devices: &[UsbDevice]) -> String {
    let rows: Vec<DeviceRow> = devices
        .iter()
        .map(|dev| DeviceRow {
            device: truncate(&dev.platform_id, 38),
            vid: format!("0x{:04X}", dev.vendor_id),
            pid: format!("0x{:04X}", dev.product_id),
            class: format!("{:?}", dev.device_class),
            link: speed_label(dev.current_link_speed),
            port: dev
                .port_number
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into()),
            name: dev.product.as_deref().unwrap_or("-").into(),
        })
        .collect();
    Table::new(rows).with(Style::blank()).to_string()
}

/// ASCII tree: controllers → root hubs → devices by tier.
///
/// Tier-1 attachments live under their synthesized root hub
/// (`root_hub_id` set, no device-level parent), deeper chains nest via
/// `parent_id`.
pub fn render_tree(topo: &SystemTopology) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    let by_parent = children_by_parent(topo);

    // ------------------------------------------------------------------
    // INTERNAL: controllers, root hubs, integrated devices.
    // ------------------------------------------------------------------
    let _ = writeln!(out, "{}", "INTERNAL".bold());
    for hc in &topo.host_controllers {
        let _ = writeln!(out, "[{}] {}", hc.platform_id, hc.name);
        for rh in topo
            .root_hubs
            .iter()
            .filter(|r| r.host_controller_id == hc.id)
        {
            let _ = writeln!(
                out,
                "  └─ RootHub {} ({} ports)",
                rh.platform_id, rh.port_count
            );
            let internal_roots: Vec<&UsbDevice> = topo
                .devices
                .iter()
                .filter(|d| d.is_internal && d.parent_id.is_none() && d.root_hub_id == Some(rh.id))
                .collect();
            for dev in &internal_roots {
                write_chain(&mut out, dev, &by_parent, "    ", true);
            }
        }
    }
    let _ = writeln!(out);

    // ------------------------------------------------------------------
    // EXTERNAL: one chain per occupied physical port of each root hub.
    // A dock/hub plugged into a port starts a chain that continues through
    // every hub inside it, so the culprit product in a long chain is
    // visible at a glance.
    // ------------------------------------------------------------------
    let _ = writeln!(out, "{}", "EXTERNAL".bold());
    let mut any_external = false;
    for rh in &topo.root_hubs {
        let hc_name = topo
            .host_controllers
            .iter()
            .find(|h| h.id == rh.host_controller_id)
            .map(|h| h.platform_id.as_str())
            .unwrap_or("?");
        let tier1_external: Vec<&UsbDevice> = topo
            .devices
            .iter()
            .filter(|d| !d.is_internal && d.parent_id.is_none() && d.root_hub_id == Some(rh.id))
            .collect();

        if tier1_external.is_empty() {
            continue;
        }
        any_external = true;
        let _ = writeln!(
            out,
            "RootHub {} (on {hc_name}, {} ports)",
            rh.platform_id, rh.port_count
        );

        // Occupied physical ports, ascending.
        let mut heads: Vec<&UsbDevice> = tier1_external
            .iter()
            .copied()
            .filter(|d| d.port_number.is_some())
            .collect();
        heads.sort_by_key(|d| d.port_number.unwrap_or(0));
        for dev in &heads {
            let _ = write!(out, "  Port {} ── ", dev.port_number.unwrap_or(0));
            write_node(&mut out, dev);
            write_chain(&mut out, dev, &by_parent, "  ", false);
        }

        // Devices without a port number can't be placed on the map.
        let unplaced: Vec<&UsbDevice> = tier1_external
            .iter()
            .copied()
            .filter(|d| d.port_number.is_none())
            .collect();
        for dev in &unplaced {
            let _ = write!(out, "  Port ? ── ");
            write_node(&mut out, dev);
            write_chain(&mut out, dev, &by_parent, "  ", false);
        }

        let mut free: Vec<u8> = (1..=rh.port_count)
            .filter(|p| !heads.iter().any(|d| d.port_number == Some(*p)))
            .collect();
        if !free.is_empty() {
            free.sort_unstable();
            let list: Vec<String> = free.iter().map(|p| p.to_string()).collect();
            let _ = writeln!(out, "  Ports free: {}", list.join(", "));
        }
        let _ = writeln!(out);
    }
    if !any_external {
        let _ = writeln!(out, "(nothing attached)");
        let _ = writeln!(out);
    }

    // Devices no controller claimed (shouldn't happen post-topology, but
    // never silently drop data from the view).
    let orphans: Vec<&UsbDevice> = topo
        .devices
        .iter()
        .filter(|d| d.parent_id.is_none() && d.root_hub_id.is_none())
        .collect();
    if !orphans.is_empty() {
        let _ = writeln!(out, "[unattributed]");
        for dev in orphans {
            write_chain(&mut out, dev, &by_parent, "", true);
        }
    }
    out
}

fn children_by_parent(topo: &SystemTopology) -> HashMap<uuid::Uuid, Vec<&UsbDevice>> {
    let mut by_parent: HashMap<uuid::Uuid, Vec<&UsbDevice>> = HashMap::new();
    for dev in &topo.devices {
        if let Some(pid) = dev.parent_id {
            by_parent.entry(pid).or_default().push(dev);
        }
    }
    for children in by_parent.values_mut() {
        children.sort_by_key(|d| {
            (
                d.port_number.unwrap_or(0),
                d.product.clone().unwrap_or_default(),
            )
        });
    }
    by_parent
}

/// One node line: product (vid:pid) [speed] [HUB np] [flags].
fn write_node(out: &mut String, dev: &UsbDevice) {
    use std::fmt::Write;
    let label = dev
        .product
        .as_deref()
        .or(dev.manufacturer.as_deref())
        .unwrap_or("device");
    let hub_mark = match dev.hub_info.as_ref().map(|h| h.port_count) {
        Some(n) => format!(" [HUB {n}p]"),
        None => {
            if dev.is_hub {
                " [HUB]".to_string()
            } else {
                String::new()
            }
        }
    };
    let speed = match dev.current_link_speed.mbps() {
        0 => String::new(),
        mbps => format!(" [{mbps} Mbps]"),
    };
    let dock = if dev.properties.contains_key("dock_family") {
        format!(" ({})", dev.properties["dock_family"])
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        "{} ({:04X}:{:04X}){}{speed}{dock}",
        label.bold(),
        dev.vendor_id,
        dev.product_id,
        hub_mark
    );
}

/// Recursive chain writer using proper tree glyphs. `prefix` carries the
/// ancestor indentation; `last` styles this level's branch end.
fn write_chain(
    out: &mut String,
    dev: &UsbDevice,
    by_parent: &HashMap<uuid::Uuid, Vec<&UsbDevice>>,
    prefix: &str,
    last: bool,
) {
    use std::fmt::Write;
    let Some(children) = by_parent.get(&dev.id) else {
        return;
    };
    let branch = if last { "  " } else { "│ " };
    let child_prefix = format!("{prefix}{branch}");
    for (i, child) in children.iter().enumerate() {
        let is_last_child = i + 1 == children.len();
        let glyph = if is_last_child { "└─" } else { "├─" };
        let port = child
            .port_number
            .map(|p| format!("p{p} "))
            .unwrap_or_default();
        let _ = write!(out, "{child_prefix}{glyph} {port}");
        write_node(out, child);
        write_chain(out, child, by_parent, &child_prefix, is_last_child);
    }
}

/// Shoko-style FACT/RULE/VERDICT report with color-coded verdicts.
pub fn format_diagnosis(result: &DiagnosticResult) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let _ = writeln!(out, "{}", "==== Skirr Diagnostic Report ====".bold());
    let _ = writeln!(out, "profile : {}", result.profile_version);

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
            "RULE   | {} [{}] expected={} actual={} :: {}",
            r.rule_id.bold(),
            verdict_tag(r.verdict),
            r.expected,
            r.actual,
            r.explanation
        );
    }

    if !result.bottlenecks.is_empty() {
        let _ = writeln!(out, "--- BOTTLENECKS ---");
        for b in &result.bottlenecks {
            let _ = writeln!(
                out,
                "SLOW   | {} runs at {} but supports {} [{severity}]",
                b.device_id,
                speed_label(b.current_speed),
                speed_label(b.max_speed),
                severity = severity_label(b.severity)
            );
        }
    }

    let issue_total = result.topology_issues.len()
        + result.display_issues.len()
        + result.usb_c_issues.len()
        + result.power_issues.len();
    if issue_total > 0 {
        let _ = writeln!(out, "--- ISSUES ---");
        for i in &result.topology_issues {
            let _ = writeln!(out, "TOPO   | {:?} | {}", i.issue_type, i.description);
        }
        for i in &result.display_issues {
            let _ = writeln!(out, "DISP   | {}", i.description);
        }
        for i in &result.usb_c_issues {
            let _ = writeln!(out, "USBC   | {}", i.description);
        }
        for i in &result.power_issues {
            let _ = writeln!(out, "PWR    | {}", i.description);
        }
    }

    let _ = writeln!(out, "--- VERDICT ---");
    let _ = writeln!(
        out,
        "VERDICT| OVERALL | {}",
        verdict_tag(result.overall_verdict)
    );

    if !result.recommendations.is_empty() {
        let _ = writeln!(out, "--- RECOMMENDATIONS ---");
        for rec in &result.recommendations {
            let _ = writeln!(out, "* {rec}");
        }
    }
    out
}

/// Hub details with their downstream port mapping.
pub fn format_hubs(topo: &SystemTopology) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let hubs: Vec<&UsbDevice> = topo.devices.iter().filter(|d| d.is_hub).collect();
    if hubs.is_empty() && topo.root_hubs.is_empty() {
        return "No hubs present.\n".into();
    }
    for rh in &topo.root_hubs {
        let _ = writeln!(
            out,
            "{} — {} direct attachment(s)",
            rh.platform_id,
            rh.children_ids.len()
        );
    }
    for hub in hubs {
        let children: Vec<&UsbDevice> = topo
            .devices
            .iter()
            .filter(|d| d.parent_id == Some(hub.id))
            .collect();
        let _ = writeln!(
            out,
            "\n{} ({}) — {} downstream",
            truncate(&hub.platform_id, 40),
            hub.product.as_deref().unwrap_or("hub"),
            children.len()
        );
        for child in children {
            let _ = writeln!(
                out,
                "   p{} → {} ({})",
                child
                    .port_number
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "?".into()),
                child.product.as_deref().unwrap_or("device"),
                truncate(&child.platform_id, 30)
            );
        }
    }
    out
}

/// Port occupancy across every hub plus synthesized root-hub attachments.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PortEntry {
    pub hub: String,
    pub port: Option<u8>,
    pub occupant: String,
}

pub fn port_map(topo: &SystemTopology) -> Vec<PortEntry> {
    let mut entries = Vec::new();
    let occupant_of = |d: &UsbDevice| d.product.clone().unwrap_or_else(|| "device".into());

    for rh in &topo.root_hubs {
        for cid in &rh.children_ids {
            if let Some(dev) = topo.devices.iter().find(|d| d.id == *cid) {
                entries.push(PortEntry {
                    hub: rh.platform_id.clone(),
                    port: dev.port_number,
                    occupant: occupant_of(dev),
                });
            }
        }
    }
    for hub in topo.devices.iter().filter(|d| d.is_hub) {
        for child in topo.devices.iter().filter(|d| d.parent_id == Some(hub.id)) {
            entries.push(PortEntry {
                hub: truncate(&hub.platform_id, 40),
                port: child.port_number,
                occupant: occupant_of(child),
            });
        }
    }
    entries
}

pub fn format_ports(topo: &SystemTopology) -> String {
    let entries = port_map(topo);
    if entries.is_empty() {
        return "No occupied ports.\n".into();
    }
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "{:<34} {:>4}  OCCUPANT", "HUB", "PORT");
    for e in entries {
        let _ = writeln!(
            out,
            "{:<34} {:>4}  {}",
            e.hub,
            e.port.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            e.occupant
        );
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use skirr_core::{
        ConnectionStatus, EventSummary, Fact, FactCategory, HostController,
        HostControllerCapabilities, PlatformInfo, RootHub, RuleEvaluation, SystemTopology,
        UsbClass, UsbDevice, UsbSpeed,
    };

    /// Force colors off so assertions see plain text regardless of env.
    fn no_color() {
        colored::control::set_override(false);
    }

    fn platform() -> PlatformInfo {
        PlatformInfo {
            os: "macOS".into(),
            os_version: "test".into(),
            kernel_version: None,
            architecture: "arm64".into(),
            hostname: None,
            username: None,
            is_admin: false,
            is_virtual_machine: false,
            boot_time: None,
        }
    }

    fn device(vid: u16, product: &str, class: UsbClass, port: Option<u8>) -> UsbDevice {
        let mut d = UsbDevice::new(vid, 0x1234);
        d.product = Some(product.into());
        d.device_class = class;
        d.is_hub = class == UsbClass::Hub;
        d.port_number = port;
        d.connection_status = ConnectionStatus::Connected;
        d
    }

    /// Controller BUS_1 → RootHub → [external hub] → [leaf].
    fn fixture_topology() -> SystemTopology {
        let hc_id = uuid::Uuid::new_v4();
        let rh_id = uuid::Uuid::new_v4();

        let mut hub = device(0x2109, "ExtHub", UsbClass::Hub, Some(4));
        hub.hop_count = 0;
        hub.tier = 1;
        let mut leaf = device(0x0781, "FlashDrive", UsbClass::MassStorage, Some(3));
        leaf.parent_id = Some(hub.id);
        leaf.hop_count = 1;
        leaf.tier = 2;
        hub.children_ids = vec![leaf.id];
        hub.root_hub_id = Some(rh_id);
        leaf.root_hub_id = Some(rh_id);

        SystemTopology {
            timestamp: Utc::now(),
            host_controllers: vec![HostController {
                id: hc_id,
                platform_id: "USB_BUS_1".into(),
                name: "Apple USB Host Controller".into(),
                vendor_id: None,
                device_id: None,
                revision: None,
                usb_version: UsbSpeed::SuperSpeed,
                root_hub_ids: vec![rh_id],
                port_count: 1,
                is_xhci: true,
                pci_address: None,
                driver_version: None,
                capabilities: HostControllerCapabilities::default(),
            }],
            root_hubs: vec![RootHub {
                id: rh_id,
                platform_id: r"ROOT_HUB\BUS_1".into(),
                host_controller_id: hc_id,
                port_count: 1,
                hub_speed: UsbSpeed::SuperSpeed,
                is_integrated: true,
                children_ids: vec![hub.id],
            }],
            devices: vec![hub, leaf],
            hubs: Vec::new(),
            displays: Vec::new(),
            events: Vec::new(),
            platform_info: platform(),
        }
    }

    fn fixture_diagnosis() -> DiagnosticResult {
        DiagnosticResult {
            timestamp: Utc::now(),
            profile_version: "1.0".into(),
            overall_verdict: Verdict::Warning,
            facts: vec![Fact {
                id: "f1".into(),
                category: FactCategory::Platform,
                description: "os version".into(),
                value: serde_json::json!("test"),
                source: "test".into(),
                confidence: 100,
            }],
            rules_applied: vec![RuleEvaluation {
                rule_id: "R01".into(),
                rule_description: "check".into(),
                rule_category: FactCategory::Speed,
                expected: serde_json::json!(">=5G"),
                actual: serde_json::json!("480M"),
                verdict: Verdict::Warning,
                explanation: "link fell back".into(),
            }],
            bottlenecks: Vec::new(),
            topology_issues: Vec::new(),
            display_issues: Vec::new(),
            usb_c_issues: Vec::new(),
            power_issues: Vec::new(),
            event_summary: EventSummary::default(),
            recommendations: vec!["plug into a faster port".into()],
        }
    }

    #[test]
    fn tree_renders_internal_external_split_with_port_chains() {
        no_color();
        let tree = render_tree(&fixture_topology());

        assert!(tree.contains("INTERNAL"), "internal header:\n{tree}");
        assert!(tree.contains("EXTERNAL"), "external header:\n{tree}");
        assert!(tree.contains("[USB_BUS_1]"), "controller:\n{tree}");
        assert!(tree.contains("RootHub"), "root hub:\n{tree}");
        assert!(tree.contains("Port 4 ──"), "chain head:\n{tree}");
        assert!(tree.contains("[HUB]"), "hub marker:\n{tree}");
        // The leaf hangs off the hub's port 3 via tree glyphs.
        let leaf_line = tree
            .lines()
            .find(|l| l.contains("FlashDrive"))
            .expect("leaf");
        assert!(leaf_line.contains("└─ p3"), "glyph+port:\n{tree}");
    }

    #[test]
    fn dock_chain_shows_every_hub_level_and_free_ports() {
        no_color();
        let mut topo = fixture_topology();

        // Dock: ExtHub on root port 4 → internal dock sub-hub on p2 →
        // two leaf devices (p1, p4). Root hub has 6 ports, so 5 stay free.
        topo.root_hubs[0].port_count = 6;
        let hub_id = topo.devices[0].id;
        let rh_id = topo.root_hubs[0].id;

        let mut sub_hub = device(0x2109, "DockSubHub", UsbClass::Hub, Some(2));
        sub_hub.parent_id = Some(hub_id);
        sub_hub.tier = 2;
        sub_hub.root_hub_id = Some(rh_id);

        let mut kb = device(0x05AC, "Keyboard", UsbClass::HID, Some(1));
        kb.parent_id = Some(sub_hub.id);
        kb.tier = 3;
        kb.root_hub_id = Some(rh_id);
        let mut cam = device(0x046D, "Camera", UsbClass::Video, Some(4));
        cam.parent_id = Some(sub_hub.id);
        cam.tier = 3;
        cam.root_hub_id = Some(rh_id);

        sub_hub.children_ids = vec![kb.id, cam.id];
        topo.devices[0].children_ids.push(sub_hub.id);
        topo.devices.push(sub_hub);
        topo.devices.push(kb);
        topo.devices.push(cam);

        let tree = render_tree(&topo);

        assert!(
            tree.contains("Ports free: 1, 2, 3, 5, 6"),
            "free ports:\n{tree}"
        );
        let lines: Vec<&str> = tree.lines().collect();
        let pos = |needle: &str| {
            lines
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle} missing:\n{tree}"))
        };
        // Chain order top-to-bottom: head, then sub-hub, then leaves.
        let head = pos("Port 4 ──");
        let sub = pos("DockSubHub");
        let kbd = pos("Keyboard");
        let camera = pos("Camera");
        assert!(head < sub && sub < kbd && kbd < camera, "order:\n{tree}");
        // Sub-hub nests under the head; leaves nest under the sub-hub.
        // Head may be followed by more ports, so its subtree keeps │.
        // Fixture also has FlashDrive on p3 below the head, so the
        // sub-hub branch itself continues with │ too.
        assert!(
            lines[sub].starts_with("  │ ├─ p2"),
            "sub-hub line: {}",
            lines[sub]
        );
        assert!(
            lines[kbd].starts_with("  │ │ ├─ p1"),
            "keyboard line: {}",
            lines[kbd]
        );
        assert!(
            lines[camera].starts_with("  │ │ └─ p4"),
            "camera line: {}",
            lines[camera]
        );
    }

    #[test]
    fn empty_topology_renders_header_only() {
        let topo = SystemTopology {
            timestamp: Utc::now(),
            host_controllers: Vec::new(),
            root_hubs: Vec::new(),
            devices: Vec::new(),
            hubs: Vec::new(),
            displays: Vec::new(),
            events: Vec::new(),
            platform_info: platform(),
        };
        assert!(render_tree(&topo).contains("(nothing attached)"));
        assert_eq!(format_hubs(&topo), "No hubs present.\n");
        assert_eq!(format_ports(&topo), "No occupied ports.\n");
    }

    #[test]
    fn truncation_preserves_width() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(
            truncate("a-very-long-platform-id-string", 10)
                .chars()
                .count(),
            10
        );
    }

    #[test]
    fn diagnosis_has_all_sections_in_order() {
        no_color();
        let report = format_diagnosis(&fixture_diagnosis());
        let fact = report.find("--- FACT ---").expect("FACT section");
        let rule = report.find("--- RULE ---").expect("RULE section");
        let verdict = report.find("--- VERDICT ---").expect("VERDICT section");
        let recs = report
            .find("--- RECOMMENDATIONS ---")
            .expect("RECOMMENDATIONS section");
        assert!(fact < rule && rule < verdict && verdict < recs);
        assert!(report.contains("R01"), "rule id:\n{report}");
        assert!(report.contains("PASS") || report.contains("WARNING"));
        assert!(report.contains("* plug into a faster port"));
    }

    #[test]
    fn empty_sections_are_omitted() {
        no_color();
        let mut result = fixture_diagnosis();
        result.recommendations.clear();
        result.rules_applied.clear();
        result.facts.clear();
        let report = format_diagnosis(&result);
        assert!(!report.contains("RECOMMENDATIONS"));
        assert!(report.contains("--- VERDICT ---"));
    }

    #[test]
    fn device_table_lists_every_row() {
        no_color();
        let topo = fixture_topology();
        let table = devices_table(&topo.devices);
        assert!(table.contains("ExtHub"));
        assert!(table.contains("FlashDrive"));
        assert!(table.contains("0x2109"));
    }
}
