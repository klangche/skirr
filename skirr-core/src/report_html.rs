//! Self-contained HTML report generation (Phase 8.3).
//!
//! Renders a [`SkirrReport`] into one offline-capable HTML file: embedded
//! CSS, no external assets, no JavaScript requirements (`<details>` provides
//! the collapsible topology tree). All dynamic strings are HTML-escaped —
//! device names come straight off the bus and are attacker-controlled input.

use crate::correlate::CorrelatedEvent;
use crate::report::SkirrReport;
use crate::{BottleneckSeverity, EventSeverity, Verdict};
use std::collections::HashMap;
use std::fmt::Write as _;

/// Render the full report as a standalone HTML document.
pub fn render_html(report: &SkirrReport) -> String {
    let mut out = String::with_capacity(32 * 1024);
    let topo = &report.topology;
    let diag = &report.diagnosis;

    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str(&format!(
        "<title>Skirr Report — {}</title>\n",
        html_escape(&report.platform.hostname.clone().unwrap_or_default())
    ));
    out.push_str(EMBEDDED_CSS);
    out.push_str("</head>\n<body>\n");

    // Header: tool, verdict, context.
    let _ = write!(
        out,
        "<header><h1>Skirr <span class=\"muted\">{}</span></h1>",
        html_escape(&report.tool.version)
    );
    out.push_str(&verdict_badge(diag.overall_verdict));
    let _ = writeln!(
        out,
        "<p class=\"meta\">{} · profile {} · schema {}</p></header>",
        html_escape(&report.generated.to_rfc3339()),
        html_escape(&report.profile_name),
        html_escape(&report.schema_version),
    );

    // Platform summary.
    let p = &report.platform;
    out.push_str("<section id=\"platform\"><h2>Platform</h2><table>");
    platform_row(&mut out, "OS", &format!("{} {}", p.os, p.os_version));
    platform_row(&mut out, "Architecture", &p.architecture);
    if let Some(host) = &p.hostname {
        platform_row(&mut out, "Host", host);
    }
    if let Some(user) = &p.username {
        platform_row(&mut out, "User", user);
    }
    if let Some(kernel) = &p.kernel_version {
        platform_row(&mut out, "Kernel", kernel);
    }
    platform_row(
        &mut out,
        "Privileges",
        if p.is_admin { "admin" } else { "standard user" },
    );
    if p.is_virtual_machine {
        platform_row(&mut out, "Virtualization", "VM detected");
    }
    out.push_str("</table></section>\n");

    // Topology: internal vs external per-port chains.
    out.push_str("<section id=\"topology\"><h2>Topology</h2>");
    push_topology_html(&mut out, topo);
    out.push_str("</section>\n");

    // Thunderbolt / USB4 fabric.
    if !topo.thunderbolt_routers.is_empty() {
        out.push_str("<section id=\"thunderbolt\"><h2>Thunderbolt / USB4</h2><ul>");
        for r in &topo.thunderbolt_routers {
            let kind = if r.is_usb4 { "USB4" } else { "Thunderbolt" };
            let gen = r
                .generation
                .as_ref()
                .map(|g| format!(" {g:?}"))
                .unwrap_or_default();
            let sec = r
                .security_level
                .map(|s| format!(" · security {s:?}"))
                .unwrap_or_default();
            let nvm = r
                .nvm_version
                .as_deref()
                .map(|v| format!(" · NVM {v}"))
                .unwrap_or_default();
            let _ = write!(
                out,
                "<li>{} — {}{kind}{gen}{sec}{nvm} · depth {}",
                html_escape(&r.name),
                html_escape(r.vendor_name.as_deref().unwrap_or("")),
                r.depth,
            );
            if let Some(status) = &r.status {
                let _ = write!(out, " · {}", html_escape(status));
            }
            for rec in &r.receptacles {
                let _ = write!(
                    out,
                    "<br/><small>Receptacle {}: {}{}</small>",
                    html_escape(rec.id.as_deref().unwrap_or("?")),
                    html_escape(rec.status.as_deref().unwrap_or("status unknown")),
                    rec.current_speed
                        .as_deref()
                        .map(|s| format!(" ({})", html_escape(s)))
                        .unwrap_or_default(),
                );
            }
            out.push_str("</li>");
        }
        out.push_str("</ul></section>\n");
    }

    // Hub bandwidth budget.
    let bandwidth = crate::bandwidth::analyze_bandwidth(topo);
    if !bandwidth.is_empty() {
        out.push_str("<section id=\"bandwidth\"><h2>Hub bandwidth</h2><table><tr><th>Hub</th><th>Uplink</th><th>Downstream</th><th>Utilization</th></tr>");
        for hbw in &bandwidth {
            let class = match hbw.severity() {
                crate::BottleneckSeverity::Critical => " class=\"sev-critical\"",
                crate::BottleneckSeverity::Major => " class=\"sev-warning\"",
                crate::BottleneckSeverity::Minor => "",
            };
            let pct = hbw
                .utilization_pct
                .map(|p| format!("{p:.0}%"))
                .unwrap_or_else(|| "?".into());
            let _ = write!(
                out,
                "<tr{class}><td>{}</td><td>{} Mb/s</td><td>{} Mb/s</td><td>{pct}</td></tr>",
                html_escape(&hbw.hub_label),
                hbw.uplink_mbps,
                hbw.used_downstream_mbps,
            );
        }
        out.push_str("</table></section>\n");
    }

    // Displays.
    if !topo.displays.is_empty() {
        out.push_str("<section id=\"displays\"><h2>Displays</h2><ul>");
        for d in &topo.displays {
            let name = d
                .name
                .as_deref()
                .or(d.manufacturer_id.as_deref())
                .unwrap_or("Display");
            let hdr = if d.hdr_supported { " · HDR" } else { "" };
            let res = d
                .current_resolution
                .as_ref()
                .map(|r| format!(" · {}×{}", r.width, r.height))
                .unwrap_or_default();
            let _ = write!(
                out,
                "<li>{} ({}){hdr}{res}</li>",
                html_escape(name),
                html_escape(&d.platform_id),
            );
        }
        out.push_str("</ul></section>\n");
    }

    // USB-C power status (Phase 10.2).
    if !topo.type_c_ports.is_empty() {
        out.push_str("<section id=\"usb-c-power\"><h2>USB-C power</h2><table><tr><th>Port</th><th>Power role</th><th>PD</th><th>Orientation</th><th>Status</th><th>Cable</th></tr>");
        for port in &topo.type_c_ports {
            let role = port
                .power_role
                .map(|r| format!("{r:?}"))
                .unwrap_or_else(|| "?".into());
            let pd = port
                .pd_revision
                .as_deref()
                .map(|r| format!("PD {r}"))
                .unwrap_or_else(|| "—".into());
            let orient = match port.orientation {
                crate::ConnectorOrientation::Normal => "normal",
                crate::ConnectorOrientation::Flipped => "flipped",
                _ => "?",
            };
            let status = if port.pd_active() {
                "PD contract active"
            } else if port.partner_attached {
                "partner attached"
            } else {
                "no partner"
            };
            let cable = port
                .emarker
                .as_ref()
                .map(|e| {
                    let mut s = String::from("E-marker");
                    if let Some(rating) = e.current_rating_a {
                        let _ = write!(s, " {rating}A");
                    }
                    s
                })
                .unwrap_or_else(|| "—".into());
            let _ = write!(
                out,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{orient}</td><td>{status}</td><td>{}</td></tr>",
                html_escape(&port.port_name),
                html_escape(&role),
                html_escape(&pd),
                html_escape(&cable),
            );
        }
        out.push_str("</table></section>\n");
    }

    // Display bandwidth plan (Phase 10.3).
    let display_plan = crate::bandwidth::plan_display_bandwidth(topo);
    if !display_plan.is_empty() {
        out.push_str(
            "<section id=\"display-bandwidth\"><h2>Display bandwidth</h2><table><tr><th>Display</th><th>Estimate</th><th>Via hub</th></tr>",
        );
        for req in &display_plan.requirements {
            let gb = req.required_mbps as f64 / 1000.0;
            let flag = if req.exceeds_upstream_uplink {
                " <span class=\"sev-critical\">exceeds hub uplink</span>"
            } else {
                ""
            };
            let _ = write!(
                out,
                "<tr><td>{}</td><td>~{gb:.1} Gb/s{flag}</td><td>{}</td></tr>",
                html_escape(&req.name),
                html_escape(req.upstream_hub_label.as_deref().unwrap_or("—")),
            );
        }
        let _ = write!(
            out,
            "<tr><td><strong>Combined</strong></td><td colspan=\"2\">~{:.1} Gb/s</td></tr>",
            display_plan.total_required_mbps as f64 / 1000.0,
        );
        out.push_str("</table></section>\n");
    }

    // Issues + bottlenecks.
    let by_id: HashMap<uuid::Uuid, &crate::UsbDevice> =
        topo.devices.iter().map(|d| (d.id, d)).collect();
    push_bottlenecks_html(&mut out, diag, &by_id);
    push_issues_html(
        &mut out,
        "Topology issues",
        diag.topology_issues
            .iter()
            .map(|i| issue_line(&by_id, i.device_id, &i.description, i.severity)),
    );
    push_issues_html(
        &mut out,
        "Display issues",
        diag.display_issues
            .iter()
            .map(|i| issue_line(&by_id, i.display_id, &i.description, i.severity)),
    );
    push_issues_html(
        &mut out,
        "USB-C issues",
        diag.usb_c_issues
            .iter()
            .map(|i| issue_line(&by_id, i.device_id, &i.description, i.severity)),
    );
    push_issues_html(
        &mut out,
        "Power issues",
        diag.power_issues
            .iter()
            .map(|i| issue_line(&by_id, i.device_id, &i.description, i.severity)),
    );

    // Recommendations.
    if !diag.recommendations.is_empty() {
        out.push_str("<section id=\"recommendations\"><h2>Recommendations</h2><ul>");
        for rec in &diag.recommendations {
            let _ = write!(out, "<li>{}</li>", html_escape(rec));
        }
        out.push_str("</ul></section>\n");
    }

    // Events timeline.
    push_events_html(&mut out, report);

    let _ = write!(
        out,
        "<footer>Generated by skirr {} · {} controller(s), {} device(s) enumerated.</footer>\n</body>\n</html>\n",
        html_escape(&report.tool.version),
        topo.host_controllers.len(),
        topo.devices.len(),
    );
    out
}

