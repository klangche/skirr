//! Skirr CLI - command-line interface for the Skirr USB diagnostic tool.
//!
//! Commands are implemented in Phase 4. This stub provides a compiling binary
//! with `--help` and `--version` wired through clap.

use clap::Parser;

/// Cross-platform USB analysis tool.
#[derive(Debug, Parser)]
#[command(name = "skirr", version, about, long_about = None)]
struct Cli {
    /// Emit machine-readable JSON where supported
    #[arg(long, global = true)]
    json: bool,
}

fn main() {
    let _cli = Cli::parse();
    println!("skirr: not implemented yet (Phase 4)");
}
