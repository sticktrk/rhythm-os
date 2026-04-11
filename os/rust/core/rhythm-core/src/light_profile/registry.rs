//! Light profile registry for managing active and state profiles.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use rhythm_profile::profile_config::DEFAULT_FADE_MS;
use rhythm_profile::{CurveContext, LightCurveShape, LightProfileConfig, LightingValues};

use super::defaults::{
    default_builtin_profiles, default_day_idle_profile, default_rhythm_profile,
    default_sleep_idle_profile, default_sleep_profile, is_builtin_state_profile_id,
    RHYTHM_PROFILE_ID, SLEEP_PROFILE_ID,
};
use super::{LightProfile, LightProfileModule};
use crate::room::{
    default_mode_configs, ModeConfig, RhythmMode, RoomModeState, RoomProfileSettings,
};

#[derive(Clone)]
struct InheritedStateProfile {
    config: LightProfileConfig,
    active_profile: Arc<dyn LightProfileModule>,
}

impl InheritedStateProfile {
    fn new(config: LightProfileConfig, active_profile: Arc<dyn LightProfileModule>) -> Self {
        Self {
            config,
            active_profile,
        }
    }

    fn constant_brightness(&self) -> u8 {
        self.config.max_brightness.max(self.config.min_brightness)
    }

    fn motion_timeout(&self, ctx: &CurveContext, active_values: &LightingValues) -> u16 {
        self.config
            .motion_timeout_secs
            .resolve(ctx.current_hour)
            .map(|v| v as u16)
            .unwrap_or(active_values.motion_timeout_secs)
    }
}

impl LightProfileModule for InheritedStateProfile {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    fn calculate(&self, ctx: &CurveContext) -> LightingValues {
        let mut values = self.active_profile.calculate(ctx);
        values.brightness = self.constant_brightness();
        values.transition_ms = self
            .config
            .fade_ms
            .resolve(ctx.current_hour)
            .unwrap_or(DEFAULT_FADE_MS as u32);
        values.motion_timeout_secs = self.motion_timeout(ctx, &values);
        values
    }

    fn calculate_brightness(&self, _ctx: &CurveContext) -> u8 {
        self.constant_brightness()
    }

    fn calculate_color_temperature(&self, ctx: &CurveContext) -> u16 {
        self.active_profile.calculate(ctx).kelvin
    }

    fn calculate_step(
        &self,
        ctx: &CurveContext,
        _action: rhythm_profile::StepAction,
    ) -> rhythm_profile::StepResult {
        rhythm_profile::StepResult {
            values: self.calculate(ctx),
            time_offset_minutes: 0.0,
            at_boundary: true,
        }
    }

    fn is_at_maximum(&self, _ctx: &CurveContext) -> bool {
        true
    }

    fn is_at_minimum(&self, _ctx: &CurveContext) -> bool {
        true
    }

    fn min_brightness(&self) -> u8 {
        self.config.min_brightness
    }

    fn max_brightness(&self) -> u8 {
        self.config.max_brightness
    }

    fn min_color_temp(&self) -> u16 {
        self.config.min_color_temp
    }

    fn max_color_temp(&self) -> u16 {
        self.config.max_color_temp
    }
}

/// Registry for light profiles.
///
/// The registry stores profile configs as the source of truth and materializes
/// runtime profile modules on demand.
pub struct LightProfileRegistry {
    profiles: BTreeMap<String, LightProfileConfig>,
    default_profile_id: String,
    active_profile_id: String,
    mode_configs: BTreeMap<RhythmMode, ModeConfig>,
}

impl LightProfileRegistry {
    /// Create a new registry with the built-in rhythm, sleep, day idle, and sleep idle profiles.
    pub fn new() -> Self {
        Self::with_profiles(default_builtin_profiles(), RHYTHM_PROFILE_ID)
    }

