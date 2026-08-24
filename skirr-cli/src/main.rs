//! Skirr CLI - command-line interface for the Skirr USB diagnostic tool.

use clap::Parser;
use skirr_core::{BackendError, BackendResult, RuleEngine, SystemTopology, UsbBackend};
use std::process::ExitCode;

mod backend;
mod render;

/// Cross-platform USB analysis tool.
#[derive(Debug, Parser)]
#[command(name = "skirr", version, about, long_about = None)]
struct Cli {
    /// Emit machine-readable JSON where supported
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Full enumeration: platform info + every USB device
    Scan,
    /// USB devices only
    Usb,
    /// Tree view with hops/tiers
    Topology,
    /// Hub details with port mapping
    Hubs,
    /// Port-level details across all hubs
    Ports,
    /// Run the rule engine and show a verdict
    Diagnose,
    /// Live hotplug monitoring (Ctrl-C to exit)
    Monitor,
    /// Write a JSON report of the current state
    Report {
        /// Output path (default: ./skirr-report.json)
        #[arg(short, long, default_value = "skirr-report.json")]
        out: std::path::PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("skirr: {err}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, BackendError> {
    let backend = backend::create_backend()?;

    match cli.command {
        Command::Scan => cmd_scan(&*backend, cli.json)?,
        Command::Usb => cmd_usb(&*backend, cli.json)?,
        Command::Topology => cmd_topology(&*backend, cli.json)?,
        Command::Hubs => cmd_hubs(&*backend, cli.json)?,
        Command::Ports => cmd_ports(&*backend, cli.json)?,
        Command::Diagnose => return cmd_diagnose(&*backend, cli.json),
        Command::Monitor => cmd_monitor(&*backend)?,
        Command::Report { out } => cmd_report(&*backend, &out)?,
    }
    Ok(ExitCode::SUCCESS)
}

fn load_topology(backend: &dyn UsbBackend) -> BackendResult<SystemTopology> {
    let mut topo = backend.get_topology()?;
    // Enrich devices with speed data where cheaply available; failures are
    // non-fatal for display commands (speeds stay Unknown).
    if let Ok(devices) = backend.enumerate_devices() {
        let by_id: std::collections::HashMap<String, _> =
            devices.iter().map(|d| (d.platform_id.clone(), d)).collect();
        for dev in &mut topo.devices {
            if let Some(fresh) = by_id.get(&dev.platform_id) {
                dev.max_supported_speed = fresh.max_supported_speed;
                dev.current_link_speed = fresh.current_link_speed;
            }
        }
    }
    Ok(topo)
}

fn cmd_scan(backend: &dyn UsbBackend, json: bool) -> BackendResult<()> {
    let topo = load_topology(backend)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&topo).map_err(json_err)?);
        return Ok(());
    }
    let info = &topo.platform_info;
    println!(
        "Host: {} ({}, {}){}",
        info.hostname.as_deref().unwrap_or("unknown"),
        info.os,
        info.os_version,
        if info.is_admin { " [elevated]" } else { "" }
    );
    println!(
        "Controllers: {}  Root hubs: {}  Devices: {}  Hubs: {}",
        topo.host_controllers.len(),
        topo.root_hubs.len(),
        topo.devices.len(),
        topo.hubs.len()
    );
    println!("{}", render::devices_table(&topo.devices));
    Ok(())
}

fn cmd_usb(backend: &dyn UsbBackend, json: bool) -> BackendResult<()> {
    let topo = load_topology(backend)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&topo.devices).map_err(json_err)?
        );
        return Ok(());
    }
    println!("{}", render::devices_table(&topo.devices));
    Ok(())
}
fn cmd_topology(backend: &dyn UsbBackend, json: bool) -> BackendResult<()> {
    let topo = load_topology(backend)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&topo).map_err(json_err)?);
        return Ok(());
    }
    print!("{}", render::render_tree(&topo));
    Ok(())
}

fn cmd_hubs(backend: &dyn UsbBackend, json: bool) -> BackendResult<()> {
    let topo = load_topology(backend)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&topo.hubs).map_err(json_err)?
        );
        return Ok(());
    }
    print!("{}", render::format_hubs(&topo));
    Ok(())
}

fn cmd_ports(backend: &dyn UsbBackend, json: bool) -> BackendResult<()> {
    let topo = load_topology(backend)?;
    if json {
        let ports = render::port_map(&topo);
        println!(
            "{}",
            serde_json::to_string_pretty(&ports).map_err(json_err)?
        );
        return Ok(());
    }
    print!("{}", render::format_ports(&topo));
    Ok(())
}

/// Diagnose exits 1 when the overall verdict is Fail so scripts can branch.
fn cmd_diagnose(backend: &dyn UsbBackend, json: bool) -> Result<ExitCode, BackendError> {
    let topo = backend.get_topology()?;
    let result = RuleEngine::standard().evaluate(&topo);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(json_err)?
        );
    } else {
        print!("{}", render::format_diagnosis(&result));
    }

    Ok(match result.overall_verdict {
        skirr_core::Verdict::Fail => ExitCode::from(1),
        _ => ExitCode::SUCCESS,
    })
}

fn cmd_monitor(backend: &dyn UsbBackend) -> BackendResult<()> {
    let session = backend.monitor()?;
    let mut session = session;
    session.start_monitoring()?;
    println!("Monitoring USB events - press Ctrl-C to stop.");
    loop {
        match session.poll_event(std::time::Duration::from_millis(500))? {
            Some(event) => {
                let port = event
                    .port_number
                    .map(|p| format!(" port {p}"))
                    .unwrap_or_default();
                println!(
                    "[{}] {:?}{} {}",
                    event.timestamp.to_rfc3339(),
                    event.event_type,
                    port,
                    event.details
                );
            }
            None => continue,
        }
    }
}

fn cmd_report(backend: &dyn UsbBackend, out: &std::path::Path) -> BackendResult<()> {
    let topo = backend.get_topology()?;
    let diagnosis = RuleEngine::standard().evaluate(&topo);

    let report = serde_json::json!({
        "tool": "skirr",
        "generated": chrono::Utc::now().to_rfc3339(),
        "platform": topo.platform_info,
        "topology": topo,
        "diagnosis": diagnosis,
    });
    let body = serde_json::to_string_pretty(&report).map_err(json_err)?;
    std::fs::write(out, body)
        .map_err(|e| BackendError::os_api("skirr-cli", format!("write {}: {e}", out.display())))?;
    println!("Report written to {}", out.display());
    Ok(())
}

fn json_err(e: serde_json::Error) -> BackendError {
    BackendError::os_api("skirr-cli", format!("json encode: {e}"))
}
