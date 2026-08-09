//! Standalone smart-bulb laboratory primitives.
//!
//! The crate intentionally contains no direct dependency on Rhythm product
//! crates. Shipping systems are reached through replaceable driver adapters.

pub mod config;
pub mod drivers;
pub mod manifest;
pub mod plan;
pub mod profile;

pub use config::{BenchConfig, MeasurementMode};
pub use manifest::RunManifest;
pub use plan::{build_plan, PlannedStep, RunPlan, StepKind, Suite};
pub use profile::{
    analyze_profile, update_rhythm_devices_database, ComparisonDataset, DatabaseUpdate,
    MatterProfileProposal,
};

use anyhow::{bail, Result};

/// Live execution stays unavailable until guarded hardware drivers exist.
pub fn require_dry_run(dry_run: bool) -> Result<()> {
    if !dry_run {
        bail!("live hardware execution is not available in the scaffold; rerun with --dry-run");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::require_dry_run;

    #[test]
    fn scaffold_fails_closed_for_live_execution() {
        let error = require_dry_run(false).expect_err("live execution must be rejected");
        assert!(error.to_string().contains("not available"));
        require_dry_run(true).expect("dry runs should be allowed");
    }
}
