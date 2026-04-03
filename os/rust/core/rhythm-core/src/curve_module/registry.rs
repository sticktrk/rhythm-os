//! Curve module registry for managing available modules.
//!
//! The registry provides a central place to register, retrieve, and
//! switch between different curve module implementations.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{LightCurveModule, RhythmCurveModule};
use crate::config::CurveConfig;

/// Registry for curve modules.
///
/// Manages available curve modules and tracks the currently active module.
/// By default, registers the `RhythmCurveModule` as the default.
///
/// # Example
///
/// ```
/// use rhythm_core::curve_module::{CurveModuleRegistry, RhythmCurveModule};
///
/// let mut registry = CurveModuleRegistry::new();
///
/// // Get available modules
/// for (id, name) in registry.available_modules() {
///     println!("{}: {}", id, name);
/// }
///
/// // Get the active module
/// let module = registry.active_module();
/// println!("Active: {}", module.name());
/// ```
pub struct CurveModuleRegistry {
    modules: BTreeMap<String, Arc<dyn LightCurveModule>>,
    default_module_id: String,
    active_module_id: String,
}

impl CurveModuleRegistry {
    /// Create a new registry with the default RhythmCurveModule.
    pub fn new() -> Self {
        let mut registry = Self {
            modules: BTreeMap::new(),
            default_module_id: RhythmCurveModule::ID.into(),
            active_module_id: RhythmCurveModule::ID.into(),
        };

        // Register built-in modules
        registry.register(Arc::new(RhythmCurveModule::with_defaults()));

        registry
    }

    /// Create a new registry with custom default configuration.
    pub fn with_config(config: CurveConfig) -> Self {
        let mut registry = Self {
            modules: BTreeMap::new(),
            default_module_id: RhythmCurveModule::ID.into(),
            active_module_id: RhythmCurveModule::ID.into(),
        };

        // Register built-in modules
        registry.register(Arc::new(RhythmCurveModule::new(config)));

        registry
    }

    /// Register a new curve module.
    ///
    /// If a module with the same ID already exists, it will be replaced.
    pub fn register(&mut self, module: Arc<dyn LightCurveModule>) {
        let id = module.id().to_string();
        self.modules.insert(id, module);
    }

    /// Unregister a module by ID.
    ///
    /// Returns true if the module was found and removed.
    /// Cannot remove the currently active module or the default module.
    pub fn unregister(&mut self, id: &str) -> bool {
        if id == self.active_module_id || id == self.default_module_id {
            return false;
        }
        self.modules.remove(id).is_some()
    }

    /// Get a module by ID.
    pub fn get(&self, id: &str) -> Option<Arc<dyn LightCurveModule>> {
        self.modules.get(id).cloned()
    }

    /// Get the currently active module.
    pub fn active_module(&self) -> Arc<dyn LightCurveModule> {
        self.modules
            .get(&self.active_module_id)
            .cloned()
            .expect("Active module must exist in registry")
    }

    /// Get the ID of the currently active module.
    pub fn active_module_id(&self) -> &str {
        &self.active_module_id
    }

    /// Set the active module by ID.
    ///
    /// Returns true if the module was found and set as active.
    pub fn set_active_module(&mut self, id: &str) -> bool {
        if self.modules.contains_key(id) {
            self.active_module_id = id.to_string();
            true
        } else {
            false
        }
    }

    /// Reset to the default module.
    pub fn reset_to_default(&mut self) {
        self.active_module_id = self.default_module_id.clone();
    }

    /// Get the ID of the default module.
    pub fn default_module_id(&self) -> &str {
        &self.default_module_id
    }

    /// Set a new default module ID.
    ///
    /// Returns true if the module exists in the registry.
    pub fn set_default_module(&mut self, id: &str) -> bool {
        if self.modules.contains_key(id) {
            self.default_module_id = id.to_string();
            true
        } else {
            false
        }
    }

    /// Get a list of all available modules as (id, name) pairs.
    pub fn available_modules(&self) -> Vec<(&str, &str)> {
        self.modules
            .values()
            .map(|m: &Arc<dyn LightCurveModule>| (m.id(), m.name()))
            .collect()
    }

