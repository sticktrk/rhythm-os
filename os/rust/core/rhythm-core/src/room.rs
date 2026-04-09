//! Room/Area abstraction for adaptive lighting.
//!
//! This module provides the `Room` struct which represents a room or area
//! that can have Rhythm lighting enabled, along with `RoomManager` for
//! tracking multiple rooms.

use std::collections::HashMap;

use crate::light_profile::{
    is_builtin_state_profile_id, LightProfileConfig, TimerSetting, DAY_IDLE_PROFILE_ID,
    RHYTHM_PROFILE_ID, SLEEP_IDLE_PROFILE_ID, SLEEP_PROFILE_ID,
};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

const REMOVED_LEGACY_IDLE_PROFILE_ID: &str = "idle";

fn is_removed_legacy_idle_profile_id(id: &str) -> bool {
    id == REMOVED_LEGACY_IDLE_PROFILE_ID
}

/// High-level global mode selected by the user.
///
/// This currently maps onto the built-in active profiles:
/// - `day` -> any non-sleep active profile
/// - `sleep` -> the built-in sleep profile
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum RhythmMode {
    #[default]
    Day,
    Sleep,
}

impl RhythmMode {
    pub const ALL: [Self; 2] = [Self::Day, Self::Sleep];

    /// Resolve the current high-level mode from the active profile ID.
    pub fn from_profile_id(profile_id: &str) -> Self {
        if profile_id == SLEEP_PROFILE_ID {
            Self::Sleep
        } else {
            Self::Day
        }
    }

    /// Built-in fallback active profile for this mode.
    pub fn default_active_profile_id(self) -> &'static str {
        match self {
            Self::Day => RHYTHM_PROFILE_ID,
            Self::Sleep => SLEEP_PROFILE_ID,
        }
    }

    /// Built-in fallback idle profile for this mode.
    pub fn default_idle_profile_id(self) -> &'static str {
        match self {
            Self::Day => DAY_IDLE_PROFILE_ID,
            Self::Sleep => SLEEP_IDLE_PROFILE_ID,
        }
    }
}

/// User-facing room state within the current mode.
///
/// This is intentionally separate from the stored room config so the runtime
/// can evolve toward mode-specific states (for example `wake`) without
/// hard-coding behavior into the room struct itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum RoomModeState {
    #[default]
    Active,
    Idle,
    Wake,
    Warning,
    HardOff,
}

impl RoomModeState {
    /// Derive the current user-facing room state from existing runtime flags.
    pub fn from_flags(hard_off: bool, soft_off: bool, warning_active: bool) -> Self {
        if hard_off {
            Self::HardOff
        } else if warning_active {
            Self::Warning
        } else if soft_off {
            Self::Idle
        } else {
            Self::Active
        }
    }
}

/// Profile mapping for the room states inside one high-level mode.
///
/// The active state continues to respect the currently selected active profile
/// when it already belongs to this mode. The stored `active_profile_id` acts as
/// the shareable fallback/default for this mode when needed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ModeConfig {
    pub mode: RhythmMode,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub active_profile_id: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub idle_profile_id: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub wake_profile_id: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub warning_profile_id: Option<String>,
}

impl ModeConfig {
    /// Normalize invalid profile selections into mode-specific built-ins.
    ///
    /// Active profiles must never point at state-only profiles.
    pub fn normalize_profile_ids(&mut self) -> bool {
        let mut changed = false;

        if self.active_profile_id.as_deref().is_some_and(|id| {
            is_builtin_state_profile_id(id) || is_removed_legacy_idle_profile_id(id)
        }) {
            self.active_profile_id = Some(self.mode.default_active_profile_id().to_string());
            changed = true;
        }

        if self
            .idle_profile_id
            .as_deref()
            .is_some_and(is_removed_legacy_idle_profile_id)
        {
            self.idle_profile_id = None;
            changed = true;
        }

        changed
    }

    pub fn default_for_mode(mode: RhythmMode) -> Self {
        Self {
            mode,
            active_profile_id: Some(mode.default_active_profile_id().to_string()),
            idle_profile_id: None,
            wake_profile_id: None,
            warning_profile_id: None,
        }
    }