    /// Create a new registry with explicit profile configs.
    ///
    /// Missing built-in configs are filled from defaults so rhythm, sleep, and
    /// both idle profiles always exist.
    pub fn with_profiles<I>(profiles: I, active_profile_id: &str) -> Self
    where
        I: IntoIterator<Item = LightProfileConfig>,
    {
        let mut registry = Self {
            profiles: BTreeMap::new(),
            default_profile_id: RHYTHM_PROFILE_ID.into(),
            active_profile_id: RHYTHM_PROFILE_ID.into(),
            mode_configs: default_mode_configs()
                .into_iter()
                .map(|config| (config.mode, config))
                .collect(),
        };

        for profile in profiles {
            registry.register_config(profile);
        }

        for builtin in default_builtin_profiles() {
            if !registry.profiles.contains_key(&builtin.id) {
                registry.register_config(builtin);
            }
        }

        if registry.contains(active_profile_id) && !is_builtin_state_profile_id(active_profile_id) {
            registry.active_profile_id = active_profile_id.into();
        }

        registry
    }

    fn default_idle_profile_for_mode(mode: RhythmMode) -> LightProfileConfig {
        match mode {
            RhythmMode::Day => default_day_idle_profile(),
            RhythmMode::Sleep => default_sleep_idle_profile(),
        }
    }

    fn profile_for_config_with_active(
        &self,
        config: &LightProfileConfig,
        active_profile: Arc<dyn LightProfileModule>,
    ) -> Arc<dyn LightProfileModule> {
        if matches!(config.curve, LightCurveShape::InheritActive) {
            Arc::new(InheritedStateProfile::new(config.clone(), active_profile))
        } else {
            Arc::new(LightProfile::new(config.clone()))
        }
    }

    fn profile_for_config(&self, config: &LightProfileConfig) -> Arc<dyn LightProfileModule> {
        if matches!(config.curve, LightCurveShape::InheritActive) {
            let active_config =
                self.active_profile_config_for_room_settings(self.active_mode(), None);
            self.profile_for_config_with_active(config, Arc::new(LightProfile::new(active_config)))
        } else {
            Arc::new(LightProfile::new(config.clone()))
        }
    }

    fn mode_config_or_default(&self, mode: RhythmMode) -> ModeConfig {
        self.mode_configs
            .get(&mode)
            .cloned()
            .unwrap_or_else(|| ModeConfig::default_for_mode(mode))
    }

    fn active_profile_id_for_mode(&self, mode: RhythmMode) -> String {
        let current_active = self.active_profile_id();
        if RhythmMode::from_profile_id(current_active) == mode {
            current_active.to_string()
        } else {
            self.mode_config_or_default(mode)
                .active_profile_id
                .unwrap_or_else(|| mode.default_active_profile_id().to_string())
        }
    }

    fn active_profile_config_for_room_settings(
        &self,
        mode: RhythmMode,
        settings: Option<&RoomProfileSettings>,
    ) -> LightProfileConfig {
        let active_profile_id = self.active_profile_id_for_mode(mode);
        let requested_id = settings
            .map(|settings| settings.resolved_profile_id(&active_profile_id))
            .unwrap_or(active_profile_id.as_str());
        let base_id = if self.contains(requested_id) && !is_builtin_state_profile_id(requested_id) {
            requested_id
        } else {
            active_profile_id.as_str()
        };

        let mut config = self
            .profile_config_cloned(base_id)
            .or_else(|| self.profile_config_cloned(active_profile_id.as_str()))
            .unwrap_or_else(|| {
                if mode == RhythmMode::Sleep {
                    default_sleep_profile()
                } else {
                    default_rhythm_profile()
                }
            });

        if let Some(settings) = settings {
            settings.apply_to_config(&mut config);
        }

        config
    }

    /// Register or replace a profile config.
    pub fn register_config(&mut self, config: LightProfileConfig) {
        self.profiles.insert(config.id.clone(), config);
    }

    /// Unregister a profile by ID.
    ///
    /// Returns false when attempting to remove the active, default, or built-in state profile.
    pub fn unregister(&mut self, id: &str) -> bool {
        if id == self.active_profile_id
            || id == self.default_profile_id
            || is_builtin_state_profile_id(id)
        {
            return false;
        }
        self.profiles.remove(id).is_some()
    }

    /// Get a runtime profile by ID.
    pub fn get(&self, id: &str) -> Option<Arc<dyn LightProfileModule>> {
        self.profiles
            .get(id)
            .map(|config| self.profile_for_config(config))
    }

    /// Get a stored profile config by ID.
    pub fn profile_config(&self, id: &str) -> Option<&LightProfileConfig> {
        self.profiles.get(id)
    }

