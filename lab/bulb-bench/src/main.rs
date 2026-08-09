use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use bulb_bench::{build_plan, require_dry_run, BenchConfig, RunManifest, Suite};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "bulb-bench",
    about = "Plan and run reproducible smart-bulb laboratory tests"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a versioned bench configuration without touching hardware.
    ValidateConfig {
        #[arg(long)]
        config: PathBuf,
    },
    /// Print the ordered steps for a test suite.
    Plan {
        #[arg(long, value_enum)]
        suite: Suite,
    },
    /// Build a run manifest. Live execution remains disabled in this phase.
    Run {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, value_enum)]
        suite: Suite,
        #[arg(long)]
        sample_id: String,
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::ValidateConfig { config } => {
            let config = BenchConfig::load(&config)?;
            println!("valid bench configuration: {}", config.bench_id);
        }
        Command::Plan { suite } => {
            let plan = build_plan(suite);
            println!("suite: {}", suite);
            for (index, step) in plan.steps.iter().enumerate() {
                println!("{:>2}. {:?}: {}", index + 1, step.kind, step.description);
            }
        }
        Command::Run {
            config,
            suite,
            sample_id,
            dry_run,
        } => {
            if sample_id.trim().is_empty() {
                bail!("sample-id must not be empty");
            }
            require_dry_run(dry_run)?;
            let config = BenchConfig::load(&config)?;
            let manifest = RunManifest::dry_run(&config, sample_id, build_plan(suite));
            println!(
                "{}",
                serde_json::to_string_pretty(&manifest).context("serializing run manifest")?
            );
        }
    }
    Ok(())
}
