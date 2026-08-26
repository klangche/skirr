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
    // HOST: physical ports from Thunderbolt/USB4 receptacles.
    // ------------------------------------------------------------------
    let _ = writeln!(out, "{}", "HOST".bold());

    // Collect all physical ports: Thunderbolt receptacles first, then root
    // hub ports for any not covered by Thunderbolt.
    let mut physical_ports: Vec<(u8, String)> = Vec::new();

    // Thunderbolt / USB4 receptacles are the physical ports.
    for router in &topo.thunderbolt_routers {
        for (i, rec) in router.receptacles.iter().enumerate() {
            let port_num = (i + 1) as u8;
            let rec_id = rec.id.as_deref().unwrap_or("?");
            let speed = rec.current_speed.as_deref().unwrap_or("");
            let label = if speed.is_empty() {
                format!("Thunderbolt {rec_id}")
            } else {
                format!("Thunderbolt {rec_id} ({speed})")
            };
            physical_ports.push((port_num, label));
        }
    }

    // If no Thunderbolt, fall back to root hub ports.
    if physical_ports.is_empty() {
        for rh in &topo.root_hubs {
            for p in 1..=rh.port_count {
                physical_ports.push((p, format!("Port {p}")));
            }
        }
    }

    // For each physical port, find the tier-1 device (dock) attached to it.
    // We map root hub port numbers to physical ports by position.
    let mut root_port_devices: Vec<&UsbDevice> = topo
        .devices
        .iter()
        .filter(|d| !d.is_internal && d.parent_id.is_none())
        .collect();
    root_port_devices.sort_by_key(|d| d.port_number.unwrap_or(0));

    // Map root hub port → device.
    let mut root_port_map: std::collections::HashMap<u8, &UsbDevice> =
        std::collections::HashMap::new();
    for dev in &root_port_devices {
        if let Some(pn) = dev.port_number {
            root_port_map.insert(pn, dev);
        }
    }

    for (port_num, _port_label) in &physical_ports {
        let _ = writeln!(out, "  Port {port_num} ── ",);
        if let Some(dev) = root_port_map.get(port_num) {
            // Occupied port — show the dock with its internal structure.
            write_dock_node(&mut out, dev, &by_parent, "    ");
        } else {
            let _ = writeln!(out, "    n/a");
        }
        let _ = writeln!(out);
    }

    // ------------------------------------------------------------------
    // BANDWIDTH: per-hub oversubscription check (Phase 10.1).
    // ------------------------------------------------------------------
    let bandwidth = skirr_core::bandwidth::analyze_bandwidth(topo);
    if !bandwidth.is_empty() {
        let _ = writeln!(out, "{}", "BANDWIDTH".bold());
        for hbw in &bandwidth {
            let pct = hbw.utilization_pct.unwrap_or(0.0);
            if hbw.severity() == skirr_core::BottleneckSeverity::Minor {
                continue;
            }
            let sev = match hbw.severity() {
                skirr_core::BottleneckSeverity::Major => "MAJOR".yellow(),
                _ => "CRITICAL".red(),
            };
            let _ = writeln!(
                out,
                "{}: {} Mb/s uplink feeds {} Mb/s downstream ({:.0}%) {}",
                hbw.hub_label.bold(),
                hbw.uplink_mbps,
                hbw.used_downstream_mbps,
                pct,
                sev
            );
        }
        let saturated = bandwidth
            .iter()
            .any(|hbw| hbw.severity() != skirr_core::BottleneckSeverity::Minor);
        if !saturated {
            let _ = writeln!(out, "all hubs within budget");
        }
        let _ = writeln!(out);
    }

    // ------------------------------------------------------------------
    // USB-C POWER (Phase 10.2 + 11.1–11.2): PD role/orientation/E-marker,
    // PDO source caps, PPS, cable wattage per connector.
    // ------------------------------------------------------------------
    if !topo.type_c_ports.is_empty() {
        let _ = writeln!(out, "{}", "USB-C POWER".bold());
        for port in &topo.type_c_ports {
            let mut line = format!("{}", port.port_name.bold());
            if let Some(role) = port.power_role {
                let _ = write!(line, " · {role:?}");
            }
            if let Some(pd) = &port.pd_revision {
                let _ = write!(line, " · PD {pd}");
            }
            if let Some(pt) = &port.port_type {
                let _ = write!(line, " · type {pt}");
            }
            if let Some(cc) = port.cc_pin_active() {
                let _ = write!(line, " · {cc}");
            }
            let _ = writeln!(
                out,
                "{line} · {}",
                if port.pd_active() {
                    "PD contract active".green()
                } else if port.partner_attached {
                    "partner attached".yellow()
                } else {
                    "no partner".normal()
                }
            );
            // Source capabilities list.
            if !port.source_caps.is_empty() {
                let _ = write!(out, "  Source caps:");
                for pdo in &port.source_caps {
                    let (label, c) = match pdo.pdo_type {
                        skirr_core::PdoType::FixedSupply => ("fixed", Some(pdo.current_ma)),
                        skirr_core::PdoType::VariableSupply => ("var", Some(pdo.current_ma)),
                        skirr_core::PdoType::BatterySupply => ("bat", None),
                        skirr_core::PdoType::AugmentedPower => ("pps", Some(pdo.current_ma)),
                    };
                    let _ = write!(out, " [{label}] {}V", pdo.voltage_mv as f64 / 1000.0);
                    if let Some(ma) = c {
                        let _ = write!(out, " {}A", ma as f64 / 1000.0);
                    }
                }
                let _ = writeln!(out);
                if port.supports_pps() {
                    let _ = writeln!(out, "  {}", "PPS supported".cyan());
                }
            }
            // Active contract summary.
            if let Some(pinfo) = &port.power_info {
                if let Some(v) = pinfo.contract_voltage_mv {
                    let _ = write!(out, "  Contract: {v}mV");
                    if let Some(c) = pinfo.contract_current_ma {
                        let _ = write!(out, " × {c}mA");
                    }
                    if let Some(p) = pinfo.contract_power_mw {
                        let _ = write!(out, " ({:.1}W)", p as f64 / 1000.0);
                    }
                    let _ = writeln!(out);
                }
            }
            // Cable details.
            if let Some(cd) = port.cable_details() {
                let mut parts = Vec::new();
                parts.push("Cable:".into());
                if let (Some(v), Some(p)) = (cd.vendor_id, cd.product_id) {
                    parts.push(format!("VID:PID {v:04X}:{p:04X}"));
                }
                if let Some(a) = cd.current_rating_a {
                    parts.push(format!("{a}A"));
                }
                if let Some(w) = cd.wattage_limit_w {
                    parts.push(format!("≤{w}W"));
                }
                if let Some(mode) = &cd.plug_mode {
                    parts.push(mode.clone());
                }
                if let Some(ty) = &cd.product_type {
                    parts.push(ty.clone());
                }
                if let Some(sp) = &cd.speed_rating {
                    parts.push(sp.clone());
                }
                let _ = writeln!(out, "  {}", parts.join(" · "));
            }
        }
        let _ = writeln!(out);
    }

    // ------------------------------------------------------------------
    // DISPLAY BANDWIDTH (Phase 10.3): estimated payloads vs hub uplinks.
    // ------------------------------------------------------------------
    let display_plan = skirr_core::bandwidth::plan_display_bandwidth(topo);
    if !display_plan.is_empty() {
        let _ = writeln!(out, "{}", "DISPLAY BANDWIDTH".bold());
        for req in &display_plan.requirements {
            let gb = req.required_mbps as f64 / 1000.0;
            let upstream = req
                .upstream_hub_label
                .as_deref()
                .map(|h| format!(" via {h}"))
                .unwrap_or_default();
            let flag = if req.exceeds_upstream_uplink {
                " EXCEEDS HUB UPLINK".red().to_string()
            } else {
                String::new()
            };
            let _ = writeln!(out, "{}: ~{gb:.1} Gb/s{upstream}{flag}", req.name.bold());
        }
        let total_gb = display_plan.total_required_mbps as f64 / 1000.0;
        let _ = writeln!(out, "Combined estimate: ~{total_gb:.1} Gb/s");
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

    // If this hub has dock_ports, render the full physical port layout.
    if let Some(info) = &dev.hub_info {
        if !info.dock_ports.is_empty() {
            let total = info.dock_ports.len();
            for (i, dp) in info.dock_ports.iter().enumerate() {
                let is_last = i + 1 == total;
                let glyph = if is_last { "└─" } else { "├─" };

                if let Some(usb_port) = dp.usb_hub_port {
                    // USB port — find the connected child device.
                    if let Some(kid) = children.iter().find(|c| c.port_number == Some(usb_port)) {
                        let _ = write!(out, "{child_prefix}{glyph} ");
                        write_node(out, kid);
                        write_chain(out, kid, by_parent, &child_prefix, is_last);
                    } else {
                        let _ = writeln!(out, "{child_prefix}{glyph} {} — n/a", dp.label);
                    }
                } else {
                    // Non-USB port (HDMI, Ethernet, Audio, etc.)
                    let _ = writeln!(out, "{child_prefix}{glyph} {}", dp.label);
                }
            }
            // Emit children without port numbers (compound interfaces).
            for kid in children.iter() {
                if kid.port_number.is_none() {
                    let _ = write!(out, "{child_prefix}├─ ");
                    write_node(out, kid);
                    write_chain(out, kid, by_parent, &child_prefix, false);
                }
            }
            return;
        }
    }

    // Fallback: hub with known port count but no dock_ports.
    if let Some(port_count) = dev.hub_info.as_ref().map(|h| h.port_count) {
        for pn in 1..=port_count {
            let is_last_port = pn == port_count;
            let glyph = if is_last_port { "└─" } else { "├─" };
            if let Some(kid) = children.iter().find(|c| c.port_number == Some(pn)) {
                let _ = write!(out, "{child_prefix}{glyph} p{pn} ");
                write_node(out, kid);
                write_chain(out, kid, by_parent, &child_prefix, is_last_port);
            } else {
                let _ = writeln!(out, "{child_prefix}{glyph} p{pn} n/a");
            }
        }
        // Emit children without port numbers (compound interfaces).
        for (i, kid) in children.iter().enumerate() {
            if kid.port_number.is_none() {
                let is_last = i + 1 == children.len() && true;
                let glyph = "├─";
                let _ = write!(out, "{child_prefix}{glyph} ");
                write_node(out, kid);
                write_chain(out, kid, by_parent, &child_prefix, is_last);
            }
        }
    } else {
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
}

/// Write a dock node: the root-most external hub with all its internal hubs
/// and non-USB ports as children. Each physical port block is self-contained.
fn write_dock_node(
    out: &mut String,
    dev: &UsbDevice,
    by_parent: &HashMap<uuid::Uuid, Vec<&UsbDevice>>,
    prefix: &str,
) {
    use std::fmt::Write;

    // Compute metrics for this dock.
    let mut total_hubs: u32 = 0;
    let mut total_devices: u32 = 0;
    fn count_subtree(
        dev_id: uuid::Uuid,
        by_parent: &HashMap<uuid::Uuid, Vec<&UsbDevice>>,
        hubs: &mut u32,
        devices: &mut u32,
    ) {
        if let Some(kids) = by_parent.get(&dev_id) {
            for kid in kids {
                *devices += 1;
                if (kid.is_hub || kid.device_class == skirr_core::UsbClass::Hub) && !kid.is_internal
                {
                    *hubs += 1;
                }
                count_subtree(kid.id, by_parent, hubs, devices);
            }
        }
    }
    count_subtree(dev.id, by_parent, &mut total_hubs, &mut total_devices);
    let hops = dev.hop_count;
    let tiers = dev.tier;

    // Write the dock header.
    let hub_mark = if dev.is_hub {
        if let Some(info) = &dev.hub_info {
            format!(" [HUB {}p]", info.port_count)
        } else {
            " [HUB]".to_string()
        }
    } else {
        String::new()
    };
    let _ = write!(
        out,
        "{}{hub_mark} (Hubs {total_hubs}, Hops {hops}, Tiers {tiers}, Devices {total_devices})",
        dev.product
            .as_deref()
            .or(dev.manufacturer.as_deref())
            .unwrap_or("device"),
    );
    let _ = writeln!(
        out,
        " ({:04X}:{:04X}){}",
        dev.vendor_id,
        dev.product_id,
        if let Some(family) = dev.properties.get("dock_family") {
            format!(" ({family})")
        } else {
            String::new()
        }
    );

    // Find direct children (internal hubs).
    let Some(direct_kids) = by_parent.get(&dev.id) else {
        return;
    };

    // Filter to hub children only — these are the internal hubs.
    let mut hubs: Vec<&&UsbDevice> = direct_kids
        .iter()
        .filter(|k| k.is_hub || k.device_class == skirr_core::UsbClass::Hub)
        .collect();
    hubs.sort_by_key(|k| k.port_number.unwrap_or(0));

    // Non-hub children (compound interfaces, etc.)
    let non_hubs: Vec<&&UsbDevice> = direct_kids
        .iter()
        .filter(|k| !k.is_hub && k.device_class != skirr_core::UsbClass::Hub)
        .collect();

    // Write each internal hub.
    for (i, hub) in hubs.iter().enumerate() {
        let is_last_hub = i + 1 == hubs.len() && non_hubs.is_empty();
        let glyph = if is_last_hub { "└─" } else { "├─" };
        let child_prefix = format!("{prefix}{glyph} ");

        // Hub header.
        let _ = write!(out, "{prefix}{glyph} ");
        write_node(out, hub);

        // Hub's own children (looked up from by_parent, not direct_kids).
        let hub_kids: Vec<&&UsbDevice> = by_parent
            .get(&hub.id)
            .map(|v| v.iter().collect())
            .unwrap_or_default();

        // Hub's downstream ports.
        if let Some(info) = &hub.hub_info {
            if !info.dock_ports.is_empty() {
                let total = info.dock_ports.len();
                for (j, dp) in info.dock_ports.iter().enumerate() {
                    let is_last = j + 1 == total;
                    let port_glyph = if is_last { "└─" } else { "├─" };
                    let port_prefix = format!("{child_prefix}│ ");

                    if let Some(usb_port) = dp.usb_hub_port {
                        if let Some(kid) = hub_kids.iter().find(|c| c.port_number == Some(usb_port))
                        {
                            let _ = write!(out, "{port_prefix}{port_glyph} ");
                            write_node(out, kid);
                            write_chain(out, kid, by_parent, &port_prefix, is_last);
                        } else {
                            let _ = writeln!(out, "{port_prefix}{port_glyph} {} — n/a", dp.label);
                        }
                    } else {
                        // Non-USB port (HDMI, Ethernet, etc.)
                        let _ = writeln!(out, "{port_prefix}{port_glyph} {}", dp.label);
                    }
                }
            } else {
                // Fallback: expand by port count.
                let port_count = info.port_count;
                for pn in 1..=port_count {
                    let is_last = pn == port_count;
                    let port_glyph = if is_last { "└─" } else { "├─" };
                    let port_prefix = format!("{child_prefix}│ ");
                    if let Some(kid) = hub_kids.iter().find(|c| c.port_number == Some(pn)) {
                        let _ = write!(out, "{port_prefix}{port_glyph} ");
                        write_node(out, kid);
                        write_chain(out, kid, by_parent, &port_prefix, is_last);
                    } else {
                        let _ = writeln!(out, "{port_prefix}{port_glyph} free — n/a");
                    }
                }
            }
        } else if !hub_kids.is_empty() {
            // Hub without hub_info — render children directly.
            for (j, kid) in hub_kids.iter().enumerate() {
                let is_last = j + 1 == hub_kids.len();
                let port_glyph = if is_last { "└─" } else { "├─" };
                let port_prefix = format!("{child_prefix}│ ");
                let port = kid
                    .port_number
                    .map(|p| format!("p{p} "))
                    .unwrap_or_default();
                let _ = write!(out, "{port_prefix}{port_glyph} {port}");
                write_node(out, kid);
                write_chain(out, kid, by_parent, &port_prefix, is_last);
            }
        }
    }

    // Non-hub children (compound interfaces, direct devices, etc.)
    for (i, kid) in non_hubs.iter().enumerate() {
        let is_last = i + 1 == non_hubs.len();
        let glyph = if is_last { "└─" } else { "├─" };
        let child_prefix = format!("{prefix}{glyph} ");
        let port = kid
            .port_number
            .map(|p| format!("p{p} "))
            .unwrap_or_default();
        let _ = write!(out, "{prefix}{glyph} {port}");
        write_node(out, kid);
        write_chain(out, kid, by_parent, &child_prefix, is_last);
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

        let mut hub = device(0x2109, "ExtHub", UsbClass::Hub, Some(1));
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
                port_count: 4,
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
                children_ids: vec![hub.id],
            }],
            devices: vec![hub, leaf],
            hubs: Vec::new(),
            displays: Vec::new(),
            thunderbolt_routers: Vec::new(),
            type_c_ports: Vec::new(),
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

        assert!(tree.contains("HOST"), "host header:\n{tree}");
        assert!(tree.contains("Port 1 ──"), "chain head:\n{tree}");
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

        // Dock: ExtHub on root port 1 → internal dock sub-hub on p2 →
        // two leaf devices (p1, p4). Root hub has 4 ports.
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

        let lines: Vec<&str> = tree.lines().collect();
        let pos = |needle: &str| {
            lines
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle} missing:\n{tree}"))
        };
        // Chain order top-to-bottom: head, then sub-hub, then leaves.
        let head = pos("Port 1 ──");
        let sub = pos("DockSubHub");
        let kbd = pos("Keyboard");
        let camera = pos("Camera");
        assert!(head < sub && sub < kbd && kbd < camera, "order:\n{tree}");
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
            thunderbolt_routers: Vec::new(),
            type_c_ports: Vec::new(),
            events: Vec::new(),
            platform_info: platform(),
        };
        assert!(render_tree(&topo).contains("HOST"));
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