fn platform_row(out: &mut String, key: &str, value: &str) {
    let _ = write!(
        out,
        "<tr><th>{}</th><td>{}</td></tr>",
        html_escape(key),
        html_escape(value)
    );
}

/// Topology as nested `<details>`/`<ul>` chains, split internal/external.
fn push_topology_html(out: &mut String, topo: &crate::SystemTopology) {
    let mut by_parent: HashMap<Option<uuid::Uuid>, Vec<&crate::UsbDevice>> = HashMap::new();
    for dev in &topo.devices {
        by_parent.entry(dev.parent_id).or_default().push(dev);
    }
    for children in by_parent.values_mut() {
        children.sort_by_key(|d| (d.port_number.unwrap_or(0), d.id));
    }

    let mut any_external = false;
    out.push_str("<h3>Internal</h3>");
    for hc in &topo.host_controllers {
        let _ = write!(out, "<p class=\"ctrl\">{}</p>", html_escape(&hc.name));
        for rh in topo
            .root_hubs
            .iter()
            .filter(|r| r.host_controller_id == hc.id)
        {
            let _ = write!(
                out,
                "<p class=\"rh\">RootHub {} ({} ports)</p>",
                html_escape(&rh.platform_id),
                rh.port_count
            );
            if let Some(children) = by_parent.get(&None) {
                for dev in children
                    .iter()
                    .filter(|d| d.is_internal && d.root_hub_id == Some(rh.id))
                {
                    push_device_tree(out, dev, &by_parent);
                }
            }
        }
    }

    out.push_str("<h3>External — one chain per port</h3>");
    for rh in &topo.root_hubs {
        let Some(children) = by_parent.get(&None) else {
            continue;
        };
        let heads: Vec<&crate::UsbDevice> = children
            .iter()
            .copied()
            .filter(|d| !d.is_internal && d.root_hub_id == Some(rh.id))
            .collect();
        if heads.is_empty() {
            continue;
        }
        any_external = true;
        let _ = write!(
            out,
            "<details open><summary>RootHub {} ({} ports)</summary><div class=\"chain\">",
            html_escape(&rh.platform_id),
            rh.port_count
        );
        for head in &heads {
            match head.port_number {
                Some(p) => {
                    let _ = write!(out, "<p class=\"port\">Port {p}</p>");
                }
                None => {
                    let _ = write!(out, "<p class=\"port\">Port ?</p>");
                }
            }
            push_node_label(out, head);
            push_device_tree(out, head, &by_parent);
        }
        let occupied: Vec<u8> = heads.iter().filter_map(|d| d.port_number).collect();
        let free: Vec<String> = (1..=rh.port_count)
            .filter(|p| !occupied.contains(p))
            .map(|p| p.to_string())
            .collect();
        if !free.is_empty() {
            let _ = write!(out, "<p class=\"free\">Ports free: {}</p>", free.join(", "));
        }
        out.push_str("</div></details>");
    }
    if !any_external && topo.host_controllers.is_empty() {
        out.push_str("<p>(nothing attached)</p>");
    }
}

