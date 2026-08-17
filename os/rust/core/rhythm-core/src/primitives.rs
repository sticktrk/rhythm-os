//! Service primitives for Rhythm OS.
//!
//! This module provides the `RhythmEngine` which implements the core
//! service primitives (rhythm_on, rhythm_off, step_up, step_down, etc.)
//! in a platform-agnostic way.
//!
//! The engine is generic over the `LightController` trait, allowing it
//! to work with any backend (Home Assistant, Hue, ZigBee, etc.).

extern crate alloc;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::collections::HashMap;

use crate::controller::{LightControlResult, LightController};
use crate::light_profile::{
    CurveContext, LightCurveTarget, LightProfileConfig, LightProfileModule, LightProfileRegistry,
};
use crate::lighting::LightingCommand;
use crate::room::{
    EffectiveRoomState, ModeConfig, RoomManager, RoomModeState, RoomProfileSettings,
};
use crate::runtime::events::ButtonAction;
use crate::solar::{SolarTime, SunTimes};
use crate::steps::StepAction;
use crate::LightingValues;

/// Default power save mode (true = off actions turn lights fully off).
pub const DEFAULT_POWER_SAVE: bool = true;

/// Result of a single-room periodic tick.
pub enum PeriodicTickResult {
    /// Room was updated with new adaptive lighting values.
    Updated,
    /// Room skipped (not rhythm-enabled, lights off, network error, etc.).
    Skipped,
    /// An error occurred during the tick.
    Error(String),
}

pub(crate) enum PeriodicTickPlan {
    Skipped,
    RequiresLightCheck,
    Dispatch {
        command: LightingCommand,
        room_state: RoomModeState,
        profile_id: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum ManualDispatchPlan {
    TurnOn {
        source_room_id: String,
        target_id: String,
        command: LightingCommand,
    },
    TurnOff {
        target_id: String,
        transition_ms: Option<u32>,
    },
}

pub(crate) enum ManualActionPlan {
    Noop {
        turned_on: bool,
    },
    RequiresLightCheck,
    Dispatch {
        dispatch: ManualDispatchPlan,
        turned_on: bool,
    },
}

const PERIODIC_DEDUPE_MAX_SKIPS: u8 = 5;

#[derive(Clone)]
struct PeriodicCommandCacheEntry {
    command: LightingCommand,
    skipped_cycles: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PeriodicCommandCacheKey {
    source_room_id: String,
    target_id: String,
}

/// Check if solar midnight was crossed between two time checks.
///
/// This handles edge cases around clock midnight (0:00) correctly.
///
/// # Arguments
///
/// * `last_hour` - Hour at the last check (0-24)
/// * `current_hour` - Current hour (0-24)
/// * `midnight_hour` - Solar midnight hour (0-24)
///
/// # Returns
///
/// `true` if solar midnight was crossed, `false` otherwise.
pub fn crossed_solar_midnight(last_hour: f32, current_hour: f32, midnight_hour: f32) -> bool {
    // Normal case: both hours on same side of clock midnight
    if last_hour <= current_hour {
        // Time moved forward within same day
        last_hour < midnight_hour && current_hour >= midnight_hour
    } else {
        // We crossed clock midnight (e.g., 23:30 -> 0:30)
        // Check if solar midnight is in the window we crossed
        if midnight_hour > last_hour {
            // Solar midnight is after last check (before clock midnight)
            true
        } else if midnight_hour <= current_hour {
            // Solar midnight is at or before current time (after clock midnight)
            true
        } else {
            false
        }
    }
}

/// The Rhythm OS engine.
///
/// This struct combines the light-profile system with a light controller
/// to provide the service primitives (rhythm_on, step_up, etc.).
///
/// # Type Parameters
///
/// * `C` - The light controller implementation
///
/// # Example
///
/// ```ignore
/// use rhythm_core::primitives::RhythmEngine;
/// use rhythm_core::controller::NoOpController;
///
/// let controller = NoOpController::new();
/// let mut engine = RhythmEngine::new(controller);
///
/// engine.turn_on("living_room", 12.0).await.unwrap();
/// ```
pub struct RhythmEngine<C: LightController> {
    /// The light controller for sending commands.
    controller: C,

    /// Registry of available light profiles.
    profile_registry: LightProfileRegistry,

    /// Solar time reference for coordinate-based calculations.
    solar: SolarTime,

    /// Sunrise/sunset times for dynamic midpoint resolution.
    sun_times: Option<SunTimes>,

    /// Room state manager.
    rooms: RoomManager,

    /// Legacy power-save switch retained for stored settings compatibility.
    /// Off actions always turn lights fully off.
    power_save: bool,

    /// Last periodic command sent per source-room/target pair.
    periodic_command_cache: HashMap<PeriodicCommandCacheKey, PeriodicCommandCacheEntry>,
}

impl<C: LightController> RhythmEngine<C> {
    /// Create a new RhythmEngine with the given controller.
    pub fn new(controller: C) -> Self {
        Self {
            controller,
            profile_registry: LightProfileRegistry::new(),
            solar: SolarTime::default(),
            sun_times: None,
            rooms: RoomManager::new(),
            power_save: DEFAULT_POWER_SAVE,
            periodic_command_cache: HashMap::new(),
        }
    }

    /// Create a new RhythmEngine with custom profile configs.
    pub fn with_profiles(
        controller: C,
        profiles: Vec<LightProfileConfig>,
        active_profile_id: &str,
        solar: SolarTime,
    ) -> Self {
        Self {
            controller,
            profile_registry: LightProfileRegistry::with_profiles(profiles, active_profile_id),
            solar,
            sun_times: None,
            rooms: RoomManager::new(),
            power_save: DEFAULT_POWER_SAVE,
            periodic_command_cache: HashMap::new(),
        }
    }

    /// Set or replace a profile configuration.
    pub fn set_light_profile_config(&mut self, config: LightProfileConfig) -> bool {
        self.profile_registry.set_profile_config(config)
    }

    /// Replace the mode/state profile mappings.
    pub fn set_mode_configs<I>(&mut self, configs: I)
    where
        I: IntoIterator<Item = ModeConfig>,
    {
        self.profile_registry.set_mode_configs(configs);
    }

    fn node_log_label(&self, node_id: &str) -> String {
        let node_name = self.rooms.get(node_id).map(|node| node.name.as_str());
        crate::composite_controller::format_node_log_label(node_id, node_name)
    }

    /// Set the solar time reference.
    pub fn set_solar(&mut self, solar: SolarTime) {
        self.solar = solar;
    }

    /// Set the sunrise/sunset times for dynamic midpoint resolution.
    pub fn set_sun_times(&mut self, sun_times: SunTimes) {
        self.sun_times = Some(sun_times);
    }

    /// Clear the sunrise/sunset times (will use fallback values).
    pub fn clear_sun_times(&mut self) {
        self.sun_times = None;
    }

    /// Get a reference to the room manager.
    pub fn rooms(&self) -> &RoomManager {
        &self.rooms
    }

    /// Get a mutable reference to the room manager.
    pub fn rooms_mut(&mut self) -> &mut RoomManager {
        &mut self.rooms
    }

    /// Get a reference to the profile registry.
    pub fn profile_registry(&self) -> &LightProfileRegistry {
        &self.profile_registry
    }

    /// Get a mutable reference to the profile registry.
    pub fn profile_registry_mut(&mut self) -> &mut LightProfileRegistry {
        &mut self.profile_registry
    }

    /// Get the currently active light profile.
    pub fn active_profile(&self) -> Arc<dyn LightProfileModule> {
        self.profile_registry.active_profile()
    }

    /// Set the active light profile by ID.
    ///
    /// Returns true if the profile was found and set as active.
    pub fn set_light_profile(&mut self, id: &str) -> bool {
        self.profile_registry.set_active_profile(id)
    }

    /// Get a list of available active profiles as `(id, name)` pairs.
    pub fn available_profiles(&self) -> Vec<(&str, &str)> {
        self.profile_registry.available_profiles()
    }

    /// Get a reference to the controller.
    pub fn controller(&self) -> &C {
        &self.controller
    }

    /// Get whether power save mode is enabled.
    pub fn power_save(&self) -> bool {
        self.power_save
    }

    /// Get the mood brightness from the current mode's mood profile.
    pub fn idle_brightness(&self, current_hour: f32) -> u8 {
        let ctx = self.create_context(current_hour);
        self.profile_registry
            .profile_for_room_state(
                self.profile_registry.active_mode(),
                RoomModeState::Mood,
                None,
            )
            .calculate(&ctx)
            .brightness
    }

    /// Set legacy global power-save mode.
    pub fn set_power_save(&mut self, enabled: bool) -> Vec<String> {
        self.power_save = enabled;
        self.periodic_command_cache.clear();
        for room in self.rooms.iter_mut() {
            room.clear_warning_state();
        }

        Vec::new()
    }

    /// Create a curve context for the given hour.
    fn create_context(&self, current_hour: f32) -> CurveContext {
        CurveContext::new(current_hour, self.solar, self.sun_times)
    }

    fn effective_room_state(&self, room_id: &str) -> EffectiveRoomState {
        self.rooms
            .effective_state(room_id)
            .unwrap_or(EffectiveRoomState {
                rhythm_enabled: false,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                mood_active: false,
                standby_enabled: false,
                hard_off: false,
                warning_active: false,
                profile_settings: RoomProfileSettings::default(),
            })
    }

    fn effective_mood_active(&self, effective: &EffectiveRoomState) -> bool {
        effective.mood_active
    }

    fn effective_standby_active(&self, effective: &EffectiveRoomState) -> bool {
        effective.soft_off && !effective.hard_off
    }

    fn periodic_cache_key(source_room_id: &str, target_id: &str) -> PeriodicCommandCacheKey {
        PeriodicCommandCacheKey {
            source_room_id: source_room_id.to_string(),
            target_id: target_id.to_string(),
        }
    }

    fn clear_periodic_dedupe_target(&mut self, source_room_id: &str, target_id: &str) {
        self.periodic_command_cache
            .remove(&Self::periodic_cache_key(source_room_id, target_id));
    }

    fn clear_periodic_dedupe_room(&mut self, source_room_id: &str) {
        self.periodic_command_cache
            .retain(|key, _| key.source_room_id != source_room_id);
    }

    fn clear_all_periodic_dedupe(&mut self) {
        self.periodic_command_cache.clear();
    }

    pub(crate) fn invalidate_periodic_cache_for_room(&mut self, source_room_id: &str) {
        self.clear_periodic_dedupe_room(source_room_id);
    }

    pub(crate) fn record_turn_on_dispatch(
        &mut self,
        source_room_id: &str,
        target_id: &str,
        command: LightingCommand,
    ) {
        self.periodic_command_cache.insert(
            Self::periodic_cache_key(source_room_id, target_id),
            PeriodicCommandCacheEntry {
                command,
                skipped_cycles: 0,
            },
        );
    }

    pub(crate) fn plan_non_periodic_turn_on_target(
        &mut self,
        source_room_id: &str,
        target_id: &str,
        command: LightingCommand,
    ) -> ManualDispatchPlan {
        self.clear_periodic_dedupe_target(source_room_id, target_id);
        ManualDispatchPlan::TurnOn {
            source_room_id: source_room_id.to_string(),
            target_id: target_id.to_string(),
            command,
        }
    }

    pub(crate) fn plan_non_periodic_turn_on(
        &mut self,
        room_id: &str,
        command: LightingCommand,
    ) -> ManualDispatchPlan {
        self.clear_periodic_dedupe_room(room_id);
        self.plan_non_periodic_turn_on_target(room_id, room_id, command)
    }

    pub(crate) fn plan_non_periodic_turn_off_target(
        &mut self,
        source_room_id: &str,
        target_id: &str,
        transition_ms: Option<u32>,
    ) -> ManualDispatchPlan {
        self.clear_periodic_dedupe_target(source_room_id, target_id);
        ManualDispatchPlan::TurnOff {
            target_id: target_id.to_string(),
            transition_ms,
        }
    }

    pub(crate) fn plan_non_periodic_turn_off(
        &mut self,
        room_id: &str,
        transition_ms: Option<u32>,
    ) -> ManualDispatchPlan {
        self.clear_periodic_dedupe_room(room_id);
        self.plan_non_periodic_turn_off_target(room_id, room_id, transition_ms)
    }

    pub(crate) fn plan_turn_on(&mut self, room_id: &str, current_hour: f32) -> ManualDispatchPlan {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.enable_rhythm();
            room.clear_off_states();
        };
        let effective = self.effective_room_state(room_id);
        let offset_minutes = effective.time_offset_minutes;
        let brightness_offset = effective.brightness_offset;
        let profile_settings = effective.profile_settings;

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, offset_minutes);
        let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
        let command = Self::build_command(&values, brightness);
        self.plan_non_periodic_turn_on(room_id, command)
    }

    pub(crate) fn plan_dim_to_factor(
        &mut self,
        room_id: &str,
        current_hour: f32,
        factor: f32,
    ) -> Option<ManualDispatchPlan> {
        {
            let room = self.rooms.get_mut(room_id)?;
            room.clear_off_states();
            // A factor below 1.0 is the motion-timeout warning dim. Mark the
            // room so the periodic worker doesn't immediately overwrite it
            // with the active curve. Restoring to 1.0 (motion returned)
            // clears the flag.
            room.warning_active = factor < 0.999;
        };
        let effective = self.effective_room_state(room_id);
        let offset_minutes = effective.time_offset_minutes;
        let brightness_offset = effective.brightness_offset;
        let profile_settings = effective.profile_settings;

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, offset_minutes);
        let brightness =
            ((values.brightness as f32 + brightness_offset) * factor).clamp(1.0, 100.0) as u8;
        let command = Self::build_command(&values, brightness);
        Some(self.plan_non_periodic_turn_on(room_id, command))
    }

