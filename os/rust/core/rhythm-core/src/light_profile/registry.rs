//! Light profile registry for managing active and idle profiles.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use rhythm_curve::LightProfileConfig;

use super::defaults::{
    default_idle_profile, default_rhythm_profile, default_sleep_profile, IDLE_PROFILE_ID,
    RHYTHM_PROFILE_ID, SLEEP_PROFILE_ID,
};
use super::{LightProfile, LightProfileModule};
use crate::config::CurveConfig;

fn sleep_profile_from_curve_config(config: CurveConfig) -> LightProfileConfig {
    let mut profile = LightProfileConfig::from(config);
    let defaults = default_sleep_profile();
    profile.id = defaults.id;
    profile.name = defaults.name;
    profile.direct_color = defaults.direct_color;
    profile
}

/// Registry for light profiles.
///
/// The registry keeps active profiles separate from the idle soft-off profile.
pub struct LightProfileRegistry {
    profiles: BTreeMap<String, Arc<dyn LightProfileModule>>,
    default_profile_id: String,
    active_profile_id: String,
    idle_profile_id: String,
}

impl LightProfileRegistry {
    /// Create a new registry with the built-in rhythm, sleep, and idle profiles.
    pub fn new() -> Self {
        let mut registry = Self {
            profiles: BTreeMap::new(),
            default_profile_id: RHYTHM_PROFILE_ID.into(),
            active_profile_id: RHYTHM_PROFILE_ID.into(),
            idle_profile_id: IDLE_PROFILE_ID.into(),
        };

        registry.register(Arc::new(LightProfile::new(default_rhythm_profile())));
        registry.register(Arc::new(LightProfile::new(default_sleep_profile())));
        registry.register(Arc::new(LightProfile::new(default_idle_profile())));
        registry
    }

    /// Create a new registry with a custom rhythm configuration.
    pub fn with_config(config: CurveConfig) -> Self {
        let mut registry = Self {
            profiles: BTreeMap::new(),
            default_profile_id: RHYTHM_PROFILE_ID.into(),
            active_profile_id: RHYTHM_PROFILE_ID.into(),
            idle_profile_id: IDLE_PROFILE_ID.into(),
        };

        registry.register(Arc::new(LightProfile::new(LightProfileConfig::from(
            config,
        ))));
        registry.register(Arc::new(LightProfile::new(default_sleep_profile())));
        registry.register(Arc::new(LightProfile::new(default_idle_profile())));
        registry
    }

    /// Register a profile.
    pub fn register(&mut self, profile: Arc<dyn LightProfileModule>) {
        let id = profile.id().to_string();
        self.profiles.insert(id, profile);
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

    /// Get a profile by ID.
    pub fn get(&self, id: &str) -> Option<Arc<dyn LightProfileModule>> {
        self.profiles.get(id).cloned()
    }

    /// Get the currently active profile.
    pub fn active_profile(&self) -> Arc<dyn LightProfileModule> {
        self.profiles
            .get(&self.active_profile_id)
            .cloned()
            .expect("Active profile must exist in registry")
    }

    /// Get the dedicated idle profile.
    pub fn idle_profile(&self) -> Arc<dyn LightProfileModule> {
        self.profiles
            .get(&self.idle_profile_id)
            .cloned()
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
            .filter(|profile| profile.id() != self.idle_profile_id)
            .map(|profile| (profile.id(), profile.name()))
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

    /// Replace the rhythm profile using the backward-compatible `CurveConfig` shape.
    pub fn update_rhythm_config(&mut self, config: CurveConfig) {
        self.register(Arc::new(LightProfile::new(LightProfileConfig::from(
            config,
        ))));
    }

    /// Replace the sleep profile using a `CurveConfig`-shaped input.
    pub fn update_sleep_config(&mut self, config: CurveConfig) {
        self.register(Arc::new(LightProfile::new(
            sleep_profile_from_curve_config(config),
        )));
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
    fn registry_with_config_replaces_rhythm_defaults() {
        let config = CurveConfig {
            min_brightness: 10,
            max_brightness: 90,
            ..Default::default()
        };

        let registry = LightProfileRegistry::with_config(config);
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
    fn update_rhythm_config_preserves_rhythm_identity() {
        let mut registry = LightProfileRegistry::new();
        registry.update_rhythm_config(CurveConfig {
            min_brightness: 12,
            ..Default::default()
        });

        let profile = registry.rhythm_profile().unwrap();
        assert_eq!(profile.id(), RHYTHM_PROFILE_ID);
        assert_eq!(profile.name(), RHYTHM_PROFILE_NAME);
        assert_eq!(profile.min_brightness(), 12);
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
