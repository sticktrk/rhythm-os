//! Hardware and protocol boundaries.
//!
//! Driver implementations belong in this laboratory project. The traits keep
//! instrument-specific SDKs and experimental dependencies away from shipping
//! Rhythm binaries.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub struct LightTarget {
    pub power: bool,
    pub brightness_percent: Option<f64>,
    pub kelvin: Option<u16>,
    pub xy: Option<(f64, f64)>,
    pub transition_milliseconds: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PowerCommand {
    pub enabled: bool,
    /// Every energized command carries an independent maximum duration. A
    /// driver must remove power when the lease expires even if the runner dies.
    pub lease_milliseconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SpectrumSample {
    pub wavelengths_nm: Vec<f64>,
    pub values: Vec<f64>,
    pub saturated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub struct EnvironmentSnapshot {
    pub chamber_celsius: f64,
    pub socket_celsius: f64,
}

pub trait ProtocolController {
    fn discover_capabilities(&mut self) -> Result<serde_json::Value>;
    fn apply_target(&mut self, target: LightTarget) -> Result<serde_json::Value>;
    fn read_state(&mut self) -> Result<serde_json::Value>;
}

pub trait MainsController {
    fn apply_power(&mut self, command: PowerCommand) -> Result<()>;
    fn emergency_off(&mut self) -> Result<()>;
}

pub trait Spectrometer {
    fn dark_sample(&mut self) -> Result<SpectrumSample>;
    fn capture(&mut self) -> Result<SpectrumSample>;
}

pub trait FastLightMeter {
    fn capture_waveform(&mut self, duration_milliseconds: u32) -> Result<Vec<f64>>;
}

pub trait EnvironmentMonitor {
    fn snapshot(&mut self) -> Result<EnvironmentSnapshot>;
}
