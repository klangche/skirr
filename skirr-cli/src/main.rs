//! Skirr CLI - command-line interface for the Skirr USB diagnostic tool.

use clap::Parser;
use skirr_core::{BackendError, BackendResult, Profile, RuleEngine, SystemTopology, UsbBackend};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

mod backend;
mod render;

/// Cross-platform USB analysis tool.
#[derive(Debug, Parser)]
#[command(
    name = "skirr",
    version,
    about,
    long_about = None,
    after_help = "Exit codes: 0 = ok, 1 = diagnose found FAIL verdicts, 2 = backend or input error"
)]
struct Cli {
    /// Emit machine-readable JSON where supported
    #[arg(long, global = true)]
    json: bool,

    /// Path to a custom profile JSON (defaults to Skirr Standard Profile v1.0)
    #[arg(long, global = true, value_name = "FILE")]
    profile: Option<PathBuf>,

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
    Monitor {
        /// Stop automatically after this many seconds (default: run until Ctrl-C)
        #[arg(long)]
        duration: Option<u64>,
        /// Poll interval in milliseconds
        #[arg(long, default_value_t = 500)]
        interval: u64,
    },
    /// Write a JSON report of the current state
    Report {
        /// Output path (default: ./skirr-report.json)
        #[arg(short, long, default_value = "skirr-report.json")]
        out: std::path::PathBuf,
        /// Minified single-line JSON instead of pretty-printed
        #[arg(long)]
        compact: bool,
        /// Also write a self-contained HTML report to this path
        #[arg(long)]
        html: Option<std::path::PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", friendly_error(&err));
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, BackendError> {
    let profile = load_profile(cli.profile.as_deref())?;
    let backend = backend::create_backend()?;

    match cli.command {
        Command::Scan => cmd_scan(&*backend, cli.json)?,
        Command::Usb => cmd_usb(&*backend, cli.json)?,
        Command::Topology => cmd_topology(&*backend, cli.json)?,
        Command::Hubs => cmd_hubs(&*backend, cli.json)?,
        Command::Ports => cmd_ports(&*backend, cli.json)?,
        Command::Diagnose => return cmd_diagnose(&*backend, cli.json, &profile),
        Command::Monitor { duration, interval } => cmd_monitor(&*backend, duration, interval)?,
        Command::Report { out, compact, html } => {
            cmd_report(&*backend, &out, &profile, compact, html)?
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Load a user-supplied profile JSON; `None` selects the built-in standard.
pub fn load_profile(path: Option<&Path>) -> BackendResult<Profile> {
    match path {
        None => Ok(Profile::standard_v1()),
        Some(path) => {
            let body = std::fs::read_to_string(path).map_err(|e| {
                BackendError::os_api("skirr-cli", format!("read {}: {e}", path.display()))
            })?;
            serde_json::from_str(&body).map_err(|e| invalid_profile(path, &e))
        }
    }
}

fn invalid_profile(path: &Path, e: &serde_json::Error) -> BackendError {
    BackendError::os_api(
        "skirr-cli",
        format!("invalid profile {}: {e}", path.display()),
    )
}

/// Human-facing error lines with actionable hints where the cause is known.
fn friendly_error(err: &BackendError) -> String {
    match err {
        BackendError::PermissionDenied { .. } => {
            format!("{err}\nhint: re-run elevated (sudo / admin shell), or replug the device")
        }
        BackendError::Unsupported { backend, .. } if *backend == "skirr-cli" => {
            format!("{err}\nhint: skirr currently supports macOS and Windows")
        }
        _ => err.to_string(),
    }
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
fn cmd_diagnose(
    backend: &dyn UsbBackend,
    json: bool,
    profile: &Profile,
) -> Result<ExitCode, BackendError> {
    let topo = backend.get_topology()?;
    let result = RuleEngine::new(profile.clone()).evaluate(&topo);

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

/// Flag set by the signal watcher so the poll loop can shut down gracefully.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn install_shutdown_watcher() {
    std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("signal runtime");
        rt.block_on(async {
            if tokio::signal::ctrl_c().await.is_ok() {
                SHUTDOWN.store(true, Ordering::SeqCst);
            }
        });
    });
}

fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

fn cmd_monitor(
    backend: &dyn UsbBackend,
    duration: Option<u64>,
    interval_ms: u64,
) -> BackendResult<()> {
    let mut session = backend.monitor()?;
    session.start_monitoring()?;
    install_shutdown_watcher();
    match duration {
        Some(secs) => println!("Monitoring USB events for {secs}s (Ctrl-C to stop early)."),
        None => println!("Monitoring USB events - press Ctrl-C to stop."),
    }

    let started = std::time::Instant::now();
    let poll_interval = Duration::from_millis(interval_ms.max(50));

    // Correlate raw events against the current topology snapshot.
    let topo = backend.get_topology()?;
    let mut correlator = skirr_core::correlate::EventCorrelator::new(&topo);

    while !shutdown_requested() {
        if let Some(secs) = duration {
            if started.elapsed().as_secs() >= secs {
                break;
            }
        }
        match session.poll_event(poll_interval)? {
            Some(event) => {
                let outcome = correlator.ingest(event);
                match outcome.correlated {
                    Some(skirr_core::correlate::CorrelatedEvent::HubRemoval {
                        hub_event,
                        child_disconnects,
                        max_depth,
                    }) => {
                        println!(
                            "[{}] HUB REMOVAL {} ({} device(s), depth {max_depth})",
                            hub_event.timestamp.to_rfc3339(),
                            hub_event.details,
                            child_disconnects
                        );
                    }
                    Some(skirr_core::correlate::CorrelatedEvent::Single(event)) => {
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
                    None => {}
                }
                if let Some(count) = outcome.flap_count {
                    eprintln!(
                        "  WARNING: device flapping ({} connects in the last minute)",
                        count
                    );
                }
            }
            None => continue,
        }
    }
    session.stop_monitoring()?;

    let summary = correlator.finish(duration.unwrap_or_else(|| started.elapsed().as_secs()));
    println!(
        "Monitor stopped. {} event(s): {} connect(s), {} disconnect(s), {} re-enum(s); stability {}/100.",
        summary.total_events,
        summary.connects,
        summary.disconnects,
        summary.re_enumerations,
        summary.stability_score
    );
    Ok(())
}

fn cmd_report(
    backend: &dyn UsbBackend,
    out: &Path,
    profile: &Profile,
    compact: bool,
    html: Option<PathBuf>,
) -> BackendResult<()> {
    let topo = backend.get_topology()?;
    let diagnosis = RuleEngine::new(profile.clone()).evaluate(&topo);

    let report = skirr_core::report::SkirrReport::new(
        profile.name.clone(),
        topo.platform_info.clone(),
        topo,
        diagnosis,
    );
    let body = if compact {
        report.to_compact_json()
    } else {
        report.to_pretty_json()
    }
    .map_err(json_err)?;
    std::fs::write(out, body)
        .map_err(|e| BackendError::os_api("skirr-cli", format!("write {}: {e}", out.display())))?;

    if let Some(html_path) = html {
        let page = skirr_core::report_html::render_html(&report);
        std::fs::write(&html_path, page).map_err(|e| {
            BackendError::os_api("skirr-cli", format!("write {}: {e}", html_path.display()))
        })?;
        println!("HTML report written to {}", html_path.display());
    }
    println!("Report written to {}", out.display());
    Ok(())
}

fn json_err(e: serde_json::Error) -> BackendError {
    BackendError::os_api("skirr-cli", format!("json encode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(body: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("skirr-profile-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn default_profile_is_standard() {
        let p = load_profile(None).unwrap();
        assert_eq!(p.version, "1.0");
        assert_eq!(p.name, Profile::standard_v1().name);
    }

    #[test]
    fn custom_profile_round_trips() {
        let standard = Profile::standard_v1();
        let body = serde_json::to_string_pretty(&standard).unwrap();
        let path = write_temp(&body);
        let loaded = load_profile(Some(&path)).unwrap();
        assert_eq!(loaded, standard);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_file_is_clean_os_api_error() {
        let err = load_profile(Some(Path::new("/nonexistent/skirr-profile.json"))).unwrap_err();
        assert!(matches!(err, BackendError::OsApi { .. }));
        assert!(err.to_string().contains("/nonexistent"));
    }

    #[test]
    fn malformed_json_reports_path_and_cause() {
        let path = write_temp("{ not json ");
        let err = load_profile(Some(&path)).unwrap_err();
        assert!(matches!(err, BackendError::OsApi { .. }));
        assert!(err.to_string().contains("invalid profile"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn permission_errors_get_a_hint() {
        let msg = friendly_error(&BackendError::PermissionDenied {
            backend: "skirr-macos",
            reason: "IOServiceOpen".into(),
        });
        assert!(msg.contains("hint:"), "{msg}");
    }

    #[test]
    fn unsupported_cli_error_suggests_platforms() {
        let msg = friendly_error(&BackendError::unsupported("skirr-cli", "no backend"));
        assert!(msg.contains("macOS and Windows"), "{msg}");
    }
}
