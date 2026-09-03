//! Typed Matter light-control strategy resolved from safe defaults, the
//! builtin device database, an approved cloud profile, and a local audition.
//!
//! `DeviceQuirk` remains a compatibility input. Runtime planning consumes the
//! resolved profile so precedence is explicit and contradictory quirks cannot
//! decide behavior through check order.

use rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK;
use rhythm_devices::{ColorMode, DeviceQuirk, LightCapabilities};
use serde::{Deserialize, Serialize};

pub const DEFAULT_ASSUMED_COMMAND_SPACING_MS: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterProfileSource {
    SafeDefault,
    Builtin,
    Cloud,
    Audition,
    TryWith,
}

impl Default for MatterProfileSource {
    fn default() -> Self {
        Self::SafeDefault
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterColorRoute {
    HueSaturation,
    Xy,
    ColorTemperature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterTurnOnStrategy {
    StageColorThenLevelWithOnOff,
    ExplicitOnFirst,
    LevelWithOnOffThenColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterLevelCommand {
    MoveToLevelWithOnOff,
    MoveToLevel,
    StepWithOnOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterPowerOnBehavior {
    RestorePrevious,
    Off,
    On,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterMeasurementBasis {
    Measured,
    Assumed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterCommandSpacing {
    pub value_ms: u32,
    pub basis: MatterMeasurementBasis,
    pub source: MatterProfileSource,
}

/// One operator-validated white point for a bulb that renders adaptive white
/// through the Hue/Saturation command path. Hue and saturation use Matter's
/// normalized 0..=254 units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterHsWhitePoint {
    pub kelvin: u16,
    pub hue: u8,
    pub saturation: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterReadbackTrust {
    pub on_off: bool,
    pub level: bool,
    pub color: bool,
}

impl Default for MatterReadbackTrust {
    fn default() -> Self {
        Self {
            on_off: true,
            level: false,
            color: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MatterSubscriptionProfile {
    pub works: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truth_matches_direct_read: Option<bool>,
    pub subscription_survived: bool,
    pub resubscribe_after_power_cycle: bool,
    pub reports_external_changes: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness_interval_s: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MatterControlProfileSources {
    pub color_route: MatterProfileSource,
    pub hs_white_curve: MatterProfileSource,
    pub turn_on: MatterProfileSource,
    pub level_command: MatterProfileSource,
    pub execute_if_off_honoured: MatterProfileSource,
    pub on_restores_previous: MatterProfileSource,
    pub power_on_behavior: MatterProfileSource,
    pub kelvin_range: MatterProfileSource,
    pub min_brightness: MatterProfileSource,
    pub supports_transition: MatterProfileSource,
    pub readback_trust: MatterProfileSource,
    pub subscription: MatterProfileSource,
}

impl Default for MatterControlProfileSources {
    fn default() -> Self {
        Self {
            color_route: MatterProfileSource::SafeDefault,
            hs_white_curve: MatterProfileSource::SafeDefault,
            turn_on: MatterProfileSource::SafeDefault,
            level_command: MatterProfileSource::SafeDefault,
            execute_if_off_honoured: MatterProfileSource::SafeDefault,
            on_restores_previous: MatterProfileSource::SafeDefault,
            power_on_behavior: MatterProfileSource::SafeDefault,
            kelvin_range: MatterProfileSource::SafeDefault,
            min_brightness: MatterProfileSource::SafeDefault,
            supports_transition: MatterProfileSource::SafeDefault,
            readback_trust: MatterProfileSource::SafeDefault,
            subscription: MatterProfileSource::SafeDefault,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterControlProfile {
    #[serde(default = "profile_schema_version")]
    pub schema_version: u8,
    pub color_route: MatterColorRoute,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hs_white_curve: Vec<MatterHsWhitePoint>,
    pub turn_on: MatterTurnOnStrategy,
    pub level_command: MatterLevelCommand,
    pub command_spacing_ms: MatterCommandSpacing,
    pub execute_if_off_honoured: bool,
    pub on_restores_previous: bool,
    pub power_on_behavior: MatterPowerOnBehavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kelvin_range: Option<(u16, u16)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_brightness: Option<u8>,
    pub supports_transition: bool,
    #[serde(default)]
    pub readback_trust: MatterReadbackTrust,
    #[serde(default)]
    pub subscription: MatterSubscriptionProfile,
    #[serde(default)]
    pub source: MatterControlProfileSources,
}

fn profile_schema_version() -> u8 {
    1
}

impl Default for MatterControlProfile {
    fn default() -> Self {
        Self {
            schema_version: profile_schema_version(),
            color_route: MatterColorRoute::HueSaturation,
            hs_white_curve: Vec::new(),
            turn_on: MatterTurnOnStrategy::StageColorThenLevelWithOnOff,
            level_command: MatterLevelCommand::MoveToLevelWithOnOff,
            command_spacing_ms: MatterCommandSpacing {
                value_ms: DEFAULT_ASSUMED_COMMAND_SPACING_MS,
                basis: MatterMeasurementBasis::Assumed,
                source: MatterProfileSource::SafeDefault,
            },
            execute_if_off_honoured: true,
            on_restores_previous: false,
            power_on_behavior: MatterPowerOnBehavior::Unknown,
            kelvin_range: None,
            min_brightness: None,
            supports_transition: true,
            readback_trust: MatterReadbackTrust::default(),
            subscription: MatterSubscriptionProfile::default(),
            source: MatterControlProfileSources::default(),
        }
    }
}

/// Resolve one deterministic profile. Later inputs win per field:
/// safe default < builtin metadata < approved cloud < local audition.
pub fn resolve_control_profile(
    builtin_caps: &LightCapabilities,
    builtin_quirks: &[DeviceQuirk],
    cloud: Option<&MatterControlProfile>,
    audition: Option<&MatterControlProfile>,
) -> MatterControlProfile {
    let mut profile =
        profile_from_legacy(builtin_caps, builtin_quirks, MatterProfileSource::Builtin);
    if let Some(cloud) = cloud {
        overlay_sourced_profile(&mut profile, cloud, |source| {
            source == MatterProfileSource::Cloud
        });
    }
    if let Some(audition) = audition {
        overlay_sourced_audition_profile(&mut profile, audition);
    }
    profile
}

/// Translate the previous quirk/capability representation into a complete
/// profile. This is the compatibility reader for persisted and cloud v2 data.
pub fn profile_from_legacy(
    caps: &LightCapabilities,
    quirks: &[DeviceQuirk],
    source: MatterProfileSource,
) -> MatterControlProfile {
    let mut profile = MatterControlProfile::default();
    profile.color_route = if quirks.iter().any(|quirk| {
        matches!(quirk, DeviceQuirk::Other(value) if value == PREFER_COLOR_TEMPERATURE_QUIRK)
    }) {
        MatterColorRoute::ColorTemperature
    } else if quirks
        .iter()
        .any(|quirk| matches!(quirk, DeviceQuirk::NeedsXyNotCt))
    {
        MatterColorRoute::Xy
    } else if caps.color_modes.contains(&ColorMode::HueSaturation) {
        MatterColorRoute::HueSaturation
    } else if caps.color_modes.contains(&ColorMode::ColorTemperature) {
        MatterColorRoute::ColorTemperature
    } else {
        MatterColorRoute::Xy
    };
    profile.turn_on = if quirks
        .iter()
        .any(|quirk| matches!(quirk, DeviceQuirk::NeedsExplicitOn))
    {
        MatterTurnOnStrategy::ExplicitOnFirst
    } else {
        MatterTurnOnStrategy::StageColorThenLevelWithOnOff
    };
    if let Some(spacing_ms) = quirks.iter().find_map(|quirk| match quirk {
        DeviceQuirk::CommandThrottleMs(value) if *value > 0 => Some(*value),
        _ => None,
    }) {
        profile.command_spacing_ms = MatterCommandSpacing {
            value_ms: spacing_ms,
            // Legacy values did not preserve whether spacing was measured.
            basis: MatterMeasurementBasis::Assumed,
            source,
        };
    }
    profile.kelvin_range = caps.min_kelvin.zip(caps.max_kelvin);
    profile.min_brightness = caps.min_brightness;
    profile.supports_transition = caps.supports_transition;
    profile.source = MatterControlProfileSources {
        color_route: source,
        turn_on: source,
        kelvin_range: source,
        min_brightness: source,
        supports_transition: source,
        ..MatterControlProfileSources::default()
    };
    profile
}

/// Translate only the fields that a legacy local tester result could express.
/// An omitted legacy value is unknown, not evidence that should replace a
/// newer cloud or builtin field.
pub fn profile_overlay_from_legacy_local(
    quirks: &[DeviceQuirk],
    min_brightness: Option<u8>,
    supports_transition: Option<bool>,
) -> MatterControlProfile {
    let mut profile = MatterControlProfile::default();
    let source = MatterProfileSource::Audition;
    if quirks.iter().any(|quirk| {
        matches!(quirk, DeviceQuirk::Other(value) if value == PREFER_COLOR_TEMPERATURE_QUIRK)
    }) {
        profile.color_route = MatterColorRoute::ColorTemperature;
        profile.source.color_route = source;
    } else if quirks
        .iter()
        .any(|quirk| matches!(quirk, DeviceQuirk::NeedsXyNotCt))
    {
        profile.color_route = MatterColorRoute::Xy;
        profile.source.color_route = source;
    } else if quirks
        .iter()
        .any(|quirk| matches!(quirk, DeviceQuirk::NeedsHueSaturationNotCt))
    {
        profile.color_route = MatterColorRoute::HueSaturation;
        profile.source.color_route = source;
    }
    if quirks
        .iter()
        .any(|quirk| matches!(quirk, DeviceQuirk::NeedsExplicitOn))
    {
        profile.turn_on = MatterTurnOnStrategy::ExplicitOnFirst;
        profile.source.turn_on = source;
    }
    if let Some(value_ms) = quirks.iter().find_map(|quirk| match quirk {
        DeviceQuirk::CommandThrottleMs(value) if *value > 0 => Some(*value),
        _ => None,
    }) {
        profile.command_spacing_ms = MatterCommandSpacing {
            value_ms,
            basis: MatterMeasurementBasis::Assumed,
            source,
        };
    }
    if let Some(min_brightness) = min_brightness {
        profile.min_brightness = Some(min_brightness);
        profile.source.min_brightness = source;
    }
    if let Some(supports_transition) = supports_transition {
        profile.supports_transition = supports_transition;
        profile.source.supports_transition = source;
    }
    profile
}

/// Overlay only fields whose persisted evidence says they were learned by an
/// audition. A local report contains the complete effective profile for audit
/// purposes, but copied cloud/builtin/default fields must not shadow newer
/// lower-precedence inputs on a later resolve.
pub(crate) fn overlay_sourced_audition_profile(
    target: &mut MatterControlProfile,
    audition: &MatterControlProfile,
) {
    overlay_sourced_profile(target, audition, |source| {
        matches!(
            source,
            MatterProfileSource::Audition | MatterProfileSource::TryWith
        )
    });
}

fn overlay_sourced_profile(
    target: &mut MatterControlProfile,
    overlay: &MatterControlProfile,
    learned: impl Fn(MatterProfileSource) -> bool,
) {
    macro_rules! overlay_field {
        ($field:ident) => {
            if learned(overlay.source.$field) {
                target.$field = overlay.$field.clone();
                target.source.$field = overlay.source.$field;
            }
        };
    }

    overlay_field!(color_route);
    overlay_field!(hs_white_curve);
    overlay_field!(turn_on);
    overlay_field!(level_command);
    if learned(overlay.command_spacing_ms.source) {
        target.command_spacing_ms = overlay.command_spacing_ms.clone();
    }
    overlay_field!(execute_if_off_honoured);
    overlay_field!(on_restores_previous);
    overlay_field!(power_on_behavior);
    overlay_field!(kelvin_range);
    overlay_field!(min_brightness);
    overlay_field!(supports_transition);
    overlay_field!(readback_trust);
    overlay_field!(subscription);
}

pub(crate) fn overlay_profile(
    target: &mut MatterControlProfile,
    overlay: &MatterControlProfile,
    source: MatterProfileSource,
) {
    target.color_route = overlay.color_route;
    target.hs_white_curve = overlay.hs_white_curve.clone();
    target.turn_on = overlay.turn_on;
    target.level_command = overlay.level_command;
    target.command_spacing_ms = overlay.command_spacing_ms.clone();
    target.command_spacing_ms.source = source;
    target.execute_if_off_honoured = overlay.execute_if_off_honoured;
    target.on_restores_previous = overlay.on_restores_previous;
    target.power_on_behavior = overlay.power_on_behavior;
    target.kelvin_range = overlay.kelvin_range;
    target.min_brightness = overlay.min_brightness;
    target.supports_transition = overlay.supports_transition;
    target.readback_trust = overlay.readback_trust.clone();
    target.subscription = overlay.subscription.clone();
    target.source = MatterControlProfileSources {
        color_route: source,
        hs_white_curve: source,
        turn_on: source,
        level_command: source,
        execute_if_off_honoured: source,
        on_restores_previous: source,
        power_on_behavior: source,
        kelvin_range: source,
        min_brightness: source,
        supports_transition: source,
        readback_trust: source,
        subscription: source,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_devices::LightType;

    #[test]
    fn resolver_uses_fixed_per_field_source_order() {
        let caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
        let cloud = MatterControlProfile {
            color_route: MatterColorRoute::Xy,
            source: MatterControlProfileSources {
                color_route: MatterProfileSource::Cloud,
                ..MatterControlProfileSources::default()
            },
            ..MatterControlProfile::default()
        };
        let audition = MatterControlProfile {
            color_route: MatterColorRoute::ColorTemperature,
            command_spacing_ms: MatterCommandSpacing {
                value_ms: 125,
                basis: MatterMeasurementBasis::Measured,
                source: MatterProfileSource::TryWith,
            },
            source: MatterControlProfileSources {
                color_route: MatterProfileSource::TryWith,
                ..MatterControlProfileSources::default()
            },
            ..MatterControlProfile::default()
        };

        let resolved = resolve_control_profile(&caps, &[], Some(&cloud), Some(&audition));

        assert_eq!(resolved.color_route, MatterColorRoute::ColorTemperature);
        assert_eq!(resolved.source.color_route, MatterProfileSource::TryWith);
        assert_eq!(resolved.command_spacing_ms.value_ms, 125);
        assert_eq!(
            resolved.command_spacing_ms.basis,
            MatterMeasurementBasis::Measured
        );
        assert_eq!(
            resolved.command_spacing_ms.source,
            MatterProfileSource::TryWith
        );
    }

    #[test]
    fn legacy_spacing_is_never_mislabelled_measured() {
        let caps = LightCapabilities::defaults_for(LightType::Dimmable);
        let profile = profile_from_legacy(
            &caps,
            &[DeviceQuirk::CommandThrottleMs(250)],
            MatterProfileSource::Builtin,
        );

        assert_eq!(profile.command_spacing_ms.value_ms, 250);
        assert_eq!(
            profile.command_spacing_ms.basis,
            MatterMeasurementBasis::Assumed
        );
    }

    #[test]
    fn local_profile_overlays_only_fields_with_audition_evidence() {
        let mut resolved = MatterControlProfile {
            color_route: MatterColorRoute::Xy,
            source: MatterControlProfileSources {
                color_route: MatterProfileSource::Cloud,
                ..MatterControlProfileSources::default()
            },
            ..MatterControlProfile::default()
        };
        let audition = MatterControlProfile {
            color_route: MatterColorRoute::ColorTemperature,
            turn_on: MatterTurnOnStrategy::ExplicitOnFirst,
            source: MatterControlProfileSources {
                color_route: MatterProfileSource::Cloud,
                turn_on: MatterProfileSource::TryWith,
                ..MatterControlProfileSources::default()
            },
            ..MatterControlProfile::default()
        };

        overlay_sourced_audition_profile(&mut resolved, &audition);

        assert_eq!(resolved.color_route, MatterColorRoute::Xy);
        assert_eq!(resolved.source.color_route, MatterProfileSource::Cloud);
        assert_eq!(resolved.turn_on, MatterTurnOnStrategy::ExplicitOnFirst);
        assert_eq!(resolved.source.turn_on, MatterProfileSource::TryWith);
    }

    #[test]
    fn legacy_local_overlay_changes_only_fields_with_saved_evidence() {
        let profile =
            profile_overlay_from_legacy_local(&[DeviceQuirk::NeedsExplicitOn], Some(12), None);

        assert_eq!(profile.source.turn_on, MatterProfileSource::Audition);
        assert_eq!(profile.source.min_brightness, MatterProfileSource::Audition);
        assert_eq!(
            profile.source.supports_transition,
            MatterProfileSource::SafeDefault
        );
        assert_eq!(profile.source.color_route, MatterProfileSource::SafeDefault);
    }
}