    /// Resolve which stored profile ID should back the requested room state.
    pub fn resolve_state_profile_id<'a>(
        &'a self,
        state: RoomModeState,
        active_profile_id: &'a str,
    ) -> Option<&'a str> {
        match state {
            RoomModeState::Active => Some(
                self.active_profile_id
                    .as_deref()
                    .filter(|id| {
                        !is_builtin_state_profile_id(id) && !is_removed_legacy_idle_profile_id(id)
                    })
                    .unwrap_or(active_profile_id),
            ),
            RoomModeState::Idle => self
                .idle_profile_id
                .as_deref()
                .filter(|id| !is_removed_legacy_idle_profile_id(id)),
            RoomModeState::Wake => {
                Some(self.wake_profile_id.as_deref().unwrap_or(active_profile_id))
            }
            RoomModeState::Warning => Some(
                self.warning_profile_id
                    .as_deref()
                    .unwrap_or(active_profile_id),
            ),
            RoomModeState::HardOff => self
                .idle_profile_id
                .as_deref()
                .filter(|id| !is_removed_legacy_idle_profile_id(id)),
        }
    }
}

pub fn default_mode_configs() -> Vec<ModeConfig> {
    RhythmMode::ALL
        .into_iter()
        .map(ModeConfig::default_for_mode)
        .collect()
}

/// Trigger that initiates a configured mode transition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ModeTransitionTrigger {
    #[default]
    Manual,
    Sunrise,
    Sunset,
    CivilTwilight,
    NauticalTwilight,
    AstronomicalTwilight,
}

fn default_preserve_hard_off() -> bool {
    true
}

fn default_mode_transition_duration_ms() -> u32 {
    5_000
}

/// Configured transition between two high-level modes.
///
/// Transitions operate in rendered output space: the runtime captures the
/// current visible room output and fades it to the target mode/state output.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ModeTransitionConfig {
    pub from_mode: RhythmMode,
    pub to_mode: RhythmMode,
    #[cfg_attr(feature = "serde", serde(default))]
    pub trigger: ModeTransitionTrigger,
    #[cfg_attr(
        feature = "serde",
        serde(default = "default_mode_transition_duration_ms")
    )]
    pub duration_ms: u32,
    #[cfg_attr(feature = "serde", serde(default = "default_preserve_hard_off"))]
    pub preserve_hard_off: bool,
}

impl ModeTransitionConfig {
    pub fn new(from_mode: RhythmMode, to_mode: RhythmMode, duration_ms: u32) -> Self {
        Self {
            from_mode,
            to_mode,
            trigger: ModeTransitionTrigger::Manual,
            duration_ms,
            preserve_hard_off: true,
        }
    }

    pub fn with_trigger(mut self, trigger: ModeTransitionTrigger) -> Self {
        self.trigger = trigger;
        self
    }
}

pub fn default_mode_transition_configs() -> Vec<ModeTransitionConfig> {
    vec![
        ModeTransitionConfig::new(
            RhythmMode::Sleep,
            RhythmMode::Day,
            default_mode_transition_duration_ms(),
        )
        .with_trigger(ModeTransitionTrigger::AstronomicalTwilight),
        ModeTransitionConfig::new(
            RhythmMode::Day,
            RhythmMode::Sleep,
            default_mode_transition_duration_ms(),
        )
        .with_trigger(ModeTransitionTrigger::NauticalTwilight),
    ]
}

/// Per-room light profile selection and timer overrides.
///
/// This layer sits on top of the globally active profile:
/// - `profile_id`: optionally selects a different stored base profile for this room
/// - timer fields: optionally override the selected profile's timer settings
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RoomProfileSettings {
    /// Optional stored profile ID to use instead of the global active profile.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub profile_id: Option<String>,

    /// Optional per-room fade override.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub fade_ms: Option<TimerSetting>,

    /// Optional per-room motion timeout override.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub motion_timeout_secs: Option<TimerSetting>,
}

