//! Light profile registry for managing active and idle profiles.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use rhythm_profile::profile_config::DEFAULT_FADE_MS;
use rhythm_profile::{CurveContext, LightCurveShape, LightProfileConfig, LightingValues};

use super::defaults::{
    default_idle_profile, default_rhythm_profile, default_sleep_profile, IDLE_PROFILE_ID,
    RHYTHM_PROFILE_ID, SLEEP_PROFILE_ID,
};
use super::{LightProfile, LightProfileModule};

#[derive(Clone)]
struct InheritedIdleProfile {
    config: LightProfileConfig,
    active_profile: Arc<dyn LightProfileModule>,
}

impl InheritedIdleProfile {
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

impl LightProfileModule for InheritedIdleProfile {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    fn calculate(&self, ctx: &CurveContext) -> LightingValues {
        let mut values = self.active_profile.calculate(ctx);
        values.brightness = self.constant_brightness();
        values.kelvin = 0;
        values.is_direct_color = true;
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
    idle_profile_id: String,
}

impl LightProfileRegistry {
    /// Create a new registry with the built-in rhythm, sleep, and idle profiles.
    pub fn new() -> Self {
        Self::with_profiles(
            vec![
                default_rhythm_profile(),
                default_sleep_profile(),
                default_idle_profile(),
            ],
            RHYTHM_PROFILE_ID,
        )
    }

    /// Create a new registry with explicit profile configs.
    ///
    /// Missing built-in configs are filled from defaults so rhythm, sleep, and
    /// idle always exist.
    pub fn with_profiles<I>(profiles: I, active_profile_id: &str) -> Self
    where
        I: IntoIterator<Item = LightProfileConfig>,
    {
        let mut registry = Self {
            profiles: BTreeMap::new(),
            default_profile_id: RHYTHM_PROFILE_ID.into(),
            active_profile_id: RHYTHM_PROFILE_ID.into(),
            idle_profile_id: IDLE_PROFILE_ID.into(),
        };

        for profile in profiles {
            registry.register_config(profile);
        }

        for builtin in [
            default_rhythm_profile(),
            default_sleep_profile(),
            default_idle_profile(),
        ] {
            if !registry.profiles.contains_key(&builtin.id) {
                registry.register_config(builtin);
            }
        }

        if registry.contains(active_profile_id) && active_profile_id != registry.idle_profile_id {
            registry.active_profile_id = active_profile_id.into();
        }

        registry
    }

    fn profile_for_config(&self, config: &LightProfileConfig) -> Arc<dyn LightProfileModule> {
        if config.id == self.idle_profile_id
            && matches!(config.curve, LightCurveShape::InheritActive)
        {
            Arc::new(InheritedIdleProfile::new(
                config.clone(),
                self.active_profile(),
            ))
        } else {
            Arc::new(LightProfile::new(config.clone()))
        }
    }

    /// Register or replace a profile config.
    pub fn register_config(&mut self, config: LightProfileConfig) {
        self.profiles.insert(config.id.clone(), config);
    }

    /// Unregister a profile by ID.
    ///
    /// Returns false when attempting to remove the active, default, or idle profile.
    pub fn unregister(&mut self, id: &str) -> bool {
        if id == self.active_profile_id
            || id == self.default_profile_id
            || id == self.idle_profile_id
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

    /// Get the dedicated idle profile.
    pub fn idle_profile(&self) -> Arc<dyn LightProfileModule> {
        self.profiles
            .get(&self.idle_profile_id)
            .map(|config| self.profile_for_config(config))
            .expect("Idle profile must exist in registry")
    }

    /// Get the ID of the active profile.
    pub fn active_profile_id(&self) -> &str {
        &self.active_profile_id
    }

    /// Get the ID of the idle profile.
    pub fn idle_profile_id(&self) -> &str {
        &self.idle_profile_id
    }

    /// Set the active profile by ID.
    ///
    /// The idle profile cannot be activated directly.
    pub fn set_active_profile(&mut self, id: &str) -> bool {
        if id == self.idle_profile_id {
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
    /// The idle profile cannot be used as the default active profile.
    pub fn set_default_profile(&mut self, id: &str) -> bool {
        if id == self.idle_profile_id {
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
            .filter(|profile| profile.id != self.idle_profile_id)
            .map(|profile| (profile.id.as_str(), profile.name.as_str()))
            .collect()
    }

    /// Get the number of registered profiles, including the idle profile.
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
            .field("idle_profile_id", &self.idle_profile_id)
            .field("profiles", &self.profiles.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::super::defaults::{IDLE_PROFILE_NAME, RHYTHM_PROFILE_NAME, SLEEP_PROFILE_NAME};
    use super::*;

    fn test_context(hour: f32) -> CurveContext {
        CurveContext::new(hour, crate::SolarTime::new(12.0, 35.0, 172), None)
    }

    #[test]
    fn registry_new_registers_builtin_profiles() {
        let registry = LightProfileRegistry::new();

        assert_eq!(registry.profile_count(), 3);
        assert!(registry.contains(RHYTHM_PROFILE_ID));
        assert!(registry.contains(SLEEP_PROFILE_ID));
        assert!(registry.contains(IDLE_PROFILE_ID));
        assert_eq!(registry.active_profile_id(), RHYTHM_PROFILE_ID);
        assert_eq!(registry.default_profile_id(), RHYTHM_PROFILE_ID);
        assert_eq!(registry.idle_profile_id(), IDLE_PROFILE_ID);
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
    fn available_profiles_excludes_idle_profile() {
        let registry = LightProfileRegistry::new();
        let profiles = registry.available_profiles();

        assert_eq!(profiles.len(), 2);
        assert!(profiles
            .iter()
            .any(|(id, name)| *id == RHYTHM_PROFILE_ID && *name == RHYTHM_PROFILE_NAME));
        assert!(profiles
            .iter()
            .any(|(id, name)| *id == SLEEP_PROFILE_ID && *name == SLEEP_PROFILE_NAME));
        assert!(profiles.iter().all(|(id, _)| *id != IDLE_PROFILE_ID));
    }

    #[test]
    fn idle_profile_is_accessible_but_not_selectable() {
        let mut registry = LightProfileRegistry::new();

        let idle = registry.idle_profile();
        assert_eq!(idle.id(), IDLE_PROFILE_ID);
        assert_eq!(idle.name(), IDLE_PROFILE_NAME);
        assert!(!registry.set_active_profile(IDLE_PROFILE_ID));
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
    fn idle_inherit_active_uses_active_profile_color() {
        let registry = LightProfileRegistry::new();
        let ctx = test_context(12.0);

        let active = registry.active_profile().calculate(&ctx);
        let idle = registry.idle_profile().calculate(&ctx);

        assert_eq!(idle.brightness, 1);
        assert_eq!(idle.rgb, active.rgb);
        assert_eq!(idle.xy, active.xy);
        assert_eq!(idle.kelvin, 0);
        assert!(idle.is_direct_color);
    }

    #[test]
    fn cannot_unregister_reserved_profiles() {
        let mut registry = LightProfileRegistry::new();
        assert!(!registry.unregister(RHYTHM_PROFILE_ID));
        assert!(!registry.unregister(IDLE_PROFILE_ID));
    }

    #[test]
    fn debug_lists_registry_shape() {
        let registry = LightProfileRegistry::new();
        let debug_str = format!("{registry:?}");
        assert!(debug_str.contains("LightProfileRegistry"));
        assert!(debug_str.contains("idle_profile_id"));
    }
}