    /// Get a cloned profile config by ID.
    pub fn profile_config_cloned(&self, id: &str) -> Option<LightProfileConfig> {
        self.profiles.get(id).cloned()
    }

    /// Get all stored profile configs.
    pub fn profile_configs(&self) -> Vec<LightProfileConfig> {
        self.profiles.values().cloned().collect()
    }

    /// Replace an existing profile config.
    ///
    /// Returns false when the profile ID is unknown.
    pub fn set_profile_config(&mut self, config: LightProfileConfig) -> bool {
        if !self.profiles.contains_key(&config.id) {
            return false;
        }
        self.profiles.insert(config.id.clone(), config);
        true
    }

    /// Get the currently active profile.
    pub fn active_profile(&self) -> Arc<dyn LightProfileModule> {
        self.profiles
            .get(&self.active_profile_id)
            .map(|config| self.profile_for_config(config))
            .expect("Active profile must exist in registry")
    }

    /// Resolve a profile module for a room state inside the given high-level mode.
    ///
    /// `active` continues to honor the room's selected profile override and timer
    /// overrides. Other states resolve through the mode mapping and only inherit
    /// the active room profile when the target profile itself is `inherit-active`.
    pub fn profile_for_room_state(
        &self,
        mode: RhythmMode,
        state: RoomModeState,
        settings: Option<&RoomProfileSettings>,
    ) -> Arc<dyn LightProfileModule> {
        let active_profile_id = self.active_profile_id_for_mode(mode);
        let active_config = self.active_profile_config_for_room_settings(mode, settings);
        let active_profile = Arc::new(LightProfile::new(active_config.clone()));

        if state == RoomModeState::Active {
            return active_profile;
        }

        let mode_config = self.mode_config_or_default(mode);
        let state_config = mode_config
            .resolve_state_profile_id(state, active_profile_id.as_str())
            .and_then(|target_id| self.profile_config_cloned(target_id))
            .unwrap_or_else(|| {
                if matches!(state, RoomModeState::Idle | RoomModeState::HardOff) {
                    Self::default_idle_profile_for_mode(mode)
                } else {
                    active_config.clone()
                }
            });

        self.profile_for_config_with_active(&state_config, active_profile)
    }

    /// Replace the mode/state profile mappings.
    ///
    /// Missing modes fall back to their built-in defaults.
    pub fn set_mode_configs<I>(&mut self, configs: I)
    where
        I: IntoIterator<Item = ModeConfig>,
    {
        self.mode_configs = default_mode_configs()
            .into_iter()
            .map(|config| (config.mode, config))
            .collect();
        for mut config in configs {
            config.normalize_profile_ids();
            self.mode_configs.insert(config.mode, config);
        }
    }

    /// Get the stored mode/state profile mappings in stable mode order.
    pub fn mode_configs(&self) -> Vec<ModeConfig> {
        RhythmMode::ALL
            .into_iter()
            .map(|mode| self.mode_config_or_default(mode))
            .collect()
    }

    /// Get the active high-level mode derived from the current active profile.
    pub fn active_mode(&self) -> RhythmMode {
        RhythmMode::from_profile_id(self.active_profile_id())
    }

    /// Get the ID of the active profile.
    pub fn active_profile_id(&self) -> &str {
        &self.active_profile_id
    }

    /// Set the active profile by ID.
    ///
    /// Built-in state profiles cannot be activated directly.
    pub fn set_active_profile(&mut self, id: &str) -> bool {
        if is_builtin_state_profile_id(id) {
            return false;
        }
        if self.profiles.contains_key(id) {
            self.active_profile_id = id.to_string();
            true
        } else {
            false
        }
    }

    /// Reset to the default active profile.
    pub fn reset_to_default(&mut self) {
        self.active_profile_id = self.default_profile_id.clone();
    }

    /// Get the ID of the default profile.
    pub fn default_profile_id(&self) -> &str {
        &self.default_profile_id
    }

    /// Set the default profile ID.
    ///
    /// Built-in state profiles cannot be used as the default active profile.
    pub fn set_default_profile(&mut self, id: &str) -> bool {
        if is_builtin_state_profile_id(id) {
            return false;
        }
        if self.profiles.contains_key(id) {
            self.default_profile_id = id.to_string();
            true
        } else {
            false
        }
    }