    pub(crate) fn plan_turn_off(&mut self, room_id: &str, current_hour: f32) -> ManualDispatchPlan {
        let standby_enabled = self
            .rooms
            .get(room_id)
            .map(|room| room.standby_enabled)
            .unwrap_or(false);
        if !standby_enabled {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.set_hard_off();
            return self.plan_non_periodic_turn_off(room_id, None);
        }

        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.enable_rhythm();
            room.set_standby();
        }
        let effective = self.effective_room_state(room_id);
        let values = self.idle_values_for_settings(
            Some(&effective.profile_settings),
            current_hour,
            effective.time_offset_minutes,
        );
        let command = Self::build_command(&values, values.brightness);
        self.plan_non_periodic_turn_on(room_id, command)
    }

    pub(crate) fn plan_lights_off(
        &mut self,
        room_id: &str,
        transition_ms: Option<u32>,
    ) -> ManualDispatchPlan {
        let room = self.rooms.get_or_create(room_id, room_id);
        room.set_hard_off();
        self.plan_non_periodic_turn_off(room_id, transition_ms)
    }

    pub(crate) fn plan_soft_off_tick(
        &mut self,
        room_id: &str,
        current_hour: f32,
    ) -> ManualDispatchPlan {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.enable_rhythm();
            room.set_standby();
        }

        let effective = self.effective_room_state(room_id);
        let profile_settings = effective.profile_settings;
        let values = self.idle_values_for_settings(
            Some(&profile_settings),
            current_hour,
            effective.time_offset_minutes,
        );
        let command = Self::build_command(&values, values.brightness);
        self.plan_non_periodic_turn_on(room_id, command)
    }

    pub(crate) fn plan_mood_tick(
        &mut self,
        room_id: &str,
        current_hour: f32,
    ) -> ManualDispatchPlan {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.enable_rhythm();
            room.set_mood();
        }

        let effective = self.effective_room_state(room_id);
        let values = self.values_for_room_state(
            Some(&effective.profile_settings),
            RoomModeState::Mood,
            current_hour,
            effective.time_offset_minutes,
        );
        let command = Self::build_command(&values, values.brightness);
        self.plan_non_periodic_turn_on(room_id, command)
    }

    pub(crate) fn plan_step(
        &mut self,
        room_id: &str,
        current_hour: f32,
        action: StepAction,
    ) -> ManualDispatchPlan {
        if let Some(room) = self.rooms.get_mut(room_id) {
            room.clear_off_states();
        }

        let (current_offset, profile_settings) = self
            .rooms
            .effective_state(room_id)
            .map(|state| (state.time_offset_minutes, state.profile_settings))
            .unwrap_or((0.0, RoomProfileSettings::default()));

        let effective_hour = (current_hour + current_offset / 60.0).rem_euclid(24.0);
        let ctx = self.create_context(effective_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let step_result = module.calculate_step(&ctx, action);

        let room = self.rooms.get_or_create(room_id, room_id);
        room.apply_time_offset(step_result.time_offset_minutes);

        let command = LightingCommand::from_values(&step_result.values);
        self.plan_non_periodic_turn_on(room_id, command)
    }

    pub(crate) fn plan_dim(
        &mut self,
        room_id: &str,
        current_hour: f32,
        amount: f32,
    ) -> ManualDispatchPlan {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.clear_off_states();
            room.apply_brightness_offset(amount);
        };
        let effective = self.effective_room_state(room_id);
        let offset_minutes = effective.time_offset_minutes;
        let brightness_offset = effective.brightness_offset;
        let profile_settings = effective.profile_settings;

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, offset_minutes);
        let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
        let command = Self::build_command(&values, brightness);
        self.plan_non_periodic_turn_on(room_id, command)
    }

    pub(crate) fn plan_set_brightness(
        &mut self,
        room_id: &str,
        current_hour: f32,
        target: u8,
    ) -> ManualDispatchPlan {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.clear_off_states();
        };
        let effective = self.effective_room_state(room_id);
        let offset_minutes = effective.time_offset_minutes;
        let profile_settings = effective.profile_settings;

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, offset_minutes);

        let room = self.rooms.get_or_create(room_id, room_id);
        room.brightness_offset = target as f32 - values.brightness as f32;

        let command = Self::build_command(&values, target);
        self.plan_non_periodic_turn_on(room_id, command)
    }

    pub(crate) fn plan_set_curve_color_temperature(
        &mut self,
        room_id: &str,
        current_hour: f32,
        target_kelvin: u16,
        preserve_brightness: bool,
    ) -> Result<ManualDispatchPlan, String> {
        let effective = self.effective_room_state(room_id);
        let current_time_offset = effective.time_offset_minutes;
        let current_brightness_offset = effective.brightness_offset;
        let profile_settings = effective.profile_settings.clone();
        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));

        let current_values = module.calculate_with_offset(&ctx, current_time_offset);
        if current_values.is_direct_color {
            return Err(
                "Active curve uses direct color; color-temperature modifiers are unavailable"
                    .to_string(),
            );
        }

        let current_brightness =
            (current_values.brightness as f32 + current_brightness_offset).clamp(1.0, 100.0);
        let Some(position) = module.find_curve_position(
            &ctx,
            LightCurveTarget::ColorTemperature(target_kelvin),
            current_time_offset,
        ) else {
            return Err("Active curve cannot resolve a color-temperature position".to_string());
        };

        if position.values.is_direct_color || position.values.kelvin == 0 {
            return Err(
                "Active curve uses direct color; color-temperature modifiers are unavailable"
                    .to_string(),
            );
        }

        let (inherited_time_offset, inherited_brightness_offset) =
            if let Some(local) = self.rooms.get(room_id) {
                (
                    current_time_offset - local.time_offset_minutes,
                    current_brightness_offset - local.brightness_offset,
                )
            } else {
                (0.0, 0.0)
            };

        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.clear_off_states();
            room.set_time_offset(position.time_offset_minutes - inherited_time_offset);
            if preserve_brightness {
                room.brightness_offset = (current_brightness
                    - inherited_brightness_offset
                    - position.values.brightness as f32)
                    .clamp(-100.0, 100.0);
            }
        }

        let brightness_offset = self.effective_room_state(room_id).brightness_offset;
        let brightness =
            (position.values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
        let command = Self::build_command(&position.values, brightness);
        Ok(self.plan_non_periodic_turn_on(room_id, command))
    }

    pub(crate) fn plan_set_time_offset(
        &mut self,
        room_id: &str,
        current_hour: f32,
        offset_minutes: f32,
    ) -> Option<ManualDispatchPlan> {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.set_time_offset(offset_minutes);
            room.clear_warning_state();
        }

        let effective = self.effective_room_state(room_id);
        let rhythm_enabled = effective.rhythm_enabled;
        let soft_off = effective.soft_off;
        let mood_active = effective.mood_active;
        let hard_off = effective.hard_off;
        let brightness_offset = effective.brightness_offset;
        let time_offset = effective.time_offset_minutes;
        let profile_settings = effective.profile_settings;

        if hard_off || mood_active || !rhythm_enabled {
            return None;
        }

        if soft_off {
            let values =
                self.idle_values_for_settings(Some(&profile_settings), current_hour, time_offset);
            let command = Self::build_command(&values, values.brightness);
            return Some(self.plan_non_periodic_turn_on(room_id, command));
        }

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, time_offset);

        let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
        let command = Self::build_command(&values, brightness);

        Some(self.plan_non_periodic_turn_on(room_id, command))
    }

    pub(crate) fn plan_reset(&mut self, room_id: &str, current_hour: f32) -> ManualDispatchPlan {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.reset_offsets();
            room.clear_off_states();
            room.enable_rhythm();
        };
        let profile_settings = self.effective_room_state(room_id).profile_settings;

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate(&ctx);
        let command = LightingCommand::from_values(&values);
        self.plan_non_periodic_turn_on(room_id, command)
    }

    pub(crate) fn plan_toggle(
        &mut self,
        room_id: &str,
        current_hour: f32,
        lights_on: Option<bool>,
    ) -> ManualActionPlan {
        let effective = self.effective_room_state(room_id);
        let effectively_off = if self.effective_mood_active(&effective)
            || self.effective_standby_active(&effective)
        {
            Some(true)
        } else {
            lights_on.map(|is_on| !is_on)
        };

        match effectively_off {
            None => ManualActionPlan::RequiresLightCheck,
            Some(true) => ManualActionPlan::Dispatch {
                dispatch: self.plan_turn_on(room_id, current_hour),
                turned_on: true,
            },
            Some(false) => ManualActionPlan::Dispatch {
                dispatch: self.plan_turn_off(room_id, current_hour),
                turned_on: false,
            },
        }
    }

    pub(crate) fn plan_button_action(
        &mut self,
        room_id: &str,
        action: ButtonAction,
        current_hour: f32,
        lights_on: Option<bool>,
    ) -> ManualActionPlan {
        match action {
            ButtonAction::OnPress => ManualActionPlan::Dispatch {
                dispatch: self.plan_turn_on(room_id, current_hour),
                turned_on: true,
            },
            ButtonAction::Toggle => self.plan_toggle(room_id, current_hour, lights_on),
            ButtonAction::OffPress => ManualActionPlan::Dispatch {
                dispatch: self.plan_turn_off(room_id, current_hour),
                turned_on: false,
            },
            ButtonAction::Reset => ManualActionPlan::Dispatch {
                dispatch: self.plan_reset(room_id, current_hour),
                turned_on: true,
            },
            ButtonAction::UpPress => ManualActionPlan::Dispatch {
                dispatch: self.plan_dim(room_id, current_hour, 20.0),
                turned_on: true,
            },
            ButtonAction::DownPress => ManualActionPlan::Dispatch {
                dispatch: self.plan_dim(room_id, current_hour, -20.0),
                turned_on: true,
            },
            ButtonAction::UpHold => ManualActionPlan::Dispatch {
                dispatch: self.plan_step(room_id, current_hour, StepAction::Brighten),
                turned_on: true,
            },
            ButtonAction::DownHold => ManualActionPlan::Dispatch {
                dispatch: self.plan_step(room_id, current_hour, StepAction::Dim),
                turned_on: true,
            },
            ButtonAction::Stop => ManualActionPlan::Noop { turned_on: false },
            ButtonAction::RhythmOn => {
                let room = self.rooms.get_or_create(room_id, room_id);
                room.enable_rhythm();
                ManualActionPlan::Noop { turned_on: false }
            }
            ButtonAction::RhythmOff => {
                if let Some(room) = self.rooms.get_mut(room_id) {
                    room.disable_rhythm();
                }
                self.clear_periodic_dedupe_room(room_id);
                ManualActionPlan::Noop { turned_on: false }
            }
            ButtonAction::LightsOff => ManualActionPlan::Dispatch {
                dispatch: self.plan_lights_off(room_id, None),
                turned_on: false,
            },
            ButtonAction::SleepOn => {
                self.set_light_profile(crate::light_profile::SLEEP_PROFILE_ID);
                ManualActionPlan::Dispatch {
                    dispatch: self.plan_turn_off(room_id, current_hour),
                    turned_on: false,
                }
            }
            ButtonAction::SleepOff => {
                self.set_light_profile(crate::light_profile::RHYTHM_PROFILE_ID);
                ManualActionPlan::Dispatch {
                    dispatch: self.plan_turn_on(room_id, current_hour),
                    turned_on: true,
                }
            }
        }
    }

    fn should_dispatch_periodic_command(
        &mut self,
        source_room_id: &str,
        target_id: &str,
        command: &LightingCommand,
    ) -> bool {
        let cache_key = Self::periodic_cache_key(source_room_id, target_id);
        if let Some(entry) = self.periodic_command_cache.get_mut(&cache_key) {
            if entry.command == *command && entry.skipped_cycles < PERIODIC_DEDUPE_MAX_SKIPS {
                entry.skipped_cycles = entry.skipped_cycles.saturating_add(1);
                return false;
            }
        }

        true
    }

    pub(crate) fn plan_periodic_tick_node(
        &mut self,
        target_id: &str,
        source_room_id: &str,
        current_hour: f32,
        lights_on: Option<bool>,
    ) -> PeriodicTickPlan {
        let effective = self.effective_room_state(source_room_id);
        let rhythm_enabled = effective.rhythm_enabled;
        let soft_off = effective.soft_off;
        let mood_active = effective.mood_active;
        let hard_off = effective.hard_off;
        let warning_active = effective.warning_active;

        if !rhythm_enabled || hard_off {
            return PeriodicTickPlan::Skipped;
        }

        // The motion-timeout warning dim is a transient "lights will turn off
        // soon" signal. Re-applying the active curve here would visibly undo
        // the dim before the timeout fires.
        if warning_active && !soft_off {
            return PeriodicTickPlan::Skipped;
        }

        if mood_active {
            return PeriodicTickPlan::Skipped;
        }

        if soft_off {
            let offset = effective.time_offset_minutes;
            let profile_settings = effective.profile_settings.clone();
            let values =
                self.idle_values_for_settings(Some(&profile_settings), current_hour, offset);
            let command = Self::build_command(&values, values.brightness);
            if !self.should_dispatch_periodic_command(source_room_id, target_id, &command) {
                return PeriodicTickPlan::Skipped;
            }

            let profile_id = self
                .profile_registry
                .profile_for_room_state(
                    self.profile_registry.active_mode(),
                    RoomModeState::Standby,
                    Some(&profile_settings),
                )
                .id()
                .to_string();
            return PeriodicTickPlan::Dispatch {
                command,
                room_state: RoomModeState::Standby,
                profile_id,
            };
        }

        match lights_on {
            None => return PeriodicTickPlan::RequiresLightCheck,
            Some(false) => return PeriodicTickPlan::Skipped,
            Some(true) => {}
        }

        let offset_minutes = effective.time_offset_minutes;
        let brightness_offset = effective.brightness_offset;
        let profile_settings = effective.profile_settings.clone();
        let ctx = self.create_context(current_hour);
        let effective_mode =
            profile_settings.schedule_mode(self.profile_registry.active_mode(), current_hour);
        let module = self.profile_registry.profile_for_room_state(
            effective_mode,
            RoomModeState::Active,
            Some(&profile_settings),
        );
        let values = module.calculate_with_offset(&ctx, offset_minutes);
        let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
        let command = Self::build_command(&values, brightness);
        if !self.should_dispatch_periodic_command(source_room_id, target_id, &command) {
            return PeriodicTickPlan::Skipped;
        }

        let profile_id = self
            .profile_registry
            .profile_for_room_state(
                effective_mode,
                RoomModeState::Active,
                Some(&profile_settings),
            )
            .id()
            .to_string();
        PeriodicTickPlan::Dispatch {
            command,
            room_state: RoomModeState::Active,
            profile_id,
        }
    }

    pub(crate) fn remove_room_state(&mut self, room_id: &str) {
        // Iterative worklist with a visited set: parent links are stored
        // unchecked, so a cycle (A.parent = B, B.parent = A) would make the
        // recursive form overflow the stack and abort the process. See the
        // matching cycle guard in RoomManager::effective_state.
        let mut visited = std::collections::HashSet::new();
        let mut worklist = vec![room_id.to_string()];
        while let Some(id) = worklist.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            worklist.extend(self.rooms.child_iter(&id).map(|node| node.id.clone()));
            self.rooms.remove(&id);
            self.clear_periodic_dedupe_room(&id);
        }
    }

    pub(crate) fn clear_motion_warning_state(&mut self, room_id: &str) {
        if let Some(room) = self.rooms.get_mut(room_id) {
            room.clear_warning_state();
        }
    }

    pub(crate) fn reset_restored_room_state(&mut self, room_id: &str) {
        self.clear_motion_warning_state(room_id);
        self.clear_periodic_dedupe_room(room_id);
    }

    async fn send_non_periodic_turn_on_target(
        &mut self,
        source_room_id: &str,
        target_id: &str,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        self.clear_periodic_dedupe_target(source_room_id, target_id);
        self.controller.turn_on(target_id, command.clone()).await?;
        self.periodic_command_cache.insert(
            Self::periodic_cache_key(source_room_id, target_id),
            PeriodicCommandCacheEntry {
                command,
                skipped_cycles: 0,
            },
        );
        Ok(())
    }

    async fn send_non_periodic_turn_on(
        &mut self,
        room_id: &str,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        self.clear_periodic_dedupe_room(room_id);
        self.send_non_periodic_turn_on_target(room_id, room_id, command)
            .await
    }

    async fn send_non_periodic_turn_off_target(
        &mut self,
        source_room_id: &str,
        target_id: &str,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        self.clear_periodic_dedupe_target(source_room_id, target_id);
        self.controller.turn_off(target_id, transition_ms).await
    }

    async fn send_non_periodic_turn_off(
        &mut self,
        room_id: &str,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        self.clear_periodic_dedupe_room(room_id);
        self.send_non_periodic_turn_off_target(room_id, room_id, transition_ms)
            .await
    }

    async fn send_periodic_turn_on_target(
        &mut self,
        source_room_id: &str,
        target_id: &str,
        command: LightingCommand,
    ) -> LightControlResult<bool> {
        let cache_key = Self::periodic_cache_key(source_room_id, target_id);
        if let Some(entry) = self.periodic_command_cache.get_mut(&cache_key) {
            if entry.command == command && entry.skipped_cycles < PERIODIC_DEDUPE_MAX_SKIPS {
                entry.skipped_cycles = entry.skipped_cycles.saturating_add(1);
                return Ok(false);
            }
        }

        self.controller.turn_on(target_id, command.clone()).await?;
        self.periodic_command_cache.insert(
            cache_key,
            PeriodicCommandCacheEntry {
                command,
                skipped_cycles: 0,
            },
        );
        Ok(true)
    }

    /// Apply an explicit rendered lighting command to a room.
    ///
    /// Used by mode transitions, which operate in rendered output space
    /// instead of reusing one of the steady-state profile actions.
    pub async fn apply_room_command(
        &mut self,
        room_id: &str,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        if let Some(room) = self.rooms.get_mut(room_id) {
            room.clear_warning_state();
        }
        self.send_non_periodic_turn_on(room_id, command).await
    }

    fn active_profile_for_settings(
        &self,
        settings: Option<&RoomProfileSettings>,
    ) -> Arc<dyn LightProfileModule> {
        self.profile_registry.profile_for_room_state(
            self.profile_registry.active_mode(),
            RoomModeState::Active,
            settings,
        )
    }

    fn values_for_room_state(
        &self,
        settings: Option<&RoomProfileSettings>,
        state: RoomModeState,
        current_hour: f32,
        time_offset_minutes: f32,
    ) -> LightingValues {
        let ctx = self.create_context(current_hour);
        let mode = settings
            .map(|settings| {
                settings.schedule_mode(self.profile_registry.active_mode(), current_hour)
            })
            .unwrap_or_else(|| self.profile_registry.active_mode());
        self.profile_registry
            .profile_for_room_state(mode, state, settings)
            .calculate_with_offset(&ctx, time_offset_minutes)
    }

    fn idle_values_for_settings(
        &self,
        settings: Option<&RoomProfileSettings>,
        current_hour: f32,
        time_offset_minutes: f32,
    ) -> LightingValues {
        self.values_for_room_state(
            settings,
            RoomModeState::Standby,
            current_hour,
            time_offset_minutes,
        )
    }

    // =========================================================================
    // Service Primitives
    // =========================================================================

    /// Enable Rhythm timer participation (no light commands).
    ///
    /// This only enables Rhythm mode so the room participates in the
    /// periodic 60s tick. It does NOT send any light commands.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    pub async fn rhythm_on(&mut self, room_id: &str) -> LightControlResult<()> {
        let room = self.rooms.get_or_create(room_id, room_id);
        room.enable_rhythm();
        room.clear_warning_state();
        Ok(())
    }

    /// Turn on lights with adaptive values and enable Rhythm.
    ///
    /// This is the primary "turn on lights" entry point. It enables Rhythm
    /// mode, calculates the current adaptive values, and turns on the lights.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    pub async fn turn_on(&mut self, room_id: &str, current_hour: f32) -> LightControlResult<()> {
        // Get or create room and enable rhythm
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.enable_rhythm();
            room.clear_off_states();
        };
        let effective = self.effective_room_state(room_id);
        let offset_minutes = effective.time_offset_minutes;
        let brightness_offset = effective.brightness_offset;
        let profile_settings = effective.profile_settings;

        // Calculate lighting values with any stored offset
        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, offset_minutes);

        // Apply brightness offset if any
        let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;

        // Create command and send
        let command = Self::build_command(&values, brightness);
        self.send_non_periodic_turn_on(room_id, command).await
    }

    /// Dim lights to a fraction of current adaptive brightness.
    ///
    /// Computes the current adaptive brightness (with offsets), multiplies
    /// by `factor`, and sends the command. Does NOT modify room state
    /// (no rhythm enable/disable, no offset changes). No-op if room
    /// doesn't exist.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    /// * `factor` - Brightness multiplier (0.0-1.0)
    pub async fn dim_to_factor(
        &mut self,
        room_id: &str,
        current_hour: f32,
        factor: f32,
    ) -> LightControlResult<()> {
        {
            let Some(room) = self.rooms.get_mut(room_id) else {
                return Ok(());
            };
            room.clear_off_states();
            room.warning_active = factor < 0.999;
        };
        let effective = self.effective_room_state(room_id);
        let offset_minutes = effective.time_offset_minutes;
        let brightness_offset = effective.brightness_offset;
        let profile_settings = effective.profile_settings;

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, offset_minutes);

        let brightness =
            ((values.brightness as f32 + brightness_offset) * factor).clamp(1.0, 100.0) as u8;

        let command = Self::build_command(&values, brightness);
        self.send_non_periodic_turn_on(room_id, command).await
    }

    /// Disable Rhythm mode (lights remain unchanged).
    ///
    /// This disables Rhythm mode for the room but does not turn off the lights.
    /// The lights will stay at their current state but will no longer be
    /// automatically adjusted.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    pub async fn rhythm_off(&mut self, room_id: &str) -> LightControlResult<()> {
        if let Some(room) = self.rooms.get_mut(room_id) {
            room.disable_rhythm();
            room.clear_warning_state();
        }
        self.clear_periodic_dedupe_room(room_id);
        // Note: We don't turn off the lights, just disable rhythm mode
        Ok(())
    }

    /// Toggle lights on/off (always adaptive).
    ///
    /// If any lights are on, turns them off (rhythm state unchanged).
    /// If all lights are off, turns on with adaptive values and enables Rhythm.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    ///
    /// # Returns
    ///
    /// `true` if lights were turned on, `false` if turned off.
    pub async fn toggle(&mut self, room_id: &str, current_hour: f32) -> LightControlResult<bool> {
        // Determine if the room is effectively off
        let effective = self.effective_room_state(room_id);
        let effectively_off = if self.effective_mood_active(&effective)
            || self.effective_standby_active(&effective)
        {
            true
        } else {
            !self.controller.any_lights_on(room_id).await?
        };

        if effectively_off {
            // Room is off - turn on with adaptive lighting (auto-enables rhythm)
            self.turn_on(room_id, current_hour).await?;
            Ok(true)
        } else {
            // Room is on - turn off (rhythm state unchanged)
            self.turn_off(room_id, current_hour).await?;
            Ok(false)
        }
    }

    /// Step up (brighten and cool) along the adaptive curve.
    ///
    /// This moves the lighting forward along the curve, making it brighter
    /// and cooler (higher color temperature).
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    pub async fn step_up(&mut self, room_id: &str, current_hour: f32) -> LightControlResult<()> {
        self.do_step(room_id, current_hour, StepAction::Brighten)
            .await
    }

    /// Step down (dim and warm) along the adaptive curve.
    ///
    /// This moves the lighting backward along the curve, making it dimmer
    /// and warmer (lower color temperature).
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    pub async fn step_down(&mut self, room_id: &str, current_hour: f32) -> LightControlResult<()> {
        self.do_step(room_id, current_hour, StepAction::Dim).await
    }

    /// Internal helper for step operations.
    async fn do_step(
        &mut self,
        room_id: &str,
        current_hour: f32,
        action: StepAction,
    ) -> LightControlResult<()> {
        // User is interacting — clear soft_off
        if let Some(room) = self.rooms.get_mut(room_id) {
            room.clear_off_states();
        }

        // Get the room's current time offset
        let (current_offset, profile_settings) = self
            .rooms
            .effective_state(room_id)
            .map(|state| (state.time_offset_minutes, state.profile_settings))
            .unwrap_or((0.0, RoomProfileSettings::default()));

        // Calculate effective current hour with offset
        let effective_hour = (current_hour + current_offset / 60.0).rem_euclid(24.0);

        // Calculate the step using the active module
        let ctx = self.create_context(effective_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let step_result = module.calculate_step(&ctx, action);

        // Update room offset (don't change rhythm mode state)
        let room = self.rooms.get_or_create(room_id, room_id);
        room.apply_time_offset(step_result.time_offset_minutes);

        // Send command
        let command = LightingCommand::from_values(&step_result.values);
        self.send_non_periodic_turn_on(room_id, command).await
    }

    /// Increase brightness without following the curve.
    ///
    /// This directly increases the brightness offset without changing
    /// the time position on the curve.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    /// * `amount` - Amount to increase brightness (default: 10%)
    pub async fn dim_up(
        &mut self,
        room_id: &str,
        current_hour: f32,
        amount: Option<f32>,
    ) -> LightControlResult<()> {
        let amount = amount.unwrap_or(20.0);
        self.do_dim(room_id, current_hour, amount).await
    }

    /// Decrease brightness without following the curve.
    ///
    /// This directly decreases the brightness offset without changing
    /// the time position on the curve.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    /// * `amount` - Amount to decrease brightness (default: 10%)
    pub async fn dim_down(
        &mut self,
        room_id: &str,
        current_hour: f32,
        amount: Option<f32>,
    ) -> LightControlResult<()> {
        let amount = amount.unwrap_or(20.0);
        self.do_dim(room_id, current_hour, -amount).await
    }

    /// Internal helper for dim operations.
    async fn do_dim(
        &mut self,
        room_id: &str,
        current_hour: f32,
        amount: f32,
    ) -> LightControlResult<()> {
        // User is interacting — clear soft_off
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.clear_off_states();
            room.apply_brightness_offset(amount);
        };
        let effective = self.effective_room_state(room_id);
        let offset_minutes = effective.time_offset_minutes;
        let brightness_offset = effective.brightness_offset;
        let profile_settings = effective.profile_settings;

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, offset_minutes);

        // Apply brightness offset
        let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;

        // Send command
        let command = Self::build_command(&values, brightness);
        self.send_non_periodic_turn_on(room_id, command).await
    }

    /// Set absolute brightness for a room.
    ///
    /// Computes the brightness offset needed so that the effective brightness
    /// equals `target`. Rhythm stays enabled (only brightness is overridden,
    /// color temperature continues tracking the curve).
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    /// * `target` - Desired brightness (1-100)
    pub async fn set_brightness(
        &mut self,
        room_id: &str,
        current_hour: f32,
        target: u8,
    ) -> LightControlResult<()> {
        // First borrow: clear soft_off and extract time offset + profile settings
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.clear_off_states();
        };
        let effective = self.effective_room_state(room_id);
        let offset_minutes = effective.time_offset_minutes;
        let profile_settings = effective.profile_settings;

        // Calculate curve values at current time
        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, offset_minutes);

        // Second borrow: set offset so curve_brightness + offset = target
        let room = self.rooms.get_or_create(room_id, room_id);
        room.brightness_offset = target as f32 - values.brightness as f32;

        let command = Self::build_command(&values, target);
        self.send_non_periodic_turn_on(room_id, command).await
    }

    /// Set the time offset for a room directly.
    ///
    /// Sets `room.time_offset_minutes` to the given value (not additive).
    /// If the room is on, sends full
    /// adaptive values at the new offset position, preserving brightness_offset.
    /// If off, just sets the offset without sending light commands.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    /// * `offset_minutes` - The time offset in minutes (absolute, not additive)
    pub async fn set_time_offset(
        &mut self,
        room_id: &str,
        current_hour: f32,
        offset_minutes: f32,
    ) -> LightControlResult<()> {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.set_time_offset(offset_minutes);
            room.clear_warning_state();
        }

        let effective = self.effective_room_state(room_id);
        let rhythm_enabled = effective.rhythm_enabled;
        let soft_off = effective.soft_off;
        let mood_active = effective.mood_active;
        let hard_off = effective.hard_off;
        let brightness_offset = effective.brightness_offset;
        let time_offset = effective.time_offset_minutes;
        let profile_settings = effective.profile_settings;

        if hard_off || mood_active || !rhythm_enabled {
            // Room is off — just set the offset, no light command
            return Ok(());
        }

        if soft_off {
            let values =
                self.idle_values_for_settings(Some(&profile_settings), current_hour, time_offset);
            let cmd = Self::build_command(&values, values.brightness);
            return self.send_non_periodic_turn_on(room_id, cmd).await;
        }

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, time_offset);

        // On: send full adaptive values preserving brightness_offset.
        let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
        let cmd = Self::build_command(&values, brightness);
        self.send_non_periodic_turn_on(room_id, cmd).await
    }

    /// Reset to current solar time (clear all offsets).
    ///
    /// This clears any time and brightness offsets, returning the room
    /// to the current adaptive lighting values. Also enables rhythm mode.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    pub async fn reset(&mut self, room_id: &str, current_hour: f32) -> LightControlResult<()> {
        // Reset room offsets, clear soft_off, and enable rhythm mode
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.reset_offsets();
            room.clear_off_states();
            room.enable_rhythm();
        };
        let profile_settings = self.effective_room_state(room_id).profile_settings;

        // Apply current values using the active module
        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate(&ctx);
        let command = LightingCommand::from_values(&values);
        self.send_non_periodic_turn_on(room_id, command).await
    }

    /// Turn off lights in a room (rhythm state unchanged).
    ///
    /// Turn lights off, or into Standby when this node opted in.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    pub async fn turn_off(&mut self, room_id: &str, current_hour: f32) -> LightControlResult<()> {
        let standby_enabled = self
            .rooms
            .get(room_id)
            .map(|room| room.standby_enabled)
            .unwrap_or(false);
        if !standby_enabled {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.set_hard_off();
            return self.send_non_periodic_turn_off(room_id, None).await;
        }

        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.enable_rhythm();
            room.set_standby();
        }
        let effective = self.effective_room_state(room_id);
        let values = self.idle_values_for_settings(
            Some(&effective.profile_settings),
            current_hour,
            effective.time_offset_minutes,
        );
        let cmd = Self::build_command(&values, values.brightness);
        self.send_non_periodic_turn_on(room_id, cmd).await
    }

    /// Turn lights fully off.
    ///
    /// Always calls `controller.turn_off()` regardless of power_save setting.
    /// Also clears the legacy `soft_off` flag on the room if it was set.
    pub async fn lights_off(
        &mut self,
        room_id: &str,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        let room = self.rooms.get_or_create(room_id, room_id);
        room.set_hard_off();
        self.send_non_periodic_turn_off(room_id, transition_ms)
            .await
    }

    // =========================================================================
    // Periodic Update Methods
    // =========================================================================

    /// Perform a periodic tick for a single room.
    ///
    /// Consolidates the per-room logic: check rhythm_enabled, handle soft_off,
    /// check any_lights_on (disabling rhythm if lights are off), and apply
    /// adaptive lighting.
    pub async fn periodic_tick_single_room(
        &mut self,
        room_id: &str,
        current_hour: f32,
    ) -> PeriodicTickResult {
        self.periodic_tick_node(room_id, room_id, current_hour)
            .await
    }

    /// Perform a periodic tick for a derived dispatch node that inherits a
    /// parent room's settings and state.
    pub async fn periodic_tick_node(
        &mut self,
        target_id: &str,
        source_room_id: &str,
        current_hour: f32,
    ) -> PeriodicTickResult {
        let effective = self.effective_room_state(source_room_id);
        let rhythm_enabled = effective.rhythm_enabled;
        let soft_off = effective.soft_off;
        let mood_active = effective.mood_active;
        let hard_off = effective.hard_off;
        let warning_active = effective.warning_active;

        if !rhythm_enabled || hard_off {
            return PeriodicTickResult::Skipped;
        }

        // Honor the motion-timeout warning dim — see `plan_periodic_tick_node`
        // for the rationale.
        if warning_active && !soft_off {
            return PeriodicTickResult::Skipped;
        }

        if mood_active {
            return PeriodicTickResult::Skipped;
        }

        if soft_off {
            let offset = effective.time_offset_minutes;
            let profile_settings = effective.profile_settings;
            let values =
                self.idle_values_for_settings(Some(&profile_settings), current_hour, offset);
            let command = Self::build_command(&values, values.brightness);
            return match self
                .send_periodic_turn_on_target(source_room_id, target_id, command)
                .await
            {
                Ok(true) => PeriodicTickResult::Updated,
                Ok(false) => PeriodicTickResult::Skipped,
                Err(e) => PeriodicTickResult::Error(format!(
                    "{}: {}",
                    self.node_log_label(source_room_id),
                    e
                )),
            };
        }

        // Check if lights are still on — skip if off (don't disable rhythm)
        match self.controller.any_lights_on(target_id).await {
            Ok(false) => return PeriodicTickResult::Skipped,
            Err(_) => return PeriodicTickResult::Skipped, // Network error — skip
            Ok(true) => {}
        }

        let (offset_minutes, brightness_offset, profile_settings) = self
            .rooms
            .effective_state(source_room_id)
            .map(|state| {
                (
                    state.time_offset_minutes,
                    state.brightness_offset,
                    state.profile_settings,
                )
            })
            .unwrap_or((0.0, 0.0, RoomProfileSettings::default()));

        let ctx = self.create_context(current_hour);
        let module = self.active_profile_for_settings(Some(&profile_settings));
        let values = module.calculate_with_offset(&ctx, offset_minutes);
        let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
        let command = Self::build_command(&values, brightness);

        match self
            .send_periodic_turn_on_target(source_room_id, target_id, command)
            .await
        {
            Ok(true) => PeriodicTickResult::Updated,
            Ok(false) => PeriodicTickResult::Skipped,
            Err(e) => {
                PeriodicTickResult::Error(format!("{}: {}", self.node_log_label(source_room_id), e))
            }
        }
    }

    /// Perform a periodic update of all rhythm-enabled rooms.
    ///
    /// This should be called every minute (or at your desired interval) to
    /// keep lights in sync with the adaptive lighting curve.
    ///
    /// # Arguments
    ///
    /// * `current_hour` - Current time in hours (0-24)
    ///
    /// # Returns
    ///
    /// A tuple of (updated, errors) room ID lists.
    pub async fn periodic_update(&mut self, current_hour: f32) -> (Vec<String>, Vec<String>) {
        let room_ids: Vec<String> = self
            .rooms
            .rhythm_enabled_rooms()
            .iter()
            .map(|r| r.id.clone())
            .collect();

        let mut updated = Vec::new();
        let mut errors = Vec::new();

        for room_id in room_ids {
            match self.periodic_tick_single_room(&room_id, current_hour).await {
                PeriodicTickResult::Updated => updated.push(room_id),
                PeriodicTickResult::Error(e) => errors.push(e),
                PeriodicTickResult::Skipped => {}
            }
        }

        (updated, errors)
    }

    /// Build a LightingCommand from profile values, respecting `is_direct_color`.
    ///
    /// When `is_direct_color` is true (idle or sleep profiles), sends XY/RGB.
    /// Otherwise sends kelvin.
    fn build_command(values: &crate::adaptive::LightingValues, brightness: u8) -> LightingCommand {
        if values.is_direct_color {
            LightingCommand::from_color(
                brightness,
                values.rgb,
                values.xy,
                Some(values.transition_ms),
            )
        } else {
            LightingCommand::with_transition(brightness, values.kelvin, values.transition_ms)
        }
    }

    /// Render and enter Standby using the active mode's idle profile.
    pub async fn soft_off_tick(
        &mut self,
        room_id: &str,
        current_hour: f32,
    ) -> LightControlResult<()> {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.enable_rhythm();
            room.set_standby();
        }

        let effective = self.effective_room_state(room_id);
        let profile_settings = effective.profile_settings;
        let values = self.idle_values_for_settings(
            Some(&profile_settings),
            current_hour,
            effective.time_offset_minutes,
        );
        let command = Self::build_command(&values, values.brightness);
        self.send_non_periodic_turn_on(room_id, command).await
    }

    /// Render and enter Mood without making it eligible for periodic ticks.
    pub async fn mood_tick(&mut self, room_id: &str, current_hour: f32) -> LightControlResult<()> {
        {
            let room = self.rooms.get_or_create(room_id, room_id);
            room.enable_rhythm();
            room.set_mood();
        }

        let effective = self.effective_room_state(room_id);
        let values = self.values_for_room_state(
            Some(&effective.profile_settings),
            RoomModeState::Mood,
            current_hour,
            effective.time_offset_minutes,
        );
        let command = Self::build_command(&values, values.brightness);
        self.send_non_periodic_turn_on(room_id, command).await
    }

    /// Check if solar midnight was crossed and reset all room offsets if so.
    ///
    /// Call this periodically (e.g., every minute) to automatically reset
    /// room offsets at solar midnight, giving users a fresh start each day.
    ///
    /// # Arguments
    ///
    /// * `last_hour` - The hour at the last check (0-24)
    /// * `current_hour` - Current hour (0-24)
    /// * `solar_midnight_hour` - Solar midnight hour (typically solar_noon + 12, mod 24)
    ///
    /// # Returns
    ///
    /// `true` if midnight was crossed and offsets were reset, `false` otherwise.
    pub fn check_solar_midnight_reset(
        &mut self,
        last_hour: f32,
        current_hour: f32,
        solar_midnight_hour: f32,
    ) -> bool {
        if crossed_solar_midnight(last_hour, current_hour, solar_midnight_hour) {
            self.reset_all_offsets();
            true
        } else {
            false
        }
    }

    /// Reset time and brightness offsets for all rooms.
    ///
    /// This is called at solar midnight to give users a fresh start,
    /// but can also be called manually if needed.
    pub fn reset_all_offsets(&mut self) {
        for room in self.rooms.iter_mut() {
            room.reset_offsets();
        }
        self.clear_all_periodic_dedupe();
    }

    /// Get the IDs of all rooms with rhythm mode enabled.
    pub fn rhythm_enabled_room_ids(&self) -> Vec<String> {
        self.rooms
            .rhythm_enabled_rooms()
            .iter()
            .map(|r| r.id.clone())
            .collect()
    }

    /// Sync rooms from the controller.
    ///
    /// This queries the light controller for available rooms and adds
    /// them to the room manager.
    pub async fn sync_rooms(&mut self) -> LightControlResult<()> {
        let rooms = self.controller.get_rooms().await?;
        for room in rooms {
            if !self.rooms.contains(&room.id) {
                self.rooms.add_room(room);
            }
        }
        Ok(())
    }

    /// Get the current adaptive lighting values for a room.
    ///
    /// This calculates what the lighting should be based on the current
    /// time and any stored offsets, without actually sending a command.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room
    /// * `current_hour` - Current time in hours (0-24)
    pub fn get_current_values(&self, room_id: &str, current_hour: f32) -> LightingCommand {
        let (time_offset, brightness_offset) = self
            .rooms
            .get(room_id)
            .map(|r| (r.effective_time_offset(), r.effective_brightness_offset()))
            .unwrap_or((0.0, 0.0));

        let ctx = self.create_context(current_hour);
        let module = self.profile_registry.active_profile();
        let values = module.calculate_with_offset(&ctx, time_offset);
        let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;

        Self::build_command(&values, brightness)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::NoOpController;
    use crate::spy_controller::SpyLightController;
    use std::sync::Arc;

    fn test_engine() -> RhythmEngine<NoOpController> {
        RhythmEngine::new(NoOpController::new())
    }

    fn spy_engine() -> (
        RhythmEngine<Arc<SpyLightController>>,
        Arc<SpyLightController>,
    ) {
        let spy = Arc::new(SpyLightController::new());
        let engine = RhythmEngine::new(spy.clone());
        (engine, spy)
    }

    async fn enter_warning_dim(engine: &mut RhythmEngine<Arc<SpyLightController>>) {
        engine.rhythm_on("room1").await.unwrap();
        engine.dim_to_factor("room1", 12.0, 0.5).await.unwrap();
        assert!(engine.rooms.get("room1").unwrap().warning_active);
    }

    #[tokio::test]
    async fn test_rhythm_on() {
        let mut engine = test_engine();

        // rhythm_on only enables rhythm, no light commands
        let result = engine.rhythm_on("living_room").await;
        assert!(result.is_ok());
        assert!(engine.rooms.is_rhythm_enabled("living_room"));
    }

    #[tokio::test]
    async fn test_turn_on() {
        let mut engine = test_engine();

        // turn_on enables rhythm AND sends light commands
        let result = engine.turn_on("living_room", 12.0).await;
        assert!(result.is_ok());
        assert!(engine.rooms.is_rhythm_enabled("living_room"));
    }

    #[tokio::test]
    async fn test_rhythm_off() {
        let mut engine = test_engine();

        engine.rhythm_on("living_room").await.unwrap();
        assert!(engine.rooms.is_rhythm_enabled("living_room"));

        engine.rhythm_off("living_room").await.unwrap();
        assert!(!engine.rooms.is_rhythm_enabled("living_room"));
    }

    #[tokio::test]
    async fn test_turn_off_preserves_rhythm() {
        let mut engine = test_engine();

        // Enable rhythm first
        engine.rhythm_on("living_room").await.unwrap();
        assert!(engine.rooms.is_rhythm_enabled("living_room"));

        // turn_off should NOT disable rhythm
        engine.turn_off("living_room", 12.0).await.unwrap();
        assert!(engine.rooms.is_rhythm_enabled("living_room"));
    }

    #[tokio::test]
    async fn test_toggle() {
        let mut engine = test_engine();

        // NoOpController returns false for any_lights_on, so toggle should turn lights on
        let enabled = engine.toggle("living_room", 12.0).await.unwrap();
        assert!(enabled); // Lights were turned on
        assert!(engine.rooms.is_rhythm_enabled("living_room"));

        // NoOpController still returns false (it doesn't track state), so toggle turns on again
        // In real usage, the controller would report lights as on after turn_on
        let enabled = engine.toggle("living_room", 12.0).await.unwrap();
        assert!(enabled); // Still turns on because NoOpController always says lights are off
    }

    #[tokio::test]
    async fn test_step_up() {
        let mut engine = test_engine();

        // Use hour 8.0 (morning, not at boundary)
        engine.step_up("living_room", 8.0).await.unwrap();

        let room = engine.rooms.get("living_room").unwrap();
        // Step operations should NOT enable rhythm mode
        assert!(!room.rhythm_enabled);
        // Step up in morning should increase time offset (toward noon)
        assert!(room.time_offset_minutes > 0.0);
    }

    #[tokio::test]
    async fn test_step_down() {
        let mut engine = test_engine();

        // Use hour 8.0 (morning, not at boundary)
        engine.step_down("living_room", 8.0).await.unwrap();

        let room = engine.rooms.get("living_room").unwrap();
        // Step operations should NOT enable rhythm mode
        assert!(!room.rhythm_enabled);
        // Step down in morning should decrease time offset (toward sunrise)
        assert!(room.time_offset_minutes < 0.0);
    }

    #[tokio::test]
    async fn test_step_preserves_rhythm_mode() {
        let mut engine = test_engine();

        // Enable rhythm first
        engine.rhythm_on("living_room").await.unwrap();
        assert!(engine.rooms.is_rhythm_enabled("living_room"));

        // Step up should preserve rhythm mode
        engine.step_up("living_room", 8.0).await.unwrap();
        assert!(engine.rooms.is_rhythm_enabled("living_room"));

        // Step down should also preserve rhythm mode
        engine.step_down("living_room", 8.0).await.unwrap();
        assert!(engine.rooms.is_rhythm_enabled("living_room"));
    }

    #[tokio::test]
    async fn test_reset() {
        let mut engine = test_engine();

        // Make some adjustments (use hour 8.0 for step testing)
        engine.step_up("living_room", 8.0).await.unwrap();
        engine.dim_up("living_room", 8.0, Some(10.0)).await.unwrap();

        let room = engine.rooms.get("living_room").unwrap();
        assert!(room.time_offset_minutes != 0.0 || room.brightness_offset != 0.0);

        // Reset
        engine.reset("living_room", 6.0).await.unwrap();

        let room = engine.rooms.get("living_room").unwrap();
        assert_eq!(room.time_offset_minutes, 0.0);
        assert_eq!(room.brightness_offset, 0.0);
    }

    #[tokio::test]
    async fn test_dim_up_down() {
        let mut engine = test_engine();

        engine
            .dim_up("living_room", 12.0, Some(20.0))
            .await
            .unwrap();
        let room = engine.rooms.get("living_room").unwrap();
        assert_eq!(room.brightness_offset, 20.0);
        // Dim operations should NOT enable rhythm mode
        assert!(!room.rhythm_enabled);

        engine
            .dim_down("living_room", 12.0, Some(10.0))
            .await
            .unwrap();
        let room = engine.rooms.get("living_room").unwrap();
        assert_eq!(room.brightness_offset, 10.0);
        assert!(!room.rhythm_enabled);
    }

    #[tokio::test]
    async fn test_dim_preserves_rhythm_mode() {
        let mut engine = test_engine();

        // Enable rhythm first
        engine.rhythm_on("living_room").await.unwrap();
        assert!(engine.rooms.is_rhythm_enabled("living_room"));

        // Dim up should preserve rhythm mode
        engine
            .dim_up("living_room", 12.0, Some(10.0))
            .await
            .unwrap();
        assert!(engine.rooms.is_rhythm_enabled("living_room"));

        // Dim down should also preserve rhythm mode
        engine
            .dim_down("living_room", 12.0, Some(5.0))
            .await
            .unwrap();
        assert!(engine.rooms.is_rhythm_enabled("living_room"));
    }

    #[test]
    fn test_get_current_values() {
        let engine = test_engine();

        let cmd = engine.get_current_values("living_room", 12.0);

        // At noon, should be bright and cool
        assert!(cmd.brightness > 90);
        assert!(cmd.kelvin >= rhythm_profile::DEFAULT_MAX_COLOR_TEMP - 100);
    }

    // =========================================================================
    // Periodic Update Tests
    // =========================================================================

    #[test]
    fn test_crossed_midnight_normal_day() {
        use super::crossed_solar_midnight;

        // Solar midnight at 0:30 (0.5)
        // Time went from 0:00 to 1:00 -> should cross
        assert!(crossed_solar_midnight(0.0, 1.0, 0.5));

        // Time went from 1:00 to 2:00 -> should NOT cross
        assert!(!crossed_solar_midnight(1.0, 2.0, 0.5));

        // Time went from 22:00 to 23:00 -> should NOT cross
        assert!(!crossed_solar_midnight(22.0, 23.0, 0.5));
    }

    #[test]
    fn test_crossed_midnight_wrap_around() {
        use super::crossed_solar_midnight;

        // Solar midnight at 0:30 (0.5)
        // Time went from 23:30 to 0:45 -> should cross
        assert!(crossed_solar_midnight(23.5, 0.75, 0.5));

        // Time went from 23:30 to 0:15 -> should NOT cross (haven't reached 0.5 yet)
        assert!(!crossed_solar_midnight(23.5, 0.25, 0.5));
    }

    #[test]
    fn test_crossed_midnight_late_evening() {
        use super::crossed_solar_midnight;

        // Solar midnight at 23:30 (23.5)
        // Time went from 23:00 to 23:45 -> should cross
        assert!(crossed_solar_midnight(23.0, 23.75, 23.5));

        // Time went from 22:00 to 23:00 -> should NOT cross
        assert!(!crossed_solar_midnight(22.0, 23.0, 23.5));
    }

    #[test]
    fn test_crossed_midnight_late_evening_wrap() {
        use super::crossed_solar_midnight;

        // Solar midnight at 23:30 (23.5)
        // Time went from 23:00 to 0:30 -> should cross (went past 23.5)
        assert!(crossed_solar_midnight(23.0, 0.5, 23.5));
    }

    #[tokio::test]
    async fn test_periodic_update_no_rooms() {
        let mut engine = test_engine();

        let (updated, errors) = engine.periodic_update(12.0).await;

        assert!(updated.is_empty());
        assert!(errors.is_empty());
    }

    #[tokio::test]
    async fn test_periodic_update_with_rhythm_rooms() {
        let mut engine = test_engine();

        // Enable rhythm for two rooms
        engine.rhythm_on("living_room").await.unwrap();
        engine.rhythm_on("bedroom").await.unwrap();

        // NoOpController.any_lights_on() returns false, so periodic_update
        // skips these rooms (lights off) but keeps rhythm_enabled = true
        let (updated, errors) = engine.periodic_update(12.0).await;

        assert!(updated.is_empty());
        assert!(errors.is_empty());
        // Rhythm should still be enabled (lights off just means skip)
        assert!(engine.rooms().is_rhythm_enabled("living_room"));
        assert!(engine.rooms().is_rhythm_enabled("bedroom"));
    }

    #[test]
    fn test_check_solar_midnight_reset() {
        let mut engine = test_engine();

        // Set up some offsets
        engine
            .rooms_mut()
            .get_or_create("room1", "Room 1")
            .apply_time_offset(60.0);
        engine
            .rooms_mut()
            .get_or_create("room2", "Room 2")
            .apply_brightness_offset(20.0);

        // Not crossing midnight - should return false and not reset
        let reset = engine.check_solar_midnight_reset(1.0, 2.0, 0.5);
        assert!(!reset);
        assert_eq!(
            engine.rooms().get("room1").unwrap().time_offset_minutes,
            60.0
        );

        // Crossing midnight - should return true and reset
        let reset = engine.check_solar_midnight_reset(0.0, 1.0, 0.5);
        assert!(reset);
        assert_eq!(
            engine.rooms().get("room1").unwrap().time_offset_minutes,
            0.0
        );
        assert_eq!(engine.rooms().get("room2").unwrap().brightness_offset, 0.0);
    }

    #[test]
    fn test_reset_all_offsets() {
        let mut engine = test_engine();

        // Set up offsets in multiple rooms
        engine
            .rooms_mut()
            .get_or_create("room1", "Room 1")
            .apply_time_offset(30.0);
        engine
            .rooms_mut()
            .get_or_create("room1", "Room 1")
            .apply_brightness_offset(10.0);
        engine
            .rooms_mut()
            .get_or_create("room2", "Room 2")
            .apply_time_offset(-45.0);

        // Reset all
        engine.reset_all_offsets();

        assert_eq!(
            engine.rooms().get("room1").unwrap().time_offset_minutes,
            0.0
        );
        assert_eq!(engine.rooms().get("room1").unwrap().brightness_offset, 0.0);
        assert_eq!(
            engine.rooms().get("room2").unwrap().time_offset_minutes,
            0.0
        );
    }

    // =========================================================================
    // Additional Periodic Update Tests
    // =========================================================================

    #[test]
    fn test_crossed_midnight_exact_boundary() {
        use super::crossed_solar_midnight;

        // Exactly at the midnight hour boundary
        assert!(crossed_solar_midnight(0.0, 0.5, 0.5)); // last < midnight, current == midnight
        assert!(crossed_solar_midnight(0.4, 0.5, 0.5)); // just before to exactly at
        assert!(!crossed_solar_midnight(0.5, 0.6, 0.5)); // at midnight to after (already crossed)
    }

    #[test]
    fn test_crossed_midnight_same_hour() {
        use super::crossed_solar_midnight;

        // Same hour check - should not cross
        assert!(!crossed_solar_midnight(12.0, 12.0, 0.5));
        assert!(!crossed_solar_midnight(0.5, 0.5, 0.5));
    }

    #[test]
    fn test_crossed_midnight_typical_values() {
        use super::crossed_solar_midnight;

        // Typical solar midnight around 0:00-1:00
        // 60 second update interval: 23:59 to 0:00
        assert!(crossed_solar_midnight(23.983, 0.017, 0.0)); // 23:59 to 0:01, midnight at 0:00

        // Solar midnight at 0:30 (common in winter)
        assert!(crossed_solar_midnight(0.4, 0.6, 0.5)); // 0:24 to 0:36
        assert!(!crossed_solar_midnight(0.6, 0.8, 0.5)); // 0:36 to 0:48 (already passed)

        // Solar midnight at 1:00 (summer DST)
        assert!(crossed_solar_midnight(0.9, 1.1, 1.0));
        assert!(!crossed_solar_midnight(1.1, 1.2, 1.0));
    }

    #[test]
    fn test_crossed_midnight_large_gap() {
        use super::crossed_solar_midnight;

        // Large time gaps (e.g., system was suspended)
        // Gap from 22:00 to 2:00 with midnight at 0:30
        assert!(crossed_solar_midnight(22.0, 2.0, 0.5));

        // Gap from 10:00 to 14:00 with midnight at 0:30 - should NOT cross
        assert!(!crossed_solar_midnight(10.0, 14.0, 0.5));
    }

    #[tokio::test]
    async fn test_periodic_update_only_rhythm_enabled() {
        let mut engine = test_engine();

        // Create rooms with mixed rhythm states
        engine.rhythm_on("room1").await.unwrap();
        engine.rooms_mut().get_or_create("room2", "Room 2"); // rhythm disabled
        engine.rhythm_on("room3").await.unwrap();
        engine.rhythm_off("room3").await.unwrap(); // explicitly disabled

        // NoOpController.any_lights_on() returns false — room1 is skipped but stays enabled
        let (updated, errors) = engine.periodic_update(12.0).await;

        assert!(updated.is_empty());
        assert!(errors.is_empty());
        // room1 lights off → skipped, but rhythm stays enabled
        assert!(engine.rooms().is_rhythm_enabled("room1"));
        // room2 never had rhythm, room3 was explicitly off — neither affected
        assert!(!engine.rooms().is_rhythm_enabled("room2"));
        assert!(!engine.rooms().is_rhythm_enabled("room3"));
    }

    #[tokio::test]
    async fn test_periodic_update_preserves_offsets() {
        let mut engine = test_engine();

        // Enable rhythm and set some offsets
        engine.rhythm_on("living_room").await.unwrap();
        engine
            .rooms_mut()
            .get_mut("living_room")
            .unwrap()
            .apply_time_offset(30.0);
        engine
            .rooms_mut()
            .get_mut("living_room")
            .unwrap()
            .apply_brightness_offset(10.0);

        // Periodic update: lights off → skipped, rhythm and offsets preserved
        let (updated, _) = engine.periodic_update(14.0).await;

        assert!(updated.is_empty());
        let room = engine.rooms().get("living_room").unwrap();
        assert_eq!(room.time_offset_minutes, 30.0);
        assert_eq!(room.brightness_offset, 10.0);
        // Rhythm stays enabled, offsets preserved
        assert!(engine.rooms().is_rhythm_enabled("living_room"));
    }

    #[tokio::test]
    async fn test_periodic_update_skips_on_lights_off() {
        let mut engine = test_engine();

        engine.rhythm_on("room1").await.unwrap();
        assert!(engine.rooms().is_rhythm_enabled("room1"));

        // First tick: lights off (NoOpController) → skipped, rhythm stays enabled
        let (updated, _) = engine.periodic_update(9.0).await;
        assert!(updated.is_empty());
        assert!(engine.rooms().is_rhythm_enabled("room1"));

        // Subsequent ticks: still skipped (lights still off), rhythm still enabled
        let (updated, _) = engine.periodic_update(10.0).await;
        assert!(updated.is_empty());
        assert!(engine.rooms().is_rhythm_enabled("room1"));
    }

    #[tokio::test]
    async fn test_periodic_tick_dedupes_identical_commands() {
        let (mut engine, spy) = spy_engine();

        engine.rhythm_on("room1").await.unwrap();
        spy.set_any_lights_on(true);

        let first = engine.periodic_tick_single_room("room1", 12.0).await;
        let second = engine.periodic_tick_single_room("room1", 12.0).await;

        assert!(matches!(first, PeriodicTickResult::Updated));
        assert!(matches!(second, PeriodicTickResult::Skipped));
        assert_eq!(spy.turn_on_count(), 1);
    }

    #[tokio::test]
    async fn test_periodic_tick_forces_refresh_after_repeated_skips() {
        let (mut engine, spy) = spy_engine();

        engine.rhythm_on("room1").await.unwrap();
        spy.set_any_lights_on(true);

        let runs = usize::from(PERIODIC_DEDUPE_MAX_SKIPS) + 2;
        for _ in 0..runs {
            let _ = engine.periodic_tick_single_room("room1", 12.0).await;
        }

        assert_eq!(spy.turn_on_count(), 2);
    }

    #[tokio::test]
    async fn test_manual_turn_on_seeds_periodic_dedupe_cache() {
        let (mut engine, spy) = spy_engine();

        engine.rhythm_on("room1").await.unwrap();
        spy.set_any_lights_on(true);

        assert!(matches!(
            engine.periodic_tick_single_room("room1", 12.0).await,
            PeriodicTickResult::Updated
        ));
        assert!(matches!(
            engine.periodic_tick_single_room("room1", 12.0).await,
            PeriodicTickResult::Skipped
        ));

        spy.reset();
        engine.turn_on("room1", 12.0).await.unwrap();
        assert_eq!(spy.turn_on_count(), 1);

        spy.reset();
        assert!(matches!(
            engine.periodic_tick_single_room("room1", 12.0).await,
            PeriodicTickResult::Skipped
        ));
        assert_eq!(spy.turn_on_count(), 0);
    }

    // Regression for #70: the periodic worker used to keep dispatching the
    // Active brightness curve during the motion-timeout warning-dim window,
    // visibly overwriting the dim a few seconds after it was applied. The
    // user saw "on → dim → on → off" — a "weird double on/off".
    #[tokio::test]
    async fn test_periodic_tick_skipped_during_motion_warning_dim() {
        let (mut engine, spy) = spy_engine();

        // Enable rhythm and apply the warning dim at hour=12. A factor < 1.0
        // marks the room as warning_active.
        spy.set_any_lights_on(true);
        enter_warning_dim(&mut engine).await;
        let dim_brightness = spy
            .last_command_for("room1")
            .expect("dim should dispatch")
            .brightness;
        spy.reset();

        // Periodic tick at a different hour produces a different brightness
        // command than what's cached, so dedupe would normally let it
        // through. The warning_active flag must short-circuit the tick or
        // the dim is undone within ~60s — the user-visible "double on/off".
        assert!(matches!(
            engine.periodic_tick_single_room("room1", 18.0).await,
            PeriodicTickResult::Skipped
        ));
        assert_eq!(
            spy.turn_on_count(),
            0,
            "periodic tick must not dispatch while warning dim is active"
        );

        // Motion returns: factor=1.0 clears the warning and restores
        // brightness. Periodic ticks resume on the next cycle.
        engine.dim_to_factor("room1", 12.0, 1.0).await.unwrap();
        assert!(!engine.rooms.get("room1").unwrap().warning_active);
        let restored_brightness = spy
            .last_command_for("room1")
            .expect("restore should dispatch")
            .brightness;
        assert!(
            restored_brightness > dim_brightness,
            "restoring (factor=1.0) should brighten past the dim level"
        );
        spy.reset();

        assert!(matches!(
            engine.periodic_tick_single_room("room1", 18.0).await,
            PeriodicTickResult::Updated
        ));
        assert_eq!(spy.turn_on_count(), 1);
    }

    #[tokio::test]
    async fn test_explicit_async_actions_clear_motion_warning_dim() {
        let (mut engine, _spy) = spy_engine();

        enter_warning_dim(&mut engine).await;
        engine.reset("room1", 12.0).await.unwrap();
        assert!(!engine.rooms.get("room1").unwrap().warning_active);

        enter_warning_dim(&mut engine).await;
        engine.turn_off("room1", 12.0).await.unwrap();
        assert!(!engine.rooms.get("room1").unwrap().warning_active);

        enter_warning_dim(&mut engine).await;
        engine.lights_off("room1", None).await.unwrap();
        assert!(!engine.rooms.get("room1").unwrap().warning_active);

        enter_warning_dim(&mut engine).await;
        engine.set_time_offset("room1", 12.0, 30.0).await.unwrap();
        assert!(!engine.rooms.get("room1").unwrap().warning_active);

        enter_warning_dim(&mut engine).await;
        engine.soft_off_tick("room1", 12.0).await.unwrap();
        assert!(!engine.rooms.get("room1").unwrap().warning_active);

        enter_warning_dim(&mut engine).await;
        engine.set_power_save(true);
        assert!(!engine.rooms.get("room1").unwrap().warning_active);
    }

    #[tokio::test]
    async fn test_manual_room_action_invalidates_periodic_node_cache_for_same_room() {
        let (mut engine, spy) = spy_engine();

        engine.rhythm_on("room1").await.unwrap();
        spy.set_any_lights_on(true);

        assert!(matches!(
            engine.periodic_tick_node("node-a", "room1", 12.0).await,
            PeriodicTickResult::Updated
        ));
        assert!(matches!(
            engine.periodic_tick_node("node-a", "room1", 12.0).await,
            PeriodicTickResult::Skipped
        ));

        spy.reset();
        engine.turn_on("room1", 12.0).await.unwrap();
        assert_eq!(spy.turn_on_count(), 1);

        spy.reset();
        assert!(matches!(
            engine.periodic_tick_node("node-a", "room1", 12.0).await,
            PeriodicTickResult::Updated
        ));
        assert_eq!(spy.turn_on_count(), 1);
    }

    #[tokio::test]
    async fn test_periodic_tick_node_uses_effective_child_overrides() {
        let (mut engine, spy) = spy_engine();

        let parent = engine.rooms_mut().get_or_create("room1", "Room 1");
        parent.rhythm_enabled = true;
        parent.time_offset_minutes = 30.0;
        parent.brightness_offset = 10.0;
        parent.profile_settings.profile_id = Some("sleep".into());

        let child = engine.rooms_mut().get_or_create_node(
            "device-1",
            "Device 1",
            crate::room::LightNodeKind::LightDevice,
            Some("room1".into()),
        );
        child.profile_settings.fade_ms = Some(crate::TimerSetting::Fixed { value: 1234 });
        child.brightness_offset = -3.0;

        let expected = engine.rooms_mut().get_or_create("expected", "Expected");
        expected.rhythm_enabled = true;
        expected.time_offset_minutes = 30.0;
        expected.brightness_offset = 7.0;
        expected.profile_settings.profile_id = Some("sleep".into());
        expected.profile_settings.fade_ms = Some(crate::TimerSetting::Fixed { value: 1234 });

        spy.set_any_lights_on(true);

        assert!(matches!(
            engine
                .periodic_tick_node("device-1", "device-1", 12.0)
                .await,
            PeriodicTickResult::Updated
        ));
        assert!(matches!(
            engine.periodic_tick_single_room("expected", 12.0).await,
            PeriodicTickResult::Updated
        ));

        let child_cmd = spy.last_command_for("device-1").unwrap();
        let expected_cmd = spy.last_command_for("expected").unwrap();
        assert_eq!(child_cmd, expected_cmd);
    }

    #[tokio::test]
    async fn test_periodic_update_different_times() {
        let mut engine = test_engine();

        // Update at different times — all skip (NoOpController: lights off),
        // but rhythm stays enabled throughout
        engine.rhythm_on("room1").await.unwrap();
        let times = [0.0, 6.0, 12.0, 18.0, 23.5];
        for time in times {
            let (updated, errors) = engine.periodic_update(time).await;
            assert!(
                updated.is_empty(),
                "Should skip at time {} (lights off)",
                time
            );
            assert!(errors.is_empty(), "Errors at time {}: {:?}", time, errors);
            assert!(
                engine.rooms().is_rhythm_enabled("room1"),
                "Rhythm should stay enabled at time {}",
                time
            );
        }
    }

    #[test]
    fn test_rhythm_enabled_room_ids() {
        let mut engine = test_engine();

        // No rooms initially
        assert!(engine.rhythm_enabled_room_ids().is_empty());

        // Add rooms with mixed states
        engine
            .rooms_mut()
            .get_or_create("room1", "Room 1")
            .enable_rhythm();
        engine.rooms_mut().get_or_create("room2", "Room 2"); // disabled
        engine
            .rooms_mut()
            .get_or_create("room3", "Room 3")
            .enable_rhythm();

        let ids = engine.rhythm_enabled_room_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"room1".to_string()));
        assert!(ids.contains(&"room3".to_string()));
        assert!(!ids.contains(&"room2".to_string()));
    }

    #[test]
    fn test_check_solar_midnight_preserves_rhythm_state() {
        let mut engine = test_engine();

        // Set up rooms with rhythm enabled and offsets
        let room1 = engine.rooms_mut().get_or_create("room1", "Room 1");
        room1.enable_rhythm();
        room1.apply_time_offset(60.0);

        let room2 = engine.rooms_mut().get_or_create("room2", "Room 2");
        room2.enable_rhythm();
        room2.apply_brightness_offset(20.0);

        // Crossing midnight resets offsets but keeps rhythm enabled
        let reset = engine.check_solar_midnight_reset(0.0, 1.0, 0.5);
        assert!(reset);

        // Offsets should be reset
        assert_eq!(
            engine.rooms().get("room1").unwrap().time_offset_minutes,
            0.0
        );
        assert_eq!(engine.rooms().get("room2").unwrap().brightness_offset, 0.0);

        // Rhythm should still be enabled
        assert!(engine.rooms().is_rhythm_enabled("room1"));
        assert!(engine.rooms().is_rhythm_enabled("room2"));
    }

    #[test]
    fn test_reset_all_offsets_preserves_rhythm_state() {
        let mut engine = test_engine();

        // Set up rooms
        let room = engine.rooms_mut().get_or_create("room1", "Room 1");
        room.enable_rhythm();
        room.apply_time_offset(120.0);
        room.apply_brightness_offset(-15.0);

        // Reset offsets
        engine.reset_all_offsets();

        // Rhythm should still be enabled
        assert!(engine.rooms().is_rhythm_enabled("room1"));
        // Offsets should be zero
        assert_eq!(
            engine.rooms().get("room1").unwrap().time_offset_minutes,
            0.0
        );
        assert_eq!(engine.rooms().get("room1").unwrap().brightness_offset, 0.0);
    }

    #[test]
    fn test_check_solar_midnight_no_double_reset() {
        let mut engine = test_engine();

        // Set up room with offset
        engine
            .rooms_mut()
            .get_or_create("room1", "Room 1")
            .apply_time_offset(60.0);

        // First crossing resets
        let reset1 = engine.check_solar_midnight_reset(0.0, 1.0, 0.5);
        assert!(reset1);
        assert_eq!(
            engine.rooms().get("room1").unwrap().time_offset_minutes,
            0.0
        );

        // Add new offset
        engine
            .rooms_mut()
            .get_mut("room1")
            .unwrap()
            .apply_time_offset(30.0);

        // Same time check again - should NOT reset (no crossing)
        let reset2 = engine.check_solar_midnight_reset(1.0, 1.5, 0.5);
        assert!(!reset2);
        assert_eq!(
            engine.rooms().get("room1").unwrap().time_offset_minutes,
            30.0
        );
    }

    // =========================================================================
    // Edge Case Tests - Actions on Disabled Rooms
    // =========================================================================

    #[tokio::test]
    async fn test_rhythm_on_disabled_room() {
        let mut engine = test_engine();

        // Create and disable room
        engine
            .rooms_mut()
            .get_or_create("room1", "Room 1")
            .set_disabled(true);

        // rhythm_on should still work (disabled doesn't block manual actions)
        let result = engine.rhythm_on("room1").await;
        assert!(result.is_ok());
        assert!(engine.rooms.is_rhythm_enabled("room1"));
    }

    #[tokio::test]
    async fn test_periodic_update_includes_rhythm_enabled_rooms() {
        let mut engine = test_engine();

        // Create two rhythm-enabled rooms
        engine.rhythm_on("room1").await.unwrap();
        engine.rhythm_on("room2").await.unwrap();

        // Disable rhythm on room2
        engine.rhythm_off("room2").await.unwrap();

        // Periodic update: room1 lights off → skipped (rhythm stays), room2 already off
        let (updated, _) = engine.periodic_update(14.0).await;

        assert!(updated.is_empty());
        assert!(engine.rooms().is_rhythm_enabled("room1"));
        assert!(!engine.rooms().is_rhythm_enabled("room2"));
    }

    #[tokio::test]
    async fn test_disabled_room_with_rhythm_still_checked() {
        let mut engine = test_engine();

        // Note: disabled flag is separate from rhythm_enabled
        // A disabled room with rhythm_enabled will still be checked in periodic_update
        engine.rhythm_on("room1").await.unwrap();
        engine
            .rooms_mut()
            .get_mut("room1")
            .unwrap()
            .set_disabled(true);

        let (updated, _) = engine.periodic_update(14.0).await;

        // NoOpController: lights off → skipped, but rhythm stays enabled
        assert!(updated.is_empty());
        assert!(engine.rooms().is_rhythm_enabled("room1"));
    }

    #[tokio::test]
    async fn test_step_on_nonexistent_room() {
        let mut engine = test_engine();

        // Step on room that doesn't exist - should create it
        let result = engine.step_up("new_room", 12.0).await;
        assert!(result.is_ok());

        // Room should now exist with an offset
        let room = engine.rooms().get("new_room");
        assert!(room.is_some());
    }

    // =========================================================================
    // Edge Case Tests - Time Boundaries
    // =========================================================================

    #[tokio::test]
    async fn test_turn_on_at_midnight() {
        let mut engine = test_engine();

        // Test at exactly midnight (0.0)
        let result = engine.turn_on("room1", 0.0).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_turn_on_at_end_of_day() {
        let mut engine = test_engine();

        // Test at 23:59 (23.983...)
        let result = engine.turn_on("room1", 23.983).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_current_values_at_boundaries() {
        let engine = test_engine();

        // At midnight - should be dim and warm
        let cmd_midnight = engine.get_current_values("room1", 0.0);
        assert!(cmd_midnight.brightness <= 50);
        assert!(cmd_midnight.kelvin <= 3500);

        // At noon - should be bright and cool
        let cmd_noon = engine.get_current_values("room1", 12.0);
        assert!(cmd_noon.brightness >= 90);
        assert!(cmd_noon.kelvin >= rhythm_profile::DEFAULT_MAX_COLOR_TEMP - 100);

        // At end of day (23.99)
        let cmd_late = engine.get_current_values("room1", 23.99);
        assert!(cmd_late.brightness <= 50);
        assert!(cmd_late.kelvin <= 3500);
    }

    #[test]
    fn test_get_current_values_handles_hour_24() {
        let engine = test_engine();

        // Hour 24.0 should be treated as midnight (0.0)
        let cmd_24 = engine.get_current_values("room1", 24.0);
        let cmd_0 = engine.get_current_values("room1", 0.0);

        // Values should be similar (allowing for floating point)
        assert!((cmd_24.brightness as i32 - cmd_0.brightness as i32).abs() <= 1);
    }

    // =========================================================================
    // Edge Case Tests - Offset Boundaries
    // =========================================================================

    #[tokio::test]
    async fn test_multiple_step_ups_accumulate() {
        let mut engine = test_engine();

        // Multiple step ups should accumulate offset
        for _ in 0..5 {
            engine.step_up("room1", 8.0).await.unwrap();
        }

        let room = engine.rooms().get("room1").unwrap();
        assert!(room.time_offset_minutes > 0.0);
    }

    #[tokio::test]
    async fn test_step_up_then_step_down_changes_offset() {
        let mut engine = test_engine();

        // Step up a few times
        for _ in 0..3 {
            engine.step_up("room1", 8.0).await.unwrap();
        }

        let offset_after_up = engine.rooms().get("room1").unwrap().time_offset_minutes;
        assert!(offset_after_up > 0.0, "Step up should increase offset");

        // Step down same number of times
        for _ in 0..3 {
            engine.step_down("room1", 8.0).await.unwrap();
        }

        let offset_after_down = engine.rooms().get("room1").unwrap().time_offset_minutes;
        // After stepping down, offset should decrease (may not be zero due to curve shape)
        assert!(
            offset_after_down < offset_after_up,
            "Step down should decrease offset"
        );
    }

    #[tokio::test]
    async fn test_brightness_offset_clamps_to_range() {
        let mut engine = test_engine();

        // Brightness offset is clamped to [-100, 100]
        engine.dim_up("room1", 12.0, Some(200.0)).await.unwrap();
        let room = engine.rooms().get("room1").unwrap();
        // Offset should be clamped to 100
        assert_eq!(room.brightness_offset, 100.0);

        // Reset and test negative clamping
        engine.rooms_mut().get_mut("room1").unwrap().reset_offsets();
        engine.dim_down("room1", 12.0, Some(200.0)).await.unwrap();
        let room = engine.rooms().get("room1").unwrap();
        assert_eq!(room.brightness_offset, -100.0);
    }

    // =========================================================================
    // Module and Config Tests
    // =========================================================================

    #[test]
    fn test_available_profiles() {
        let engine = test_engine();
        let profiles = engine.available_profiles();

        assert!(!profiles.is_empty());
        assert!(profiles.iter().any(|(id, _)| *id == "rhythm"));
    }

    #[test]
    fn test_set_invalid_light_profile() {
        let mut engine = test_engine();

        // Setting a non-existent profile should return false
        let result = engine.set_light_profile("nonexistent");
        assert!(!result);
    }

    #[test]
    fn test_set_valid_light_profile() {
        let mut engine = test_engine();

        // Setting the default profile should work
        let result = engine.set_light_profile("rhythm");
        assert!(result);
    }

    // =========================================================================
    // Legacy idle/soft-off compatibility tests
    // =========================================================================

    #[tokio::test]
    async fn test_turn_off_uses_hard_off() {
        let (mut engine, spy) = spy_engine();

        engine.set_power_save(false);
        engine.rhythm_on("room1").await.unwrap();
        spy.reset();
        engine.turn_off("room1", 12.0).await.unwrap();

        assert_eq!(spy.turn_on_count(), 0, "off should not dim via turn_on");
        assert_eq!(spy.turn_off_calls().len(), 1, "off should hard-off");
        let room = engine.rooms().get("room1").unwrap();
        assert!(room.hard_off);
        assert!(!room.soft_off);
    }

    #[tokio::test]
    async fn test_lights_off_passes_transition_to_controller() {
        let (mut engine, spy) = spy_engine();

        engine.lights_off("room1", Some(1_200)).await.unwrap();

        assert_eq!(
            spy.turn_off_with_transition_calls(),
            vec![("room1".to_string(), Some(1_200))]
        );
        let room = engine.rooms().get("room1").unwrap();
        assert!(room.hard_off);
        assert!(!room.soft_off);
    }

    #[tokio::test]
    async fn test_soft_off_tick_renders_standby() {
        let (mut engine, spy) = spy_engine();

        engine.set_power_save(false);
        engine.rhythm_on("room1").await.unwrap();
        engine.turn_off("room1", 12.0).await.unwrap();

        spy.reset();
        engine.soft_off_tick("room1", 14.0).await.unwrap();

        assert_eq!(spy.turn_on_count(), 1);
        assert_eq!(spy.turn_off_calls().len(), 0);
        let room = engine.rooms().get("room1").unwrap();
        assert!(!room.hard_off);
        assert!(room.soft_off);
    }

    #[tokio::test]
    async fn test_idle_brightness_returns_curve_value() {
        let (engine, _spy) = spy_engine();
        assert_eq!(engine.idle_brightness(12.0), 1);
        assert_eq!(engine.idle_brightness(0.0), 1);
        assert_eq!(engine.idle_brightness(23.5), 1);
    }

    #[tokio::test]
    async fn test_set_time_offset_hard_off_does_not_dispatch() {
        let (mut engine, spy) = spy_engine();

        engine.set_power_save(false);
        engine.rhythm_on("room1").await.unwrap();
        engine.turn_off("room1", 12.0).await.unwrap();

        spy.reset();
        engine.set_time_offset("room1", 14.0, 30.0).await.unwrap();

        assert_eq!(spy.turn_on_count(), 0);
        assert_eq!(spy.turn_off_calls().len(), 0);
        let room = engine.rooms().get("room1").unwrap();
        assert!(room.hard_off);
        assert!(!room.soft_off);
        assert_eq!(room.time_offset_minutes, 30.0);
    }
}
