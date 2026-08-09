use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::BenchConfig;
use crate::plan::RunPlan;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RunManifest {
    pub schema_version: u32,
    pub status: String,
    pub dry_run: bool,
    pub bench_id: String,
    pub sample_id: String,
    pub measurement_mode: crate::MeasurementMode,
    pub instrument_readiness: BTreeMap<String, bool>,
    pub plan: RunPlan,
}

impl RunManifest {
    pub fn dry_run(config: &BenchConfig, sample_id: impl Into<String>, plan: RunPlan) -> Self {
        Self {
            schema_version: 1,
            status: "planned".to_string(),
            dry_run: true,
            bench_id: config.bench_id.clone(),
            sample_id: sample_id.into(),
            measurement_mode: config.measurement_mode,
            instrument_readiness: config.instruments.readiness(),
            plan,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InstrumentConfig, InstrumentSet, SafetyLimits, CONFIG_SCHEMA_VERSION};
    use crate::{build_plan, MeasurementMode, Suite};

    #[test]
    fn dry_run_manifest_exposes_unconfigured_instruments() {
        let instrument = InstrumentConfig {
            driver: "not-selected".to_string(),
            endpoint: None,
            configured: false,
        };
        let config = BenchConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            bench_id: "bench".to_string(),
            measurement_mode: MeasurementMode::Comparative,
            safety: SafetyLimits {
                max_bulb_watts: 25.0,
                max_chamber_celsius: 45.0,
                max_socket_celsius: 90.0,
                max_run_minutes: 60,
                door_interlock_required: true,
                emergency_stop_required: true,
                independent_thermal_cutoff_required: true,
            },
            instruments: InstrumentSet {
                protocol_controller: instrument.clone(),
                spectrometer: instrument.clone(),
                mains_controller: instrument.clone(),
                power_analyzer: instrument.clone(),
                fast_light_meter: instrument.clone(),
                environment_monitor: instrument,
            },
        };
        let manifest = RunManifest::dry_run(&config, "sample", build_plan(Suite::Quick));
        assert!(manifest.dry_run);
        assert!(manifest.instrument_readiness.values().all(|ready| !ready));
    }
}