/// Recursive `<ul>` tree below a node; label first via [`push_node_label`].
fn push_device_tree(
    out: &mut String,
    dev: &crate::UsbDevice,
    by_parent: &HashMap<Option<uuid::Uuid>, Vec<&crate::UsbDevice>>,
) {
    let Some(children) = by_parent.get(&Some(dev.id)) else {
        return;
    };
    out.push_str("<ul>");
    for child in children {
        out.push_str("<li>");
        push_node_label(out, child);
        push_device_tree(out, child, by_parent);
        out.push_str("</li>");
    }
    out.push_str("</ul>");
}

/// One device label line with speed/hub/dock annotations.
fn push_node_label(out: &mut String, dev: &crate::UsbDevice) {
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
        mbps => format!(" <span class=\"speed\">{mbps} Mbps</span>"),
    };
    let dock = match dev.properties.get("dock_family") {
        Some(family) => format!(" <span class=\"dock\">{}</span>", html_escape(family)),
        None => String::new(),
    };
    let _ = write!(
        out,
        "<span class=\"dev\">{} <span class=\"ids\">{:04X}:{:04X}</span>{hub_mark}{speed}{dock}</span>",
        html_escape(label),
        dev.vendor_id,
        dev.product_id,
    );
}

fn push_bottlenecks_html(
    out: &mut String,
    diag: &crate::DiagnosticResult,
    by_id: &HashMap<uuid::Uuid, &crate::UsbDevice>,
) {
    if diag.bottlenecks.is_empty() {
        return;
    }
    let _ = write!(
        out,
        "<section id=\"bottlenecks\"><h2>Speed bottlenecks</h2><table><tr><th>Device</th><th>Max</th><th>Current</th><th>Severity</th></tr>"
    );
    for b in &diag.bottlenecks {
        let name = by_id
            .get(&b.device_id)
            .and_then(|d| d.product.as_deref())
            .unwrap_or("unknown device");
        let _ = write!(
            out,
            "<tr><td>{}</td><td>{} Mbps</td><td>{} Mbps</td><td>{}</td></tr>",
            html_escape(name),
            b.max_speed.mbps(),
            b.current_speed.mbps(),
            bottleneck_badge(b.severity),
        );
    }
    out.push_str("</table></section>\n");
}