impl RoomProfileSettings {
    /// Returns `true` when this room uses the global active profile unchanged.
    pub fn is_empty(&self) -> bool {
        self.profile_id.is_none() && self.fade_ms.is_none() && self.motion_timeout_secs.is_none()
    }

    /// Resolve which stored profile ID should back this room.
    pub fn resolved_profile_id<'a>(&'a self, active_profile_id: &'a str) -> &'a str {
        self.profile_id.as_deref().unwrap_or(active_profile_id)
    }

    /// Apply any per-room timer overrides to a profile config.
    pub fn apply_to_config(&self, config: &mut LightProfileConfig) {
        if let Some(fade_ms) = &self.fade_ms {
            config.fade_ms = fade_ms.clone();
        }
        if let Some(motion_timeout_secs) = &self.motion_timeout_secs {
            config.motion_timeout_secs = motion_timeout_secs.clone();
        }
    }
}

/// A room or area that can have Rhythm lighting enabled.
///
/// Each room tracks its Rhythm state and any time/brightness offsets
/// from manual adjustments (step up/down).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Room {
    /// Unique identifier for the room
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Whether Rhythm mode is enabled for this room
    pub rhythm_enabled: bool,

    /// Whether this room is disabled (excluded from kiosk mode)
    #[cfg_attr(feature = "serde", serde(default))]
    pub disabled: bool,

    /// Time offset in minutes from current solar time
    /// (positive = brighter/cooler, negative = dimmer/warmer)
    pub time_offset_minutes: f32,

    /// Direct brightness offset (for dim_up/dim_down)
    pub brightness_offset: f32,

    /// Whether this room is in "soft off" state (at soft-off brightness level).
    /// Used when power_save is disabled: lights dim to the configured
    /// soft-off brightness instead of turning fully off, maintaining
    /// color temperature readiness.
    #[cfg_attr(feature = "serde", serde(default))]
    pub soft_off: bool,

    /// Whether this room is intentionally fully off.
    ///
    /// Unlike `soft_off`, this is a true off state entered by explicit actions
    /// such as a bottom-button long press.
    #[cfg_attr(feature = "serde", serde(default))]
    pub hard_off: bool,

    /// Optional per-room base profile selection and timer overrides.
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            rename = "room_profile",
            skip_serializing_if = "RoomProfileSettings::is_empty"
        )
    )]
    pub profile_settings: RoomProfileSettings,
}

impl Room {
    /// Create a new room with default state (Rhythm disabled, no offsets).
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            rhythm_enabled: false,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off: false,
            hard_off: false,
            profile_settings: RoomProfileSettings::default(),
        }
    }

    /// Enable Rhythm mode for this room.
    pub fn enable_rhythm(&mut self) {
        self.rhythm_enabled = true;
    }

    /// Disable Rhythm mode for this room.
    pub fn disable_rhythm(&mut self) {
        self.rhythm_enabled = false;
    }

    /// Toggle Rhythm mode for this room.
    pub fn toggle_rhythm(&mut self) -> bool {
        self.rhythm_enabled = !self.rhythm_enabled;
        self.rhythm_enabled
    }

    /// Reset offsets to zero (return to current solar time).
    pub fn reset_offsets(&mut self) {
        self.time_offset_minutes = 0.0;
        self.brightness_offset = 0.0;
    }

    /// Apply a time offset (from step_up/step_down).
    pub fn apply_time_offset(&mut self, offset_minutes: f32) {
        self.time_offset_minutes += offset_minutes;
    }

    /// Set the time offset directly.
    pub fn set_time_offset(&mut self, offset_minutes: f32) {
        self.time_offset_minutes = offset_minutes;
    }

    /// Apply a brightness offset (from dim_up/dim_down).
    pub fn apply_brightness_offset(&mut self, offset: f32) {
        self.brightness_offset = (self.brightness_offset + offset).clamp(-100.0, 100.0);
    }

    /// Get the effective time offset including any adjustments.
    pub fn effective_time_offset(&self) -> f32 {
        self.time_offset_minutes
    }

    /// Get the effective brightness offset.
    pub fn effective_brightness_offset(&self) -> f32 {
        self.brightness_offset
    }

    /// Set the disabled state for this room.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    /// Check if this room is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

