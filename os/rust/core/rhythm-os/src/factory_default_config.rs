//! Factory-default developer-owned configuration bundle.
//!
//! The server factory defaults live in a checked-in JSON bundle so developers
//! can tune shipped lighting behavior without editing Rust code. Runtime and
//! storage code clone from this module when they need the baseline profile,
//! mode, or transition configuration.

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use rhythm_core::{
    is_builtin_state_profile_id, normalize_builtin_state_profile_config,
    normalize_mode_transition_configs, LightProfileConfig, ModeConfig, ModeTransitionConfig,
    RhythmMode, DAY_IDLE_PROFILE_ID, RHYTHM_PROFILE_ID, SLEEP_IDLE_PROFILE_ID, SLEEP_PROFILE_ID,
};

use crate::bundle::{BundleKind, ConfigurationBundle, BUNDLE_SCHEMA_VERSION};

const FACTORY_DEFAULT_CONFIGURATION_JSON: &str =
    include_str!("../config/factory-default/default_configuration.json");

static FACTORY_DEFAULT_CONFIGURATION_BUNDLE: OnceLock<ConfigurationBundle> = OnceLock::new();
static FACTORY_DEFAULT_LIGHT_PROFILE_CONFIGS: OnceLock<BTreeMap<String, LightProfileConfig>> =
    OnceLock::new();
static FACTORY_DEFAULT_MODE_CONFIGS: OnceLock<BTreeMap<RhythmMode, ModeConfig>> = OnceLock::new();
static FACTORY_DEFAULT_MODE_TRANSITIONS: OnceLock<Vec<ModeTransitionConfig>> = OnceLock::new();

fn parse_factory_default_configuration_bundle() -> Result<ConfigurationBundle> {
    let bundle: ConfigurationBundle = serde_json::from_str(FACTORY_DEFAULT_CONFIGURATION_JSON)
        .context("failed to parse factory-default configuration JSON")?;

    if bundle.schema_version != BUNDLE_SCHEMA_VERSION {
        return Err(anyhow!(
            "factory-default configuration schema_version {} does not match supported schema {}",
            bundle.schema_version,
            BUNDLE_SCHEMA_VERSION
        ));
    }

    if bundle.kind != BundleKind::ConfigurationBundle {
        return Err(anyhow!(
            "factory-default configuration must use kind=configuration_bundle"
        ));
    }

    let mut seen_ids = HashSet::new();
    for profile in &bundle.configuration.profiles {
        if !seen_ids.insert(profile.id.clone()) {
            return Err(anyhow!(
                "factory-default configuration defines duplicate profile id '{}'",
                profile.id
            ));
        }
    }

    for required_id in [
        RHYTHM_PROFILE_ID,
        SLEEP_PROFILE_ID,
        DAY_IDLE_PROFILE_ID,
        SLEEP_IDLE_PROFILE_ID,
    ] {
        if !seen_ids.contains(required_id) {
            return Err(anyhow!(
                "factory-default configuration is missing required profile '{}'",
                required_id
            ));
        }
    }

    Ok(bundle)
}

fn factory_default_configuration_bundle_ref() -> &'static ConfigurationBundle {
    FACTORY_DEFAULT_CONFIGURATION_BUNDLE.get_or_init(|| {
        parse_factory_default_configuration_bundle().unwrap_or_else(|e| {
            panic!("invalid factory-default configuration: {e:#}");
        })
    })
}

fn factory_default_light_profile_configs_ref() -> &'static BTreeMap<String, LightProfileConfig> {
    FACTORY_DEFAULT_LIGHT_PROFILE_CONFIGS.get_or_init(|| {
        let mut configs = BTreeMap::new();
        for mut profile in factory_default_configuration_bundle_ref()
            .configuration
            .profiles
            .clone()
        {
            normalize_builtin_state_profile_config(&mut profile);
            configs.insert(profile.id.clone(), profile);
        }
        configs
    })
}

