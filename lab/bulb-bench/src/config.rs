use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementMode {
    /// Repeatable rankings from a characterized chamber without an absolute
    /// total-flux claim.
    Comparative,
    /// Calibrated total-flux measurements with a traceable reference and
    /// self-absorption correction.
    Absolute,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SafetyLimits {
    pub max_bulb_watts: f64,
    pub max_chamber_celsius: f64,
    pub max_socket_celsius: f64,
    pub max_run_minutes: u32,
    pub door_interlock_required: bool,
    pub emergency_stop_required: bool,
    pub independent_thermal_cutoff_required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstrumentConfig {
    pub driver: String,
    pub endpoint: Option<String>,
    pub configured: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstrumentSet {
    pub protocol_controller: InstrumentConfig,
    pub spectrometer: InstrumentConfig,
    pub mains_controller: InstrumentConfig,
    pub power_analyzer: InstrumentConfig,
    pub fast_light_meter: InstrumentConfig,
    pub environment_monitor: InstrumentConfig,
}

impl InstrumentSet {
    pub fn readiness(&self) -> BTreeMap<String, bool> {
        BTreeMap::from([
            (
                "environment_monitor".to_string(),
                self.environment_monitor.configured,
            ),
            (
                "fast_light_meter".to_string(),
                self.fast_light_meter.configured,
            ),
            (
                "mains_controller".to_string(),
                self.mains_controller.configured,
            ),
            ("power_analyzer".to_string(), self.power_analyzer.configured),
            (
                "protocol_controller".to_string(),
                self.protocol_controller.configured,
            ),
            ("spectrometer".to_string(), self.spectrometer.configured),
        ])
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BenchConfig {
    pub schema_version: u32,
    pub bench_id: String,
    pub measurement_mode: MeasurementMode,
    pub safety: SafetyLimits,
    pub instruments: InstrumentSet,
}

impl BenchConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path)
            .with_context(|| format!("reading bench configuration {}", path.display()))?;
        let config = serde_json::from_slice::<Self>(&bytes)
            .with_context(|| format!("parsing bench configuration {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            bail!(
                "unsupported bench config schema {}; expected {}",
                self.schema_version,
                CONFIG_SCHEMA_VERSION
            );
        }
        if self.bench_id.trim().is_empty() {
            bail!("bench_id must not be empty");
        }
        if !(0.1..=200.0).contains(&self.safety.max_bulb_watts) {
            bail!("max_bulb_watts must be between 0.1 and 200");
        }
        if !(20.0..=80.0).contains(&self.safety.max_chamber_celsius) {
            bail!("max_chamber_celsius must be between 20 and 80");
        }
        if self.safety.max_socket_celsius <= self.safety.max_chamber_celsius {
            bail!("max_socket_celsius must exceed max_chamber_celsius");
        }
        if self.safety.max_run_minutes == 0 {
            bail!("max_run_minutes must be greater than zero");
        }
        if !self.safety.door_interlock_required
            || !self.safety.emergency_stop_required
            || !self.safety.independent_thermal_cutoff_required
        {
            bail!("all independent mains safety controls must remain required");
        }
        for (name, instrument) in [
            ("protocol_controller", &self.instruments.protocol_controller),
            ("spectrometer", &self.instruments.spectrometer),
            ("mains_controller", &self.instruments.mains_controller),
            ("power_analyzer", &self.instruments.power_analyzer),
            ("fast_light_meter", &self.instruments.fast_light_meter),
            ("environment_monitor", &self.instruments.environment_monitor),
        ] {
            if instrument.driver.trim().is_empty() {
                bail!("instrument {name} must name a driver");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> BenchConfig {
        let instrument = InstrumentConfig {
            driver: "unconfigured".to_string(),
            endpoint: None,
            configured: false,
        };
        BenchConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            bench_id: "test-bench".to_string(),
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
        }
    }

    #[test]
    fn accepts_safe_comparative_scaffold() {
        config().validate().expect("valid scaffold config");
    }

    #[test]
    fn safety_controls_cannot_be_disabled_by_configuration() {
        let mut config = config();
        config.safety.door_interlock_required = false;
        let error = config.validate().expect_err("unsafe config must fail");
        assert!(error.to_string().contains("safety controls"));
    }
}
