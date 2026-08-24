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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Build the platform backend, mirroring the CLI dispatch.
fn build_backend() -> BackendResult<Box<dyn UsbBackend>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(skirr_macos::create_backend()))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(skirr_windows::create_backend()))
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
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
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
            .store(false, Ordering::SeqCst);
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
        .store(false, Ordering::SeqCst);
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
        .load(Ordering::SeqCst)
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
            get_topology_json,
            diagnose,
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
    fn nodes_carry_speed_hub_and_dock_annotations() {
        let chains = build_chains(&fixture());
        let head = &chains.external[0].ports[0].root;

        assert_eq!(head.speed_mbps, 5000);
        assert!(head.is_hub);
        assert_eq!(head.hub_ports, None, "fixture hub has no HubInfo");
    }
}