fn factory_default_mode_config_map_ref() -> &'static BTreeMap<RhythmMode, ModeConfig> {
    FACTORY_DEFAULT_MODE_CONFIGS.get_or_init(|| {
        let mut configs: BTreeMap<_, _> = RhythmMode::ALL
            .into_iter()
            .map(|mode| (mode, ModeConfig::default_for_mode(mode)))
            .collect();

        for mut config in factory_default_configuration_bundle_ref()
            .configuration
            .mode_configs
            .clone()
        {
            config.normalize_profile_ids();
            configs.insert(config.mode, config);
        }

        configs
    })
}

fn factory_default_resolved_active_profile_id_for_mode(mode: RhythmMode) -> String {
    let requested = factory_default_mode_config_map_ref()
        .get(&mode)
        .and_then(|config| config.active_profile_id.clone())
        .unwrap_or_else(|| mode.default_active_profile_id().to_string());

    if !is_builtin_state_profile_id(&requested)
        && factory_default_light_profile_configs_ref().contains_key(&requested)
    {
        requested
    } else {
        mode.default_active_profile_id().to_string()
    }
}

pub fn factory_default_configuration_bundle() -> ConfigurationBundle {
    factory_default_configuration_bundle_ref().clone()
}

pub fn factory_default_power_save() -> bool {
    factory_default_configuration_bundle_ref()
        .configuration
        .power_save
}

pub fn factory_default_active_mode() -> RhythmMode {
    factory_default_configuration_bundle_ref()
        .configuration
        .active_mode
}

pub fn factory_default_light_profile_config_map() -> BTreeMap<String, LightProfileConfig> {
    factory_default_light_profile_configs_ref().clone()
}

pub fn factory_default_light_profile_config(id: &str) -> Option<LightProfileConfig> {
    factory_default_light_profile_configs_ref().get(id).cloned()
}

pub fn factory_default_mode_config_map() -> BTreeMap<RhythmMode, ModeConfig> {
    factory_default_mode_config_map_ref().clone()
}

pub fn factory_default_mode_transition_configs() -> Vec<ModeTransitionConfig> {
    FACTORY_DEFAULT_MODE_TRANSITIONS
        .get_or_init(|| {
            normalize_mode_transition_configs(
                factory_default_configuration_bundle_ref()
                    .configuration
                    .mode_transitions
                    .clone(),
            )
        })
        .clone()
}

pub fn factory_default_active_profile_config_for_mode(mode: RhythmMode) -> LightProfileConfig {
    let requested_id = factory_default_resolved_active_profile_id_for_mode(mode);
    factory_default_light_profile_config(requested_id.as_str()).unwrap_or_else(|| {
        factory_default_light_profile_config(mode.default_active_profile_id()).unwrap_or_else(
            || {
                panic!(
                    "factory-default configuration missing required active profile '{}'",
                    mode.default_active_profile_id()
                )
            },
        )
    })
}