fn push_issues_html(out: &mut String, title: &str, lines: impl Iterator<Item = String>) {
    let items: Vec<String> = lines.collect();
    if items.is_empty() {
        return;
    }
    let slug = title.to_lowercase().replace(' ', "-");
    let _ = write!(out, "<section id=\"{slug}\"><h2>{title}</h2><ul>");
    for item in items {
        let _ = write!(out, "<li>{item}</li>");
    }
    out.push_str("</ul></section>\n");
}

fn issue_line(
    by_id: &HashMap<uuid::Uuid, &crate::UsbDevice>,
    device_id: uuid::Uuid,
    description: &str,
    severity: Verdict,
) -> String {
    let name = by_id
        .get(&device_id)
        .and_then(|d| d.product.as_deref())
        .unwrap_or("unknown device");
    format!(
        "{} {} — {}",
        verdict_badge(severity),
        html_escape(name),
        html_escape(description)
    )
}

fn push_events_html(out: &mut String, report: &SkirrReport) {
    out.push_str("<section id=\"events\"><h2>Events</h2>");
    let mut correlated = crate::correlate::EventCorrelator::new(&report.topology);
    let mut rendered = 0usize;
    for event in &report.topology.events {
        let outcome = correlated.ingest(event.clone());
        if let Some(correlated_event) = outcome.correlated {
            let (class, text) = match &correlated_event {
                CorrelatedEvent::Single(event) => (
                    severity_class(event.severity),
                    format!("{:?} — {}", event.event_type, event.details),
                ),
                CorrelatedEvent::HubRemoval {
                    hub_event,
                    child_disconnects,
                    max_depth,
                } => (
                    "sev-critical",
                    format!(
                        "{:?} — {} (collapsed: {child_disconnects} device(s), depth {max_depth})",
                        hub_event.event_type, hub_event.details
                    ),
                ),
            };
            let _ = write!(
                out,
                "<div class=\"event\"><span class=\"when\">{}</span><span class=\"{class}\">{}</span></div>",
                html_escape(&event.timestamp.to_rfc3339()),
                html_escape(&text),
            );
            rendered += 1;
        }
    }
    if rendered == 0 {
        out.push_str("<p>No events recorded.</p>");
    }
    out.push_str("</section>\n");
}

