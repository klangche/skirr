//! Skirr GUI — Tauri 2 shell around skirr-core.
//!
//! All data flows through the same public core APIs the CLI uses:
//! enumeration via [`skirr_core::UsbBackend`], analysis via
//! [`skirr_core::RuleEngine`], reports via `SkirrReport`/`render_html`.
//! The per-port chain view is computed server-side here so the web layer
//! only renders what it receives.

use serde::Serialize;
use skirr_core::{
    BackendResult, DiagnosticEvent, DiagnosticResult, EventSummary, EventType, RuleEngine,
    SystemTopology, UsbBackend, UsbDevice,
};
use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

// ---------------------------------------------------------------------------
// Version comparison for update checking.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    Alpha = 0,
    Beta = 1,
    Stable = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    major: u32,
    minor: u32,
    patch: u32,
    tier: Tier,
    revision: u32,
}

impl Version {
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim_start_matches('v');
        let (semver_str, pre_str) = match s.find('-') {
            Some(pos) => (&s[..pos], Some(&s[pos + 1..])),
            None => (s, None),
        };
        let parts: Vec<u32> = semver_str
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();
        if parts.len() != 3 {
            return None;
        }
        let (tier, revision) = match pre_str {
            Some(p) if p.starts_with("alpha.") => {
                let rev = p.strip_prefix("alpha.")?.parse().ok()?;
                (Tier::Alpha, rev)
            }
            Some(p) if p.starts_with("beta.") => {
                let rev = p.strip_prefix("beta.")?.parse().ok()?;
                (Tier::Beta, rev)
            }
            Some(_) => return None,
            None => (Tier::Stable, 0),
        };
        Some(Version {
            major: parts[0],
            minor: parts[1],
            patch: parts[2],
            tier,
            revision,
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then(self.tier.cmp(&other.tier))
            .then(self.revision.cmp(&other.revision))
    }
}

/// Decide whether `candidate` is a valid update from `current`.
///
/// Default mode: candidate must be strictly newer AND tier must move forward
/// (candidate.tier >= current.tier).
///
/// Always-latest mode: candidate must be strictly newer, any tier allowed.
///
/// Universal rule: never downgrade.
fn should_update(current: &Version, candidate: &Version, always_latest: bool) -> bool {
    if candidate <= current {
        return false;
    }
    if always_latest {
        return true;
    }
    candidate.tier >= current.tier
}

/// Build the platform backend, mirroring the CLI dispatch.
fn build_backend() -> BackendResult<Box<dyn UsbBackend>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(skirr_macos::create_backend()))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(skirr_windows::create_backend())
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(skirr_linux::create_backend()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        use skirr_core::BackendError;
        Err(BackendError::unsupported(
            "skirr-gui",
            "no backend available for this platform",
        ))
    }
}

// ---------------------------------------------------------------------------
// Serializable view models for the web layer.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct ChainNode {
    id: uuid::Uuid,
    label: String,
    vid: u16,
    pid: u16,
    speed_mbps: u64,
    is_hub: bool,
    hub_ports: Option<u8>,
    dock_family: Option<String>,
    internal: bool,
    children: Vec<ChainNode>,
}

