use std::fmt;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Suite {
    MatterProfile,
    Quick,
    OpticalFull,
    MatterFull,
    BleFull,
    PublishedGrade,
    Soak24h,
}

impl fmt::Display for Suite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::MatterProfile => "matter-profile",
            Self::Quick => "quick",
            Self::OpticalFull => "optical-full",
            Self::MatterFull => "matter-full",
            Self::BleFull => "ble-full",
            Self::PublishedGrade => "published-grade",
            Self::Soak24h => "soak-24h",
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Preflight,
    DarkBaseline,
    PowerOn,
    Commission,
    Stabilize,
    CapabilityDiscovery,
    ReferenceBaseline,
    BrightnessSweep,
    ColorTemperatureSweep,
    ProfileAnalysis,
    OpticalQuick,
    OpticalFull,
    TemporalLight,
    Electrical,
    Thermal,
    MatterBehavior,
    BleBehavior,
    Resilience,
    Soak,
    ReferenceCheck,
    SafeShutdown,
    Report,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlannedStep {
    pub kind: StepKind,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RunPlan {
    pub schema_version: u32,
    pub suite: Suite,
    pub steps: Vec<PlannedStep>,
}

fn step(kind: StepKind, description: &'static str) -> PlannedStep {
    PlannedStep {
        kind,
        description: description.to_string(),
    }
}

pub fn build_plan(suite: Suite) -> RunPlan {
    let mut steps = vec![
        step(
            StepKind::Preflight,
            "verify interlocks, emergency stop, thermal cutoff, instruments, storage, and sample limits",
        ),
        step(
            StepKind::DarkBaseline,
            "capture powered-off optical baselines and reject excess chamber leakage",
        ),
        step(
            StepKind::PowerOn,
            "energize the socket with an independently bounded power lease",
        ),
        step(
            StepKind::Commission,
            "establish the selected protocol connection without storing public setup secrets",
        ),
        step(
            StepKind::Stabilize,
            "hold a neutral reference state until optical and thermal drift meet the recipe threshold",
        ),
        step(
            StepKind::CapabilityDiscovery,
            "record claimed identity, firmware, protocols, endpoints, attributes, and controllable modes",
        ),
    ];

    match suite {
        Suite::MatterProfile => {
            steps.push(step(
                StepKind::ReferenceBaseline,
                "load a current same-bench Hue reference baseline and reject stale or configuration-mismatched evidence",
            ));
            steps.push(step(
                StepKind::BrightnessSweep,
                "find the candidate's stable floor and measure its command-to-output curve against the Hue baseline",
            ));
            steps.push(step(
                StepKind::ColorTemperatureSweep,
                "measure representative white targets and derive Hue-relative Kelvin corrections plus uncorrectable Duv error",
            ));
            steps.push(step(
                StepKind::MatterBehavior,
                "check explicit-on, CT-level reset, command spacing, readback, and final-state convergence behaviors",
            ));
            steps.push(step(
                StepKind::ProfileAnalysis,
                "emit deployment guidance, current runtime hints, proposed correction tables, and residual limitations",
            ));
        }
        Suite::Quick => {
            steps.push(step(
                StepKind::OpticalQuick,
                "sample representative full, middle, and low output states",
            ));
            steps.push(step(
                StepKind::MatterBehavior,
                "run a bounded protocol smoke test when Matter is present",
            ));
        }
        Suite::OpticalFull => append_optical_full(&mut steps),
        Suite::MatterFull => {
            append_optical_full(&mut steps);
            steps.push(step(
                StepKind::MatterBehavior,
                "exercise commissioning, commands, attributes, subscriptions, fabrics, groups, scenes, and recovery as supported",
            ));
            steps.push(step(
                StepKind::Resilience,
                "inject bounded network and power interruptions and measure physical recovery",
            ));
        }
        Suite::BleFull => {
            append_optical_full(&mut steps);
            steps.push(step(
                StepKind::BleBehavior,
                "inventory advertising and GATT behavior, then exercise operational control, reconnect, security, and coexistence",
            ));
            steps.push(step(
                StepKind::Resilience,
                "exercise bounded disconnect, power-cycle, and multi-device recovery cases",
            ));
        }
        Suite::PublishedGrade => {
            append_optical_full(&mut steps);
            steps.push(step(
                StepKind::MatterBehavior,
                "run the applicable complete Matter behavior suite",
            ));
            steps.push(step(
                StepKind::BleBehavior,
                "run the applicable complete BLE behavior suite",
            ));
            steps.push(step(
                StepKind::Resilience,
                "run the publication power, network, reconnect, and repeated-command matrix",
            ));
            steps.push(step(
                StepKind::ReferenceCheck,
                "repeat control points and require multi-run, multi-sample evidence before Gold confidence",
            ));
        }
        Suite::Soak24h => {
            steps.push(step(
                StepKind::Soak,
                "run a bounded 24-hour command, connectivity, thermal, and reference-point endurance recipe",
            ));
            steps.push(step(
                StepKind::Resilience,
                "verify final state convergence and recovery after the endurance window",
            ));
        }
    }

    steps.extend([
        step(
            StepKind::ReferenceCheck,
            "repeat a fixed optical reference point to quantify drift during the run",
        ),
        step(
            StepKind::SafeShutdown,
            "remove mains power, verify the de-energized state, and capture the final dark baseline",
        ),
        step(
            StepKind::Report,
            "persist raw evidence, calculations, environment, versions, anomalies, grades, and confidence",
        ),
    ]);

    RunPlan {
        schema_version: 1,
        suite,
        steps,
    }
}

fn append_optical_full(steps: &mut Vec<PlannedStep>) {
    steps.extend([
        step(
            StepKind::OpticalFull,
            "measure the capability-aware brightness, white-temperature, and color target grid with repeated samples",
        ),
        step(
            StepKind::TemporalLight,
            "capture flicker, physical latency, transition duration, smoothness, overshoot, and settle time",
        ),
        step(
            StepKind::Electrical,
            "capture watts, current, apparent power, power factor, standby draw, energy, and supported waveform metrics",
        ),
        step(
            StepKind::Thermal,
            "measure warm-up, steady-state temperature, output droop, spectral drift, and cool-down recovery",
        ),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains(plan: &RunPlan, kind: StepKind) -> bool {
        plan.steps.iter().any(|step| step.kind == kind)
    }

    #[test]
    fn every_suite_starts_safe_and_ends_with_shutdown_and_report() {
        for suite in [
            Suite::MatterProfile,
            Suite::Quick,
            Suite::OpticalFull,
            Suite::MatterFull,
            Suite::BleFull,
            Suite::PublishedGrade,
            Suite::Soak24h,
        ] {
            let plan = build_plan(suite);
            assert_eq!(
                plan.steps.first().map(|step| step.kind),
                Some(StepKind::Preflight)
            );
            assert!(contains(&plan, StepKind::SafeShutdown));
            assert_eq!(
                plan.steps.last().map(|step| step.kind),
                Some(StepKind::Report)
            );
        }
    }

    #[test]
    fn complete_protocol_suites_include_physical_measurement() {
        let matter = build_plan(Suite::MatterFull);
        assert!(contains(&matter, StepKind::OpticalFull));
        assert!(contains(&matter, StepKind::TemporalLight));
        assert!(contains(&matter, StepKind::MatterBehavior));

        let ble = build_plan(Suite::BleFull);
        assert!(contains(&ble, StepKind::OpticalFull));
        assert!(contains(&ble, StepKind::TemporalLight));
        assert!(contains(&ble, StepKind::BleBehavior));
    }

    #[test]
    fn matter_profile_suite_prioritizes_actionable_corrections() {
        let plan = build_plan(Suite::MatterProfile);
        assert!(contains(&plan, StepKind::ReferenceBaseline));
        assert!(contains(&plan, StepKind::BrightnessSweep));
        assert!(contains(&plan, StepKind::ColorTemperatureSweep));
        assert!(contains(&plan, StepKind::ProfileAnalysis));
    }
}
