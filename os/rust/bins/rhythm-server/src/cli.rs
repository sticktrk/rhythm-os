//! rhythm-cli — manage the rhythm-server service and binary updates.
//!
//! Usage:
//!   rhythm-cli start     Start the service
//!   rhythm-cli stop      Stop the service
//!   rhythm-cli restart   Restart the service
//!   rhythm-cli status    Show service status
//!   rhythm-cli update    Self-update from the configured OTA feed

mod service_ctl;

use clap::{Parser, Subcommand};
use rhythm_server::self_update;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "rhythm-cli", version, about = "Manage the Rhythm OS server")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the service
    Start,
    /// Stop the service
    Stop,
    /// Restart the service
    Restart,
    /// Show service status
    Status,
    /// Self-update to the latest version from the configured OTA feed
    Update,
    /// Uninstall service, binaries, and optionally data
    Uninstall,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Start => service_ctl::start(),
        Commands::Stop => service_ctl::stop(),
        Commands::Restart => service_ctl::restart(),
        Commands::Status => service_ctl::status(),
        Commands::Update => do_update(),
        Commands::Uninstall => service_ctl::uninstall(),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn do_update() -> Result<(), String> {
    println!("Rhythm Server v{}", VERSION);
    println!("Checking for updates...");

    let info = self_update::check_blocking(VERSION)?;

    if !info.update_available {
        println!("Already up to date.");
        return Ok(());
    }

    println!(
        "Update available: v{} -> v{}",
        info.current_version, info.latest_version
    );

    let url = info
        .download_url
        .clone()
        .ok_or("No download URL for this platform")?;
    let asset_name = info
        .asset_name
        .clone()
        .ok_or("No release asset for this platform")?;

    println!("Downloading...");
    let apply_result = self_update::apply_blocking(
        &url,
        &asset_name,
        info.expected_sha256.as_deref(),
        info.checksum_url.as_deref(),
    )?;

    println!("Updated to v{}.", info.latest_version);
    if apply_result.checksum_verified == Some(true) {
        println!("Checksum verified.");
    }

    // Restart service with new binary
    println!("Restarting service...");
    service_ctl::restart()
}
