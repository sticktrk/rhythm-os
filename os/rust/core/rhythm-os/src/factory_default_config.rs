//! Factory-default developer-owned profile bundle.
//!
//! Built-in profile configs live in `rhythm-core` so there is one source of
//! truth for profile behavior. This module reads the checked-in JSON bundle
//! for factory metadata, power-save, and transition configuration, then injects
//! the core built-in profiles for runtime, storage, and API exports.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use rhythm_core::{
    default_builtin_profiles, is_builtin_state_profile_id, normalize_builtin_state_profile_config,
    normalize_mode_transition_configs, LightProfileConfig, ModeConfig, ModeTransitionConfig,
    RhythmMode, DAY_IDLE_PROFILE_ID, RHYTHM_PROFILE_ID, SLEEP_IDLE_PROFILE_ID, SLEEP_PROFILE_ID,
};

use crate::bundle::{BundleKind, ProfileBundle, BUNDLE_SCHEMA_VERSION};
use crate::scenes::SceneDefinition;

const FACTORY_DEFAULT_PROFILE_BUNDLE_JSON: &str =
    include_str!("../config/factory-default/default_profile_bundle.json");

static FACTORY_DEFAULT_PROFILE_BUNDLE: OnceLock<ProfileBundle> = OnceLock::new();
static FACTORY_DEFAULT_LIGHT_PROFILE_CONFIGS: OnceLock<BTreeMap<String, LightProfileConfig>> =
    OnceLock::new();
static FACTORY_DEFAULT_SCENES: OnceLock<BTreeMap<String, SceneDefinition>> = OnceLock::new();
static FACTORY_DEFAULT_MODE_CONFIGS: OnceLock<BTreeMap<RhythmMode, ModeConfig>> = OnceLock::new();
static FACTORY_DEFAULT_MODE_TRANSITIONS: OnceLock<Vec<ModeTransitionConfig>> = OnceLock::new();

fn parse_factory_default_profile_bundle() -> Result<ProfileBundle> {
    let mut bundle: ProfileBundle = serde_json::from_str(FACTORY_DEFAULT_PROFILE_BUNDLE_JSON)
        .context("failed to parse factory-default profile bundle JSON")?;

    if bundle.schema_version != BUNDLE_SCHEMA_VERSION {
        return Err(anyhow!(
            "factory-default profile bundle schema_version {} does not match supported schema {}",
            bundle.schema_version,
            BUNDLE_SCHEMA_VERSION
        ));
    }

    if bundle.kind != BundleKind::ProfileBundle {
        return Err(anyhow!(
            "factory-default profile bundle must use kind=profile_bundle"
        ));
    }

    if !bundle.profile.profiles.is_empty() {
        return Err(anyhow!(
            "factory-default profile bundle must not define profiles; built-in profiles come from rhythm-core"
        ));
    }

    bundle.profile.profiles = default_builtin_profiles().into();

    let mut seen_ids = std::collections::HashSet::new();
    for profile in &bundle.profile.profiles {
        if !seen_ids.insert(profile.id.clone()) {
            return Err(anyhow!(
                "factory-default profile bundle defines duplicate profile id '{}'",
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
                "factory-default profile bundle is missing required profile '{}'",
                required_id
            ));
        }
    }

    if bundle.profile.scenes.len() > 3 {
        return Err(anyhow!(
            "factory-default profile bundle must define at most 3 sample scenes"
        ));
    }

    let mut seen_scene_ids = std::collections::HashSet::new();
    for scene in &mut bundle.profile.scenes {
        scene.normalize();
        if !seen_scene_ids.insert(scene.id.clone()) {
            return Err(anyhow!(
                "factory-default profile bundle defines duplicate scene id '{}'",
                scene.id
            ));
        }
    }

    Ok(bundle)
}

fn factory_default_profile_bundle_ref() -> &'static ProfileBundle {
    FACTORY_DEFAULT_PROFILE_BUNDLE.get_or_init(|| {
        parse_factory_default_profile_bundle().unwrap_or_else(|e| {
            panic!("invalid factory-default profile bundle: {e:#}");
        })
    })
}

fn factory_default_light_profile_configs_ref() -> &'static BTreeMap<String, LightProfileConfig> {
    FACTORY_DEFAULT_LIGHT_PROFILE_CONFIGS.get_or_init(|| {
        let mut configs = BTreeMap::new();
        for mut profile in factory_default_profile_bundle_ref()
            .profile
            .profiles
            .clone()
        {
            normalize_builtin_state_profile_config(&mut profile);
            configs.insert(profile.id.clone(), profile);
        }
        configs
    })
}