impl ChainNode {
    fn from_device(dev: &UsbDevice, children: Vec<ChainNode>) -> Self {
        Self {
            id: dev.id,
            label: dev
                .product
                .as_deref()
                .or(dev.manufacturer.as_deref())
                .unwrap_or("device")
                .to_string(),
            vid: dev.vendor_id,
            pid: dev.product_id,
            speed_mbps: dev.current_link_speed.mbps(),
            is_hub: dev.is_hub,
            hub_ports: dev.hub_info.as_ref().map(|h| h.port_count),
            dock_family: dev.properties.get("dock_family").cloned(),
            internal: dev.is_internal,
            children,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct PortChain {
    port: Option<u8>,
    root: ChainNode,
}

#[derive(Debug, Clone, Serialize)]
struct RootHubChains {
    platform_id: String,
    port_count: u8,
    ports: Vec<PortChain>,
    free_ports: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct TopologyChains {
    controllers: usize,
    devices: usize,
    internal: Vec<ChainNode>,
    external: Vec<RootHubChains>,
    /// Thunderbolt/USB4 fabric, separate from the USB tree (Phase 10.1).
    tb_routers: Vec<skirr_core::ThunderboltRouter>,
}

/// Group a topology into internal chains and one external chain per
/// occupied physical port — the same shape the CLI renders.
fn build_chains(topo: &SystemTopology) -> TopologyChains {
    let mut by_parent: std::collections::HashMap<Option<uuid::Uuid>, Vec<&UsbDevice>> =
        std::collections::HashMap::new();
    for dev in &topo.devices {
        by_parent.entry(dev.parent_id).or_default().push(dev);
    }
    for children in by_parent.values_mut() {
        children.sort_by_key(|d| (d.port_number.unwrap_or(0), d.id));
    }

    fn build_node(
        dev: &UsbDevice,
        by_parent: &std::collections::HashMap<Option<uuid::Uuid>, Vec<&UsbDevice>>,
    ) -> ChainNode {
        let children = by_parent
            .get(&Some(dev.id))
            .map(|kids| kids.iter().map(|c| build_node(c, by_parent)).collect())
            .unwrap_or_default();
        ChainNode::from_device(dev, children)
    }

    let mut internal = Vec::new();
    let mut external = Vec::new();

    for rh in &topo.root_hubs {
        let tier1: Vec<&UsbDevice> = by_parent
            .get(&None)
            .map(|roots| {
                roots
                    .iter()
                    .copied()
                    .filter(|d| d.root_hub_id == Some(rh.id))
                    .collect()
            })
            .unwrap_or_default();

        let mut ports = Vec::new();
        let mut free_ports = Vec::new();
        for dev in &tier1 {
            if dev.is_internal {
                internal.push(build_node(dev, &by_parent));
            } else if dev.port_number.is_some() {
                ports.push(PortChain {
                    port: dev.port_number,
                    root: build_node(dev, &by_parent),
                });
            }
        }
        ports.sort_by_key(|p| p.port.unwrap_or(0));
        let occupied: Vec<u8> = ports.iter().filter_map(|p| p.port).collect();
        free_ports.extend((1..=rh.port_count).filter(|p| !occupied.contains(p)));
        if !ports.is_empty() || !free_ports.is_empty() {
            external.push(RootHubChains {
                platform_id: rh.platform_id.clone(),
                port_count: rh.port_count,
                ports,
                free_ports,
            });
        }

        // Internal tier-1 devices of this hub already pushed above.
        let _ = tier1;
    }

    // Internal devices whose root hub vanished (defensive).
    for dev in topo
        .devices
        .iter()
        .filter(|d| d.is_internal && d.parent_id.is_none())
    {
        if !topo
            .root_hubs
            .iter()
            .any(|rh| Some(rh.id) == dev.root_hub_id)
        {
            internal.push(build_node(dev, &by_parent));
        }
    }

    TopologyChains {
        controllers: topo.host_controllers.len(),
        devices: topo.devices.len(),
        internal,
        external,
        tb_routers: topo.thunderbolt_routers.clone(),
    }
}

// ---------------------------------------------------------------------------
// Topology view — flat-tree structure for the ASCII connector renderer.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct TopologyView {
    root: TreeNode,
    warnings: Vec<WarningLine>,
    platform: Option<PlatformLimits>,
    tb_routers: Vec<skirr_core::ThunderboltRouter>,
}

#[derive(Debug, Clone, Serialize)]
struct TreeNode {
    label: String,
    /// "(Hops 2, Hubs 1, Tiers 3)" — only on port nodes.
    meta: Option<String>,
    vid: Option<u16>,
    pid: Option<u16>,
    speed_mbps: Option<u64>,
    bold: bool,
    hub_ports: Option<u8>,
    dock_family: Option<String>,
    display: Option<DisplayNode>,
    children: Vec<TreeNode>,
}

#[derive(Debug, Clone, Serialize)]
struct DisplayNode {
    name: String,
    resolution: Option<String>,
    refresh_hz: Option<u16>,
    hdr: bool,
}

#[derive(Debug, Clone, Serialize)]
struct WarningLine {
    severity: String,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct PlatformLimits {
    name: String,
    max_hops: u8,
    max_tiers: u8,
    max_hubs: u8,
}

/// Compute per-device external-hub count by walking the parent chain.
/// Returns (hops, external_hubs, tiers) for one device.
fn chain_metrics_for(
    device_id: uuid::Uuid,
    by_id: &std::collections::HashMap<uuid::Uuid, &UsbDevice>,
) -> (u8, u8, u8) {
    let mut hops: u8 = 0;
    let mut external_hubs: u8 = 0;
    let mut cursor = by_id.get(&device_id).and_then(|d| d.parent_id);
    while let Some(cid) = cursor {
        if let Some(parent) = by_id.get(&cid) {
            if parent.is_hub || parent.device_class == skirr_core::UsbClass::Hub {
                hops += 1;
                if !parent.is_internal {
                    external_hubs += 1;
                }
            }
            cursor = parent.parent_id;
        } else {
            break;
        }
    }
    (hops, external_hubs, hops.saturating_add(1))
}

/// Build a `TreeNode` subtree for one device and its descendants.
fn build_tree_node(
    dev: &UsbDevice,
    by_parent: &std::collections::HashMap<Option<uuid::Uuid>, Vec<&UsbDevice>>,
    by_id: &std::collections::HashMap<uuid::Uuid, &UsbDevice>,
    displays: &[skirr_core::DisplayInfo],
    bold_ids: &std::collections::HashSet<uuid::Uuid>,
) -> TreeNode {
    let mut children: Vec<TreeNode> = Vec::new();

    // If this hub has dock_ports, render the full physical port layout.
    if let Some(info) = &dev.hub_info {
        if !info.dock_ports.is_empty() {
            let direct_kids = by_parent.get(&Some(dev.id));
            for dp in &info.dock_ports {
                if let Some(usb_port) = dp.usb_hub_port {
                    // USB port — find the connected child device.
                    if let Some(kid) = direct_kids.and_then(|kids| {
                        kids.iter().find(|k| k.port_number == Some(usb_port))
                    }) {
                        children.push(build_tree_node(kid, by_parent, by_id, displays, bold_ids));
                    } else {
                        // Free USB port.
                        children.push(TreeNode {
                            label: format!("{} — n/a", dp.label),
                            meta: None,
                            vid: None,
                            pid: None,
                            speed_mbps: None,
                            bold: false,
                            hub_ports: None,
                            dock_family: None,
                            display: None,
                            children: Vec::new(),
                        });
                    }
                } else {
                    // Non-USB port (HDMI, Ethernet, Audio, etc.)
                    children.push(TreeNode {
                        label: dp.label.clone(),
                        meta: Some(dp.port_type.label().to_string()),
                        vid: None,
                        pid: None,
                        speed_mbps: None,
                        bold: false,
                        hub_ports: None,
                        dock_family: None,
                        display: None,
                        children: Vec::new(),
                    });
                }
            }
            // Emit children without port numbers (compound interfaces).
            if let Some(kids) = direct_kids {
                for kid in kids {
                    if kid.port_number.is_none() {
                        children.push(build_tree_node(kid, by_parent, by_id, displays, bold_ids));
                    }
                }
            }
        } else {
            // Hub without dock_ports — fallback to port-count expansion.
            let port_count = info.port_count;
            let direct_kids = by_parent.get(&Some(dev.id));
            for pn in 1..=port_count {
                if let Some(kid) = direct_kids.and_then(|kids| {
                    kids.iter().find(|k| k.port_number == Some(pn))
                }) {
                    children.push(build_tree_node(kid, by_parent, by_id, displays, bold_ids));
                } else {
                    children.push(TreeNode {
                        label: "n/a".to_string(),
                        meta: None,
                        vid: None,
                        pid: None,
                        speed_mbps: None,
                        bold: false,
                        hub_ports: None,
                        dock_family: None,
                        display: None,
                        children: Vec::new(),
                    });
                }
            }
            // Emit children without port numbers (compound interfaces).
            if let Some(kids) = direct_kids {
                for kid in kids {
                    if kid.port_number.is_none() {
                        children.push(build_tree_node(kid, by_parent, by_id, displays, bold_ids));
                    }
                }
            }
        }
    } else {
        // Non-hub: just emit all direct children sorted by port.
        if let Some(kids) = by_parent.get(&Some(dev.id)) {
            let mut sorted = kids.clone();
            sorted.sort_by_key(|k| k.port_number.unwrap_or(0));
            for kid in sorted {
                children.push(build_tree_node(kid, by_parent, by_id, displays, bold_ids));
            }
        }
    }

    // Attach display info if this device is the last hop in a display's USB path.
    let display_info = displays.iter().find_map(|d| {
        if let Some(ref path) = d.usb_path {
            if path.last() == Some(&dev.id) {
                return Some(DisplayNode {
                    name: d.name.clone().unwrap_or_else(|| "Display".to_string()),
                    resolution: d.current_resolution.as_ref().map(|r| {
                        format!(
                            "{}x{}@{}Hz",
                            r.width,
                            r.height,
                            d.current_refresh_rate.unwrap_or(0)
                        )
                    }),
                    refresh_hz: d.current_refresh_rate,
                    hdr: d.hdr_supported,
                });
            }
        }
        None
    });

    TreeNode {
        label: dev
            .product
            .as_deref()
            .or(dev.manufacturer.as_deref())
            .unwrap_or("device")
            .to_string(),
        meta: None,
        vid: Some(dev.vendor_id),
        pid: Some(dev.product_id),
        speed_mbps: Some(dev.current_link_speed.mbps()),
        bold: bold_ids.contains(&dev.id),
        hub_ports: dev.hub_info.as_ref().map(|h| h.port_count),
        dock_family: dev.properties.get("dock_family").cloned(),
        display: display_info,
        children,
    }
}

/// Build the complete topology view for the GUI tree renderer.
fn build_topology_view(topo: &SystemTopology) -> TopologyView {
    let by_parent: std::collections::HashMap<Option<uuid::Uuid>, Vec<&UsbDevice>> = {
        let mut m: std::collections::HashMap<Option<uuid::Uuid>, Vec<&UsbDevice>> =
            std::collections::HashMap::new();
        for dev in &topo.devices {
            m.entry(dev.parent_id).or_default().push(dev);
        }
        for kids in m.values_mut() {
            kids.sort_by_key(|d| (d.port_number.unwrap_or(0), d.id));
        }
        m
    };
    let by_id: std::collections::HashMap<uuid::Uuid, &UsbDevice> =
        topo.devices.iter().map(|d| (d.id, d)).collect();

    // Run rule engine to identify devices exceeding platform limits.
    let profile = skirr_core::Profile::standard_v1();
    let diagnosis = RuleEngine::new(profile.clone()).evaluate(topo);
    let platform = skirr_core::PlatformKey::detect(
        &topo.platform_info.os,
        &topo.platform_info.architecture,
    );
    let limits = platform.and_then(|p| profile.limits_for(p));
    let platform_name = platform
        .map(|p| p.display_name().to_string())
        .unwrap_or_default();

    // Collect device IDs that exceed limits (for bold rendering).
    let mut bold_ids = std::collections::HashSet::new();
    for issue in &diagnosis.topology_issues {
        use skirr_core::TopologyIssueType;
        matches!(
            issue.issue_type,
            TopologyIssueType::TooManyHubs
                | TopologyIssueType::TooManyHops
                | TopologyIssueType::TooManyTiers
        )
        .then(|| bold_ids.insert(issue.device_id));
    }

    // Build warning lines from failed/warning rule evaluations.
    let mut warnings: Vec<WarningLine> = diagnosis
        .rules_applied
        .iter()
        .filter(|r| {
            matches!(
                r.verdict,
                skirr_core::Verdict::Warning | skirr_core::Verdict::Fail
            ) && (r.rule_id == "max_hops"
                || r.rule_id == "max_tiers"
                || r.rule_id == "max_hubs"
                || r.rule_id == "platform_limits")
        })
        .map(|r| WarningLine {
            severity: format!("{:?}", r.verdict).to_lowercase(),
            text: r.explanation.clone(),
        })
        .collect();

    // Build speed bottleneck warnings.
    for b in &diagnosis.bottlenecks {
        use skirr_core::BottleneckSeverity;
        if b.severity != BottleneckSeverity::Minor {
            bold_ids.insert(b.device_id);
            warnings.push(WarningLine {
                severity: "warning".to_string(),
                text: format!(
                    "Speed bottleneck: device supports {} but runs at {}",
                    b.max_speed.marketing_name(),
                    b.current_speed.marketing_name()
                ),
            });
        }
    }

    // ------------------------------------------------------------------
    // Build port nodes from root hub ports.
    // Group root hub ports that belong to the same dock (same VID) into
    // a single logical port, since Apple Silicon splits USB2/USB3 hubs
    // across separate root ports.
    // ------------------------------------------------------------------
    let mut port_nodes: Vec<TreeNode> = Vec::new();

    // Collect root hub tier-1 external devices, keyed by root port number.
    let mut root_port_map: std::collections::HashMap<u8, &UsbDevice> =
        std::collections::HashMap::new();
    for rh in &topo.root_hubs {
        if let Some(roots) = by_parent.get(&None) {
            for dev in roots
                .iter()
                .filter(|d| d.root_hub_id == Some(rh.id) && !d.is_internal)
            {
                if let Some(pn) = dev.port_number {
                    root_port_map.insert(pn, dev);
                }
            }
        }
    }

    // Physical ports from root hubs.
    let mut physical_ports: Vec<u8> = Vec::new();
    for rh in &topo.root_hubs {
        for p in 1..=rh.port_count {
            physical_ports.push(p);
        }
    }
    physical_ports.sort_unstable();
    physical_ports.dedup();

    // Group by VID (dock identity) — same logic as CLI.
    let mut seen_ports: std::collections::HashSet<u8> = std::collections::HashSet::new();
    let mut grouped: Vec<(u8, Vec<u8>)> = Vec::new(); // (first_port, all_ports)

    for &pn in &physical_ports {
        if seen_ports.contains(&pn) {
            continue;
        }
        if let Some(dev) = root_port_map.get(&pn) {
            let vid_key = format!("vid:{:04X}", dev.vendor_id);
            let mut group = vec![pn];
            seen_ports.insert(pn);
            for &p in &physical_ports {
                if !seen_ports.contains(&p) {
                    if let Some(d) = root_port_map.get(&p) {
                        if format!("vid:{:04X}", d.vendor_id) == vid_key {
                            group.push(p);
                            seen_ports.insert(p);
                        }
                    }
                }
            }
            grouped.push((pn, group));
        } else {
            seen_ports.insert(pn);
            grouped.push((pn, vec![pn]));
        }
    }

    for (first_port, _group) in &grouped {
        if let Some(dev) = root_port_map.get(first_port) {
            // Occupied port — build dock node with internal hubs as children.
            let mut node = build_tree_node(
                dev,
                &by_parent,
                &by_id,
                &topo.displays,
                &bold_ids,
            );
            // Compute metrics.
            fn walk_subtree(
                dev_id: uuid::Uuid,
                by_parent: &std::collections::HashMap<Option<uuid::Uuid>, Vec<&UsbDevice>>,
                by_id: &std::collections::HashMap<uuid::Uuid, &UsbDevice>,
                worst: &mut (u8, u8),
                total_hubs: &mut u32,
                total_devices: &mut u32,
            ) {
                if let Some(kids) = by_parent.get(&Some(dev_id)) {
                    for kid in kids {
                        *total_devices += 1;
                        if (kid.is_hub || kid.device_class == skirr_core::UsbClass::Hub)
                            && !kid.is_internal
                        {
                            *total_hubs += 1;
                        }
                        let (h, _eh, t) = chain_metrics_for(kid.id, by_id);
                        worst.0 = worst.0.max(h);
                        worst.1 = worst.1.max(t);
                        walk_subtree(kid.id, by_parent, by_id, worst, total_hubs, total_devices);
                    }
                }
            }
            let mut worst = (0u8, 0u8);
            let mut total_hubs: u32 = 0;
            let mut total_devices: u32 = 0;
            walk_subtree(
                dev.id,
                &by_parent,
                &by_id,
                &mut worst,
                &mut total_hubs,
                &mut total_devices,
            );
            let (dh, _deh, dt) = chain_metrics_for(dev.id, &by_id);
            worst.0 = worst.0.max(dh);
            worst.1 = worst.1.max(dt);
            if (dev.is_hub || dev.device_class == skirr_core::UsbClass::Hub) && !dev.is_internal
            {
                total_hubs += 1;
            }
            node.meta = Some(format!(
                "Hops {} · Hubs {} · Tiers {} · {} device{}",
                worst.0,
                total_hubs,
                worst.1,
                total_devices,
                if total_devices == 1 { "" } else { "s" }
            ));
            node.label = format!("Port {first_port} ── {}", node.label);
            port_nodes.push(node);
        } else {
            // Unoccupied port.
            port_nodes.push(TreeNode {
                label: format!("Port {first_port} ── n/a"),
                meta: None,
                vid: None,
                pid: None,
                speed_mbps: None,
                bold: false,
                hub_ports: None,
                dock_family: None,
                display: None,
                children: Vec::new(),
            });
        }
    }
    port_nodes.sort_by_key(|n| {
        n.label
            .strip_prefix("Port ")
            .and_then(|s| s.split(' ').next()?.parse::<u8>().ok())
            .unwrap_or(0)
    });

    // Merge port nodes under HOST.
    let mut host_children: Vec<TreeNode> = Vec::new();
    for n in port_nodes {
        host_children.push(n);
    }

    let root = TreeNode {
        label: "HOST".to_string(),
        meta: None,
        vid: None,
        pid: None,
        speed_mbps: None,
        bold: false,
        hub_ports: None,
        dock_family: None,
        display: None,
        children: host_children,
    };

    TopologyView {
        root,
        warnings,
        platform: limits.map(|l| PlatformLimits {
            name: platform_name,
            max_hops: l.max_hops,
            max_tiers: l.max_tiers,
            max_hubs: l.max_hubs,
        }),
        tb_routers: topo.thunderbolt_routers.clone(),
    }
}

#[derive(Debug, Clone, Serialize)]
struct Overview {
    os: String,
    os_version: String,
    architecture: String,
    hostname: Option<String>,
    is_admin: bool,
    is_virtual_machine: bool,
    backend: String,
    controller_count: usize,
    device_count: usize,
    hub_count: usize,
    display_count: usize,
    connected_devices: usize,
    verdict: String,
    warning_rules: usize,
    failed_rules: usize,
}

// ---------------------------------------------------------------------------
// Commands.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct DeviceDetail {
    id: uuid::Uuid,
    label: String,
    platform_id: String,
    vid: u16,
    pid: u16,
    manufacturer: Option<String>,
    serial_number: Option<String>,
    class: String,
    max_speed_mbps: u64,
    current_speed_mbps: u64,
    is_hub: bool,
    hub_ports: Option<u8>,
    port_number: Option<u8>,
    tier: u8,
    hop_count: u8,
    status: String,
    dock_family: Option<String>,
    usb_c: Option<UsbCDetail>,
    power_contract_mw: Option<u32>,
    pps_supported: Option<bool>,
    has_thunderbolt: bool,
    has_usb4: bool,
}

#[derive(Debug, Clone, Serialize)]
struct UsbCDetail {
    port_type: String,
    current_mode: String,
    pd_supported: bool,
    pd_revision: Option<String>,
    alt_modes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct HubPortSlot {
    number: u8,
    device_label: Option<String>,
    vid: Option<u16>,
    pid: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
struct HubPortMap {
    id: uuid::Uuid,
    label: String,
    platform_id: String,
    port_count: u8,
    ports: Vec<HubPortSlot>,
}

#[derive(Debug, Clone, Serialize)]
struct DisplaySummary {
    name: String,
    manufacturer_id: Option<String>,
    connection_type: Option<String>,
    current_resolution: Option<String>,
    preferred_resolution: Option<String>,
    refresh_hz: Option<u16>,
    hdr: bool,
    primary: bool,
    internal: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DetailsPayload {
    devices: Vec<DeviceDetail>,
    hubs: Vec<HubPortMap>,
    displays: Vec<DisplaySummary>,
}

fn build_details(topo: &SystemTopology) -> DetailsPayload {
    let mut children_of: std::collections::HashMap<uuid::Uuid, Vec<&UsbDevice>> =
        std::collections::HashMap::new();
    for dev in &topo.devices {
        if let Some(pid) = dev.parent_id {
            children_of.entry(pid).or_default().push(dev);
        }
    }
    for kids in children_of.values_mut() {
        kids.sort_by_key(|d| d.port_number.unwrap_or(0));
    }

    let devices = topo
        .devices
        .iter()
        .map(|dev| DeviceDetail {
            id: dev.id,
            label: dev
                .product
                .as_deref()
                .or(dev.manufacturer.as_deref())
                .unwrap_or("device")
                .to_string(),
            platform_id: dev.platform_id.clone(),
            vid: dev.vendor_id,
            pid: dev.product_id,
            manufacturer: dev.manufacturer.clone(),
            serial_number: dev.serial_number.clone(),
            class: format!("{:?}", dev.device_class),
            max_speed_mbps: dev.max_supported_speed.mbps(),
            current_speed_mbps: dev.current_link_speed.mbps(),
            is_hub: dev.is_hub,
            hub_ports: dev.hub_info.as_ref().map(|h| h.port_count),
            port_number: dev.port_number,
            tier: dev.tier,
            hop_count: dev.hop_count,
            status: format!("{:?}", dev.connection_status),
            dock_family: dev.properties.get("dock_family").cloned(),
            usb_c: dev.usb_c_info.as_ref().map(|c| UsbCDetail {
                port_type: format!("{:?}", c.port_type),
                current_mode: format!("{:?}", c.current_mode),
                pd_supported: c.pd_supported,
                pd_revision: c.pd_revision.clone(),
                alt_modes: c.alt_modes.iter().map(|m| format!("{m:?}")).collect(),
            }),
            power_contract_mw: dev.power_info.as_ref().and_then(|p| p.contract_power_mw),
            pps_supported: dev.power_info.as_ref().map(|p| p.pps_supported),
            has_thunderbolt: dev.thunderbolt_info.is_some(),
            has_usb4: dev.usb4_info.is_some(),
        })
        .collect();

    let hubs = topo
        .devices
        .iter()
        .filter(|d| d.is_hub)
        .map(|hub| {
            let slots: Vec<HubPortSlot> =
                (1..=hub.hub_info.as_ref().map(|h| h.port_count).unwrap_or(0))
                    .map(|number| {
                        let occupant = children_of
                            .get(&hub.id)
                            .and_then(|kids| kids.iter().find(|k| k.port_number == Some(number)));
                        HubPortSlot {
                            number,
                            device_label: occupant
                                .and_then(|o| o.product.as_deref())
                                .map(str::to_string),
                            vid: occupant.map(|o| o.vendor_id),
                            pid: occupant.map(|o| o.product_id),
                        }
                    })
                    .collect();
            HubPortMap {
                id: hub.id,
                label: hub
                    .product
                    .as_deref()
                    .or(hub.manufacturer.as_deref())
                    .unwrap_or("hub")
                    .to_string(),
                platform_id: hub.platform_id.clone(),
                port_count: hub.hub_info.as_ref().map(|h| h.port_count).unwrap_or(0),
                ports: slots,
            }
        })
        .collect();

    let displays = topo
        .displays
        .iter()
        .map(|d| DisplaySummary {
            name: d
                .name
                .as_deref()
                .or(d.manufacturer_id.as_deref())
                .unwrap_or("Display")
                .to_string(),
            manufacturer_id: d.manufacturer_id.clone(),
            connection_type: d.connection_type.map(|t| format!("{t:?}")),
            current_resolution: d.current_resolution.as_ref().map(resolution_label),
            preferred_resolution: d.preferred_resolution.as_ref().map(resolution_label),
            refresh_hz: d.current_refresh_rate,
            hdr: d.hdr_supported,
            primary: d.is_primary,
            internal: d.is_internal,
        })
        .collect();

    DetailsPayload {
        devices,
        hubs,
        displays,
    }
}

fn resolution_label(r: &skirr_core::DisplayResolution) -> String {
    match (&r.aspect_ratio, r.is_interlaced) {
        (Some(ar), true) => format!("{}×{} {}i", r.width, r.height, ar),
        (Some(ar), false) => format!("{}×{} {}", r.width, r.height, ar),
        (None, true) => format!("{}×{}i", r.width, r.height),
        (None, false) => format!("{}×{}", r.width, r.height),
    }
}

#[tauri::command]
fn get_details() -> Result<DetailsPayload, String> {
    let topo = build_backend().map_err(|e| e.to_string())?.get_topology();
    match topo {
        Ok(t) => Ok(build_details(&t)),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn get_overview() -> Result<Overview, String> {
    let backend = build_backend().map_err(|e| e.to_string())?;
    let name = backend.name();
    let topo = backend.get_topology().map_err(|e| e.to_string())?;
    let diagnosis = RuleEngine::new(skirr_core::Profile::standard_v1()).evaluate(&topo);
    let (warning_rules, failed_rules) =
        diagnosis
            .rules_applied
            .iter()
            .fold((0, 0), |(w, f), r| match r.verdict {
                skirr_core::Verdict::Warning => (w + 1, f),
                skirr_core::Verdict::Fail => (w, f + 1),
                _ => (w, f),
            });
    Ok(Overview {
        os: topo.platform_info.os.clone(),
        os_version: topo.platform_info.os_version.clone(),
        architecture: topo.platform_info.architecture.clone(),
        hostname: topo.platform_info.hostname.clone(),
        is_admin: topo.platform_info.is_admin,
        is_virtual_machine: topo.platform_info.is_virtual_machine,
        backend: name.to_string(),
        controller_count: topo.host_controllers.len(),
        device_count: topo.devices.len(),
        hub_count: topo.hubs.len(),
        display_count: topo.displays.len(),
        connected_devices: topo
            .devices
            .iter()
            .filter(|d| d.connection_status == skirr_core::ConnectionStatus::Connected)
            .count(),
        verdict: format!("{:?}", diagnosis.overall_verdict).to_uppercase(),
        warning_rules,
        failed_rules,
    })
}

#[tauri::command]
fn get_port_chains() -> Result<TopologyChains, String> {
    let topo = build_backend().map_err(|e| e.to_string())?.get_topology();
    match topo {
        Ok(t) => Ok(build_chains(&t)),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn get_topology_view() -> Result<TopologyView, String> {
    let topo = build_backend()
        .map_err(|e| e.to_string())?
        .get_topology()
        .map_err(|e| e.to_string())?;
    Ok(build_topology_view(&topo))
}

#[tauri::command]
fn diagnose() -> Result<DiagnosticResult, String> {
    let backend = build_backend().map_err(|e| e.to_string())?;
    let topo = backend.get_topology().map_err(|e| e.to_string())?;
    Ok(RuleEngine::new(skirr_core::Profile::standard_v1()).evaluate(&topo))
}

#[tauri::command]
fn get_topology_json() -> Result<serde_json::Value, String> {
    let topo = build_backend().map_err(|e| e.to_string())?.get_topology();
    serde_json::to_value(topo.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
struct UpdateInfo {
    current: String,
    latest: String,
    url: String,
    update_available: bool,
}

#[tauri::command]
async fn check_for_update(always_latest: bool) -> Result<UpdateInfo, String> {
    let current_str = env!("CARGO_PKG_VERSION");
    let current = Version::parse(current_str).unwrap_or(Version {
        major: 0,
        minor: 0,
        patch: 0,
        tier: Tier::Stable,
        revision: 0,
    });

    let client = reqwest::Client::builder()
        .user_agent("skirr-gui")
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get("https://api.github.com/repos/klangche/skirr/releases/latest")
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }

    let release: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse error: {e}"))?;

    let tag_name = release["tag_name"]
        .as_str()
        .ok_or("missing tag_name in release")?;
    let html_url = release["html_url"]
        .as_str()
        .unwrap_or("https://github.com/klangche/skirr/releases")
        .to_string();

    let candidate = Version::parse(tag_name).ok_or_else(|| format!("invalid version: {tag_name}"))?;

    Ok(UpdateInfo {
        current: current_str.to_string(),
        latest: tag_name.to_string(),
        url: html_url,
        update_available: should_update(&current, &candidate, always_latest),
    })
}

#[tauri::command]
fn generate_report(path: String, html_too: bool) -> Result<Vec<String>, String> {
    let backend = build_backend().map_err(|e| e.to_string())?;
    let topo = backend.get_topology().map_err(|e| e.to_string())?;
    let profile = skirr_core::Profile::standard_v1();
    let diagnosis = RuleEngine::new(profile.clone()).evaluate(&topo);
    let report = skirr_core::report::SkirrReport::new(
        profile.name.clone(),
        topo.platform_info.clone(),
        topo,
        diagnosis,
    );

    let json_path = std::path::PathBuf::from(&path);
    std::fs::write(
        &json_path,
        report.to_pretty_json().map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write {}: {e}", json_path.display()))?;
    let mut written = vec![json_path.display().to_string()];

    if html_too {
        let html_path = json_path.with_extension("html");
        std::fs::write(&html_path, skirr_core::report_html::render_html(&report))
            .map_err(|e| format!("write {}: {e}", html_path.display()))?;
        written.push(html_path.display().to_string());
    }
    Ok(written)
}

// ---------------------------------------------------------------------------
// Live monitoring: background thread owns its own backend/session and pushes
// correlated events to the webview; stop is signaled through shared state.
// ---------------------------------------------------------------------------

struct AppState {
    monitor_running: AtomicBool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            monitor_running: AtomicBool::new(false),
        }
    }
}

const MONITOR_EVENT: &str = "monitor-event";
const MONITOR_STOPPED: &str = "monitor-stopped";

#[derive(Debug, Clone, Serialize)]
struct MonitorPayload {
    kind: String,
    text: String,
    severity: &'static str,
    flap_count: Option<usize>,
}

#[tauri::command]
fn monitor_start(app: AppHandle) -> Result<bool, String> {
    let state = app.state::<AppState>();
    if state
        .monitor_running
        .compare_exchange(false, true, AtomicOrdering::SeqCst, AtomicOrdering::SeqCst)
        .is_err()
    {
        return Ok(false); // already running
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        let result = run_monitor_loop(&handle);
        handle
            .state::<AppState>()
            .monitor_running
            .store(false, AtomicOrdering::SeqCst);
        let message = result
            .map_err(|e| e.to_string())
            .err()
            .unwrap_or_else(|| "stopped".to_string());
        let _ = handle.emit(MONITOR_STOPPED, message);
    });
    Ok(true)
}

#[tauri::command]
fn monitor_stop(app: AppHandle) -> Result<(), String> {
    app.state::<AppState>()
        .monitor_running
        .store(false, AtomicOrdering::SeqCst);
    Ok(())
}

fn run_monitor_loop(handle: &AppHandle) -> BackendResult<()> {
    let backend = build_backend()?;
    let mut session = backend.monitor()?;
    session.start_monitoring()?;
    let topo = build_backend()?.get_topology()?;
    let mut correlator = skirr_core::correlate::EventCorrelator::new(&topo);
    let mut summary = EventSummary::default();

    while handle
        .state::<AppState>()
        .monitor_running
        .load(AtomicOrdering::SeqCst)
    {
        match session.poll_event(Duration::from_millis(250))? {
            Some(event) => {
                let outcome = correlator.ingest(event.clone());
                apply_summary(&mut summary, &event);
                if let Some(correlated) = outcome.correlated {
                    let payload = match &correlated {
                        skirr_core::correlate::CorrelatedEvent::Single(e) => MonitorPayload {
                            kind: format!("{:?}", e.event_type),
                            text: e.details.clone(),
                            severity: severity_tag(e.severity),
                            flap_count: None,
                        },
                        skirr_core::correlate::CorrelatedEvent::HubRemoval {
                            hub_event,
                            child_disconnects,
                            max_depth,
                        } => MonitorPayload {
                            kind: "HubRemoval".to_string(),
                            text: format!(
                                "{} — collapsed {} device(s), depth {}",
                                hub_event.details, child_disconnects, max_depth
                            ),
                            severity: "critical",
                            flap_count: None,
                        },
                    };
                    let _ = handle.emit(MONITOR_EVENT, payload);
                }
                if let Some(count) = outcome.flap_count {
                    let _ = handle.emit(
                        MONITOR_EVENT,
                        MonitorPayload {
                            kind: "Flap".to_string(),
                            text: format!("device flapping ({count} connects in 60s)"),
                            severity: "warning",
                            flap_count: Some(count as usize),
                        },
                    );
                }
            }
            None => continue,
        }
    }
    session.stop_monitoring()?;
    let _ = handle.emit(
        MONITOR_EVENT,
        MonitorPayload {
            kind: "Summary".to_string(),
            text: format!(
                "{} event(s): {} connect(s), {} disconnect(s), {} re-enum(s); stability {}/100",
                summary.total_events,
                summary.connects,
                summary.disconnects,
                summary.re_enumerations,
                summary.stability_score
            ),
            severity: "info",
            flap_count: None,
        },
    );
    Ok(())
}

fn apply_summary(summary: &mut EventSummary, event: &DiagnosticEvent) {
    summary.total_events += 1;
    match event.event_type {
        EventType::DeviceConnected => summary.connects += 1,
        EventType::DeviceDisconnected => summary.disconnects += 1,
        EventType::DeviceReEnumerated => summary.re_enumerations += 1,
        _ => {}
    }
}

fn severity_tag(severity: skirr_core::EventSeverity) -> &'static str {
    match severity {
        skirr_core::EventSeverity::Info => "info",
        skirr_core::EventSeverity::Warning => "warning",
        skirr_core::EventSeverity::Error | skirr_core::EventSeverity::Critical => "critical",
    }
}

// ---------------------------------------------------------------------------
// App bootstrap.
// ---------------------------------------------------------------------------

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_overview,
            get_port_chains,
            get_topology_view,
            get_topology_json,
            get_details,
            diagnose,
            check_for_update,
            generate_report,
            monitor_start,
            monitor_stop,
        ])
        .setup(|app| {
            use tauri::menu::{MenuBuilder, PredefinedMenuItem};
            let menu = MenuBuilder::new(app)
                .item(&PredefinedMenuItem::about(
                    app,
                    Some("Skirr"),
                    Some(tauri::menu::AboutMetadata {
                        name: Some("Skirr".into()),
                        ..Default::default()
                    }),
                )?)
                .separator()
                .item(&PredefinedMenuItem::quit(app, None)?)
                .build()?;
            app.set_menu(menu)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Skirr");
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use skirr_core::{ConnectionStatus, HostController, PlatformInfo, RootHub, UsbClass, UsbSpeed};

    fn device(vid: u16, pid: u16, product: &str, class: UsbClass, port: Option<u8>) -> UsbDevice {
        let mut d = UsbDevice::new(vid, pid);
        d.product = Some(product.into());
        d.device_class = class;
        d.is_hub = class == UsbClass::Hub;
        d.port_number = port;
        d.current_link_speed = UsbSpeed::SuperSpeed;
        d.connection_status = ConnectionStatus::Connected;
        d
    }

    /// Root hub (6 ports) → ExtHub on p4 → sub-hub on p2 → leaf on p1.
    fn fixture() -> SystemTopology {
        let hc_id = uuid::Uuid::new_v4();
        let rh_id = uuid::Uuid::new_v4();

        let mut hub = device(0x2109, 0x0817, "ExtHub", UsbClass::Hub, Some(4));
        hub.properties
            .insert("dock_family".into(), "VIA Labs".into());
        hub.root_hub_id = Some(rh_id);
        let mut leaf = device(0x0781, 0x5583, "FlashDrive", UsbClass::MassStorage, Some(3));
        leaf.parent_id = Some(hub.id);

        SystemTopology {
            timestamp: Utc::now(),
            host_controllers: vec![HostController {
                id: hc_id,
                platform_id: "BUS_1".into(),
                name: "xHCI".into(),
                vendor_id: None,
                device_id: None,
                revision: None,
                usb_version: UsbSpeed::SuperSpeed,
                root_hub_ids: vec![rh_id],
                port_count: 1,
                is_xhci: true,
                pci_address: None,
                driver_version: None,
                capabilities: Default::default(),
            }],
            root_hubs: vec![RootHub {
                id: rh_id,
                platform_id: "ROOT_HUB\\BUS_1".into(),
                host_controller_id: hc_id,
                port_count: 6,
                hub_speed: UsbSpeed::SuperSpeed,
                is_integrated: true,
                children_ids: vec![],
            }],
            devices: vec![hub, leaf],
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

    #[test]
    fn chains_split_internal_and_external_per_port() {
        let chains = build_chains(&fixture());

        assert_eq!(chains.controllers, 1);
        assert_eq!(chains.devices, 2);
        assert!(chains.internal.is_empty(), "no internal devices in fixture");

        assert_eq!(chains.external.len(), 1, "one root hub section");
        let rh = &chains.external[0];
        assert_eq!(rh.port_count, 6);
        assert_eq!(rh.ports.len(), 1, "one occupied port");
        assert_eq!(rh.free_ports, vec![1, 2, 3, 5, 6]);

        let head = &rh.ports[0];
        assert_eq!(head.port, Some(4));
        assert_eq!(head.root.label, "ExtHub");
        assert_eq!(head.root.dock_family.as_deref(), Some("VIA Labs"));
        // Leaf nests under the head even though the fixture didn't wire
        // children_ids — parent_id grouping is authoritative.
        assert!(head.root.children.iter().any(|c| c.label == "FlashDrive"));
    }

    #[test]
    fn chains_surface_thunderbolt_routers() {
        let mut topo = fixture();
        topo.thunderbolt_routers
            .push(skirr_core::ThunderboltRouter {
                id: "0x05AC9DB544B7AC62".into(),
                name: "MacBook Pro".into(),
                vendor_name: Some("Apple Inc.".into()),
                route_string: Some("0".into()),
                domain_uuid: None,
                generation: None,
                is_usb4: true,
                security_level: None,
                nvm_version: None,
                depth: 0,
                status: None,
                receptacles: Vec::new(),
            });

        let chains = build_chains(&topo);
        assert_eq!(chains.tb_routers.len(), 1);
        assert!(chains.tb_routers[0].is_usb4);
    }

    #[test]
    fn details_carry_device_hub_and_display_summaries() {
        let mut topo = fixture();
        topo.devices[0].hub_info = Some(skirr_core::HubInfo {
            port_count: 4,
            is_powered: true,
            power_source: skirr_core::HubPowerSource::SelfPowered,
            supports_mtt: false,
            tt_count: 1,
            tt_type: skirr_core::HubTTType::SingleTT,
            hub_speed: UsbSpeed::SuperSpeed,
            ports: Vec::new(),
            dock_ports: Vec::new(),
        });
        topo.devices[1].serial_number = Some("SN-123".into());

        let details = build_details(&topo);

        // Device details.
        assert_eq!(details.devices.len(), 2);
        let leaf = &details.devices[1];
        assert_eq!(leaf.serial_number.as_deref(), Some("SN-123"));
        assert_eq!(leaf.class, "MassStorage");
        assert!(!leaf.is_hub);

        // Hub port map: port 3 occupied by FlashDrive, others empty.
        assert_eq!(details.hubs.len(), 1);
        let map = &details.hubs[0];
        assert_eq!(map.port_count, 4);
        let p3 = map.ports.iter().find(|p| p.number == 3).expect("port 3");
        assert_eq!(p3.device_label.as_deref(), Some("FlashDrive"));
        assert_eq!(p3.vid, Some(0x0781));
        let p1 = map.ports.iter().find(|p| p.number == 1).expect("port 1");
        assert!(p1.device_label.is_none(), "port 1 free");

        // Empty displays serialize as empty list.
        assert!(details.displays.is_empty());
    }

    #[test]
    fn nodes_carry_speed_hub_and_dock_annotations() {
        let chains = build_chains(&fixture());
        let head = &chains.external[0].ports[0].root;

        assert_eq!(head.speed_mbps, 5000);
        assert!(head.is_hub);
        assert_eq!(head.hub_ports, None, "fixture hub has no HubInfo");
    }

    // --- Version parsing tests ---

    #[test]
    fn version_parse_stable() {
        let v = Version::parse("0.1.0").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 0);
        assert_eq!(v.tier, Tier::Stable);
        assert_eq!(v.revision, 0);
    }

    #[test]
    fn version_parse_alpha() {
        let v = Version::parse("v0.1.0-alpha.5").unwrap();
        assert_eq!(v.tier, Tier::Alpha);
        assert_eq!(v.revision, 5);
    }

    #[test]
    fn version_parse_beta() {
        let v = Version::parse("v0.2.3-beta.12").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert_eq!(v.tier, Tier::Beta);
        assert_eq!(v.revision, 12);
    }

    #[test]
    fn version_ordering() {
        let alpha3 = Version::parse("0.1.0-alpha.3").unwrap();
        let alpha5 = Version::parse("0.1.0-alpha.5").unwrap();
        let beta1 = Version::parse("0.1.0-beta.1").unwrap();
        let stable = Version::parse("0.1.0").unwrap();
        let stable11 = Version::parse("0.1.1").unwrap();

        assert!(alpha3 < alpha5);
        assert!(alpha5 < beta1);
        assert!(beta1 < stable);
        assert!(stable < stable11);
    }

    #[test]
    fn should_update_within_tier() {
        let current = Version::parse("0.1.0-alpha.3").unwrap();
        let newer = Version::parse("0.1.0-alpha.5").unwrap();
        let older = Version::parse("0.1.0-alpha.2").unwrap();
        assert!(should_update(&current, &newer, false));
        assert!(!should_update(&current, &older, false));
    }

    #[test]
    fn should_update_forward_tier_default() {
        let current = Version::parse("0.1.0-alpha.3").unwrap();
        let beta = Version::parse("0.1.0-beta.1").unwrap();
        let stable = Version::parse("0.1.0").unwrap();
        assert!(should_update(&current, &beta, false));
        assert!(should_update(&current, &stable, false));
    }

    #[test]
    fn should_not_update_backward_tier_default() {
        let current = Version::parse("0.1.0-beta.2").unwrap();
        let alpha = Version::parse("0.1.0-alpha.5").unwrap();
        assert!(!should_update(&current, &alpha, false));
    }

    #[test]
    fn should_update_always_latest_any_tier() {
        let current = Version::parse("0.1.0-beta.2").unwrap();
        let alpha = Version::parse("0.2.0-alpha.1").unwrap();
        assert!(should_update(&current, &alpha, true));
    }

    #[test]
    fn should_never_downgrade() {
        let current = Version::parse("0.1.0-alpha.5").unwrap();
        let older = Version::parse("0.1.0-alpha.3").unwrap();
        let lower_tier = Version::parse("0.0.9").unwrap();
        assert!(!should_update(&current, &older, false));
        assert!(!should_update(&current, &older, true));
        assert!(!should_update(&current, &lower_tier, true));
    }
}