    /// Get the number of registered modules.
    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    /// Check if a module with the given ID is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.modules.contains_key(id)
    }

    /// Update the configuration of the rhythm module.
    ///
    /// This is a convenience method for the common case of updating
    /// the default rhythm module configuration.
    pub fn update_rhythm_config(&mut self, config: CurveConfig) {
        self.register(Arc::new(RhythmCurveModule::new(config)));
    }

    /// Get the rhythm module if it exists.
    ///
    /// This is a convenience method for accessing the rhythm module
    /// configuration.
    pub fn rhythm_module(&self) -> Option<Arc<dyn LightCurveModule>> {
        self.get(RhythmCurveModule::ID)
    }
}

impl Default for CurveModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for CurveModuleRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CurveModuleRegistry")
            .field("module_count", &self.modules.len())
            .field("default_module_id", &self.default_module_id)
            .field("active_module_id", &self.active_module_id)
            .field("modules", &self.modules.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new() {
        let registry = CurveModuleRegistry::new();

        assert_eq!(registry.module_count(), 1);
        assert!(registry.contains(RhythmCurveModule::ID));
        assert_eq!(registry.active_module_id(), RhythmCurveModule::ID);
        assert_eq!(registry.default_module_id(), RhythmCurveModule::ID);
    }

    #[test]
    fn test_registry_with_config() {
        let config = CurveConfig {
            min_brightness: 10,
            max_brightness: 90,
            ..Default::default()
        };

        let registry = CurveModuleRegistry::with_config(config);
        let module = registry.active_module();

        assert_eq!(module.min_brightness(), 10);
        assert_eq!(module.max_brightness(), 90);
    }

    #[test]
    fn test_active_module() {
        let registry = CurveModuleRegistry::new();
        let module = registry.active_module();

        assert_eq!(module.id(), RhythmCurveModule::ID);
        assert_eq!(module.name(), RhythmCurveModule::NAME);
    }

    #[test]
    fn test_available_modules() {
        let registry = CurveModuleRegistry::new();
        let modules = registry.available_modules();

        assert_eq!(modules.len(), 1);
        assert!(modules.iter().any(|(id, _)| *id == RhythmCurveModule::ID));
    }

    #[test]
    fn test_set_active_module() {
        let mut registry = CurveModuleRegistry::new();

        // Try to set an unknown module
        assert!(!registry.set_active_module("unknown"));
        assert_eq!(registry.active_module_id(), RhythmCurveModule::ID);

        // Set the existing module
        assert!(registry.set_active_module(RhythmCurveModule::ID));
        assert_eq!(registry.active_module_id(), RhythmCurveModule::ID);
    }

    #[test]
    fn test_register_and_unregister() {
        let mut registry = CurveModuleRegistry::new();

        // Register a second module (using rhythm with different config)
        let _custom_module = RhythmCurveModule::new(CurveConfig {
            min_brightness: 5,
            ..Default::default()
        });

        // For testing, we'll use a mock with a different ID
        struct TestModule;
        impl LightCurveModule for TestModule {
            fn id(&self) -> &str {
                "test"
            }
            fn name(&self) -> &str {
                "Test Module"
            }
            fn calculate(
                &self,
                _ctx: &super::super::CurveContext,
            ) -> crate::adaptive::LightingValues {
                crate::adaptive::LightingValues::new(3000, 50, 12.0, 0.0, 500, 600)
            }
            fn calculate_brightness(&self, _ctx: &super::super::CurveContext) -> u8 {
                50
            }
            fn calculate_color_temperature(&self, _ctx: &super::super::CurveContext) -> u16 {
                3000
            }
            fn calculate_step(
                &self,
                ctx: &super::super::CurveContext,
                _action: crate::steps::StepAction,
            ) -> crate::steps::StepResult {
                crate::steps::StepResult {
                    values: self.calculate(ctx),
                    time_offset_minutes: 0.0,
                    at_boundary: false,
                }
            }
            fn is_at_maximum(&self, _ctx: &super::super::CurveContext) -> bool {
                false
            }
            fn is_at_minimum(&self, _ctx: &super::super::CurveContext) -> bool {
                false
            }
            fn min_brightness(&self) -> u8 {
                1
            }
            fn max_brightness(&self) -> u8 {
                100
            }
            fn min_color_temp(&self) -> u16 {
                500
            }
            fn max_color_temp(&self) -> u16 {
                6500
            }
        }

        registry.register(Arc::new(TestModule));
        assert_eq!(registry.module_count(), 2); // rhythm + test
        assert!(registry.contains("test"));

        // Set test as active
        assert!(registry.set_active_module("test"));
        assert_eq!(registry.active_module_id(), "test");

        // Can't unregister active module
        assert!(!registry.unregister("test"));

        // Switch back and unregister
        registry.set_active_module(RhythmCurveModule::ID);
        assert!(registry.unregister("test"));
        assert_eq!(registry.module_count(), 1);
    }

    #[test]
    fn test_reset_to_default() {
        let mut registry = CurveModuleRegistry::new();

        // Register and activate a test module
        struct TestModule;
        impl LightCurveModule for TestModule {
            fn id(&self) -> &str {
                "test"
            }
            fn name(&self) -> &str {
                "Test"
            }
            fn calculate(
                &self,
                _ctx: &super::super::CurveContext,
            ) -> crate::adaptive::LightingValues {
                crate::adaptive::LightingValues::new(3000, 50, 12.0, 0.0, 500, 600)
            }
            fn calculate_brightness(&self, _ctx: &super::super::CurveContext) -> u8 {
                50
            }
            fn calculate_color_temperature(&self, _ctx: &super::super::CurveContext) -> u16 {
                3000
            }
            fn calculate_step(
                &self,
                ctx: &super::super::CurveContext,
                _action: crate::steps::StepAction,
            ) -> crate::steps::StepResult {
                crate::steps::StepResult {
                    values: self.calculate(ctx),
                    time_offset_minutes: 0.0,
                    at_boundary: false,
                }
            }
            fn is_at_maximum(&self, _ctx: &super::super::CurveContext) -> bool {
                false
            }
            fn is_at_minimum(&self, _ctx: &super::super::CurveContext) -> bool {
                false
            }
            fn min_brightness(&self) -> u8 {
                1
            }
            fn max_brightness(&self) -> u8 {
                100
            }
            fn min_color_temp(&self) -> u16 {
                500
            }
            fn max_color_temp(&self) -> u16 {
                6500
            }
        }

        registry.register(Arc::new(TestModule));
        registry.set_active_module("test");
        assert_eq!(registry.active_module_id(), "test");

        // Reset to default
        registry.reset_to_default();
        assert_eq!(registry.active_module_id(), RhythmCurveModule::ID);
    }

    #[test]
    fn test_update_rhythm_config() {
        let mut registry = CurveModuleRegistry::new();

        // Initial brightness range uses defaults
        let module = registry.active_module();
        assert_eq!(
            module.min_brightness(),
            crate::config::DEFAULT_MIN_BRIGHTNESS
        );

        // Update config
        registry.update_rhythm_config(CurveConfig {
            min_brightness: 15,
            max_brightness: 85,
            ..Default::default()
        });

        // Check new config
        let module = registry.active_module();
        assert_eq!(module.min_brightness(), 15);
        assert_eq!(module.max_brightness(), 85);
    }

    #[test]
    fn test_cannot_unregister_default() {
        let mut registry = CurveModuleRegistry::new();

        // Cannot unregister the default module
        assert!(!registry.unregister(RhythmCurveModule::ID));
        assert!(registry.contains(RhythmCurveModule::ID));
    }

    #[test]
    fn test_debug_impl() {
        let registry = CurveModuleRegistry::new();
        let debug_str = format!("{:?}", registry);

        assert!(debug_str.contains("CurveModuleRegistry"));
        assert!(debug_str.contains("module_count"));
        assert!(debug_str.contains("rhythm"));
    }
}