pub fn factory_default_idle_profile_config_for_mode(mode: RhythmMode) -> LightProfileConfig {
    factory_default_light_profile_config(mode.default_idle_profile_id()).unwrap_or_else(|| {
        panic!(
            "factory-default configuration missing required idle profile '{}'",
            mode.default_idle_profile_id()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::{ModeTransitionTrigger, TimerSetting};

    #[test]
    fn factory_default_configuration_exposes_expected_defaults() {
        assert_eq!(factory_default_active_mode(), RhythmMode::Day);
        assert!(!factory_default_power_save());

        let bundle = factory_default_configuration_bundle();
        assert_eq!(bundle.name.as_deref(), Some("Factory Default"));

        let profiles = factory_default_light_profile_config_map();
        assert_eq!(profiles.len(), 4);
        assert!(profiles.contains_key(RHYTHM_PROFILE_ID));
        assert!(profiles.contains_key(SLEEP_PROFILE_ID));
        assert!(profiles.contains_key(DAY_IDLE_PROFILE_ID));
        assert!(profiles.contains_key(SLEEP_IDLE_PROFILE_ID));

        let rhythm = profiles.get(RHYTHM_PROFILE_ID).unwrap();
        assert_eq!(rhythm.name, "Day");
        assert_eq!(rhythm.min_brightness, 20);
        assert_eq!(rhythm.max_brightness, 100);
        assert_eq!(rhythm.min_color_temp, 1800);
        assert_eq!(rhythm.max_color_temp, 5500);
        assert_eq!(rhythm.max_dim_steps, 6);
        assert!(matches!(
            rhythm.curve,
            rhythm_core::LightCurveShape::SuperGaussian {
                width_left_bri,
                width_right_bri,
                width_left_cct,
                width_right_cct,
                shape_p,
                direct_color: None,
            } if (width_left_bri - 0.95).abs() < f32::EPSILON
                && (width_right_bri - 0.85).abs() < f32::EPSILON
                && (width_left_cct - 0.95).abs() < f32::EPSILON
                && (width_right_cct - 1.15).abs() < f32::EPSILON
                && (shape_p - 6.0).abs() < f32::EPSILON
        ));

        let sleep = profiles.get(SLEEP_PROFILE_ID).unwrap();
        assert_eq!(sleep.name, "Sleep");
        assert_eq!(sleep.min_brightness, 20);
        assert_eq!(sleep.max_brightness, 20);
        assert!(matches!(
            sleep.curve,
            rhythm_core::LightCurveShape::Constant {
                brightness,
                color_temp,
                direct_color: Some(ref direct_color),
            } if (brightness - 1.0).abs() < f32::EPSILON
                && color_temp.abs() < f32::EPSILON
                && direct_color.rgb == rhythm_core::Rgb::new(255, 0, 0)
        ));

        let mode_configs = factory_default_mode_config_map();
        assert_eq!(
            mode_configs
                .get(&RhythmMode::Day)
                .and_then(|config| config.active_profile_id.as_deref()),
            Some(RHYTHM_PROFILE_ID)
        );
        assert_eq!(
            mode_configs
                .get(&RhythmMode::Sleep)
                .and_then(|config| config.active_profile_id.as_deref()),
            Some(SLEEP_PROFILE_ID)
        );

        let transitions = factory_default_mode_transition_configs();
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].id, "sleep_to_day");
        assert_eq!(transitions[0].label, "Sleep to Day");
        assert_eq!(transitions[0].from_mode, RhythmMode::Sleep);
        assert_eq!(transitions[0].to_mode, RhythmMode::Day);
        assert_eq!(
            transitions[0].trigger,
            ModeTransitionTrigger::AstronomicalTwilight
        );
        assert_eq!(transitions[0].duration_ms, TimerSetting::Auto);
        assert!(transitions[0].preserve_hard_off);
        assert_eq!(transitions[1].id, "day_to_sleep");
        assert_eq!(transitions[1].label, "Day to Sleep");
        assert_eq!(transitions[1].from_mode, RhythmMode::Day);
        assert_eq!(transitions[1].to_mode, RhythmMode::Sleep);
        assert_eq!(
            transitions[1].trigger,
            ModeTransitionTrigger::NauticalTwilight
        );
        assert_eq!(transitions[1].duration_ms, TimerSetting::Auto);
        assert!(transitions[1].preserve_hard_off);
    }

    #[test]
    fn factory_default_profile_helpers_resolve_mode_specific_profiles() {
        assert_eq!(
            factory_default_active_profile_config_for_mode(RhythmMode::Day).id,
            RHYTHM_PROFILE_ID
        );
        assert_eq!(
            factory_default_active_profile_config_for_mode(RhythmMode::Sleep).id,
            SLEEP_PROFILE_ID
        );
        assert_eq!(
            factory_default_idle_profile_config_for_mode(RhythmMode::Day).id,
            DAY_IDLE_PROFILE_ID
        );
        assert_eq!(
            factory_default_idle_profile_config_for_mode(RhythmMode::Sleep).id,
            SLEEP_IDLE_PROFILE_ID
        );
    }
}