fn factory_default_scene_map_ref() -> &'static BTreeMap<String, SceneDefinition> {
    FACTORY_DEFAULT_SCENES.get_or_init(|| {
        factory_default_profile_bundle_ref()
            .profile
            .scenes
            .iter()
            .map(|scene| (scene.id.clone(), scene.clone()))
            .collect()
    })
}

fn factory_default_mode_config_map_ref() -> &'static BTreeMap<RhythmMode, ModeConfig> {
    FACTORY_DEFAULT_MODE_CONFIGS.get_or_init(|| {
        RhythmMode::ALL
            .into_iter()
            .map(|mode| (mode, ModeConfig::default_for_mode(mode)))
            .collect()
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

pub fn factory_default_profile_bundle() -> ProfileBundle {
    factory_default_profile_bundle_ref().clone()
}

pub fn factory_default_power_save() -> bool {
    factory_default_profile_bundle_ref().profile.power_save
}

/// New rpiz appliances ship with auto-update enabled: they poll the curated
/// "stable" OTA feed and apply updates silently during the overnight window.
/// Power users can flip this off via `PUT /api/settings` to opt into the beta
/// feed with manual-only updates.
pub fn factory_default_auto_update() -> bool {
    true
}

pub fn factory_default_active_mode() -> RhythmMode {
    RhythmMode::Day
}

pub fn factory_default_light_profile_config_map() -> BTreeMap<String, LightProfileConfig> {
    factory_default_light_profile_configs_ref().clone()
}

pub fn factory_default_light_profile_config(id: &str) -> Option<LightProfileConfig> {
    factory_default_light_profile_configs_ref().get(id).cloned()
}

pub fn factory_default_scene_map() -> BTreeMap<String, SceneDefinition> {
    factory_default_scene_map_ref().clone()
}

pub fn factory_default_mode_config_map() -> BTreeMap<RhythmMode, ModeConfig> {
    factory_default_mode_config_map_ref().clone()
}

pub fn factory_default_mode_transition_configs() -> Vec<ModeTransitionConfig> {
    FACTORY_DEFAULT_MODE_TRANSITIONS
        .get_or_init(|| {
            normalize_mode_transition_configs(
                factory_default_profile_bundle_ref()
                    .profile
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
    fn factory_default_profile_bundle_exposes_expected_defaults() {
        assert_eq!(factory_default_active_mode(), RhythmMode::Day);
        assert!(factory_default_power_save());

        let bundle = factory_default_profile_bundle();
        assert_eq!(bundle.name.as_deref(), Some("Factory Default"));
        assert_eq!(bundle.profile.scenes.len(), 3);

        let scenes = factory_default_scene_map();
        assert_eq!(scenes.len(), 3);
        assert!(scenes.contains_key("color-carnival"));
        assert!(scenes.contains_key("electric-lagoon"));
        assert!(scenes.contains_key("berry-pop"));
        let carnival = scenes.get("color-carnival").unwrap();
        assert_eq!(carnival.name, "Color Carnival");
        let palette = &carnival.light.as_ref().unwrap().palette;
        assert_eq!(palette.len(), 6);
        let unique_palette_colors: std::collections::HashSet<_> = palette
            .iter()
            .map(|output| match output.color {
                Some(crate::scenes::LightSceneColor::Rgb { rgb }) => (rgb.r, rgb.g, rgb.b),
                other => panic!("expected rgb palette color, got {other:?}"),
            })
            .collect();
        assert_eq!(unique_palette_colors.len(), 6);

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
        assert_eq!(sleep.min_brightness, 1);
        assert_eq!(sleep.max_brightness, 1);
        assert_eq!(sleep.min_color_temp, rhythm.min_color_temp);
        assert_eq!(sleep.max_color_temp, rhythm.min_color_temp);
        assert!(matches!(
            sleep.curve,
            rhythm_core::LightCurveShape::Constant {
                brightness,
                color_temp,
                direct_color: None,
            } if brightness.abs() < f32::EPSILON
                && color_temp.abs() < f32::EPSILON
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
        assert!(mode_configs
            .values()
            .all(|config| config.room_defaults.is_empty()));

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
        assert!(transitions[0].trigger_enabled);
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
        assert!(transitions[1].trigger_enabled);
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