    /// Get the active-selectable profiles as `(id, name)` pairs.
    pub fn available_profiles(&self) -> Vec<(&str, &str)> {
        self.profiles
            .values()
            .filter(|profile| !is_builtin_state_profile_id(&profile.id))
            .map(|profile| (profile.id.as_str(), profile.name.as_str()))
            .collect()
    }

    /// Get the number of registered profiles, including built-in state profiles.
    pub fn profile_count(&self) -> usize {
        self.profiles.len()
    }

    /// Check whether a profile ID is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.profiles.contains_key(id)
    }

    /// Get the rhythm profile if it exists.
    pub fn rhythm_profile(&self) -> Option<Arc<dyn LightProfileModule>> {
        self.get(RHYTHM_PROFILE_ID)
    }

    /// Check if the sleep profile is currently active.
    pub fn is_sleep_active(&self) -> bool {
        self.active_profile_id == SLEEP_PROFILE_ID
    }
}

impl Default for LightProfileRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for LightProfileRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LightProfileRegistry")
            .field("profile_count", &self.profiles.len())
            .field("default_profile_id", &self.default_profile_id)
            .field("active_profile_id", &self.active_profile_id)
            .field("mode_configs", &self.mode_configs)
            .field("profiles", &self.profiles.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::super::defaults::{
        DAY_IDLE_PROFILE_ID, DAY_IDLE_PROFILE_NAME, RHYTHM_PROFILE_NAME, SLEEP_IDLE_PROFILE_ID,
        SLEEP_IDLE_PROFILE_NAME, SLEEP_PROFILE_NAME,
    };
    use super::*;
    use crate::{LightDirectColor, RoomModeState, TimerSetting};

    fn test_context(hour: f32) -> CurveContext {
        CurveContext::new(hour, crate::SolarTime::new(12.0, 35.0, 172), None)
    }

    #[test]
    fn registry_new_registers_builtin_profiles() {
        let registry = LightProfileRegistry::new();

        assert_eq!(registry.profile_count(), 4);
        assert!(registry.contains(RHYTHM_PROFILE_ID));
        assert!(registry.contains(SLEEP_PROFILE_ID));
        assert!(registry.contains(DAY_IDLE_PROFILE_ID));
        assert!(registry.contains(SLEEP_IDLE_PROFILE_ID));
        assert_eq!(registry.active_profile_id(), RHYTHM_PROFILE_ID);
        assert_eq!(registry.default_profile_id(), RHYTHM_PROFILE_ID);
    }

    #[test]
    fn registry_with_profiles_replaces_rhythm_defaults() {
        let mut config = default_rhythm_profile();
        config.min_brightness = 10;
        config.max_brightness = 90;

        let registry = LightProfileRegistry::with_profiles(vec![config], RHYTHM_PROFILE_ID);
        let profile = registry.active_profile();

        assert_eq!(profile.min_brightness(), 10);
        assert_eq!(profile.max_brightness(), 90);
    }

    #[test]
    fn available_profiles_excludes_state_profiles() {
        let registry = LightProfileRegistry::new();
        let profiles = registry.available_profiles();

        assert_eq!(profiles.len(), 2);
        assert!(profiles
            .iter()
            .any(|(id, name)| *id == RHYTHM_PROFILE_ID && *name == RHYTHM_PROFILE_NAME));
        assert!(profiles
            .iter()
            .any(|(id, name)| *id == SLEEP_PROFILE_ID && *name == SLEEP_PROFILE_NAME));
        assert!(profiles.iter().all(|(id, _)| *id != DAY_IDLE_PROFILE_ID));
        assert!(profiles.iter().all(|(id, _)| *id != SLEEP_IDLE_PROFILE_ID));
    }

    #[test]
    fn state_profiles_are_accessible_but_not_selectable() {
        let mut registry = LightProfileRegistry::new();

        let day_idle = registry.get(DAY_IDLE_PROFILE_ID).unwrap();
        let sleep_idle = registry.get(SLEEP_IDLE_PROFILE_ID).unwrap();
        assert_eq!(day_idle.id(), DAY_IDLE_PROFILE_ID);
        assert_eq!(day_idle.name(), DAY_IDLE_PROFILE_NAME);
        assert_eq!(sleep_idle.id(), SLEEP_IDLE_PROFILE_ID);
        assert_eq!(sleep_idle.name(), SLEEP_IDLE_PROFILE_NAME);
        assert!(!registry.set_active_profile(DAY_IDLE_PROFILE_ID));
        assert!(!registry.set_active_profile(SLEEP_IDLE_PROFILE_ID));
        assert_eq!(registry.active_profile_id(), RHYTHM_PROFILE_ID);
    }

    #[test]
    fn set_profile_config_preserves_rhythm_identity() {
        let mut registry = LightProfileRegistry::new();
        let mut config = registry.profile_config_cloned(RHYTHM_PROFILE_ID).unwrap();
        config.min_brightness = 12;
        assert!(registry.set_profile_config(config));

        let profile = registry.rhythm_profile().unwrap();
        assert_eq!(profile.id(), RHYTHM_PROFILE_ID);
        assert_eq!(profile.name(), RHYTHM_PROFILE_NAME);
        assert_eq!(profile.min_brightness(), 12);
    }

    #[test]
    fn day_idle_inherit_active_uses_active_profile_color() {
        let registry = LightProfileRegistry::new();
        let ctx = test_context(12.0);

        let active = registry.active_profile().calculate(&ctx);
        let idle = registry
            .profile_for_room_state(RhythmMode::Day, RoomModeState::Idle, None)
            .calculate(&ctx);

        assert_eq!(idle.brightness, 1);
        assert_eq!(idle.rgb, active.rgb);
        assert_eq!(idle.xy, active.xy);
        assert_eq!(idle.kelvin, active.kelvin);
        assert_eq!(idle.is_direct_color, active.is_direct_color);
    }

    #[test]
    fn sleep_idle_inherits_active_sleep_profile_color() {
        let mut registry = LightProfileRegistry::new();
        let mut sleep = registry.profile_config_cloned(SLEEP_PROFILE_ID).unwrap();
        sleep.curve = LightCurveShape::Constant {
            brightness: 1.0,
            color_temp: 0.0,
            direct_color: Some(LightDirectColor {
                xy: crate::rgb_to_xy(crate::Rgb::new(38, 82, 255)),
                rgb: crate::Rgb::new(38, 82, 255),
            }),
        };
        assert!(registry.set_profile_config(sleep));

        let ctx = test_context(12.0);
        let idle = registry
            .profile_for_room_state(RhythmMode::Sleep, RoomModeState::Idle, None)
            .calculate(&ctx);

        assert_eq!(idle.brightness, 1);
        assert_eq!(idle.rgb, crate::Rgb::new(38, 82, 255));
        assert_eq!(idle.kelvin, 0);
        assert!(idle.is_direct_color);
    }

    #[test]
    fn mode_configs_default_to_day_and_sleep_mappings() {
        let registry = LightProfileRegistry::new();
        let configs = registry.mode_configs();

        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].mode, RhythmMode::Day);
        assert_eq!(
            configs[0].active_profile_id.as_deref(),
            Some(RHYTHM_PROFILE_ID)
        );
        assert_eq!(configs[0].idle_profile_id, None);
        assert_eq!(configs[1].mode, RhythmMode::Sleep);
        assert_eq!(
            configs[1].active_profile_id.as_deref(),
            Some(SLEEP_PROFILE_ID)
        );
        assert_eq!(configs[1].idle_profile_id, None);
    }

    #[test]
    fn profile_for_room_state_idle_inherits_room_specific_active_profile() {
        let mut registry = LightProfileRegistry::new();
        let mut day_alt = default_rhythm_profile();
        day_alt.id = "day_alt".into();
        day_alt.min_brightness = 20;
        day_alt.max_brightness = 20;
        day_alt.fade_ms = TimerSetting::Fixed { value: 321 };
        registry.register_config(day_alt);

        let settings = RoomProfileSettings {
            profile_id: Some("day_alt".into()),
            ..RoomProfileSettings::default()
        };

        let active = registry.profile_for_room_state(
            RhythmMode::Day,
            RoomModeState::Active,
            Some(&settings),
        );
        let idle =
            registry.profile_for_room_state(RhythmMode::Day, RoomModeState::Idle, Some(&settings));
        let ctx = test_context(12.0);
        let active_values = active.calculate(&ctx);
        let idle_values = idle.calculate(&ctx);

        assert_eq!(active_values.brightness, 20);
        assert_eq!(idle_values.rgb, active_values.rgb);
        assert_eq!(idle_values.xy, active_values.xy);
        assert_eq!(idle_values.brightness, 1);
        assert_eq!(idle_values.transition_ms, DEFAULT_FADE_MS as u32);
    }

    #[test]
    fn legacy_idle_mapping_normalizes_to_synthesized_fallback() {
        let mut registry = LightProfileRegistry::new();
        let ctx = test_context(12.0);
        let active = registry.active_profile().calculate(&ctx);

        let mut day_idle = registry.profile_config_cloned(DAY_IDLE_PROFILE_ID).unwrap();
        day_idle.curve = LightCurveShape::Constant {
            brightness: 1.0,
            color_temp: 0.0,
            direct_color: Some(LightDirectColor {
                xy: crate::rgb_to_xy(crate::Rgb::new(38, 82, 255)),
                rgb: crate::Rgb::new(38, 82, 255),
            }),
        };
        day_idle.min_brightness = 1;
        day_idle.max_brightness = 1;
        assert!(registry.set_profile_config(day_idle));

        registry.set_mode_configs(vec![ModeConfig {
            mode: RhythmMode::Day,
            active_profile_id: Some(RHYTHM_PROFILE_ID.into()),
            idle_profile_id: Some("idle".into()),
            wake_profile_id: None,
            warning_profile_id: None,
            room_defaults: vec![],
        }]);

        let idle = registry
            .profile_for_room_state(RhythmMode::Day, RoomModeState::Idle, None)
            .calculate(&ctx);

        assert_eq!(idle.brightness, 1);
        assert_eq!(idle.rgb, active.rgb);
        assert_eq!(idle.xy, active.xy);
        assert_eq!(idle.is_direct_color, active.is_direct_color);
    }

    #[test]
    fn null_idle_mapping_ignores_custom_day_idle_profile() {
        let mut registry = LightProfileRegistry::new();
        let ctx = test_context(12.0);
        let active = registry.active_profile().calculate(&ctx);

        let mut day_idle = registry.profile_config_cloned(DAY_IDLE_PROFILE_ID).unwrap();
        day_idle.curve = LightCurveShape::Constant {
            brightness: 1.0,
            color_temp: 0.0,
            direct_color: Some(LightDirectColor {
                xy: crate::rgb_to_xy(crate::Rgb::new(38, 82, 255)),
                rgb: crate::Rgb::new(38, 82, 255),
            }),
        };
        day_idle.min_brightness = 1;
        day_idle.max_brightness = 1;
        assert!(registry.set_profile_config(day_idle));

        registry.set_mode_configs(vec![ModeConfig {
            mode: RhythmMode::Day,
            active_profile_id: Some(RHYTHM_PROFILE_ID.into()),
            idle_profile_id: None,
            wake_profile_id: None,
            warning_profile_id: None,
            room_defaults: vec![],
        }]);

        let idle = registry
            .profile_for_room_state(RhythmMode::Day, RoomModeState::Idle, None)
            .calculate(&ctx);

        assert_eq!(idle.brightness, 1);
        assert_eq!(idle.rgb, active.rgb);
        assert_eq!(idle.xy, active.xy);
        assert_ne!(idle.rgb, crate::Rgb::new(38, 82, 255));
    }

    #[test]
    fn cannot_unregister_reserved_profiles() {
        let mut registry = LightProfileRegistry::new();
        assert!(!registry.unregister(RHYTHM_PROFILE_ID));
        assert!(!registry.unregister(DAY_IDLE_PROFILE_ID));
        assert!(!registry.unregister(SLEEP_IDLE_PROFILE_ID));
    }

    #[test]
    fn debug_lists_registry_shape() {
        let registry = LightProfileRegistry::new();
        let debug_str = format!("{registry:?}");
        assert!(debug_str.contains("LightProfileRegistry"));
        assert!(debug_str.contains("day_idle"));
        assert!(debug_str.contains("sleep_idle"));
    }
}