fn verdict_badge(verdict: Verdict) -> String {
    let (class, text) = match verdict {
        Verdict::Pass => ("verdict-pass", "PASS"),
        Verdict::Warning => ("verdict-warning", "WARNING"),
        Verdict::Fail => ("verdict-fail", "FAIL"),
        Verdict::Unknown => ("verdict-unknown", "UNKNOWN"),
    };
    format!("<span class=\"{class}\">{text}</span>")
}

fn bottleneck_badge(severity: BottleneckSeverity) -> String {
    let (class, text) = match severity {
        BottleneckSeverity::Minor => ("sev-info", "Minor"),
        BottleneckSeverity::Major => ("sev-warning", "Major"),
        BottleneckSeverity::Critical => ("sev-critical", "Critical"),
    };
    format!("<span class=\"badge {class}\">{text}</span>")
}

fn severity_class(severity: EventSeverity) -> &'static str {
    match severity {
        EventSeverity::Info => "sev-info",
        EventSeverity::Warning => "sev-warning",
        EventSeverity::Error | EventSeverity::Critical => "sev-critical",
    }
}

/// Escape `&`, `<`, `>`, `"`, `'` for safe interpolation into HTML bodies
/// and double/single-quoted attributes.
fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

const EMBEDDED_CSS: &str = r#"<style>
:root { color-scheme: light dark; }
body { font-family: -apple-system, "Segoe UI", Roboto, sans-serif; margin: 0 auto; max-width: 60rem; padding: 1rem 1.5rem 3rem; line-height: 1.5; }
h1 { display: flex; align-items: center; gap: .75rem; margin-bottom: 0; }
h2 { border-bottom: 1px solid color-mix(in srgb, currentColor 20%, transparent); padding-bottom: .25rem; margin-top: 2rem; }
.muted { color: gray; font-weight: normal; }
.meta { color: gray; }
table { border-collapse: collapse; width: 100%; }
th, td { text-align: left; padding: .3rem .6rem; border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent); }
th:first-child { width: 10rem; color: gray; font-weight: normal; }
ul { padding-left: 1.25rem; }
ul ul { border-left: 1px dotted color-mix(in srgb, currentColor 30%, transparent); margin-left: .4rem; }
.ctrl, .rh, .port { margin: .6rem 0 0; font-weight: 600; }
.port { color: #0969da; }
.chain { padding-left: .75rem; border-left: 3px solid color-mix(in srgb, currentColor 15%, transparent); }
.free { color: gray; font-style: italic; }
.ids { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; color: gray; }
.speed { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
.dock { background: color-mix(in srgb, #8250df 15%, transparent); border-radius: .3rem; padding: 0 .3rem; }
.verdict-pass { background: #1a7f37; color: white; border-radius: .4rem; padding: .15rem .6rem; }
.verdict-warning { background: #bf8700; color: white; border-radius: .4rem; padding: .15rem .6rem; }
.verdict-fail { background: #cf222e; color: white; border-radius: .4rem; padding: .15rem .6rem; }
.verdict-unknown { background: #6e7781; color: white; border-radius: .4rem; padding: .15rem .6rem; }
.badge { border-radius: .3rem; padding: .05rem .4rem; font-size: .85em; }
.sev-info { background: color-mix(in srgb, #0969da 15%, transparent); }
.sev-warning { background: color-mix(in srgb, #bf8700 20%, transparent); }
.sev-critical { background: color-mix(in srgb, #cf222e 18%, transparent); }
.event { display: flex; gap: 1rem; padding: .2rem 0; border-bottom: 1px dotted color-mix(in srgb, currentColor 15%, transparent); }
.when { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; color: gray; white-space: nowrap; }
footer { margin-top: 3rem; color: gray; font-size: .85em; }
@media print { details { display: block; } details > * { display: block !important; } }
</style>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BottleneckSeverity, ConnectionStatus, DiagnosticEvent, DiagnosticResult, EventSummary,
        EventType, Fact, FactCategory, HostController, HostControllerCapabilities, PlatformInfo,
        RootHub, RuleEvaluation, SystemTopology, UsbClass, UsbDevice, UsbSpeed,
    };
    use chrono::Utc;

    fn empty_platform_info() -> PlatformInfo {
        PlatformInfo {
            os: "macOS".into(),
            os_version: "test".into(),
            kernel_version: None,
            architecture: "arm64".into(),
            hostname: Some("test-host".into()),
            username: None,
            is_admin: false,
            is_virtual_machine: false,
            boot_time: None,
        }
    }

    fn device(vid: u16, pid: u16, product: &str, class: UsbClass, port: Option<u8>) -> UsbDevice {
        let mut d = UsbDevice::new(vid, pid);
        d.product = Some(product.into());
        d.device_class = class;
        d.is_hub = class == UsbClass::Hub;
        d.port_number = port;
        d.connection_status = ConnectionStatus::Connected;
        d
    }

    /// Root hub → external hub on port 4 → leaf on hub-port 3.
    fn sample_report() -> SkirrReport {
        let hc_id = uuid::Uuid::new_v4();
        let rh_id = uuid::Uuid::new_v4();

        let mut hub = device(0x2109, 0x0817, "ExtHub", UsbClass::Hub, Some(4));
        hub.current_link_speed = UsbSpeed::SuperSpeed;
        hub.root_hub_id = Some(rh_id);
        let mut leaf = device(0x0781, 0x5583, "FlashDrive", UsbClass::MassStorage, Some(3));
        leaf.parent_id = Some(hub.id);
        leaf.current_link_speed = UsbSpeed::HighSpeed;
        leaf.root_hub_id = Some(rh_id);

        let topo = SystemTopology {
            timestamp: Utc::now(),
            host_controllers: vec![HostController {
                id: hc_id,
                platform_id: "USB_BUS_1".into(),
                name: "Test xHCI".into(),
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
                port_count: 4,
                hub_speed: UsbSpeed::SuperSpeed,
                is_integrated: true,
                children_ids: vec![],
            }],
            devices: vec![hub.clone(), leaf],
            hubs: Vec::new(),
            displays: Vec::new(),
            thunderbolt_routers: Vec::new(),
            type_c_ports: Vec::new(),
            events: Vec::new(),
            platform_info: empty_platform_info(),
        };

        let diagnosis = DiagnosticResult {
            timestamp: Utc::now(),
            profile_version: "1.0".into(),
            overall_verdict: Verdict::Pass,
            facts: vec![Fact {
                id: "f".into(),
                category: FactCategory::Platform,
                description: "os".into(),
                value: serde_json::json!("test"),
                source: "test".into(),
                confidence: 100,
            }],
            rules_applied: vec![RuleEvaluation {
                rule_id: "R01".into(),
                rule_description: "check".into(),
                rule_category: FactCategory::Speed,
                expected: serde_json::json!("5000"),
                actual: serde_json::json!("480"),
                verdict: Verdict::Warning,
                explanation: "fell back".into(),
            }],
            bottlenecks: vec![crate::SpeedBottleneck {
                device_id: hub.id,
                max_speed: UsbSpeed::SuperSpeed,
                current_speed: UsbSpeed::HighSpeed,
                severity: BottleneckSeverity::Major,
            }],
            topology_issues: vec![],
            display_issues: vec![],
            usb_c_issues: vec![],
            power_issues: vec![],
            event_summary: EventSummary::default(),
            recommendations: vec!["plug into a faster port".into()],
        };

        SkirrReport::new(
            "Skirr Standard Profile v1.0",
            topo.platform_info.clone(),
            topo,
            diagnosis,
        )
    }

    #[test]
    fn renders_verdict_platform_and_chain() {
        let html = render_html(&sample_report());

        assert!(html.starts_with("<!DOCTYPE html>"), "doctype");
        assert!(html.contains("verdict-pass"), "verdict badge:\n{html}");
        assert!(html.contains("PASS"));
        assert!(html.contains("test-host"), "platform hostname:\n{html}");
        assert!(html.contains("Port 4"), "external chain head:\n{html}");
        assert!(html.contains("ExtHub"), "head device:\n{html}");
        assert!(html.contains("FlashDrive"), "leaf:\n{html}");
        assert!(html.contains("Ports free: 1, 2, 3"), "free ports:\n{html}");
        assert!(html.contains("5000 Mbps"), "speed annotation:\n{html}");
        assert!(html.contains("ExtHub"), "bottleneck names device:\n{html}");
        assert!(
            html.contains("plug into a faster port"),
            "recommendations:\n{html}"
        );
        assert!(
            html.contains("No events recorded."),
            "empty events:\n{html}"
        );
    }

    #[test]
    fn escapes_hostile_device_names() {
        let mut report = sample_report();
        report.topology.devices[0].product = Some("<script>alert('x')</script>".into());
        let html = render_html(&report);

        assert!(
            !html.contains("<script>alert"),
            "raw injection survived:\n{html}"
        );
        assert!(
            html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"),
            "escaped form missing"
        );
    }

    #[test]
    fn empty_bus_still_produces_valid_document() {
        let mut report = sample_report();
        report.topology.devices.clear();
        report.diagnosis.bottlenecks.clear();
        report.topology.host_controllers.clear();
        report.topology.root_hubs.clear();
        let html = render_html(&report);

        assert!(html.starts_with("<!DOCTYPE html>"), "doctype:\n{html}");
        assert!(html.contains("(nothing attached)"), "empty marker:\n{html}");
        assert!(
            !html.contains("Speed bottlenecks"),
            "no empty section:\n{html}"
        );
        assert!(html.ends_with("</html>\n"), "closed document");
    }

    #[test]
    fn events_render_as_collapsed_timeline() {
        let mut report = sample_report();
        report.topology.events.push(DiagnosticEvent {
            id: uuid::Uuid::new_v4(),
            timestamp: Utc::now(),
            event_type: EventType::DeviceDisconnected,
            device_id: None,
            hub_id: None,
            port_number: Some(4),
            details: "ExtHub left port 4".into(),
            severity: EventSeverity::Warning,
            metadata: Default::default(),
        });
        let html = render_html(&report);

        assert!(html.contains("sev-warning"), "severity class:\n{html}");
        assert!(html.contains("ExtHub left port 4"), "event detail:\n{html}");
        assert!(!html.contains("No events recorded."));
    }
}
