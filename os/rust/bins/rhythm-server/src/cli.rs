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

const VERSION: &str = rhythm_server::BUILD_VERSION;

#[derive(Parser)]
#[command(name = "rhythm-cli", version = VERSION, about = "Manage the Rhythm OS server")]
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

    // `rhythm-server self-update` is a manual operator action; mirror the
    // "manual = beta" runtime mapping. Override with RHYTHM_UPDATE_MANIFEST_URL
    // to pin a different feed.
    let info = self_update::check_blocking(VERSION, self_update::UpdateChannel::Beta)?;

    if !info.update_available {
        println!("Already up to date.");
        return Ok(());
    }

    println!(
        "Update available: v{} -> v{}",
        info.current_version, info.latest_version
    );
    if let Some(reason) = info.update_reason {
        let reason = match reason {
            self_update::UpdateReason::VersionMismatch => "version mismatch",
            self_update::UpdateReason::ComponentDrift => "component drift",
        };
        println!("Reason: {}", reason);
    }

    println!("Downloading...");
    let apply_result = info.apply_blocking()?;

    println!("Updated to v{}.", info.latest_version);
    if apply_result.checksum_verified == Some(true) {
        println!("Checksum verified.");
    }
    if !apply_result.installed_targets.is_empty() {
        println!("Installed: {}", apply_result.installed_targets.join(", "));
    }

    // Restart service with new binary
    println!("Restarting service...");
    service_ctl::restart()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parsed_command(arg: &str) -> Commands {
        Cli::try_parse_from(["rhythm-cli", arg]).unwrap().command
    }

    #[test]
    fn cli_parses_service_and_update_subcommands() {
        assert!(matches!(parsed_command("start"), Commands::Start));
        assert!(matches!(parsed_command("stop"), Commands::Stop));
        assert!(matches!(parsed_command("restart"), Commands::Restart));
        assert!(matches!(parsed_command("status"), Commands::Status));
        assert!(matches!(parsed_command("update"), Commands::Update));
        assert!(matches!(parsed_command("uninstall"), Commands::Uninstall));
    }

    #[test]
    fn cli_rejects_unknown_subcommand_and_exposes_version() {
        assert!(Cli::try_parse_from(["rhythm-cli", "bogus"]).is_err());

        let mut command = Cli::command();
        let version = command.get_version().unwrap_or_default().to_string();
        let help = command.render_long_help().to_string();
        assert_eq!(version, VERSION);
        assert!(help.contains("Manage the Rhythm OS server"));
        assert!(help.contains("Self-update to the latest version"));
    }
}
