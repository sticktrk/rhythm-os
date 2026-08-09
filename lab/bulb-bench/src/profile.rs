use std::cmp::Ordering;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use rhythm_devices::{
    BrightnessCorrectionPoint as RuntimeBrightnessCorrectionPoint,
    ColorTemperatureCorrectionPoint as RuntimeColorTemperatureCorrectionPoint, ControlCorrections,
    DeviceEntry, DeviceQuirk,
};
use serde::{Deserialize, Serialize};

const MIN_VISIBLE_OUTPUT_PERCENT: f64 = 0.5;
const BRIGHTNESS_MONOTONIC_TOLERANCE: f64 = 0.5;
const CT_MONOTONIC_TOLERANCE_KELVIN: f64 = 25.0;
const DUV_MISMATCH_THRESHOLD: f64 = 0.003;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ComparisonDataset {
    pub schema_version: u32,
    pub metadata: DatasetMetadata,
    pub reference: BulbMeasurements,
    pub candidate: BulbMeasurements,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DatasetMetadata {
    pub label: String,
    #[serde(default)]
    pub synthetic: bool,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BulbMeasurements {
    pub bulb_id: String,
    pub protocol: String,
    pub device: DeviceEntry,
    pub brightness: Vec<BrightnessObservation>,
    pub color_temperature: Vec<ColorTemperatureObservation>,
    #[serde(default)]
    pub behavior: BehaviorObservations,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct BrightnessObservation {
    pub command_percent: f64,
    pub measured_lux: f64,
    pub stable: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ColorTemperatureObservation {
    pub command_kelvin: u16,
    pub measured_kelvin: f64,
    pub duv: f64,
    pub stable: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct BehaviorObservations {
    #[serde(default)]
    pub needs_explicit_on: bool,
    #[serde(default)]
    pub color_temperature_command_resets_level: bool,
    #[serde(default)]
    pub recommended_command_spacing_ms: Option<u32>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MatterProfileProposal {
    pub schema_version: u32,
    pub source_label: String,
    pub reference_bulb_id: String,
    pub candidate_bulb_id: String,
    pub recommendation: DeploymentRecommendation,
    pub capability_hints: CapabilityHints,
    pub behavior_hints: BehaviorObservations,
    pub recommended_control_strategy: RecommendedControlStrategy,
    pub runtime_application: RuntimeApplication,
    pub rhythm_devices_entry: DeviceEntry,
    pub evidence: EvidenceSummary,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateClassification {
    UsableWithProfile,
    LimitedUse,
    NotRecommended,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeploymentRecommendation {
    pub classification: CandidateClassification,
    pub summary: String,
    pub suitable_for: Vec<String>,
    pub avoid_for: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CapabilityHints {
    pub min_brightness: u8,
    pub usable_min_kelvin: u16,
    pub usable_max_kelvin: u16,
    pub measured_output_min_kelvin: f64,
    pub measured_output_max_kelvin: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecommendedControlStrategy {
    pub reference_basis: String,
    pub brightness_correction_lut: Vec<BrightnessCorrectionPoint>,
    pub color_temperature_correction_lut: Vec<ColorTemperatureCorrectionPoint>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BrightnessCorrectionPoint {
    pub logical_percent: f64,
    pub reference_output_percent: f64,
    pub uncorrected_candidate_output_percent: f64,
    pub candidate_command_percent: f64,
    pub expected_candidate_output_percent: f64,
    pub clamped: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ColorTemperatureCorrectionPoint {
    pub logical_kelvin: u16,
    pub reference_measured_kelvin: f64,
    pub uncorrected_candidate_measured_kelvin: f64,
    pub candidate_command_kelvin: u16,
    pub expected_candidate_measured_kelvin: f64,
    pub error_before_kelvin: f64,
    pub expected_error_after_kelvin: f64,
    pub reference_duv: f64,
    pub expected_candidate_duv: f64,
    pub duv_delta: f64,
    pub clamped: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeApplication {
    pub can_apply_now: Vec<String>,
    pub requires_runtime_support: Vec<String>,
    pub already_accommodated: Vec<String>,
    pub not_correctable_with_ct_only: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvidenceSummary {
    pub synthetic: bool,
    pub reference_brightness_points: usize,
    pub candidate_brightness_points: usize,
    pub reference_color_temperature_points: usize,
    pub candidate_color_temperature_points: usize,
    pub corrected_color_temperature_targets_in_range: usize,
    pub corrected_color_temperature_targets_total: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseUpdateAction {
    Inserted,
    Replaced,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DatabaseUpdate {
    pub action: DatabaseUpdateAction,
    pub entry_index: usize,
    pub total_entries: usize,
    pub output_path: String,
}

#[derive(Clone, Copy, Debug)]
struct CurvePoint {
    command: f64,
    output: f64,
}

#[derive(Debug)]
struct BrightnessCurve {
    points: Vec<CurvePoint>,
    minimum_stable_command: f64,
    non_monotonic: bool,
}

#[derive(Clone, Copy, Debug)]
struct CtPoint {
    command: f64,
    measured: f64,
    duv: f64,
}

#[derive(Debug)]
struct CtCurve {
    points: Vec<CtPoint>,
    inverse_points: Vec<CurvePoint>,
    non_monotonic: bool,
}

#[derive(Clone, Copy, Debug)]
struct Interpolated {
    value: f64,
    clamped: bool,
}

impl ComparisonDataset {
    pub fn load(path: &Path) -> Result<Self> {
        let body = fs::read_to_string(path)
            .with_context(|| format!("reading comparison dataset {}", path.display()))?;
        serde_json::from_str(&body)
            .with_context(|| format!("parsing comparison dataset {}", path.display()))
    }
}

pub fn analyze_profile(dataset: &ComparisonDataset) -> Result<MatterProfileProposal> {
    validate_dataset(dataset)?;

    let reference_brightness = prepare_brightness("reference", &dataset.reference.brightness)?;
    let candidate_brightness = prepare_brightness("candidate", &dataset.candidate.brightness)?;
    let reference_ct = prepare_ct("reference", &dataset.reference.color_temperature)?;
    let candidate_ct = prepare_ct("candidate", &dataset.candidate.color_temperature)?;
    let mut limitations = vec![
        "Optical profiling does not establish Matter reliability, commissioning quality, BLE quality, safety, lifetime, or absolute lumen output."
            .to_string(),
    ];

    if dataset.metadata.synthetic {
        add_unique(
            &mut limitations,
            "This is a synthetic development dataset and must not be published as real bulb evidence.",
        );
    }
    if reference_brightness.non_monotonic {
        add_unique(
            &mut limitations,
            "Reference brightness output was non-monotonic; analysis uses a monotonic envelope.",
        );
    }
    if candidate_brightness.non_monotonic {
        add_unique(
            &mut limitations,
            "Candidate brightness output was non-monotonic; correction uses a monotonic envelope.",
        );
    }
    if reference_ct.non_monotonic {
        add_unique(
            &mut limitations,
            "Reference measured CCT was non-monotonic; analysis uses a monotonic envelope.",
        );
    }
    if candidate_ct.non_monotonic {
        add_unique(
            &mut limitations,
            "Candidate measured CCT was non-monotonic; correction uses a monotonic envelope.",
        );
    }

    let brightness_correction_lut = brightness_corrections(
        &reference_brightness,
        &candidate_brightness,
        &mut limitations,
    );
    let color_temperature_correction_lut =
        ct_corrections(&reference_ct, &candidate_ct, &mut limitations);

    let minimum_brightness = candidate_brightness.minimum_stable_command.ceil() as u8;
    let candidate_ct_first = candidate_ct
        .points
        .first()
        .context("candidate CT curve unexpectedly empty")?;
    let candidate_ct_last = candidate_ct
        .points
        .last()
        .context("candidate CT curve unexpectedly empty")?;
    let in_range_ct_targets = color_temperature_correction_lut
        .iter()
        .filter(|point| !point.clamped)
        .count();
    let total_ct_targets = color_temperature_correction_lut.len();
    let reachable_min_kelvin = color_temperature_correction_lut
        .iter()
        .find(|point| !point.clamped)
        .map(|point| point.logical_kelvin)
        .context("candidate cannot reach any reference CT target")?;
    let reachable_max_kelvin = color_temperature_correction_lut
        .iter()
        .rev()
        .find(|point| !point.clamped)
        .map(|point| point.logical_kelvin)
        .context("candidate cannot reach any reference CT target")?;
    let has_duv_mismatch = color_temperature_correction_lut
        .iter()
        .any(|point| point.duv_delta.abs() > DUV_MISMATCH_THRESHOLD);

    if minimum_brightness > 1 {
        add_unique(
            &mut limitations,
            &format!(
                "Candidate has no verified stable output below a {}% command.",
                minimum_brightness
            ),
        );
    }
    if color_temperature_correction_lut
        .iter()
        .any(|point| point.clamped)
    {
        add_unique(
            &mut limitations,
            "One or more Hue-relative CCT targets fall outside the candidate's measured output envelope and are clamped.",
        );
    }
    if has_duv_mismatch {
        add_unique(
            &mut limitations,
            "The candidate has a visible tint-axis (Duv) mismatch at one or more white points; changing Kelvin alone cannot remove it.",
        );
    }

    let recommendation = deployment_recommendation(
        &brightness_correction_lut,
        &color_temperature_correction_lut,
        has_duv_mismatch,
    );
    let capability_hints = CapabilityHints {
        min_brightness: minimum_brightness,
        usable_min_kelvin: reachable_min_kelvin,
        usable_max_kelvin: reachable_max_kelvin,
        measured_output_min_kelvin: round(candidate_ct_first.measured, 1),
        measured_output_max_kelvin: round(candidate_ct_last.measured, 1),
    };
    let runtime_application = runtime_application(&dataset.candidate.behavior, has_duv_mismatch);
    let rhythm_devices_entry = build_rhythm_devices_entry(
        &dataset.candidate,
        &capability_hints,
        &brightness_correction_lut,
        &color_temperature_correction_lut,
    )?;

    Ok(MatterProfileProposal {
        schema_version: 1,
        source_label: dataset.metadata.label.clone(),
        reference_bulb_id: dataset.reference.bulb_id.clone(),
        candidate_bulb_id: dataset.candidate.bulb_id.clone(),
        recommendation,
        capability_hints,
        behavior_hints: dataset.candidate.behavior.clone(),
        recommended_control_strategy: RecommendedControlStrategy {
            reference_basis: "Hue-relative normalized optical output; each bulb is normalized to its own measured maximum"
                .to_string(),
            brightness_correction_lut,
            color_temperature_correction_lut,
        },
        runtime_application,
        rhythm_devices_entry,
        evidence: EvidenceSummary {
            synthetic: dataset.metadata.synthetic,
            reference_brightness_points: reference_brightness.points.len(),
            candidate_brightness_points: candidate_brightness.points.len(),
            reference_color_temperature_points: reference_ct.points.len(),
            candidate_color_temperature_points: candidate_ct.points.len(),
            corrected_color_temperature_targets_in_range: in_range_ct_targets,
            corrected_color_temperature_targets_total: total_ct_targets,
        },
        limitations,
    })
}

fn validate_dataset(dataset: &ComparisonDataset) -> Result<()> {
    if dataset.schema_version != 1 {
        bail!(
            "unsupported comparison dataset schema version {}; expected 1",
            dataset.schema_version
        );
    }
    if dataset.metadata.label.trim().is_empty() {
        bail!("dataset metadata label must not be empty");
    }
    for (role, bulb) in [
        ("reference", &dataset.reference),
        ("candidate", &dataset.candidate),
    ] {
        if bulb.bulb_id.trim().is_empty() {
            bail!("{role} bulb_id must not be empty");
        }
        if bulb.protocol.trim().is_empty() {
            bail!("{role} protocol must not be empty");
        }
        if bulb.device.manufacturer.trim().is_empty()
            || bulb.device.model.trim().is_empty()
            || bulb.device.name.trim().is_empty()
        {
            bail!("{role} device manufacturer, model, and name must not be empty");
        }
        for point in &bulb.brightness {
            if !point.command_percent.is_finite() || !(0.0..=100.0).contains(&point.command_percent)
            {
                bail!("{role} brightness command must be between 0 and 100");
            }
            if !point.measured_lux.is_finite() || point.measured_lux < 0.0 {
                bail!("{role} measured lux must be finite and non-negative");
            }
        }
        for point in &bulb.color_temperature {
            if point.command_kelvin < 1000 || point.command_kelvin > 20_000 {
                bail!("{role} CT command must be between 1000 K and 20000 K");
            }
            if !point.measured_kelvin.is_finite()
                || !(1000.0..=20_000.0).contains(&point.measured_kelvin)
            {
                bail!("{role} measured CCT must be between 1000 K and 20000 K");
            }
            if !point.duv.is_finite() || !(-0.1..=0.1).contains(&point.duv) {
                bail!("{role} Duv must be finite and between -0.1 and 0.1");
            }
        }
    }
    if dataset.candidate.device.matter.is_none() {
        bail!("candidate device must include Matter metadata for rhythm-devices export");
    }
    Ok(())
}

fn prepare_brightness(
    label: &str,
    observations: &[BrightnessObservation],
) -> Result<BrightnessCurve> {
    let stable: Vec<_> = observations
        .iter()
        .copied()
        .filter(|point| point.stable && point.command_percent > 0.0)
        .collect();
    if stable.len() < 3 {
        bail!("{label} requires at least three stable non-zero brightness observations");
    }
    let max_lux = stable
        .iter()
        .map(|point| point.measured_lux)
        .fold(0.0_f64, f64::max);
    if max_lux <= 0.0 {
        bail!("{label} stable brightness observations have no measurable output");
    }

    let mut points: Vec<_> = stable
        .iter()
        .map(|point| CurvePoint {
            command: point.command_percent,
            output: point.measured_lux / max_lux * 100.0,
        })
        .collect();
    points.sort_by(|left, right| total_cmp(left.command, right.command));
    reject_duplicate_commands(
        label,
        "brightness",
        points.iter().map(|point| point.command),
    )?;

    let minimum_stable_command = points
        .iter()
        .find(|point| point.output >= MIN_VISIBLE_OUTPUT_PERCENT)
        .map(|point| point.command)
        .with_context(|| format!("{label} has no stable visible brightness point"))?;
    let non_monotonic = monotonic_envelope(&mut points, BRIGHTNESS_MONOTONIC_TOLERANCE);

    Ok(BrightnessCurve {
        points,
        minimum_stable_command,
        non_monotonic,
    })
}

fn prepare_ct(label: &str, observations: &[ColorTemperatureObservation]) -> Result<CtCurve> {
    let mut points: Vec<_> = observations
        .iter()
        .copied()
        .filter(|point| point.stable)
        .map(|point| CtPoint {
            command: f64::from(point.command_kelvin),
            measured: point.measured_kelvin,
            duv: point.duv,
        })
        .collect();
    if points.len() < 3 {
        bail!("{label} requires at least three stable color-temperature observations");
    }
    points.sort_by(|left, right| total_cmp(left.command, right.command));
    reject_duplicate_commands(
        label,
        "color-temperature",
        points.iter().map(|point| point.command),
    )?;

    let mut inverse_points: Vec<_> = points
        .iter()
        .map(|point| CurvePoint {
            command: point.command,
            output: point.measured,
        })
        .collect();
    let non_monotonic = monotonic_envelope(&mut inverse_points, CT_MONOTONIC_TOLERANCE_KELVIN);

    Ok(CtCurve {
        points,
        inverse_points,
        non_monotonic,
    })
}

fn reject_duplicate_commands(
    label: &str,
    measurement: &str,
    commands: impl Iterator<Item = f64>,
) -> Result<()> {
    let mut previous = None;
    for command in commands {
        if previous.is_some_and(|value: f64| (value - command).abs() < f64::EPSILON) {
            bail!("{label} has duplicate stable {measurement} command {command}");
        }
        previous = Some(command);
    }
    Ok(())
}

fn monotonic_envelope(points: &mut [CurvePoint], tolerance: f64) -> bool {
    let mut maximum = f64::NEG_INFINITY;
    let mut non_monotonic = false;
    for point in points {
        if point.output + tolerance < maximum {
            non_monotonic = true;
        }
        maximum = maximum.max(point.output);
        point.output = maximum;
    }
    non_monotonic
}

fn brightness_corrections(
    reference: &BrightnessCurve,
    candidate: &BrightnessCurve,
    limitations: &mut Vec<String>,
) -> Vec<BrightnessCorrectionPoint> {
    let candidate_inverse: Vec<_> = candidate
        .points
        .iter()
        .map(|point| CurvePoint {
            command: point.output,
            output: point.command,
        })
        .collect();

    reference
        .points
        .iter()
        .map(|reference_point| {
            let correction = interpolate(&candidate_inverse, reference_point.output);
            let uncorrected = interpolate(&candidate.points, reference_point.command);
            let expected = interpolate(&candidate.points, correction.value);
            if correction.clamped {
                add_unique(
                    limitations,
                    "One or more Hue-relative brightness targets are outside the candidate's stable measured output range and are clamped.",
                );
            }
            BrightnessCorrectionPoint {
                logical_percent: round(reference_point.command, 2),
                reference_output_percent: round(reference_point.output, 2),
                uncorrected_candidate_output_percent: round(uncorrected.value, 2),
                candidate_command_percent: round(correction.value, 2),
                expected_candidate_output_percent: round(expected.value, 2),
                clamped: correction.clamped,
            }
        })
        .collect()
}

fn ct_corrections(
    reference: &CtCurve,
    candidate: &CtCurve,
    limitations: &mut Vec<String>,
) -> Vec<ColorTemperatureCorrectionPoint> {
    let candidate_inverse: Vec<_> = candidate
        .inverse_points
        .iter()
        .map(|point| CurvePoint {
            command: point.output,
            output: point.command,
        })
        .collect();
    let candidate_measured: Vec<_> = candidate
        .points
        .iter()
        .map(|point| CurvePoint {
            command: point.command,
            output: point.measured,
        })
        .collect();
    let candidate_duv: Vec<_> = candidate
        .points
        .iter()
        .map(|point| CurvePoint {
            command: point.command,
            output: point.duv,
        })
        .collect();

    reference
        .points
        .iter()
        .map(|reference_point| {
            let correction = interpolate(&candidate_inverse, reference_point.measured);
            let correction_command = correction.value.round();
            let expected = interpolate(&candidate_measured, correction_command);
            let uncorrected = interpolate(&candidate_measured, reference_point.command);
            let expected_duv = interpolate(&candidate_duv, correction_command).value;
            if correction.clamped {
                add_unique(
                    limitations,
                    "At least one reference white target cannot be reached inside the candidate's tested CT command range.",
                );
            }
            ColorTemperatureCorrectionPoint {
                logical_kelvin: reference_point.command.round() as u16,
                reference_measured_kelvin: round(reference_point.measured, 1),
                uncorrected_candidate_measured_kelvin: round(uncorrected.value, 1),
                candidate_command_kelvin: correction_command as u16,
                expected_candidate_measured_kelvin: round(expected.value, 1),
                error_before_kelvin: round(uncorrected.value - reference_point.measured, 1),
                expected_error_after_kelvin: round(expected.value - reference_point.measured, 1),
                reference_duv: round(reference_point.duv, 5),
                expected_candidate_duv: round(expected_duv, 5),
                duv_delta: round(expected_duv - reference_point.duv, 5),
                clamped: correction.clamped,
            }
        })
        .collect()
}

fn interpolate(points: &[CurvePoint], target: f64) -> Interpolated {
    let first = points.first().expect("validated curve must not be empty");
    let last = points.last().expect("validated curve must not be empty");
    if target <= first.command {
        return Interpolated {
            value: first.output,
            clamped: target < first.command,
        };
    }
    if target >= last.command {
        return Interpolated {
            value: last.output,
            clamped: target > last.command,
        };
    }

    for pair in points.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if target <= right.command {
            let width = right.command - left.command;
            if width.abs() < f64::EPSILON {
                return Interpolated {
                    value: right.output,
                    clamped: false,
                };
            }
            let fraction = (target - left.command) / width;
            return Interpolated {
                value: left.output + fraction * (right.output - left.output),
                clamped: false,
            };
        }
    }
    unreachable!("target inside curve bounds must have an interpolation pair")
}

fn deployment_recommendation(
    brightness_lut: &[BrightnessCorrectionPoint],
    color_temperature_lut: &[ColorTemperatureCorrectionPoint],
    has_duv_mismatch: bool,
) -> DeploymentRecommendation {
    let brightness_targets_in_range = brightness_lut.iter().filter(|point| !point.clamped).count();
    let ct_targets_in_range = color_temperature_lut
        .iter()
        .filter(|point| !point.clamped)
        .count();
    let brightness_coverage = brightness_targets_in_range as f64 / brightness_lut.len() as f64;
    let ct_coverage = ct_targets_in_range as f64 / color_temperature_lut.len() as f64;
    let classification = if brightness_coverage >= 0.9 && ct_coverage >= 0.8 {
        CandidateClassification::UsableWithProfile
    } else if brightness_coverage >= 0.7 && ct_coverage >= 0.5 {
        CandidateClassification::LimitedUse
    } else {
        CandidateClassification::NotRecommended
    };
    let mut suitable_for = vec!["On/off and non-color-critical general illumination".to_string()];
    let mut avoid_for = Vec::new();

    if brightness_targets_in_range == brightness_lut.len() {
        suitable_for.push(
            "Hue-relative dimming across the tested logical range using the brightness correction table"
                .to_string(),
        );
    } else {
        let unreachable = brightness_lut
            .iter()
            .filter(|point| point.clamped)
            .map(|point| format!("{}%", point.logical_percent))
            .collect::<Vec<_>>()
            .join(", ");
        avoid_for.push(format!(
            "Dimming scenes requiring unreachable Hue-relative logical targets: {unreachable}"
        ));
    }
    if ct_coverage >= 0.8 {
        suitable_for
            .push("Tunable-white scenes using the Hue-relative CT correction table".to_string());
    } else {
        avoid_for.push(
            "Broad tunable-white scenes outside the measured reachable CT envelope".to_string(),
        );
    }
    if ct_targets_in_range < color_temperature_lut.len() {
        let unreachable = color_temperature_lut
            .iter()
            .filter(|point| point.clamped)
            .map(|point| format!("{} K", point.logical_kelvin))
            .collect::<Vec<_>>()
            .join(", ");
        avoid_for.push(format!(
            "White scenes requiring unreachable Hue-relative targets: {unreachable}"
        ));
    }
    if has_duv_mismatch {
        avoid_for.push(
            "Color-critical white matching where green/magenta tint differences matter".to_string(),
        );
    }

    let summary = match classification {
        CandidateClassification::UsableWithProfile => {
            "Usable in bounded roles after applying its measured floor and correction profile."
        }
        CandidateClassification::LimitedUse => {
            "Useful for limited roles, but its measured floor or CT envelope excludes important scenes."
        }
        CandidateClassification::NotRecommended => {
            "The measured optical limits are too large for general Rhythm substitution."
        }
    }
    .to_string();

    DeploymentRecommendation {
        classification,
        summary,
        suitable_for,
        avoid_for,
    }
}

fn runtime_application(
    behavior: &BehaviorObservations,
    has_duv_mismatch: bool,
) -> RuntimeApplication {
    let mut can_apply_now = vec![
        "capabilities.min_brightness".to_string(),
        "capabilities.usable_min_kelvin".to_string(),
        "capabilities.usable_max_kelvin".to_string(),
        "control_corrections.brightness".to_string(),
        "control_corrections.color_temperature".to_string(),
    ];
    if behavior.needs_explicit_on {
        can_apply_now.push("quirks.needs_explicit_on".to_string());
    }
    if behavior.recommended_command_spacing_ms.is_some() {
        can_apply_now.push("quirks.recommended_command_spacing_ms".to_string());
    }

    let mut already_accommodated = Vec::new();
    if behavior.color_temperature_command_resets_level {
        already_accommodated.push(
            "Rhythm Matter sends brightness after color temperature so a CT command cannot remain the final level-setting operation"
                .to_string(),
        );
    }
    let not_correctable_with_ct_only = if has_duv_mismatch {
        vec!["Duv / green-magenta tint mismatch".to_string()]
    } else {
        Vec::new()
    };

    RuntimeApplication {
        can_apply_now,
        requires_runtime_support: Vec::new(),
        already_accommodated,
        not_correctable_with_ct_only,
    }
}

fn build_rhythm_devices_entry(
    candidate: &BulbMeasurements,
    capability_hints: &CapabilityHints,
    brightness_lut: &[BrightnessCorrectionPoint],
    color_temperature_lut: &[ColorTemperatureCorrectionPoint],
) -> Result<DeviceEntry> {
    let mut entry = candidate.device.clone();
    entry.min_brightness = Some(capability_hints.min_brightness);
    entry.min_kelvin = Some(capability_hints.usable_min_kelvin);
    entry.max_kelvin = Some(capability_hints.usable_max_kelvin);
    entry.control_corrections = ControlCorrections {
        brightness: brightness_lut
            .iter()
            .map(|point| RuntimeBrightnessCorrectionPoint {
                logical_percent: point.logical_percent.round().clamp(1.0, 100.0) as u8,
                command_percent: point.candidate_command_percent.round().clamp(1.0, 100.0) as u8,
            })
            .collect(),
        color_temperature: color_temperature_lut
            .iter()
            .filter(|point| !point.clamped)
            .map(|point| RuntimeColorTemperatureCorrectionPoint {
                logical_kelvin: point.logical_kelvin,
                command_kelvin: point.candidate_command_kelvin,
            })
            .collect(),
    };
    if !entry.control_corrections.is_valid() {
        bail!("derived rhythm-devices control corrections are invalid");
    }

    let matter = entry
        .matter
        .as_mut()
        .context("candidate Matter metadata disappeared during export")?;
    if candidate.behavior.needs_explicit_on
        && !matter.quirks.contains(&DeviceQuirk::NeedsExplicitOn)
    {
        matter.quirks.push(DeviceQuirk::NeedsExplicitOn);
    }
    if let Some(spacing_ms) = candidate
        .behavior
        .recommended_command_spacing_ms
        .filter(|spacing_ms| *spacing_ms > 0)
    {
        let quirk = DeviceQuirk::CommandThrottleMs(spacing_ms);
        if !matter.quirks.contains(&quirk) {
            matter.quirks.push(quirk);
        }
    }
    Ok(entry)
}

pub fn update_rhythm_devices_database(
    dataset: &ComparisonDataset,
    proposal: &MatterProfileProposal,
    database_path: &Path,
    output_path: &Path,
) -> Result<DatabaseUpdate> {
    if dataset.metadata.synthetic {
        bail!("refusing to update rhythm-devices from a synthetic dataset");
    }
    let body = fs::read_to_string(database_path).with_context(|| {
        format!(
            "reading rhythm-devices database {}",
            database_path.display()
        )
    })?;
    let entries: Vec<DeviceEntry> = serde_json::from_str(&body).with_context(|| {
        format!(
            "parsing rhythm-devices database {}",
            database_path.display()
        )
    })?;
    let (object_ranges, closing_bracket) = top_level_object_ranges(&body)?;
    if object_ranges.len() != entries.len() {
        bail!(
            "rhythm-devices parser found {} entries but {} source objects",
            entries.len(),
            object_ranges.len()
        );
    }
    let candidate = &proposal.rhythm_devices_entry;
    let matching_indices: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, existing)| same_device(existing, candidate).then_some(index))
        .collect();
    if matching_indices.len() > 1 {
        bail!(
            "rhythm-devices has multiple entries matching {}/{}; refusing ambiguous replacement",
            candidate.manufacturer,
            candidate.model
        );
    }

    let entry_json =
        serde_json::to_string_pretty(candidate).context("serializing rhythm-devices entry")?;
    let (action, entry_index, updated_body) = if let Some(index) = matching_indices.first().copied()
    {
        let (start, end) = object_ranges[index];
        let replacement = indent_after_first_line(&entry_json, "  ");
        let updated = format!("{}{}{}", &body[..start], replacement, &body[end..]);
        (DatabaseUpdateAction::Replaced, index, updated)
    } else {
        let index = entries.len();
        let insertion = indent_every_line(&entry_json, "  ");
        let separator = if entries.is_empty() { "\n" } else { ",\n" };
        let insertion_point = body[..closing_bracket].trim_end().len();
        let updated = format!(
            "{}{}{}{}",
            &body[..insertion_point],
            separator,
            insertion,
            &body[insertion_point..]
        );
        (DatabaseUpdateAction::Inserted, index, updated)
    };
    let updated_entries: Vec<DeviceEntry> = serde_json::from_str(&updated_body)
        .context("validating updated rhythm-devices database")?;
    if updated_entries
        .iter()
        .any(|entry| !entry.control_corrections.is_valid())
    {
        bail!("updated rhythm-devices database contains invalid control corrections");
    }
    write_atomically(output_path, updated_body.as_bytes())?;

    Ok(DatabaseUpdate {
        action,
        entry_index,
        total_entries: updated_entries.len(),
        output_path: output_path.display().to_string(),
    })
}

fn top_level_object_ranges(body: &str) -> Result<(Vec<(usize, usize)>, usize)> {
    let mut ranges = Vec::new();
    let mut array_depth = 0_u32;
    let mut object_depth = 0_u32;
    let mut object_start = None;
    let mut closing_bracket = None;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in body.bytes().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        if byte == b'"' {
            in_string = true;
            continue;
        }

        match byte {
            b'[' => array_depth += 1,
            b']' => {
                if array_depth == 1 && object_depth == 0 {
                    closing_bracket = Some(index);
                }
                array_depth = array_depth
                    .checked_sub(1)
                    .context("unbalanced closing array in rhythm-devices JSON")?;
            }
            b'{' => {
                if array_depth == 1 && object_depth == 0 {
                    object_start = Some(index);
                }
                object_depth += 1;
            }
            b'}' => {
                object_depth = object_depth
                    .checked_sub(1)
                    .context("unbalanced closing object in rhythm-devices JSON")?;
                if array_depth == 1 && object_depth == 0 {
                    let start = object_start
                        .take()
                        .context("top-level object ended without a start")?;
                    ranges.push((start, index + 1));
                }
            }
            _ => {}
        }
    }
    if in_string || array_depth != 0 || object_depth != 0 {
        bail!("unbalanced rhythm-devices JSON source");
    }
    let closing_bracket = closing_bracket.context("rhythm-devices JSON is not a root array")?;
    Ok((ranges, closing_bracket))
}

fn indent_after_first_line(value: &str, indent: &str) -> String {
    let mut lines = value.lines();
    let mut output = lines.next().unwrap_or_default().to_string();
    for line in lines {
        output.push('\n');
        output.push_str(indent);
        output.push_str(line);
    }
    output
}

fn indent_every_line(value: &str, indent: &str) -> String {
    value
        .lines()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn same_device(left: &DeviceEntry, right: &DeviceEntry) -> bool {
    let matter_ids_match =
        left.matter
            .as_ref()
            .zip(right.matter.as_ref())
            .is_some_and(|(left, right)| {
                left.vendor_id
                    .zip(left.product_id)
                    .zip(right.vendor_id.zip(right.product_id))
                    .is_some_and(|(left, right)| left == right)
            });
    matter_ids_match
        || (left.manufacturer.eq_ignore_ascii_case(&right.manufacturer)
            && left.model.eq_ignore_ascii_case(&right.model))
}

fn write_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .with_context(|| format!("output path {} has no file name", path.display()))?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(
        ".{file_name}.bulb-bench-{}.tmp",
        std::process::id()
    ));
    fs::write(&temporary, contents)
        .with_context(|| format!("writing temporary database {}", temporary.display()))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("replacing database {}", path.display()));
    }
    Ok(())
}

fn add_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn round(value: f64, decimal_places: i32) -> f64 {
    let scale = 10_f64.powi(decimal_places);
    (value * scale).round() / scale
}

fn total_cmp(left: f64, right: f64) -> Ordering {
    left.total_cmp(&right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_dataset() -> ComparisonDataset {
        serde_json::from_str(include_str!(
            "../fixtures/synthetic-hue-vs-budget-matter.json"
        ))
        .expect("fixture should parse")
    }

    #[test]
    fn derives_floor_and_hue_relative_corrections() {
        let result = analyze_profile(&synthetic_dataset()).expect("fixture should analyze");

        assert_eq!(result.capability_hints.min_brightness, 7);
        assert_eq!(
            result.recommendation.classification,
            CandidateClassification::UsableWithProfile
        );
        let five_percent = result
            .recommended_control_strategy
            .brightness_correction_lut
            .iter()
            .find(|point| point.logical_percent == 5.0)
            .expect("5% correction");
        assert!(five_percent.candidate_command_percent > 10.0);

        let warm_white = result
            .recommended_control_strategy
            .color_temperature_correction_lut
            .iter()
            .find(|point| point.logical_kelvin == 2700)
            .expect("2700 K correction");
        assert!((2350..=2500).contains(&warm_white.candidate_command_kelvin));
        assert!(warm_white.error_before_kelvin > 250.0);

        let exported = &result.rhythm_devices_entry;
        assert_eq!(exported.min_brightness, Some(7));
        assert_eq!(exported.min_kelvin, Some(2700));
        assert_eq!(exported.max_kelvin, Some(6500));
        let adapted = rhythm_devices::adapt_command(
            &exported.capabilities(),
            5,
            rhythm_devices::ColorRequest::ColorTemperature {
                kelvin: 2700,
                xy: (0.46, 0.41),
                hue_saturation: None,
            },
            None,
            rhythm_devices::ColorPreference::PreferColorTemperature,
        );
        assert_eq!(adapted.brightness, Some(13));
        assert_eq!(adapted.kelvin, Some(2399));
        assert!(result
            .recommendation
            .suitable_for
            .iter()
            .any(|use_case| use_case.contains("tested logical range")));
        assert!(result
            .recommendation
            .avoid_for
            .iter()
            .any(|use_case| use_case.contains("2200 K")));
    }

    #[test]
    fn flags_unreachable_warm_white_and_duv() {
        let result = analyze_profile(&synthetic_dataset()).expect("fixture should analyze");
        let lowest = result
            .recommended_control_strategy
            .color_temperature_correction_lut
            .first()
            .expect("lowest CT point");

        assert!(lowest.clamped);
        assert!(!result
            .runtime_application
            .not_correctable_with_ct_only
            .is_empty());
        assert!(result
            .limitations
            .iter()
            .any(|limitation| limitation.contains("Kelvin alone")));
    }

    #[test]
    fn rejects_insufficient_stable_measurements() {
        let mut dataset = synthetic_dataset();
        dataset.candidate.brightness.truncate(2);
        let error = analyze_profile(&dataset).expect_err("insufficient evidence must fail");
        assert!(error.to_string().contains("at least three stable"));
    }

    #[test]
    fn database_update_rejects_synthetic_evidence() {
        let dataset = synthetic_dataset();
        let result = analyze_profile(&dataset).expect("fixture should analyze");
        let error = update_rhythm_devices_database(
            &dataset,
            &result,
            Path::new("does-not-matter.json"),
            Path::new("does-not-matter.json"),
        )
        .expect_err("synthetic evidence must never update the canonical database");
        assert!(error.to_string().contains("synthetic"));
    }

    #[test]
    fn database_update_appends_typed_entry_without_reformatting_existing_entries() {
        let mut dataset = synthetic_dataset();
        dataset.metadata.synthetic = false;
        dataset.metadata.label = "non-synthetic updater test".to_string();
        let result = analyze_profile(&dataset).expect("fixture should analyze");
        let database = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../os/rust/core/rhythm-devices/data/devices.json");
        let output = std::env::temp_dir().join(format!(
            "bulb-bench-rhythm-devices-upsert-{}.json",
            std::process::id()
        ));

        let update = update_rhythm_devices_database(&dataset, &result, &database, &output)
            .expect("real evidence should produce a database copy");
        assert_eq!(update.action, DatabaseUpdateAction::Inserted);
        let original = fs::read_to_string(&database).expect("read original database");
        let updated = fs::read_to_string(&output).expect("read updated database");
        let original_without_close = original
            .trim_end()
            .strip_suffix(']')
            .expect("database root array")
            .trim_end();
        assert!(updated.starts_with(original_without_close));
        let entries: Vec<DeviceEntry> =
            serde_json::from_str(&updated).expect("updated database should parse");
        assert!(entries.iter().any(|entry| {
            entry.manufacturer == "Synthetic Budget Lighting"
                && !entry.control_corrections.is_empty()
        }));
        fs::remove_file(&output).expect("remove test database copy");
    }

    #[test]
    fn database_update_replaces_an_exact_matter_device() {
        let mut dataset = synthetic_dataset();
        dataset.metadata.synthetic = false;
        dataset.metadata.label = "non-synthetic replacement test".to_string();
        dataset.candidate.device.manufacturer = "Shenzhen Qianyan Technology".to_string();
        dataset.candidate.device.model = "H6004".to_string();
        dataset.candidate.device.name = "Shenzhen Qianyan H6004".to_string();
        let matter = dataset
            .candidate
            .device
            .matter
            .as_mut()
            .expect("fixture Matter identity");
        matter.vendor_id = Some(4999);
        matter.product_id = Some(24580);
        let result = analyze_profile(&dataset).expect("fixture should analyze");
        let database = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../os/rust/core/rhythm-devices/data/devices.json");
        let output = std::env::temp_dir().join(format!(
            "bulb-bench-rhythm-devices-replace-{}.json",
            std::process::id()
        ));

        let update = update_rhythm_devices_database(&dataset, &result, &database, &output)
            .expect("exact device should be replaced");
        assert_eq!(update.action, DatabaseUpdateAction::Replaced);
        let entries: Vec<DeviceEntry> =
            serde_json::from_str(&fs::read_to_string(&output).expect("read replacement database"))
                .expect("replacement database should parse");
        let replaced = entries
            .iter()
            .find(|entry| entry.model == "H6004")
            .expect("replaced H6004 entry");
        assert_eq!(replaced.min_brightness, Some(7));
        assert!(!replaced.control_corrections.is_empty());
        fs::remove_file(&output).expect("remove replacement database copy");
    }
}