impl Default for Room {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            name: "Default Room".to_string(),
            rhythm_enabled: false,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off: false,
            hard_off: false,
            profile_settings: RoomProfileSettings::default(),
        }
    }
}

/// Manager for tracking multiple rooms.
///
/// Provides methods to add, remove, and query rooms, as well as
/// bulk operations for Rhythm mode management.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RoomManager {
    rooms: HashMap<String, Room>,
}

impl RoomManager {
    /// Create a new empty room manager.
    pub fn new() -> Self {
        Self {
            rooms: HashMap::new(),
        }
    }

    /// Add a room to the manager.
    ///
    /// If a room with the same ID already exists, it is replaced.
    pub fn add_room(&mut self, room: Room) {
        self.rooms.insert(room.id.clone(), room);
    }

    /// Add a room by ID and name.
    pub fn add(&mut self, id: impl Into<String>, name: impl Into<String>) {
        let room = Room::new(id, name);
        self.rooms.insert(room.id.clone(), room);
    }

    /// Remove a room by ID.
    pub fn remove(&mut self, id: &str) -> Option<Room> {
        self.rooms.remove(id)
    }

    /// Get a room by ID.
    pub fn get(&self, id: &str) -> Option<&Room> {
        self.rooms.get(id)
    }

    /// Get a mutable reference to a room by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Room> {
        self.rooms.get_mut(id)
    }

    /// Get or create a room by ID.
    ///
    /// If the room doesn't exist, creates it with the given name.
    pub fn get_or_create(&mut self, id: impl Into<String>, name: impl Into<String>) -> &mut Room {
        let id = id.into();
        if !self.rooms.contains_key(&id) {
            let name = name.into();
            self.rooms.insert(id.clone(), Room::new(id.clone(), name));
        }
        self.rooms.get_mut(&id).unwrap()
    }

    /// Check if a room exists.
    pub fn contains(&self, id: &str) -> bool {
        self.rooms.contains_key(id)
    }

    /// Check if Rhythm is enabled for a room.
    pub fn is_rhythm_enabled(&self, id: &str) -> bool {
        self.rooms
            .get(id)
            .map(|r| r.rhythm_enabled)
            .unwrap_or(false)
    }

    /// Enable Rhythm for a room.
    pub fn enable_rhythm(&mut self, id: &str) -> bool {
        if let Some(room) = self.rooms.get_mut(id) {
            room.enable_rhythm();
            true
        } else {
            false
        }
    }

    /// Disable Rhythm for a room.
    pub fn disable_rhythm(&mut self, id: &str) -> bool {
        if let Some(room) = self.rooms.get_mut(id) {
            room.disable_rhythm();
            true
        } else {
            false
        }
    }

    /// Get all rooms.
    pub fn all_rooms(&self) -> Vec<&Room> {
        self.rooms.values().collect()
    }

    /// Get all room IDs.
    pub fn room_ids(&self) -> Vec<&str> {
        self.rooms.keys().map(|s| s.as_str()).collect()
    }

    /// Get all rooms with Rhythm enabled.
    pub fn rhythm_enabled_rooms(&self) -> Vec<&Room> {
        self.rooms.values().filter(|r| r.rhythm_enabled).collect()
    }

    /// Get the number of rooms.
    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    /// Check if the manager is empty.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Get the number of rooms with Rhythm enabled.
    pub fn rhythm_enabled_count(&self) -> usize {
        self.rooms.values().filter(|r| r.rhythm_enabled).count()
    }

    /// Iterate over all rooms.
    pub fn iter(&self) -> impl Iterator<Item = &Room> {
        self.rooms.values()
    }

    /// Iterate over all rooms mutably.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Room> {
        self.rooms.values_mut()
    }

    /// Get all rooms that are not disabled.
    ///
    /// Useful for kiosk mode which should skip disabled rooms.
    pub fn enabled_rooms(&self) -> Vec<&Room> {
        self.rooms.values().filter(|r| !r.disabled).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_room_new() {
        let room = Room::new("living_room", "Living Room");
        assert_eq!(room.id, "living_room");
        assert_eq!(room.name, "Living Room");
        assert!(!room.rhythm_enabled);
        assert!(!room.disabled);
        assert_eq!(room.time_offset_minutes, 0.0);
        assert_eq!(room.brightness_offset, 0.0);
        assert!(!room.soft_off);
        assert!(!room.hard_off);
        assert!(room.profile_settings.is_empty());
    }

    #[test]
    fn test_room_disabled() {
        let mut room = Room::new("test", "Test");
        assert!(!room.is_disabled());

        room.set_disabled(true);
        assert!(room.is_disabled());

        room.set_disabled(false);
        assert!(!room.is_disabled());
    }

    #[test]
    fn test_room_rhythm_toggle() {
        let mut room = Room::new("test", "Test");
        assert!(!room.rhythm_enabled);

        room.enable_rhythm();
        assert!(room.rhythm_enabled);

        room.disable_rhythm();
        assert!(!room.rhythm_enabled);

        let result = room.toggle_rhythm();
        assert!(result);
        assert!(room.rhythm_enabled);
    }

    #[test]
    fn test_room_offsets() {
        let mut room = Room::new("test", "Test");

        room.apply_time_offset(30.0);
        assert_eq!(room.time_offset_minutes, 30.0);

        room.apply_time_offset(-10.0);
        assert_eq!(room.time_offset_minutes, 20.0);

        room.apply_brightness_offset(10.0);
        assert_eq!(room.brightness_offset, 10.0);

        room.reset_offsets();
        assert_eq!(room.time_offset_minutes, 0.0);
        assert_eq!(room.brightness_offset, 0.0);
    }

    #[test]
    fn test_room_profile_settings_apply_to_config() {
        let settings = RoomProfileSettings {
            profile_id: Some("sleep".into()),
            fade_ms: Some(TimerSetting::Fixed { value: 250 }),
            motion_timeout_secs: Some(TimerSetting::Fixed { value: 42 }),
        };
        let mut config = crate::default_rhythm_profile();

        settings.apply_to_config(&mut config);

        assert_eq!(settings.resolved_profile_id("rhythm"), "sleep");
        assert_eq!(config.fade_ms, TimerSetting::Fixed { value: 250 });
        assert_eq!(
            config.motion_timeout_secs,
            TimerSetting::Fixed { value: 42 }
        );
    }

    #[test]
    fn test_rhythm_mode_from_profile_id() {
        assert_eq!(RhythmMode::from_profile_id("rhythm"), RhythmMode::Day);
        assert_eq!(RhythmMode::from_profile_id("custom"), RhythmMode::Day);
        assert_eq!(RhythmMode::from_profile_id("sleep"), RhythmMode::Sleep);
    }

    #[test]
    fn test_room_mode_state_from_flags() {
        assert_eq!(
            RoomModeState::from_flags(false, false, false),
            RoomModeState::Active
        );
        assert_eq!(
            RoomModeState::from_flags(false, true, false),
            RoomModeState::Idle
        );
        assert_eq!(
            RoomModeState::from_flags(false, false, true),
            RoomModeState::Warning
        );
        assert_eq!(
            RoomModeState::from_flags(true, false, false),
            RoomModeState::HardOff
        );
    }

    #[test]
    fn test_default_mode_configs_cover_day_and_sleep() {
        let configs = default_mode_configs();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].mode, RhythmMode::Day);
        assert_eq!(configs[0].active_profile_id.as_deref(), Some("rhythm"));
        assert_eq!(configs[0].idle_profile_id, None);
        assert_eq!(configs[1].mode, RhythmMode::Sleep);
        assert_eq!(configs[1].active_profile_id.as_deref(), Some("sleep"));
        assert_eq!(configs[1].idle_profile_id, None);
    }

    #[test]
    fn test_default_mode_transitions_use_short_testing_fade() {
        let configs = default_mode_transition_configs();
        assert_eq!(configs.len(), 2);
        assert!(configs.iter().all(|config| config.duration_ms == 5_000));
        assert_eq!(
            configs[0].trigger,
            ModeTransitionTrigger::AstronomicalTwilight
        );
        assert_eq!(configs[1].trigger, ModeTransitionTrigger::NauticalTwilight);
    }

    #[test]
    fn test_mode_config_resolve_state_profile_id_uses_state_specific_defaults() {
        let config = ModeConfig::default_for_mode(RhythmMode::Sleep);

        assert_eq!(
            config.resolve_state_profile_id(RoomModeState::Active, "custom-sleep"),
            Some("sleep")
        );
        assert_eq!(
            config.resolve_state_profile_id(RoomModeState::Idle, "custom-sleep"),
            None
        );
        assert_eq!(
            config.resolve_state_profile_id(RoomModeState::Wake, "custom-sleep"),
            Some("custom-sleep")
        );
        assert_eq!(
            config.resolve_state_profile_id(RoomModeState::Warning, "custom-sleep"),
            Some("custom-sleep")
        );
        assert_eq!(
            config.resolve_state_profile_id(RoomModeState::HardOff, "custom-sleep"),
            None
        );
    }

    #[test]
    fn test_day_mode_default_idle_profile_id() {
        assert_eq!(RhythmMode::Day.default_idle_profile_id(), "day_idle");
        assert_eq!(RhythmMode::Sleep.default_idle_profile_id(), "sleep_idle");
    }

    #[test]
    fn test_mode_config_normalizes_invalid_active_state_profile() {
        let mut config = ModeConfig {
            mode: RhythmMode::Day,
            active_profile_id: Some(DAY_IDLE_PROFILE_ID.into()),
            idle_profile_id: Some(DAY_IDLE_PROFILE_ID.into()),
            wake_profile_id: None,
            warning_profile_id: None,
        };

        assert!(config.normalize_profile_ids());
        assert_eq!(config.active_profile_id.as_deref(), Some("rhythm"));
        assert_eq!(config.idle_profile_id.as_deref(), Some("day_idle"));
    }

    #[test]
    fn test_mode_config_normalizes_removed_legacy_idle_profile_ids() {
        let mut config = ModeConfig {
            mode: RhythmMode::Sleep,
            active_profile_id: Some("idle".into()),
            idle_profile_id: Some("idle".into()),
            wake_profile_id: None,
            warning_profile_id: None,
        };

        assert!(config.normalize_profile_ids());
        assert_eq!(config.active_profile_id.as_deref(), Some("sleep"));
        assert_eq!(config.idle_profile_id, None);
    }

    #[test]
    fn test_mode_config_resolve_state_profile_id_ignores_removed_legacy_idle() {
        let config = ModeConfig {
            mode: RhythmMode::Day,
            active_profile_id: Some("idle".into()),
            idle_profile_id: Some("idle".into()),
            wake_profile_id: None,
            warning_profile_id: None,
        };

        assert_eq!(
            config.resolve_state_profile_id(RoomModeState::Active, "rhythm"),
            Some("rhythm")
        );
        assert_eq!(
            config.resolve_state_profile_id(RoomModeState::Idle, "rhythm"),
            None
        );
        assert_eq!(
            config.resolve_state_profile_id(RoomModeState::HardOff, "rhythm"),
            None
        );
    }

    #[test]
    fn test_room_manager_add_get() {
        let mut manager = RoomManager::new();

        manager.add("living_room", "Living Room");
        manager.add("bedroom", "Bedroom");

        assert_eq!(manager.len(), 2);
        assert!(manager.contains("living_room"));
        assert!(manager.contains("bedroom"));
        assert!(!manager.contains("kitchen"));

        let room = manager.get("living_room").unwrap();
        assert_eq!(room.name, "Living Room");
    }

    #[test]
    fn test_room_manager_rhythm() {
        let mut manager = RoomManager::new();
        manager.add("test", "Test Room");

        assert!(!manager.is_rhythm_enabled("test"));
        assert_eq!(manager.rhythm_enabled_count(), 0);

        manager.enable_rhythm("test");
        assert!(manager.is_rhythm_enabled("test"));
        assert_eq!(manager.rhythm_enabled_count(), 1);

        manager.disable_rhythm("test");
        assert!(!manager.is_rhythm_enabled("test"));
    }

    #[test]
    fn test_room_manager_get_or_create() {
        let mut manager = RoomManager::new();

        // First call creates the room
        let room = manager.get_or_create("new_room", "New Room");
        assert_eq!(room.name, "New Room");

        // Second call returns existing room
        room.enable_rhythm();
        let room2 = manager.get_or_create("new_room", "Different Name");
        assert_eq!(room2.name, "New Room"); // Original name kept
        assert!(room2.rhythm_enabled); // State preserved
    }

    #[test]
    fn test_room_manager_rhythm_enabled_rooms() {
        let mut manager = RoomManager::new();
        manager.add("room1", "Room 1");
        manager.add("room2", "Room 2");
        manager.add("room3", "Room 3");

        manager.enable_rhythm("room1");
        manager.enable_rhythm("room3");

        let enabled = manager.rhythm_enabled_rooms();
        assert_eq!(enabled.len(), 2);
    }

    #[test]
    fn test_room_manager_enabled_rooms() {
        let mut manager = RoomManager::new();
        manager.add("room1", "Room 1");
        manager.add("room2", "Room 2");
        manager.add("room3", "Room 3");

        // Disable room2
        manager.get_mut("room2").unwrap().set_disabled(true);

        let enabled = manager.enabled_rooms();
        assert_eq!(enabled.len(), 2);
        assert!(enabled.iter().all(|r| r.id != "room2"));
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn test_room_serialize_deserialize() {
            let room = Room::new("room1", "Living Room");
            let json = serde_json::to_string(&room).unwrap();
            let deserialized: Room = serde_json::from_str(&json).unwrap();

            assert_eq!(deserialized.id, "room1");
            assert_eq!(deserialized.name, "Living Room");
        }

        #[test]
        fn test_room_serialize_with_offsets() {
            let mut room = Room::new("room1", "Test Room");
            room.rhythm_enabled = true;
            room.time_offset_minutes = 30.5;
            room.brightness_offset = -10.0;

            let json = serde_json::to_string(&room).unwrap();
            let deserialized: Room = serde_json::from_str(&json).unwrap();

            assert!(deserialized.rhythm_enabled);
            assert_eq!(deserialized.time_offset_minutes, 30.5);
            assert_eq!(deserialized.brightness_offset, -10.0);
        }

        #[test]
        fn test_room_deserialize_missing_optional_fields() {
            // JSON without optional fields (disabled, soft_off)
            let json = r#"{
                "id": "room1",
                "name": "Test Room",
                "rhythm_enabled": false,
                "time_offset_minutes": 0.0,
                "brightness_offset": 0.0
            }"#;

            let room: Room = serde_json::from_str(json).unwrap();
            assert_eq!(room.id, "room1");
            assert!(!room.disabled); // default
            assert!(!room.soft_off); // default
            assert!(!room.hard_off); // default
        }

        #[test]
        fn test_mode_enums_serialize_snake_case() {
            assert_eq!(
                serde_json::to_string(&RhythmMode::Sleep).unwrap(),
                "\"sleep\""
            );
            assert_eq!(
                serde_json::to_string(&RoomModeState::Warning).unwrap(),
                "\"warning\""
            );
            assert_eq!(
                serde_json::to_string(&RoomModeState::HardOff).unwrap(),
                "\"hard_off\""
            );
        }

        #[test]
        fn test_mode_transition_deserialize_defaults_duration_and_preserve_hard_off() {
            let json = r#"{
                "from_mode": "sleep",
                "to_mode": "day",
                "trigger": "sunrise"
            }"#;

            let config: ModeTransitionConfig = serde_json::from_str(json).unwrap();
            assert_eq!(config.duration_ms, 5_000);
            assert!(config.preserve_hard_off);
        }

        #[test]
        fn test_room_deserialize_ignores_legacy_source_field() {
            // Old rooms.json may contain a "source" field — serde should ignore it
            let json = r#"{
                "id": "room1",
                "name": "Test Room",
                "source": "Hue",
                "rhythm_enabled": true,
                "time_offset_minutes": 0.0,
                "brightness_offset": 0.0
            }"#;

            let room: Room = serde_json::from_str(json).unwrap();
            assert_eq!(room.id, "room1");
            assert!(room.rhythm_enabled);
        }

        #[test]
        fn test_room_manager_serialize_deserialize() {
            let mut manager = RoomManager::new();
            manager.add_room(Room::new("room1", "Room 1"));
            manager.add_room(Room::new("room2", "Room 2"));
            manager.get_mut("room1").unwrap().enable_rhythm();

            let json = serde_json::to_string(&manager).unwrap();
            let deserialized: RoomManager = serde_json::from_str(&json).unwrap();

            assert_eq!(deserialized.len(), 2);
            assert!(deserialized.is_rhythm_enabled("room1"));
            assert!(!deserialized.is_rhythm_enabled("room2"));
        }

        #[test]
        fn test_room_roundtrip_preserves_all_fields() {
            let mut room = Room::new("room1", "Test");
            room.rhythm_enabled = true;
            room.disabled = true;
            room.time_offset_minutes = -45.0;
            room.brightness_offset = 20.0;

            let json = serde_json::to_string(&room).unwrap();
            let roundtrip: Room = serde_json::from_str(&json).unwrap();

            assert_eq!(room, roundtrip);
        }
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_room_offset_accumulation() {
        let mut room = Room::new("test", "Test");

        room.apply_time_offset(10.0);
        room.apply_time_offset(20.0);
        room.apply_time_offset(-5.0);

        assert_eq!(room.time_offset_minutes, 25.0);
    }

    #[test]
    fn test_room_brightness_offset_clamping() {
        let mut room = Room::new("test", "Test");

        // Should clamp to 100
        room.apply_brightness_offset(150.0);
        assert_eq!(room.brightness_offset, 100.0);

        room.reset_offsets();

        // Should clamp to -100
        room.apply_brightness_offset(-150.0);
        assert_eq!(room.brightness_offset, -100.0);
    }

    #[test]
    fn test_room_reset_preserves_identity() {
        let mut room = Room::new("room1", "Living Room");
        room.rhythm_enabled = true;
        room.time_offset_minutes = 60.0;
        room.brightness_offset = 20.0;

        room.reset_offsets();

        // Identity preserved
        assert_eq!(room.id, "room1");
        assert_eq!(room.name, "Living Room");
        // Rhythm state preserved
        assert!(room.rhythm_enabled);
        // Offsets reset
        assert_eq!(room.time_offset_minutes, 0.0);
        assert_eq!(room.brightness_offset, 0.0);
    }

    #[test]
    fn test_room_manager_remove() {
        let mut manager = RoomManager::new();
        manager.add("room1", "Room 1");
        manager.add("room2", "Room 2");

        assert!(manager.remove("room1").is_some());
        assert!(manager.remove("room1").is_none()); // Already removed

        assert_eq!(manager.len(), 1);
        assert!(!manager.contains("room1"));
        assert!(manager.contains("room2"));
    }

    #[test]
    fn test_room_manager_get_or_create_preserves_existing() {
        let mut manager = RoomManager::new();

        // Create with initial name
        let room1 = manager.get_or_create("room1", "Original Name");
        room1.enable_rhythm();
        room1.apply_time_offset(30.0);

        // Get again with different name - should keep original
        let room2 = manager.get_or_create("room1", "Different Name");

        assert_eq!(room2.name, "Original Name");
        assert!(room2.rhythm_enabled);
        assert_eq!(room2.time_offset_minutes, 30.0);
    }

    #[test]
    fn test_room_manager_ids() {
        let mut manager = RoomManager::new();
        manager.add("z_room", "Z Room");
        manager.add("a_room", "A Room");
        manager.add("m_room", "M Room");

        let ids = manager.room_ids();

        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"a_room"));
        assert!(ids.contains(&"m_room"));
        assert!(ids.contains(&"z_room"));
    }
}
