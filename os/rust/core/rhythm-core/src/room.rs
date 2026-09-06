//! Addressable node abstraction for adaptive lighting.
//!
//! This module provides the `Room` struct which represents any addressable
//! topology node the runtime can target: root rooms and first-class device
//! leaves. `RoomManager` tracks that node graph and computes inherited
//! behavior for child nodes.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::light_profile::{
    is_builtin_state_profile_id, LightProfileConfig, TimerSetting, DAY_IDLE_PROFILE_ID,
    RHYTHM_PROFILE_ID, SLEEP_IDLE_PROFILE_ID, SLEEP_PROFILE_ID,
};

#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const DEFAULT_MODE_TRANSITION_DURATION_MS: u32 = 10_000;

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
pub enum RoomModeState {
    #[default]
    Active,
    Mood,
    Standby,
    Wake,
    Warning,
    HardOff,
}

impl RoomModeState {
    /// App-facing state label.
    pub const fn as_api_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Mood => "mood",
            Self::Standby => "standby",
            Self::Wake => "wake",
            Self::Warning => "warning",
            Self::HardOff => "hard_off",
        }
    }

    /// Whether this state can be stored as a mode default target.
    pub const fn is_mode_default_target(self) -> bool {
        matches!(
            self,
            Self::Active | Self::Mood | Self::Standby | Self::HardOff
        )
    }

    /// Derive the current user-facing room state from existing runtime flags.
    pub fn from_flags(
        hard_off: bool,
        soft_off: bool,
        mood_active: bool,
        warning_active: bool,
    ) -> Self {
        if hard_off {
            Self::HardOff
        } else if warning_active {
            Self::Warning
        } else if mood_active {
            Self::Mood
        } else if soft_off {
            Self::Standby
        } else {
            Self::Active
        }
    }
}

#[cfg(feature = "serde")]
impl Serialize for RoomModeState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_api_str())
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for RoomModeState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "active" => Ok(Self::Active),
            "mood" => Ok(Self::Mood),
            "standby" | "idle" | "soft_off" => Ok(Self::Standby),
            "hard_off" => Ok(Self::HardOff),
            "wake" => Ok(Self::Wake),
            "warning" => Ok(Self::Warning),
            _ => Err(serde::de::Error::custom(format!(
                "unknown room state '{raw}'"
            ))),
        }
    }
}

/// Target room state when a mode activates.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RoomModeDefault {
    pub room_id: String,
    pub state: RoomModeState,
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
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub room_defaults: Vec<RoomModeDefault>,
}

impl ModeConfig {
    fn normalize_room_defaults(&mut self) -> bool {
        let original = self.room_defaults.clone();
        let mut seen_room_ids = HashSet::new();
        let mut normalized = Vec::with_capacity(self.room_defaults.len());

        for default in self.room_defaults.iter().rev() {
            if !default.state.is_mode_default_target() {
                continue;
            }
            if seen_room_ids.insert(default.room_id.clone()) {
                normalized.push(default.clone());
            }
        }

        normalized.reverse();
        let changed = normalized != original;
        self.room_defaults = normalized;
        changed
    }

    /// Normalize invalid profile selections into mode-specific built-ins.
    ///
    /// Active profiles must never point at state-only profiles.
    pub fn normalize_profile_ids(&mut self) -> bool {
        let mut changed = self.normalize_room_defaults();

        if self
            .active_profile_id
            .as_deref()
            .is_some_and(is_builtin_state_profile_id)
        {
            self.active_profile_id = Some(self.mode.default_active_profile_id().to_string());
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
            room_defaults: Vec::new(),
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
                    .filter(|id| !is_builtin_state_profile_id(id))
                    .unwrap_or(active_profile_id),
            ),
            RoomModeState::Mood | RoomModeState::Standby => self.idle_profile_id.as_deref(),
            RoomModeState::Wake => {
                Some(self.wake_profile_id.as_deref().unwrap_or(active_profile_id))
            }
            RoomModeState::Warning => Some(
                self.warning_profile_id
                    .as_deref()
                    .unwrap_or(active_profile_id),
            ),
            RoomModeState::HardOff => None,
        }
    }
}

pub fn default_mode_configs() -> Vec<ModeConfig> {
    RhythmMode::ALL
        .into_iter()
        .map(ModeConfig::default_for_mode)
        .collect()
}

/// Local wall-clock time for a scheduled mode transition.
///
/// Stored internally as whole minutes after midnight so transition triggers
/// remain hashable and comparable across persisted configs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModeTransitionTime {
    minutes_since_midnight: u16,
}

impl ModeTransitionTime {
    pub const fn from_hour_minute(hour: u8, minute: u8) -> Option<Self> {
        if hour >= 24 || minute >= 60 {
            return None;
        }

        Some(Self {
            minutes_since_midnight: (hour as u16) * 60 + (minute as u16),
        })
    }

    pub const fn hour(self) -> u8 {
        (self.minutes_since_midnight / 60) as u8
    }

    pub const fn minute(self) -> u8 {
        (self.minutes_since_midnight % 60) as u8
    }

    pub const fn minutes_since_midnight(self) -> u16 {
        self.minutes_since_midnight
    }

    pub fn local_hour(self) -> f32 {
        self.minutes_since_midnight as f32 / 60.0
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        let mut parts = value.split(':');
        let hour = parts
            .next()
            .ok_or_else(|| "scheduled time must use HH:MM".to_string())?;
        let minute = parts
            .next()
            .ok_or_else(|| "scheduled time must use HH:MM".to_string())?;
        let second = parts.next();
        if parts.next().is_some() {
            return Err("scheduled time must use HH:MM".to_string());
        }

        let hour = hour
            .parse::<u8>()
            .map_err(|_| "scheduled time hour must be a number".to_string())?;
        let minute = minute
            .parse::<u8>()
            .map_err(|_| "scheduled time minute must be a number".to_string())?;

        if let Some(second) = second {
            let second = second
                .parse::<u8>()
                .map_err(|_| "scheduled time second must be a number".to_string())?;
            if second != 0 {
                return Err("scheduled time does not support seconds".to_string());
            }
        }

        Self::from_hour_minute(hour, minute)
            .ok_or_else(|| "scheduled time must be between 00:00 and 23:59".to_string())
    }

    pub fn display(self) -> String {
        format!("{:02}:{:02}", self.hour(), self.minute())
    }

    pub fn id_suffix(self) -> String {
        format!("scheduled_{:02}{:02}", self.hour(), self.minute())
    }
}

impl core::fmt::Display for ModeTransitionTime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:02}:{:02}", self.hour(), self.minute())
    }
}

/// Authority used to choose Day/Sleep behavior for one room.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum RoomScheduleSource {
    /// Follow the appliance-wide Wake/Sleep transitions and room defaults.
    #[default]
    WakeSleepPresets,
    /// Resolve Day/Sleep from this room's own local wall-clock times.
    FollowTime,
}

/// Persisted schedule authority for one stable room ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RoomScheduleConfig {
    #[cfg_attr(feature = "serde", serde(default))]
    pub source: RoomScheduleSource,
    pub wake_time: ModeTransitionTime,
    pub sleep_time: ModeTransitionTime,
}

/// Reusable named schedule that can be shared by any number of light roots.
///
/// Membership is stored separately on [`RoomProfileSettings`]. Keeping the
/// policy and membership independent lets rooms and standalone lights opt out
/// entirely without inventing a special "manual" schedule.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightScheduleConfig {
    pub id: String,
    pub name: String,
    #[cfg_attr(feature = "serde", serde(default = "default_light_schedule_enabled"))]
    pub enabled: bool,
    #[cfg_attr(feature = "serde", serde(default))]
    pub active_mode: RhythmMode,
    #[cfg_attr(feature = "serde", serde(default))]
    pub transitions: Vec<ModeTransitionConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ModeTransitionTriggerType {
    Manual,
    Solar,
    Scheduled,
}

/// Sparse patch over one typed transition trigger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ModeTransitionTriggerOverride {
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub kind: Option<ModeTransitionTriggerType>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub event: Option<SolarEvent>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub offset_minutes: Option<i16>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub time: Option<ModeTransitionTime>,
}

impl ModeTransitionTriggerOverride {
    pub fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.event.is_none()
            && self.offset_minutes.is_none()
            && self.time.is_none()
    }

    pub fn merged_with_parent(&self, parent: &Self) -> Self {
        // A local kind selection starts a typed trigger scope. Inherit only
        // fields compatible with that type: this preserves same-kind sparse
        // inheritance even when the parent relies on the schedule's base kind,
        // while preventing solar fields from leaking into fixed-time rules (or
        // vice versa). Explicit incompatible local fields remain so `apply_to`
        // rejects them visibly.
        if let Some(kind) = self.kind {
            return match kind {
                ModeTransitionTriggerType::Manual => self.clone(),
                ModeTransitionTriggerType::Scheduled => Self {
                    kind: self.kind,
                    event: self.event,
                    offset_minutes: self.offset_minutes,
                    time: self.time.or(parent.time),
                },
                ModeTransitionTriggerType::Solar => Self {
                    kind: self.kind,
                    event: self.event.or(parent.event),
                    offset_minutes: self.offset_minutes.or(parent.offset_minutes),
                    time: self.time,
                },
            };
        }
        Self {
            kind: self.kind.or(parent.kind),
            event: self.event.or(parent.event),
            offset_minutes: self.offset_minutes.or(parent.offset_minutes),
            time: self.time.or(parent.time),
        }
    }

    pub fn apply_to(&self, base: ModeTransitionTrigger) -> Result<ModeTransitionTrigger, String> {
        let inherited_kind = if base.is_manual() {
            ModeTransitionTriggerType::Manual
        } else if base.is_solar() {
            ModeTransitionTriggerType::Solar
        } else {
            ModeTransitionTriggerType::Scheduled
        };
        let effective_kind = self.kind.unwrap_or(inherited_kind);
        let kind_changed = self.kind.is_some_and(|kind| kind != inherited_kind);
        match effective_kind {
            ModeTransitionTriggerType::Manual => {
                if self.event.is_some()
                    || self.time.is_some()
                    || self.offset_minutes.is_some_and(|value| value != 0)
                {
                    return Err(
                        "manual trigger override cannot include event, time, or offset_minutes"
                            .to_string(),
                    );
                }
                Ok(ModeTransitionTrigger::Manual)
            }
            ModeTransitionTriggerType::Scheduled => {
                if self.event.is_some() || self.offset_minutes.is_some_and(|value| value != 0) {
                    return Err(
                        "scheduled trigger override cannot include event or offset_minutes"
                            .to_string(),
                    );
                }
                self.time
                    .or_else(|| (!kind_changed).then(|| base.scheduled_time()).flatten())
                    .map(ModeTransitionTrigger::Scheduled)
                    .ok_or_else(|| "scheduled trigger override requires time".to_string())
            }
            ModeTransitionTriggerType::Solar => {
                if self.time.is_some() {
                    return Err("solar trigger override cannot include time".to_string());
                }
                let event = self
                    .event
                    .or_else(|| (!kind_changed).then(|| base.solar_event()).flatten())
                    .ok_or_else(|| "solar trigger override requires event".to_string())?;
                let offset_minutes = self.offset_minutes.unwrap_or_else(|| {
                    if !kind_changed && base.is_solar() {
                        base.offset_minutes()
                    } else {
                        0
                    }
                });
                ModeTransitionTrigger::solar(event, 0).with_solar_offset(offset_minutes)
            }
        }
    }
}

/// Sparse patch over one stable transition ID.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ModeTransitionOverride {
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            skip_serializing_if = "ModeTransitionTriggerOverride::is_empty"
        )
    )]
    pub trigger: ModeTransitionTriggerOverride,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub trigger_enabled: Option<bool>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub duration_ms: Option<TimerSetting>,
}

impl ModeTransitionOverride {
    pub fn is_empty(&self) -> bool {
        self.trigger.is_empty() && self.trigger_enabled.is_none() && self.duration_ms.is_none()
    }

    pub fn merged_with_parent(&self, parent: &Self) -> Self {
        Self {
            trigger: self.trigger.merged_with_parent(&parent.trigger),
            trigger_enabled: self.trigger_enabled.or(parent.trigger_enabled),
            duration_ms: self
                .duration_ms
                .clone()
                .or_else(|| parent.duration_ms.clone()),
        }
    }

    pub fn apply_to(&self, base: &ModeTransitionConfig) -> Result<ModeTransitionConfig, String> {
        let mut effective = base.clone();
        effective.trigger = self.trigger.apply_to(base.trigger)?;
        if let Some(value) = self.trigger_enabled {
            effective.trigger_enabled = value;
        }
        if let Some(value) = self.duration_ms.as_ref() {
            effective.duration_ms = value.clone();
        }
        Ok(effective)
    }
}

/// Sparse transition patches for one stable named schedule ID.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightScheduleOverride {
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "BTreeMap::is_empty")
    )]
    pub transitions: BTreeMap<String, ModeTransitionOverride>,
}

impl LightScheduleOverride {
    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty()
    }

    pub fn merged_with_parent(&self, parent: &Self) -> Self {
        let mut transitions = parent.transitions.clone();
        for (transition_id, local) in &self.transitions {
            let merged = transitions
                .get(transition_id)
                .map(|inherited| local.merged_with_parent(inherited))
                .unwrap_or_else(|| local.clone());
            transitions.insert(transition_id.clone(), merged);
        }
        Self { transitions }
    }

    pub fn apply_to(&self, schedule: &LightScheduleConfig) -> Result<LightScheduleConfig, String> {
        for transition_id in self.transitions.keys() {
            if !schedule
                .transitions
                .iter()
                .any(|transition| transition.id == *transition_id)
            {
                return Err(format!(
                    "Light schedule override references unknown transition '{}'",
                    transition_id
                ));
            }
        }
        let mut effective = schedule.clone();
        for transition in &mut effective.transitions {
            if let Some(transition_override) = self.transitions.get(&transition.id) {
                *transition = transition_override.apply_to(transition)?;
            }
        }
        Ok(effective)
    }
}

/// Explicit automation ownership for one light-addressable root. Absence of
/// this field preserves the legacy appliance-wide schedule; `Unscheduled` is
/// the deliberate opt-out requested by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "kind", rename_all = "snake_case"))]
pub enum LightScheduleAssignment {
    Unscheduled {
        active_mode: RhythmMode,
    },
    Named {
        schedule_id: String,
        active_mode: RhythmMode,
    },
}

impl LightScheduleAssignment {
    pub fn schedule_id(&self) -> Option<&str> {
        match self {
            Self::Unscheduled { .. } => None,
            Self::Named { schedule_id, .. } => Some(schedule_id),
        }
    }

    pub const fn active_mode(&self) -> Option<RhythmMode> {
        match self {
            Self::Unscheduled { active_mode } | Self::Named { active_mode, .. } => {
                Some(*active_mode)
            }
        }
    }
}

const fn default_light_schedule_enabled() -> bool {
    true
}

impl Default for RoomScheduleConfig {
    fn default() -> Self {
        Self {
            source: RoomScheduleSource::WakeSleepPresets,
            wake_time: ModeTransitionTime::from_hour_minute(6, 30)
                .expect("default wake time is valid"),
            sleep_time: ModeTransitionTime::from_hour_minute(22, 30)
                .expect("default sleep time is valid"),
        }
    }
}

impl RoomScheduleConfig {
    pub const fn follows_time(self) -> bool {
        matches!(self.source, RoomScheduleSource::FollowTime)
    }

    /// Resolve the current room mode across same-day and overnight windows.
    pub fn effective_mode(self, local_hour: f32) -> RhythmMode {
        let minute = ((local_hour.rem_euclid(24.0) * 60.0).floor() as u16).min(1439);
        let wake = self.wake_time.minutes_since_midnight();
        let sleep = self.sleep_time.minutes_since_midnight();
        let is_day = if wake < sleep {
            minute >= wake && minute < sleep
        } else if wake > sleep {
            minute >= wake || minute < sleep
        } else {
            false
        };
        if is_day {
            RhythmMode::Day
        } else {
            RhythmMode::Sleep
        }
    }
}

#[cfg(feature = "serde")]
impl Serialize for ModeTransitionTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.display())
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for ModeTransitionTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

pub const MAX_SOLAR_OFFSET_MINUTES: i16 = 720;

/// Stable solar anchor used by a mode-transition trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum SolarEvent {
    Sunrise,
    Sunset,
    CivilTwilight,
    NauticalTwilight,
    AstronomicalTwilight,
}

impl SolarEvent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sunrise => "sunrise",
            Self::Sunset => "sunset",
            Self::CivilTwilight => "civil_twilight",
            Self::NauticalTwilight => "nautical_twilight",
            Self::AstronomicalTwilight => "astronomical_twilight",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Sunrise => "Sunrise",
            Self::Sunset => "Sunset",
            Self::CivilTwilight => "Civil Twilight",
            Self::NauticalTwilight => "Nautical Twilight",
            Self::AstronomicalTwilight => "Astronomical Twilight",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ModeTransitionTriggerKind {
    Manual,
    Solar(SolarEvent),
    Scheduled(ModeTransitionTime),
}

/// Trigger that initiates a configured mode transition.
///
/// Solar offsets stay attached to the typed anchor instead of being flattened
/// into a wall-clock time. Existing constructors remain zero-offset constants
/// so older callers and persisted data keep their exact behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModeTransitionTrigger {
    kind: ModeTransitionTriggerKind,
    offset_minutes: i16,
}

impl Default for ModeTransitionTrigger {
    fn default() -> Self {
        Self::Manual
    }
}

impl ModeTransitionTrigger {
    #[allow(non_upper_case_globals)]
    pub const Manual: Self = Self {
        kind: ModeTransitionTriggerKind::Manual,
        offset_minutes: 0,
    };
    #[allow(non_upper_case_globals)]
    pub const Sunrise: Self = Self::solar(SolarEvent::Sunrise, 0);
    #[allow(non_upper_case_globals)]
    pub const Sunset: Self = Self::solar(SolarEvent::Sunset, 0);
    #[allow(non_upper_case_globals)]
    pub const CivilTwilight: Self = Self::solar(SolarEvent::CivilTwilight, 0);
    #[allow(non_upper_case_globals)]
    pub const NauticalTwilight: Self = Self::solar(SolarEvent::NauticalTwilight, 0);
    #[allow(non_upper_case_globals)]
    pub const AstronomicalTwilight: Self = Self::solar(SolarEvent::AstronomicalTwilight, 0);

    #[allow(non_snake_case)]
    pub const fn Scheduled(time: ModeTransitionTime) -> Self {
        Self {
            kind: ModeTransitionTriggerKind::Scheduled(time),
            offset_minutes: 0,
        }
    }

    pub const fn solar(event: SolarEvent, offset_minutes: i16) -> Self {
        Self {
            kind: ModeTransitionTriggerKind::Solar(event),
            offset_minutes,
        }
    }

    pub fn with_solar_offset(self, offset_minutes: i16) -> Result<Self, String> {
        if !matches!(self.kind, ModeTransitionTriggerKind::Solar(_)) {
            return Err("offset_minutes is valid only for solar triggers".to_string());
        }
        if !(-MAX_SOLAR_OFFSET_MINUTES..=MAX_SOLAR_OFFSET_MINUTES).contains(&offset_minutes) {
            return Err(format!(
                "solar offset_minutes must be between -{} and {}",
                MAX_SOLAR_OFFSET_MINUTES, MAX_SOLAR_OFFSET_MINUTES
            ));
        }
        Ok(Self {
            offset_minutes,
            ..self
        })
    }

    pub const fn is_manual(self) -> bool {
        matches!(self.kind, ModeTransitionTriggerKind::Manual)
    }

    pub const fn is_solar(self) -> bool {
        matches!(self.kind, ModeTransitionTriggerKind::Solar(_))
    }

    pub const fn kind(self) -> &'static str {
        match self.kind {
            ModeTransitionTriggerKind::Manual => "manual",
            ModeTransitionTriggerKind::Solar(_) => "solar",
            ModeTransitionTriggerKind::Scheduled(_) => "scheduled",
        }
    }

    pub const fn event(self) -> Option<&'static str> {
        match self.solar_event() {
            Some(event) => Some(event.as_str()),
            None => None,
        }
    }

    pub const fn solar_event(self) -> Option<SolarEvent> {
        match self.kind {
            ModeTransitionTriggerKind::Solar(event) => Some(event),
            _ => None,
        }
    }

    pub const fn scheduled_time(self) -> Option<ModeTransitionTime> {
        match self.kind {
            ModeTransitionTriggerKind::Scheduled(time) => Some(time),
            _ => None,
        }
    }

    pub const fn offset_minutes(self) -> i16 {
        self.offset_minutes
    }

    pub fn id_suffix(self) -> String {
        match self.kind {
            ModeTransitionTriggerKind::Manual => "manual".to_string(),
            ModeTransitionTriggerKind::Solar(event) => {
                if self.offset_minutes == 0 {
                    event.as_str().to_string()
                } else {
                    format!("{}_{:+}", event.as_str(), self.offset_minutes)
                }
            }
            ModeTransitionTriggerKind::Scheduled(time) => time.id_suffix(),
        }
    }

    pub fn label_suffix(self) -> Option<String> {
        match self.kind {
            ModeTransitionTriggerKind::Manual => None,
            ModeTransitionTriggerKind::Solar(event) => {
                let label = event.display_name();
                Some(match self.offset_minutes.cmp(&0) {
                    core::cmp::Ordering::Less => {
                        format!(
                            "{} min before {}",
                            self.offset_minutes.unsigned_abs(),
                            label
                        )
                    }
                    core::cmp::Ordering::Greater => {
                        format!("{} min after {}", self.offset_minutes, label)
                    }
                    core::cmp::Ordering::Equal => label.to_string(),
                })
            }
            ModeTransitionTriggerKind::Scheduled(time) => Some(time.display()),
        }
    }
}

#[cfg(feature = "serde")]
impl Serialize for ModeTransitionTrigger {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct TriggerRepr<'a> {
            kind: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            event: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            time: Option<ModeTransitionTime>,
            #[serde(skip_serializing_if = "is_zero_i16")]
            offset_minutes: i16,
        }

        fn is_zero_i16(value: &i16) -> bool {
            *value == 0
        }

        TriggerRepr {
            kind: self.kind(),
            event: self.event(),
            time: self.scheduled_time(),
            offset_minutes: self.offset_minutes(),
        }
        .serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for ModeTransitionTrigger {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum LegacyTrigger {
            Manual,
            Sunrise,
            Sunset,
            CivilTwilight,
            NauticalTwilight,
            AstronomicalTwilight,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum TriggerKind {
            Manual,
            Solar,
            Scheduled,
        }

        #[derive(Deserialize)]
        struct TriggerObject {
            kind: TriggerKind,
            #[serde(default)]
            event: Option<LegacyTrigger>,
            #[serde(default)]
            time: Option<ModeTransitionTime>,
            #[serde(default)]
            offset_minutes: i16,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum TriggerRepr {
            Object(TriggerObject),
            Legacy(LegacyTrigger),
        }

        let repr = TriggerRepr::deserialize(deserializer)?;
        match repr {
            TriggerRepr::Legacy(trigger) => Ok(match trigger {
                LegacyTrigger::Manual => Self::Manual,
                LegacyTrigger::Sunrise => Self::Sunrise,
                LegacyTrigger::Sunset => Self::Sunset,
                LegacyTrigger::CivilTwilight => Self::CivilTwilight,
                LegacyTrigger::NauticalTwilight => Self::NauticalTwilight,
                LegacyTrigger::AstronomicalTwilight => Self::AstronomicalTwilight,
            }),
            TriggerRepr::Object(TriggerObject {
                kind,
                event,
                time,
                offset_minutes,
            }) => match kind {
                TriggerKind::Manual if offset_minutes == 0 => Ok(Self::Manual),
                TriggerKind::Manual => Err(serde::de::Error::custom(
                    "offset_minutes is valid only for solar triggers",
                )),
                TriggerKind::Solar => {
                    if !(-MAX_SOLAR_OFFSET_MINUTES..=MAX_SOLAR_OFFSET_MINUTES)
                        .contains(&offset_minutes)
                    {
                        return Err(serde::de::Error::custom(format!(
                            "solar offset_minutes must be between -{} and {}",
                            MAX_SOLAR_OFFSET_MINUTES, MAX_SOLAR_OFFSET_MINUTES
                        )));
                    }
                    let event = match event {
                        Some(LegacyTrigger::Sunrise) => SolarEvent::Sunrise,
                        Some(LegacyTrigger::Sunset) => SolarEvent::Sunset,
                        Some(LegacyTrigger::CivilTwilight) => SolarEvent::CivilTwilight,
                        Some(LegacyTrigger::NauticalTwilight) => SolarEvent::NauticalTwilight,
                        Some(LegacyTrigger::AstronomicalTwilight) => {
                            SolarEvent::AstronomicalTwilight
                        }
                        Some(LegacyTrigger::Manual) | None => {
                            return Err(serde::de::Error::custom(
                                "solar trigger requires a solar event",
                            ))
                        }
                    };
                    Ok(Self::solar(event, offset_minutes))
                }
                TriggerKind::Scheduled if offset_minutes == 0 => time
                    .map(Self::Scheduled)
                    .ok_or_else(|| serde::de::Error::custom("scheduled trigger requires time")),
                TriggerKind::Scheduled => Err(serde::de::Error::custom(
                    "offset_minutes is valid only for solar triggers",
                )),
            },
        }
    }
}

/// Why the active mode changed most recently.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ModeChangeCause {
    #[default]
    Manual,
    Schedule,
}

fn default_trigger_enabled() -> bool {
    false
}

fn default_mode_transition_duration_ms() -> TimerSetting {
    TimerSetting::Auto
}

/// Configured transition between two high-level modes.
///
/// Transitions operate in rendered output space: the runtime captures the
/// current visible room output and fades it to the target mode/state output.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ModeTransitionConfig {
    #[cfg_attr(feature = "serde", serde(default))]
    pub id: String,
    #[cfg_attr(feature = "serde", serde(default))]
    pub label: String,
    pub from_mode: RhythmMode,
    pub to_mode: RhythmMode,
    #[cfg_attr(feature = "serde", serde(default))]
    pub trigger: ModeTransitionTrigger,
    #[cfg_attr(feature = "serde", serde(default = "default_trigger_enabled"))]
    pub trigger_enabled: bool,
    #[cfg_attr(
        feature = "serde",
        serde(default = "default_mode_transition_duration_ms")
    )]
    pub duration_ms: TimerSetting,
}

impl ModeTransitionConfig {
    pub fn new(from_mode: RhythmMode, to_mode: RhythmMode, duration_ms: u32) -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            from_mode,
            to_mode,
            trigger: ModeTransitionTrigger::Manual,
            trigger_enabled: true,
            duration_ms: TimerSetting::Fixed { value: duration_ms },
        }
    }

    pub fn with_duration(mut self, duration_ms: TimerSetting) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    pub fn with_trigger(mut self, trigger: ModeTransitionTrigger) -> Self {
        self.trigger = trigger;
        self
    }

    pub fn with_trigger_enabled(mut self, trigger_enabled: bool) -> Self {
        self.trigger_enabled = trigger_enabled;
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    fn default_id_base(&self) -> String {
        format!(
            "{}_to_{}_{}",
            self.from_mode.id_fragment(),
            self.to_mode.id_fragment(),
            self.trigger.id_suffix()
        )
    }

    fn default_label(&self) -> String {
        match self.trigger.label_suffix() {
            Some(trigger) => format!(
                "{} to {} ({})",
                self.from_mode.display_name(),
                self.to_mode.display_name(),
                trigger
            ),
            None => format!(
                "{} to {}",
                self.from_mode.display_name(),
                self.to_mode.display_name()
            ),
        }
    }
}

impl RhythmMode {
    fn id_fragment(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Sleep => "sleep",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Day => "Day",
            Self::Sleep => "Sleep",
        }
    }
}

fn slugify_id(input: &str) -> String {
    let mut out = String::new();
    let mut last_was_sep = false;
    for ch in input.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            last_was_sep = false;
            ch.to_ascii_lowercase()
        } else {
            if !last_was_sep && !out.is_empty() {
                out.push('_');
            }
            last_was_sep = true;
            continue;
        };
        out.push(mapped);
    }
    out.trim_matches('_').to_string()
}

fn reserved_default_transition_id(
    from_mode: RhythmMode,
    to_mode: RhythmMode,
) -> Option<&'static str> {
    match (from_mode, to_mode) {
        (RhythmMode::Sleep, RhythmMode::Day) => Some("sleep_to_day"),
        (RhythmMode::Day, RhythmMode::Sleep) => Some("day_to_sleep"),
        _ => None,
    }
}

fn reserved_default_transition_label(
    from_mode: RhythmMode,
    to_mode: RhythmMode,
) -> Option<&'static str> {
    match (from_mode, to_mode) {
        (RhythmMode::Sleep, RhythmMode::Day) => Some("Sleep to Day"),
        (RhythmMode::Day, RhythmMode::Sleep) => Some("Day to Sleep"),
        _ => None,
    }
}

pub fn normalize_mode_transition_configs<I>(configs: I) -> Vec<ModeTransitionConfig>
where
    I: IntoIterator<Item = ModeTransitionConfig>,
{
    let configs: Vec<_> = configs.into_iter().collect();
    let mut pair_counts: HashMap<(RhythmMode, RhythmMode), usize> = HashMap::new();
    for config in &configs {
        *pair_counts
            .entry((config.from_mode, config.to_mode))
            .or_insert(0) += 1;
    }

    let mut seen_ids = HashSet::new();
    let mut normalized = Vec::new();

    for mut config in configs {
        let requested_id = slugify_id(config.id.trim());
        let pair_count = pair_counts
            .get(&(config.from_mode, config.to_mode))
            .copied()
            .unwrap_or(0);
        let reserved_default = reserved_default_transition_id(config.from_mode, config.to_mode)
            .filter(|reserved| {
                pair_count == 1 && (requested_id.is_empty() || requested_id == *reserved)
            });

        if let Some(label) = reserved_default_transition_label(config.from_mode, config.to_mode)
            .filter(|_| reserved_default.is_some())
        {
            config.label = label.to_string();
        } else if config.label.trim().is_empty() {
            config.label = config.default_label();
        }

        let base_id = reserved_default.map(str::to_string).unwrap_or_else(|| {
            if requested_id.is_empty() {
                config.default_id_base()
            } else {
                requested_id
            }
        });

        let mut candidate = base_id.clone();
        let mut suffix = 2usize;
        while !seen_ids.insert(candidate.clone()) {
            candidate = format!("{}_{}", base_id, suffix);
            suffix += 1;
        }
        config.id = candidate;
        normalized.push(config);
    }

    normalized
}

pub fn default_mode_transition_configs() -> Vec<ModeTransitionConfig> {
    normalize_mode_transition_configs(vec![
        ModeTransitionConfig::new(
            RhythmMode::Sleep,
            RhythmMode::Day,
            DEFAULT_MODE_TRANSITION_DURATION_MS,
        )
        .with_duration(TimerSetting::Auto)
        .with_id("sleep_to_day")
        .with_label("Sleep to Day")
        .with_trigger(ModeTransitionTrigger::AstronomicalTwilight),
        ModeTransitionConfig::new(
            RhythmMode::Day,
            RhythmMode::Sleep,
            DEFAULT_MODE_TRANSITION_DURATION_MS,
        )
        .with_duration(TimerSetting::Auto)
        .with_id("day_to_sleep")
        .with_label("Day to Sleep")
        .with_trigger(ModeTransitionTrigger::NauticalTwilight),
    ])
}

/// Per-node overrides for one resolved light profile.
///
/// Every field is optional so a room continues to inherit later global profile
/// changes for values it has not customized.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightProfileNodeOverride {
    /// Optional per-node curve-shape override for this profile.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub curve: Option<crate::LightCurveShape>,

    /// Optional per-node minimum brightness override.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub min_brightness: Option<u8>,

    /// Optional per-node maximum brightness override.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub max_brightness: Option<u8>,

    /// Optional per-node minimum color-temperature override.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub min_color_temp: Option<u16>,

    /// Optional per-node maximum color-temperature override.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub max_color_temp: Option<u16>,

    /// Optional per-node dim-step-count override.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub max_dim_steps: Option<u8>,

    /// Optional per-node fade override for this profile.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub fade_ms: Option<TimerSetting>,

    /// Optional per-node motion timeout override for this profile.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub motion_timeout_secs: Option<TimerSetting>,

    /// Optional per-node background rhythm interval override for this profile.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub rhythm_interval_secs: Option<TimerSetting>,
}

impl LightProfileNodeOverride {
    pub fn is_empty(&self) -> bool {
        self.curve.is_none()
            && self.min_brightness.is_none()
            && self.max_brightness.is_none()
            && self.min_color_temp.is_none()
            && self.max_color_temp.is_none()
            && self.max_dim_steps.is_none()
            && self.fade_ms.is_none()
            && self.motion_timeout_secs.is_none()
            && self.rhythm_interval_secs.is_none()
    }

    /// Merge the fields present in a partial patch while preserving newer
    /// fields unknown to an older client.
    pub fn merge_from(&mut self, patch: &Self) {
        if patch.curve.is_some() {
            self.curve = patch.curve.clone();
        }
        if patch.min_brightness.is_some() {
            self.min_brightness = patch.min_brightness;
        }
        if patch.max_brightness.is_some() {
            self.max_brightness = patch.max_brightness;
        }
        if patch.min_color_temp.is_some() {
            self.min_color_temp = patch.min_color_temp;
        }
        if patch.max_color_temp.is_some() {
            self.max_color_temp = patch.max_color_temp;
        }
        if patch.max_dim_steps.is_some() {
            self.max_dim_steps = patch.max_dim_steps;
        }
        if patch.fade_ms.is_some() {
            self.fade_ms = patch.fade_ms.clone();
        }
        if patch.motion_timeout_secs.is_some() {
            self.motion_timeout_secs = patch.motion_timeout_secs.clone();
        }
        if patch.rhythm_interval_secs.is_some() {
            self.rhythm_interval_secs = patch.rhythm_interval_secs.clone();
        }
    }

    /// Clear the timer-only fields understood by clients predating visual room
    /// overrides. This keeps an older app's "Auto" action from erasing visual
    /// settings it could not display.
    pub fn clear_legacy_timer_fields(&mut self) {
        self.fade_ms = None;
        self.motion_timeout_secs = None;
    }

    /// Stable, non-sensitive field names for diagnostics and analytics.
    pub fn field_names(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.curve.is_some() {
            fields.push("curve");
        }
        if self.min_brightness.is_some() {
            fields.push("min_brightness");
        }
        if self.max_brightness.is_some() {
            fields.push("max_brightness");
        }
        if self.min_color_temp.is_some() {
            fields.push("min_color_temp");
        }
        if self.max_color_temp.is_some() {
            fields.push("max_color_temp");
        }
        if self.max_dim_steps.is_some() {
            fields.push("max_dim_steps");
        }
        if self.fade_ms.is_some() {
            fields.push("fade_ms");
        }
        if self.motion_timeout_secs.is_some() {
            fields.push("motion_timeout_secs");
        }
        if self.rhythm_interval_secs.is_some() {
            fields.push("rhythm_interval_secs");
        }
        fields
    }

    fn apply_to_config(&self, config: &mut LightProfileConfig) {
        if let Some(curve) = &self.curve {
            config.curve = curve.clone();
        }
        if let Some(min_brightness) = self.min_brightness {
            config.min_brightness = min_brightness;
        }
        if let Some(max_brightness) = self.max_brightness {
            config.max_brightness = max_brightness;
        }
        if let Some(min_color_temp) = self.min_color_temp {
            config.min_color_temp = min_color_temp;
        }
        if let Some(max_color_temp) = self.max_color_temp {
            config.max_color_temp = max_color_temp;
        }
        if let Some(max_dim_steps) = self.max_dim_steps {
            config.max_dim_steps = max_dim_steps;
        }
        if let Some(fade_ms) = &self.fade_ms {
            config.fade_ms = fade_ms.clone();
        }
        if let Some(motion_timeout_secs) = &self.motion_timeout_secs {
            config.motion_timeout_secs = motion_timeout_secs.clone();
        }
        if let Some(rhythm_interval_secs) = &self.rhythm_interval_secs {
            config.rhythm_interval_secs = rhythm_interval_secs.clone();
        }
        config.normalize_float_precision();
    }
}

/// Per-room light profile selection and overrides.
///
/// This layer sits on top of the globally active profile:
/// - `profile_id`: optionally selects a different stored base profile for this room
/// - `mood_enabled`: legacy compatibility field, currently not runtime-active
/// - `mood_profile_id`: legacy compatibility field for stored profile payloads
/// - timer fields: optionally override the selected profile's timer settings
/// - `motion_activation_enabled`: room-level admission for motion automation
/// - `profile_overrides`: optionally override profile values for specific resolved profiles
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RoomProfileSettings {
    /// Optional stored profile ID to use instead of the global active profile.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub profile_id: Option<String>,

    /// Optional legacy room-level mood enablement.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub mood_enabled: Option<bool>,

    /// Optional legacy stored profile ID for mood/idle payloads.
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            alias = "idle_profile_id",
            skip_serializing_if = "Option::is_none"
        )
    )]
    pub mood_profile_id: Option<String>,

    /// Optional scene to render when this node enters Mood.
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            alias = "active_light_scene_id",
            skip_serializing_if = "Option::is_none"
        )
    )]
    pub mood_scene_id: Option<String>,

    /// Palette slot at which `mood_scene_id` starts rendering on this node.
    ///
    /// A whole-home scene apply spreads a multi-colour palette across the
    /// house by giving each room a different starting slot. Persisting the slot
    /// next to the binding lets a later re-apply of the bound scene (entering
    /// Mood again, cancelling a preview, editing a light) reproduce the same
    /// colours instead of restarting every room at the first palette entry.
    /// `None` means the palette starts at slot 0.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub mood_scene_palette_offset: Option<u32>,

    /// How many palette slots the apply that bound `mood_scene_id` covered.
    ///
    /// A spread palette places each slot on a colour loop relative to the
    /// whole apply, so re-rendering one room needs the span the house was
    /// rendered with, not just this room's slot. `None` means the span is
    /// this node's own light count.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub mood_scene_palette_span: Option<u32>,

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

    /// Whether motion inputs may activate this room. Missing values preserve
    /// the historical default of enabled.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub motion_activation_enabled: Option<bool>,

    /// Explicit schedule ownership. Absence preserves legacy global behavior;
    /// `unscheduled` opts out and `named` enrolls the node.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub light_schedule: Option<LightScheduleAssignment>,

    /// Sparse schedule-ID-scoped transition overrides. Entries stay dormant
    /// while a different schedule is effective and merge root-to-leaf.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "BTreeMap::is_empty")
    )]
    pub light_schedule_overrides: BTreeMap<String, LightScheduleOverride>,

    /// Per-node automatic mode materialization keyed by named schedule ID.
    /// This keeps an inherited child room's boundary state independent without
    /// copying or pinning the parent's schedule assignment. Entries stay
    /// dormant when another authority is effective.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "BTreeMap::is_empty")
    )]
    pub light_schedule_modes: BTreeMap<String, RhythmMode>,

    /// Optional room-local schedule. Absence preserves legacy preset behavior.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub room_schedule: Option<RoomScheduleConfig>,

    /// Optional per-profile node overrides keyed by light profile ID.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "BTreeMap::is_empty")
    )]
    pub profile_overrides: BTreeMap<String, LightProfileNodeOverride>,
}

impl RoomProfileSettings {
    /// Returns `true` when this room uses the global active profile unchanged.
    pub fn is_empty(&self) -> bool {
        self.profile_id.is_none()
            && self.mood_enabled.is_none()
            && self.mood_profile_id.is_none()
            && self.mood_scene_id.is_none()
            && self.mood_scene_palette_offset.is_none()
            && self.mood_scene_palette_span.is_none()
            && self.fade_ms.is_none()
            && self.motion_timeout_secs.is_none()
            && self.motion_activation_enabled.is_none()
            && self.light_schedule.is_none()
            && self.light_schedule_overrides.is_empty()
            && self.light_schedule_modes.is_empty()
            && self.room_schedule.is_none()
            && self.profile_overrides.is_empty()
    }

    /// Resolve motion admission while preserving compatibility with settings
    /// persisted before the explicit toggle existed.
    pub fn motion_activation_enabled(&self) -> bool {
        self.motion_activation_enabled.unwrap_or(true)
    }

    pub fn schedule_mode(&self, global_mode: RhythmMode, local_hour: f32) -> RhythmMode {
        if let Some(mode) = self
            .light_schedule
            .as_ref()
            .and_then(LightScheduleAssignment::active_mode)
        {
            return mode;
        }
        self.room_schedule
            .filter(|schedule| schedule.follows_time())
            .map(|schedule| schedule.effective_mode(local_hour))
            .unwrap_or(global_mode)
    }

    /// Resolve legacy mood enablement for compatibility payloads.
    pub fn mood_enabled_or(&self, default: bool) -> bool {
        self.mood_enabled
            .unwrap_or_else(|| self.mood_profile_id.is_some() || default)
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

    /// Apply per-node overrides for a specific resolved profile.
    pub fn apply_to_config_for_profile(&self, profile_id: &str, config: &mut LightProfileConfig) {
        self.apply_to_config(config);
        self.apply_profile_override_to_config(profile_id, config);
    }

    /// Apply only the delta scoped to one resolved profile.
    pub fn apply_profile_override_to_config(
        &self,
        profile_id: &str,
        config: &mut LightProfileConfig,
    ) {
        if let Some(profile_override) = self.profile_overrides.get(profile_id) {
            profile_override.apply_to_config(config);
        }
    }

    /// Merge this node-local override on top of a parent's effective settings.
    pub fn merged_with_parent(&self, parent: &Self) -> Self {
        // Schedule authority is one cascading choice even though the legacy
        // room-local schedule and reusable named assignment have separate
        // persisted fields. Any local choice must mask both parent fields;
        // otherwise an inherited named schedule would incorrectly outrank a
        // room's explicit custom wall-clock schedule in `schedule_mode`.
        let inherits_schedule_authority =
            self.light_schedule.is_none() && self.room_schedule.is_none();
        let (mut light_schedule, room_schedule) = if self.light_schedule.is_some() {
            (self.light_schedule.clone(), self.room_schedule)
        } else if self.room_schedule.is_some() {
            (None, self.room_schedule)
        } else {
            (parent.light_schedule.clone(), parent.room_schedule)
        };
        if inherits_schedule_authority {
            if let Some(LightScheduleAssignment::Named {
                schedule_id,
                active_mode,
            }) = light_schedule.as_mut()
            {
                if let Some(mode) = self.light_schedule_modes.get(schedule_id) {
                    *active_mode = *mode;
                }
            }
        }
        Self {
            profile_id: self
                .profile_id
                .clone()
                .or_else(|| parent.profile_id.clone()),
            mood_enabled: self.mood_enabled.or(parent.mood_enabled),
            mood_profile_id: self
                .mood_profile_id
                .clone()
                .or_else(|| parent.mood_profile_id.clone()),
            mood_scene_id: self
                .mood_scene_id
                .clone()
                .or_else(|| parent.mood_scene_id.clone()),
            // The palette offset belongs to whichever scene binding wins.
            mood_scene_palette_offset: if self.mood_scene_id.is_some() {
                self.mood_scene_palette_offset
            } else {
                parent.mood_scene_palette_offset
            },
            mood_scene_palette_span: if self.mood_scene_id.is_some() {
                self.mood_scene_palette_span
            } else {
                parent.mood_scene_palette_span
            },
            fade_ms: self.fade_ms.clone().or_else(|| parent.fade_ms.clone()),
            motion_timeout_secs: self
                .motion_timeout_secs
                .clone()
                .or_else(|| parent.motion_timeout_secs.clone()),
            motion_activation_enabled: self
                .motion_activation_enabled
                .or(parent.motion_activation_enabled),
            light_schedule,
            light_schedule_overrides: {
                let mut overrides = parent.light_schedule_overrides.clone();
                for (schedule_id, local) in &self.light_schedule_overrides {
                    let merged = overrides
                        .get(schedule_id)
                        .map(|inherited| local.merged_with_parent(inherited))
                        .unwrap_or_else(|| local.clone());
                    overrides.insert(schedule_id.clone(), merged);
                }
                overrides
            },
            light_schedule_modes: self.light_schedule_modes.clone(),
            room_schedule,
            profile_overrides: {
                let mut profile_overrides = parent.profile_overrides.clone();
                profile_overrides.extend(self.profile_overrides.clone());
                profile_overrides
            },
        }
    }
}

/// Addressable runtime node kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum LightNodeKind {
    #[default]
    Room,
    LightDevice,
    SwitchDevice,
    MotionSensor,
    Sensor,
    Button,
    OtherDevice,
}

impl LightNodeKind {
    pub const fn is_room(self) -> bool {
        matches!(self, Self::Room)
    }

    pub const fn is_light_addressable(self) -> bool {
        matches!(self, Self::Room | Self::LightDevice)
    }
}

/// Effective node behavior after walking parent inheritance.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveRoomState {
    pub rhythm_enabled: bool,
    pub disabled: bool,
    pub time_offset_minutes: f32,
    pub brightness_offset: f32,
    pub soft_off: bool,
    pub mood_active: bool,
    pub standby_enabled: bool,
    pub hard_off: bool,
    pub warning_active: bool,
    pub profile_settings: RoomProfileSettings,
}

/// A room or area that can have Rhythm lighting enabled.
///
/// Each node tracks its local Rhythm state and any time/brightness offsets
/// from manual adjustments (step up/down). Child nodes inherit effective
/// settings from their parent at runtime.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Room {
    /// Unique identifier for the room
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Runtime node kind.
    #[cfg_attr(feature = "serde", serde(default))]
    pub kind: LightNodeKind,

    /// Optional parent node that this node inherits from.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub parent_id: Option<String>,

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

    /// Whether this room is in Standby, using the mode's idle profile.
    #[cfg_attr(feature = "serde", serde(default))]
    pub soft_off: bool,

    /// Whether this room is in Mood, the first-class 1% mood profile state.
    #[cfg_attr(feature = "serde", serde(default))]
    pub mood_active: bool,

    /// Whether OffPress should enter Standby instead of hard-off for this node.
    #[cfg_attr(feature = "serde", serde(default))]
    pub standby_enabled: bool,

    /// Whether this room is intentionally fully off.
    ///
    /// This is the true off state entered by explicit off actions.
    #[cfg_attr(feature = "serde", serde(default))]
    pub hard_off: bool,

    /// Whether a pre-timeout warning dim is currently applied to this room.
    /// Transient — set when the motion timer enters the warning window and
    /// cleared when motion returns, the timeout fires, or the user takes
    /// any explicit action. Not persisted across restarts.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub warning_active: bool,

    /// Optional per-room base profile selection and profile overrides.
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
            kind: LightNodeKind::Room,
            parent_id: None,
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
        }
    }

    /// Create a new addressable node with explicit kind and optional parent.
    pub fn new_node(
        id: impl Into<String>,
        name: impl Into<String>,
        kind: LightNodeKind,
        parent_id: Option<String>,
    ) -> Self {
        let mut node = Self::new(id, name);
        node.kind = kind;
        node.parent_id = parent_id;
        if node.parent_id.is_some() {
            // Child nodes should inherit unless they explicitly opt out later.
            node.rhythm_enabled = true;
        }
        node
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

    /// Clear transient and persisted off-state flags, returning the room to active.
    pub fn clear_off_states(&mut self) {
        self.soft_off = false;
        self.mood_active = false;
        self.hard_off = false;
        self.warning_active = false;
    }

    /// Mark the room as mood-active and clear mutually exclusive off states.
    pub fn set_mood(&mut self) {
        self.clear_off_states();
        self.mood_active = true;
    }

    /// Mark the room as standby-active and clear mutually exclusive off states.
    pub fn set_standby(&mut self) {
        self.clear_off_states();
        self.soft_off = true;
    }

    /// Legacy soft-off entry point retained as an alias for Standby.
    pub fn set_soft_off(&mut self) {
        self.set_standby();
    }

    /// Mark the room as hard-off and clear mutually exclusive off states.
    pub fn set_hard_off(&mut self) {
        self.clear_off_states();
        self.hard_off = true;
    }

    /// Clear the transient motion warning-dim flag.
    pub fn clear_warning_state(&mut self) {
        self.warning_active = false;
    }

    /// Clamp a time offset to a sane ±24h and reject NaN/inf. Offsets arrive
    /// as unvalidated API floats and are persisted, so junk here would feed
    /// every future curve calculation on every boot.
    fn sanitize_time_offset(offset_minutes: f32) -> f32 {
        if offset_minutes.is_finite() {
            offset_minutes.clamp(-1440.0, 1440.0)
        } else {
            0.0
        }
    }

    /// Apply a time offset (from step_up/step_down).
    pub fn apply_time_offset(&mut self, offset_minutes: f32) {
        self.time_offset_minutes =
            Self::sanitize_time_offset(self.time_offset_minutes + offset_minutes);
    }

    /// Set the time offset directly.
    pub fn set_time_offset(&mut self, offset_minutes: f32) {
        self.time_offset_minutes = Self::sanitize_time_offset(offset_minutes);
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
            kind: LightNodeKind::Room,
            parent_id: None,
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

    /// Add a node by ID, name, kind, and optional parent.
    pub fn add_node(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        kind: LightNodeKind,
        parent_id: Option<String>,
    ) {
        let node = Room::new_node(id, name, kind, parent_id);
        self.rooms.insert(node.id.clone(), node);
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

    /// Get or create an addressable node.
    pub fn get_or_create_node(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        kind: LightNodeKind,
        parent_id: Option<String>,
    ) -> &mut Room {
        let id = id.into();
        if !self.rooms.contains_key(&id) {
            let name = name.into();
            self.rooms.insert(
                id.clone(),
                Room::new_node(id.clone(), name, kind, parent_id),
            );
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
        self.rooms
            .values()
            .filter(|r| r.kind.is_room() && r.rhythm_enabled)
            .collect()
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
        self.rooms
            .values()
            .filter(|r| r.kind.is_room() && !r.disabled)
            .collect()
    }

    /// Iterate only root room nodes.
    pub fn room_iter(&self) -> impl Iterator<Item = &Room> {
        self.rooms.values().filter(|room| room.kind.is_room())
    }

    /// Iterate child nodes for a parent.
    pub fn child_iter<'a>(&'a self, parent_id: &'a str) -> impl Iterator<Item = &'a Room> {
        self.rooms
            .values()
            .filter(move |room| room.parent_id.as_deref() == Some(parent_id))
    }

    /// Resolve a node's effective inherited state.
    pub fn effective_state(&self, id: &str) -> Option<EffectiveRoomState> {
        let room = self.rooms.get(id)?;
        let mut state = EffectiveRoomState {
            rhythm_enabled: room.rhythm_enabled,
            disabled: room.disabled,
            time_offset_minutes: room.time_offset_minutes,
            brightness_offset: room.brightness_offset,
            soft_off: room.soft_off,
            mood_active: room.mood_active,
            standby_enabled: room.standby_enabled,
            hard_off: room.hard_off,
            warning_active: room.warning_active,
            profile_settings: room.profile_settings.clone(),
        };

        let mut seen = HashSet::from([room.id.as_str()]);
        let mut current_parent = room.parent_id.as_deref();
        while let Some(parent_id) = current_parent {
            if !seen.insert(parent_id) {
                break;
            }
            let Some(parent) = self.rooms.get(parent_id) else {
                break;
            };
            state.rhythm_enabled &= parent.rhythm_enabled;
            state.disabled |= parent.disabled;
            state.time_offset_minutes += parent.time_offset_minutes;
            state.brightness_offset += parent.brightness_offset;
            state.soft_off |= parent.soft_off;
            state.mood_active |= parent.mood_active;
            state.hard_off |= parent.hard_off;
            state.warning_active |= parent.warning_active;
            state.profile_settings = state
                .profile_settings
                .merged_with_parent(&parent.profile_settings);
            current_parent = parent.parent_id.as_deref();
        }

        if state.hard_off {
            state.soft_off = false;
            state.mood_active = false;
        }

        Some(state)
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
        assert!(!room.warning_active);
        assert!(room.profile_settings.is_empty());
    }

    #[test]
    fn test_room_off_state_helpers_clear_mutually_exclusive_flags() {
        let mut room = Room::new("living_room", "Living Room");

        room.warning_active = true;
        room.set_soft_off();
        assert!(room.soft_off);
        assert!(!room.hard_off);
        assert!(!room.warning_active);

        room.warning_active = true;
        room.set_hard_off();
        assert!(!room.soft_off);
        assert!(room.hard_off);
        assert!(!room.warning_active);

        room.warning_active = true;
        room.clear_off_states();
        assert!(!room.soft_off);
        assert!(!room.hard_off);
        assert!(!room.warning_active);
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
            mood_enabled: None,
            mood_profile_id: None,
            mood_scene_id: None,
            mood_scene_palette_offset: None,
            mood_scene_palette_span: None,
            fade_ms: Some(TimerSetting::Fixed { value: 250 }),
            motion_timeout_secs: Some(TimerSetting::Fixed { value: 42 }),
            motion_activation_enabled: Some(false),
            light_schedule: None,
            light_schedule_overrides: BTreeMap::new(),
            light_schedule_modes: BTreeMap::new(),
            room_schedule: None,
            profile_overrides: BTreeMap::new(),
        };
        let mut config = crate::default_rhythm_profile();

        settings.apply_to_config(&mut config);

        assert_eq!(settings.resolved_profile_id("rhythm"), "sleep");
        assert!(!settings.motion_activation_enabled());
        assert_eq!(config.fade_ms, TimerSetting::Fixed { value: 250 });
        assert_eq!(
            config.motion_timeout_secs,
            TimerSetting::Fixed { value: 42 }
        );
    }

    #[test]
    fn light_profile_node_override_changes_only_explicit_profile_fields() {
        let global = crate::default_rhythm_profile();
        let mut effective = global.clone();
        let profile_override = LightProfileNodeOverride {
            min_brightness: Some(12),
            max_brightness: Some(68),
            max_color_temp: Some(4_200),
            rhythm_interval_secs: Some(TimerSetting::Fixed { value: 90 }),
            ..Default::default()
        };

        profile_override.apply_to_config(&mut effective);

        assert_eq!(effective.min_brightness, 12);
        assert_eq!(effective.max_brightness, 68);
        assert_eq!(effective.max_color_temp, 4_200);
        assert_eq!(
            effective.rhythm_interval_secs,
            TimerSetting::Fixed { value: 90 }
        );
        assert_eq!(effective.min_color_temp, global.min_color_temp);
        assert_eq!(effective.curve, global.curve);
        assert_eq!(
            profile_override.field_names(),
            vec![
                "min_brightness",
                "max_brightness",
                "max_color_temp",
                "rhythm_interval_secs"
            ]
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn legacy_room_profile_settings_default_motion_activation_to_enabled() {
        let legacy: RoomProfileSettings = serde_json::from_value(serde_json::json!({
            "motion_timeout_secs": {"mode": "fixed", "value": 300}
        }))
        .unwrap();
        assert_eq!(legacy.motion_activation_enabled, None);
        assert!(legacy.motion_activation_enabled());

        let disabled: RoomProfileSettings = serde_json::from_value(serde_json::json!({
            "motion_activation_enabled": false
        }))
        .unwrap();
        assert!(!disabled.motion_activation_enabled());

        let serialized = serde_json::to_value(RoomProfileSettings::default()).unwrap();
        assert!(serialized.get("motion_activation_enabled").is_none());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn room_schedule_profile_settings_round_trip() {
        let settings: RoomProfileSettings = serde_json::from_value(serde_json::json!({
            "room_schedule": {
                "source": "follow_time",
                "wake_time": "07:15",
                "sleep_time": "23:45"
            }
        }))
        .unwrap();
        let schedule = settings.room_schedule.unwrap();
        assert_eq!(schedule.source, RoomScheduleSource::FollowTime);
        assert_eq!(schedule.wake_time.display(), "07:15");
        assert_eq!(schedule.sleep_time.display(), "23:45");
        assert_eq!(
            serde_json::to_value(settings).unwrap()["room_schedule"]["source"],
            "follow_time"
        );
    }

    #[test]
    fn test_room_profile_settings_apply_profile_motion_timeout_to_config() {
        let mut settings = RoomProfileSettings {
            fade_ms: Some(TimerSetting::Fixed { value: 250 }),
            motion_timeout_secs: Some(TimerSetting::Fixed { value: 42 }),
            ..Default::default()
        };
        settings.profile_overrides.insert(
            "focus".into(),
            LightProfileNodeOverride {
                fade_ms: Some(TimerSetting::Fixed { value: 500 }),
                motion_timeout_secs: Some(TimerSetting::Fixed { value: 75 }),
                ..Default::default()
            },
        );

        let mut focus_config = crate::default_rhythm_profile();
        focus_config.id = "focus".into();
        settings.apply_to_config_for_profile("focus", &mut focus_config);
        assert_eq!(
            focus_config.motion_timeout_secs,
            TimerSetting::Fixed { value: 75 }
        );
        assert_eq!(focus_config.fade_ms, TimerSetting::Fixed { value: 500 });

        let mut sleep_config = crate::default_sleep_profile();
        settings.apply_to_config_for_profile("sleep", &mut sleep_config);
        assert_eq!(
            sleep_config.motion_timeout_secs,
            TimerSetting::Fixed { value: 42 }
        );
        assert_eq!(sleep_config.fade_ms, TimerSetting::Fixed { value: 250 });
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_room_profile_settings_accepts_legacy_scene_alias() {
        let settings: RoomProfileSettings =
            serde_json::from_str(r#"{"active_light_scene_id":"icy-glow"}"#).unwrap();

        assert_eq!(settings.mood_scene_id.as_deref(), Some("icy-glow"));

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["mood_scene_id"], "icy-glow");
        assert!(json.get("active_light_scene_id").is_none());
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
            RoomModeState::from_flags(false, false, false, false),
            RoomModeState::Active
        );
        assert_eq!(
            RoomModeState::from_flags(false, true, false, false),
            RoomModeState::Standby
        );
        assert_eq!(
            RoomModeState::from_flags(false, false, true, false),
            RoomModeState::Mood
        );
        assert_eq!(
            RoomModeState::from_flags(false, false, false, true),
            RoomModeState::Warning
        );
        assert_eq!(
            RoomModeState::from_flags(true, false, false, false),
            RoomModeState::HardOff
        );
        assert!(RoomModeState::Active.is_mode_default_target());
        assert!(RoomModeState::Mood.is_mode_default_target());
        assert!(RoomModeState::Standby.is_mode_default_target());
        assert!(RoomModeState::HardOff.is_mode_default_target());
        assert!(!RoomModeState::Wake.is_mode_default_target());
        assert!(!RoomModeState::Warning.is_mode_default_target());
    }

    #[test]
    fn test_default_mode_configs_cover_day_and_sleep() {
        let configs = default_mode_configs();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].mode, RhythmMode::Day);
        assert_eq!(configs[0].active_profile_id.as_deref(), Some("rhythm"));
        assert_eq!(configs[0].idle_profile_id, None);
        assert!(configs[0].room_defaults.is_empty());
        assert_eq!(configs[1].mode, RhythmMode::Sleep);
        assert_eq!(configs[1].active_profile_id.as_deref(), Some("sleep"));
        assert_eq!(configs[1].idle_profile_id, None);
        assert!(configs[1].room_defaults.is_empty());
    }

    #[test]
    fn test_default_mode_transitions_use_default_duration() {
        let configs = default_mode_transition_configs();
        assert_eq!(configs.len(), 2);
        assert!(configs
            .iter()
            .all(|config| config.duration_ms == TimerSetting::Auto));
        assert!(configs.iter().all(|config| config.trigger_enabled));
        assert_eq!(configs[0].id, "sleep_to_day");
        assert_eq!(configs[1].id, "day_to_sleep");
        assert_eq!(configs[0].label, "Sleep to Day");
        assert_eq!(configs[1].label, "Day to Sleep");
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
            config.resolve_state_profile_id(RoomModeState::Mood, "custom-sleep"),
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
            room_defaults: vec![],
        };

        assert!(config.normalize_profile_ids());
        assert_eq!(config.active_profile_id.as_deref(), Some("rhythm"));
        assert_eq!(config.idle_profile_id.as_deref(), Some("day_idle"));
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

    #[test]
    fn test_room_manager_effective_state_merges_parent_and_child() {
        let mut manager = RoomManager::new();

        let mut parent = Room::new("room1", "Room 1");
        parent.rhythm_enabled = true;
        parent.time_offset_minutes = 30.0;
        parent.brightness_offset = 12.0;
        parent.profile_settings.profile_id = Some("sleep".into());
        manager.add_room(parent);

        let mut child = Room::new_node(
            "device1",
            "Device 1",
            LightNodeKind::LightDevice,
            Some("room1".into()),
        );
        child.time_offset_minutes = -5.0;
        child.brightness_offset = -2.0;
        child.profile_settings.fade_ms = Some(TimerSetting::Fixed { value: 1234 });
        manager.add_room(child);

        let effective = manager.effective_state("device1").unwrap();
        assert!(effective.rhythm_enabled);
        assert_eq!(effective.time_offset_minutes, 25.0);
        assert_eq!(effective.brightness_offset, 10.0);
        assert_eq!(
            effective.profile_settings.profile_id.as_deref(),
            Some("sleep")
        );
        assert_eq!(
            effective.profile_settings.fade_ms,
            Some(TimerSetting::Fixed { value: 1234 })
        );
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
                serde_json::to_string(&RoomModeState::Mood).unwrap(),
                "\"mood\""
            );
            assert_eq!(
                serde_json::to_string(&RoomModeState::Standby).unwrap(),
                "\"standby\""
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
        fn test_room_mode_state_deserializes_mood_and_standby_aliases() {
            assert_eq!(
                serde_json::from_str::<RoomModeState>("\"mood\"").unwrap(),
                RoomModeState::Mood
            );
            assert_eq!(
                serde_json::from_str::<RoomModeState>("\"idle\"").unwrap(),
                RoomModeState::Standby
            );
            assert_eq!(
                serde_json::from_str::<RoomModeState>("\"standby\"").unwrap(),
                RoomModeState::Standby
            );
        }

        #[test]
        fn test_mode_transition_ignores_legacy_preserve_hard_off() {
            let json = r#"{
                "from_mode": "sleep",
                "to_mode": "day",
                "trigger": "sunrise",
                "preserve_hard_off": true
            }"#;

            let config: ModeTransitionConfig = serde_json::from_str(json).unwrap();
            assert_eq!(config.duration_ms, TimerSetting::Auto);
            assert!(!config.trigger_enabled);
            assert!(
                serde_json::to_value(config)
                    .unwrap()
                    .get("preserve_hard_off")
                    .is_none(),
                "legacy inputs remain readable but the retired field is not emitted"
            );
        }

        #[test]
        fn test_mode_transition_deserialize_trigger_enabled() {
            let json = r#"{
                "from_mode": "sleep",
                "to_mode": "day",
                "trigger": "sunrise",
                "trigger_enabled": false
            }"#;

            let config: ModeTransitionConfig = serde_json::from_str(json).unwrap();
            assert!(!config.trigger_enabled);
        }

        #[test]
        fn test_mode_transition_deserialize_auto_duration() {
            let json = r#"{
                "from_mode": "sleep",
                "to_mode": "day",
                "duration_ms": {"mode": "auto"}
            }"#;

            let config: ModeTransitionConfig = serde_json::from_str(json).unwrap();
            assert_eq!(config.duration_ms, TimerSetting::Auto);
        }

        #[test]
        fn test_mode_transition_serialize_fixed_duration_as_object() {
            let json = serde_json::to_value(ModeTransitionConfig::new(
                RhythmMode::Sleep,
                RhythmMode::Day,
                DEFAULT_MODE_TRANSITION_DURATION_MS,
            ))
            .unwrap();

            assert_eq!(json["duration_ms"]["mode"], "fixed");
            assert_eq!(
                json["duration_ms"]["value"],
                DEFAULT_MODE_TRANSITION_DURATION_MS
            );
            assert_eq!(json["trigger_enabled"], true);
        }

        #[test]
        fn test_mode_transition_serialize_auto_duration_as_object() {
            let json = serde_json::to_value(
                ModeTransitionConfig::new(
                    RhythmMode::Sleep,
                    RhythmMode::Day,
                    DEFAULT_MODE_TRANSITION_DURATION_MS,
                )
                .with_duration(TimerSetting::Auto),
            )
            .unwrap();

            assert_eq!(json["duration_ms"]["mode"], "auto");
        }

        #[test]
        fn test_mode_transition_trigger_serializes_as_object() {
            let json = serde_json::to_value(ModeTransitionTrigger::NauticalTwilight).unwrap();
            assert_eq!(json["kind"], "solar");
            assert_eq!(json["event"], "nautical_twilight");

            let manual = serde_json::to_value(ModeTransitionTrigger::Manual).unwrap();
            assert_eq!(manual["kind"], "manual");
            assert!(manual.get("event").is_none());
        }

        #[test]
        fn test_mode_transition_trigger_serializes_scheduled_as_object() {
            let json = serde_json::to_value(ModeTransitionTrigger::Scheduled(
                ModeTransitionTime::from_hour_minute(22, 0).unwrap(),
            ))
            .unwrap();

            assert_eq!(json["kind"], "scheduled");
            assert_eq!(json["time"], "22:00");
            assert!(json.get("event").is_none());
        }

        #[test]
        fn test_mode_transition_trigger_deserializes_scheduled_object() {
            let trigger: ModeTransitionTrigger =
                serde_json::from_str(r#"{"kind":"scheduled","time":"06:30"}"#).unwrap();

            assert_eq!(
                trigger,
                ModeTransitionTrigger::Scheduled(
                    ModeTransitionTime::from_hour_minute(6, 30).unwrap(),
                )
            );
        }

        #[test]
        fn test_mode_transition_trigger_rejects_scheduled_without_time() {
            let err = serde_json::from_str::<ModeTransitionTrigger>(r#"{"kind":"scheduled"}"#)
                .unwrap_err();

            assert!(err.to_string().contains("scheduled trigger requires time"));
        }

        #[test]
        fn test_mode_transition_time_rejects_invalid_values() {
            assert!(ModeTransitionTime::parse("24:00").is_err());
            assert!(ModeTransitionTime::parse("22:60").is_err());
            assert!(ModeTransitionTime::parse("22:00:01").is_err());
        }

        #[test]
        fn test_mode_config_room_defaults_round_trip_and_skip_empty() {
            let config = ModeConfig {
                mode: RhythmMode::Sleep,
                active_profile_id: Some("sleep".into()),
                idle_profile_id: None,
                wake_profile_id: None,
                warning_profile_id: None,
                room_defaults: vec![
                    RoomModeDefault {
                        room_id: "kitchen".into(),
                        state: RoomModeState::Mood,
                    },
                    RoomModeDefault {
                        room_id: "office".into(),
                        state: RoomModeState::HardOff,
                    },
                ],
            };

            let json = serde_json::to_value(&config).unwrap();
            assert_eq!(json["room_defaults"][0]["room_id"], "kitchen");
            assert_eq!(json["room_defaults"][0]["state"], "mood");

            let decoded: ModeConfig = serde_json::from_value(json).unwrap();
            assert_eq!(
                decoded.room_defaults,
                vec![
                    RoomModeDefault {
                        room_id: "kitchen".into(),
                        state: RoomModeState::Mood,
                    },
                    RoomModeDefault {
                        room_id: "office".into(),
                        state: RoomModeState::HardOff,
                    },
                ]
            );

            let empty_json =
                serde_json::to_value(ModeConfig::default_for_mode(RhythmMode::Day)).unwrap();
            assert!(empty_json.get("room_defaults").is_none());
        }

        #[test]
        fn test_mode_config_deserialize_missing_room_defaults_as_empty() {
            let json = r#"{
                "mode": "sleep",
                "active_profile_id": "sleep"
            }"#;

            let config: ModeConfig = serde_json::from_str(json).unwrap();
            assert!(config.room_defaults.is_empty());
        }

        #[test]
        fn test_normalize_mode_transition_configs_backfills_identity() {
            let configs = normalize_mode_transition_configs(vec![ModeTransitionConfig {
                id: String::new(),
                label: String::new(),
                from_mode: RhythmMode::Day,
                to_mode: RhythmMode::Sleep,
                trigger: ModeTransitionTrigger::NauticalTwilight,
                trigger_enabled: true,
                duration_ms: TimerSetting::Fixed { value: 2_000 },
            }]);

            assert_eq!(configs.len(), 1);
            assert_eq!(configs[0].id, "day_to_sleep");
            assert_eq!(configs[0].label, "Day to Sleep");
        }

        #[test]
        fn test_mode_config_normalize_room_defaults_last_wins_and_drops_runtime_only_states() {
            let mut config = ModeConfig {
                mode: RhythmMode::Sleep,
                active_profile_id: Some("sleep".into()),
                idle_profile_id: None,
                wake_profile_id: None,
                warning_profile_id: None,
                room_defaults: vec![
                    RoomModeDefault {
                        room_id: "kitchen".into(),
                        state: RoomModeState::Active,
                    },
                    RoomModeDefault {
                        room_id: "office".into(),
                        state: RoomModeState::Warning,
                    },
                    RoomModeDefault {
                        room_id: "kitchen".into(),
                        state: RoomModeState::Mood,
                    },
                ],
            };

            assert!(config.normalize_profile_ids());
            assert_eq!(
                config.room_defaults,
                vec![RoomModeDefault {
                    room_id: "kitchen".into(),
                    state: RoomModeState::Mood,
                }]
            );
        }

        #[test]
        fn test_normalize_mode_transition_configs_uses_scheduled_suffix_for_duplicate_pairs() {
            let configs = normalize_mode_transition_configs(vec![
                ModeTransitionConfig {
                    id: String::new(),
                    label: String::new(),
                    from_mode: RhythmMode::Day,
                    to_mode: RhythmMode::Sleep,
                    trigger: ModeTransitionTrigger::Manual,
                    trigger_enabled: true,
                    duration_ms: TimerSetting::Fixed { value: 1_000 },
                },
                ModeTransitionConfig {
                    id: String::new(),
                    label: String::new(),
                    from_mode: RhythmMode::Day,
                    to_mode: RhythmMode::Sleep,
                    trigger: ModeTransitionTrigger::Scheduled(
                        ModeTransitionTime::from_hour_minute(22, 0).unwrap(),
                    ),
                    trigger_enabled: true,
                    duration_ms: TimerSetting::Fixed { value: 2_000 },
                },
            ]);

            assert_eq!(configs[0].id, "day_to_sleep_manual");
            assert_eq!(configs[1].id, "day_to_sleep_scheduled_2200");
            assert_eq!(configs[1].label, "Day to Sleep (22:00)");
        }

        #[test]
        fn test_normalize_mode_transition_configs_preserves_custom_duplicate_pair_ids() {
            let configs = normalize_mode_transition_configs(vec![
                ModeTransitionConfig {
                    id: "day_to_sleep_custom".into(),
                    label: "Bedtime".into(),
                    from_mode: RhythmMode::Day,
                    to_mode: RhythmMode::Sleep,
                    trigger: ModeTransitionTrigger::Manual,
                    trigger_enabled: true,
                    duration_ms: TimerSetting::Fixed { value: 1_000 },
                },
                ModeTransitionConfig {
                    id: "day_to_sleep_nautical_twilight".into(),
                    label: "Night".into(),
                    from_mode: RhythmMode::Day,
                    to_mode: RhythmMode::Sleep,
                    trigger: ModeTransitionTrigger::NauticalTwilight,
                    trigger_enabled: true,
                    duration_ms: TimerSetting::Fixed { value: 2_000 },
                },
            ]);

            assert_eq!(configs[0].id, "day_to_sleep_custom");
            assert_eq!(configs[1].id, "day_to_sleep_nautical_twilight");
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
            room.mood_active = true;
            room.standby_enabled = true;

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
    fn mode_transition_time_parsing_labels_and_trigger_helpers_cover_all_user_forms() {
        let parsed = ModeTransitionTime::parse("07:30").unwrap();
        assert_eq!(parsed.hour(), 7);
        assert_eq!(parsed.minute(), 30);
        assert_eq!(parsed.minutes_since_midnight(), 450);
        assert_eq!(parsed.local_hour(), 7.5);
        assert_eq!(parsed.display(), "07:30");
        assert_eq!(parsed.id_suffix(), "scheduled_0730");
        assert_eq!(parsed.to_string(), "07:30");
        assert_eq!(ModeTransitionTime::parse("07:30:00").unwrap(), parsed);

        for invalid in [
            "",
            "07",
            "aa:30",
            "07:xx",
            "24:00",
            "07:30:01",
            "07:30:00:00",
        ] {
            assert!(
                ModeTransitionTime::parse(invalid).is_err(),
                "{invalid:?} should be rejected"
            );
        }

        let scheduled = ModeTransitionTrigger::Scheduled(parsed);
        assert!(!scheduled.is_manual());
        assert_eq!(scheduled.kind(), "scheduled");
        assert_eq!(scheduled.event(), None);
        assert_eq!(scheduled.scheduled_time(), Some(parsed));
        assert_eq!(scheduled.id_suffix(), "scheduled_0730");
        assert_eq!(scheduled.label_suffix(), Some("07:30".to_string()));

        let manual = ModeTransitionTrigger::Manual;
        assert!(manual.is_manual());
        assert_eq!(manual.kind(), "manual");
        assert_eq!(manual.event(), None);
        assert_eq!(manual.label_suffix(), None);

        for (trigger, event, label) in [
            (ModeTransitionTrigger::Sunrise, "sunrise", "Sunrise"),
            (ModeTransitionTrigger::Sunset, "sunset", "Sunset"),
            (
                ModeTransitionTrigger::CivilTwilight,
                "civil_twilight",
                "Civil Twilight",
            ),
            (
                ModeTransitionTrigger::NauticalTwilight,
                "nautical_twilight",
                "Nautical Twilight",
            ),
            (
                ModeTransitionTrigger::AstronomicalTwilight,
                "astronomical_twilight",
                "Astronomical Twilight",
            ),
        ] {
            assert_eq!(trigger.kind(), "solar");
            assert_eq!(trigger.event(), Some(event));
            assert_eq!(trigger.id_suffix(), event);
            assert_eq!(trigger.label_suffix(), Some(label.to_string()));
            assert_eq!(trigger.scheduled_time(), None);
        }
    }

    #[test]
    fn room_mutators_expose_effective_offsets_and_disabled_state() {
        let mut room = Room::default();
        assert_eq!(room.id, "default");
        assert_eq!(room.name, "Default Room");
        assert!(!room.is_disabled());

        room.set_disabled(true);
        room.set_time_offset(42.0);
        room.apply_time_offset(-2.0);
        room.apply_brightness_offset(15.0);
        room.clear_warning_state();

        assert!(room.is_disabled());
        assert_eq!(room.effective_time_offset(), 40.0);
        assert_eq!(room.effective_brightness_offset(), 15.0);
        assert!(!room.warning_active);
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

    #[test]
    fn room_follow_time_resolves_same_day_and_overnight_windows() {
        let same_day = RoomScheduleConfig {
            source: RoomScheduleSource::FollowTime,
            wake_time: ModeTransitionTime::parse("06:30").unwrap(),
            sleep_time: ModeTransitionTime::parse("22:30").unwrap(),
        };
        assert_eq!(same_day.effective_mode(6.49), RhythmMode::Sleep);
        assert_eq!(same_day.effective_mode(6.5), RhythmMode::Day);
        assert_eq!(same_day.effective_mode(22.5), RhythmMode::Sleep);

        let overnight = RoomScheduleConfig {
            wake_time: ModeTransitionTime::parse("22:00").unwrap(),
            sleep_time: ModeTransitionTime::parse("06:00").unwrap(),
            ..same_day
        };
        assert_eq!(overnight.effective_mode(23.0), RhythmMode::Day);
        assert_eq!(overnight.effective_mode(5.99), RhythmMode::Day);
        assert_eq!(overnight.effective_mode(6.0), RhythmMode::Sleep);
    }

    #[test]
    fn room_schedule_defaults_to_global_preset_authority() {
        let settings = RoomProfileSettings::default();
        assert_eq!(
            settings.schedule_mode(RhythmMode::Day, 23.0),
            RhythmMode::Day
        );

        let settings = RoomProfileSettings {
            room_schedule: Some(RoomScheduleConfig::default()),
            ..Default::default()
        };
        assert_eq!(
            settings.schedule_mode(RhythmMode::Sleep, 12.0),
            RhythmMode::Sleep
        );
    }

    #[test]
    fn schedule_authority_cascades_and_local_custom_schedule_overrides_parent() {
        let parent = RoomProfileSettings {
            light_schedule: Some(LightScheduleAssignment::Named {
                schedule_id: "indoor".into(),
                active_mode: RhythmMode::Sleep,
            }),
            ..Default::default()
        };
        let custom = RoomScheduleConfig {
            source: RoomScheduleSource::FollowTime,
            wake_time: ModeTransitionTime::parse("06:00").unwrap(),
            sleep_time: ModeTransitionTime::parse("23:00").unwrap(),
        };

        let inherited = RoomProfileSettings::default().merged_with_parent(&parent);
        assert_eq!(inherited.light_schedule, parent.light_schedule);
        assert_eq!(
            inherited.schedule_mode(RhythmMode::Day, 12.0),
            RhythmMode::Sleep
        );

        let overridden = RoomProfileSettings {
            room_schedule: Some(custom),
            ..Default::default()
        }
        .merged_with_parent(&parent);
        assert_eq!(overridden.light_schedule, None);
        assert_eq!(overridden.room_schedule, Some(custom));
        assert_eq!(
            overridden.schedule_mode(RhythmMode::Sleep, 12.0),
            RhythmMode::Day
        );
    }

    #[test]
    fn local_named_or_unscheduled_authority_overrides_parent_custom_schedule() {
        let custom = RoomScheduleConfig {
            source: RoomScheduleSource::FollowTime,
            wake_time: ModeTransitionTime::parse("06:00").unwrap(),
            sleep_time: ModeTransitionTime::parse("23:00").unwrap(),
        };
        let parent = RoomProfileSettings {
            room_schedule: Some(custom),
            ..Default::default()
        };

        for assignment in [
            LightScheduleAssignment::Named {
                schedule_id: "outdoor".into(),
                active_mode: RhythmMode::Sleep,
            },
            LightScheduleAssignment::Unscheduled {
                active_mode: RhythmMode::Day,
            },
        ] {
            let effective = RoomProfileSettings {
                light_schedule: Some(assignment.clone()),
                ..Default::default()
            }
            .merged_with_parent(&parent);
            assert_eq!(effective.light_schedule, Some(assignment));
            assert_eq!(effective.room_schedule, None);
        }
    }

    #[test]
    fn local_schedule_mode_materialization_only_overrides_inherited_authority() {
        let parent = RoomProfileSettings {
            light_schedule: Some(LightScheduleAssignment::Named {
                schedule_id: "outdoor".into(),
                active_mode: RhythmMode::Day,
            }),
            ..Default::default()
        };
        let dormant_modes = BTreeMap::from([("outdoor".to_string(), RhythmMode::Sleep)]);

        let inherited = RoomProfileSettings {
            light_schedule_modes: dormant_modes.clone(),
            ..Default::default()
        }
        .merged_with_parent(&parent);
        assert!(matches!(
            inherited.light_schedule,
            Some(LightScheduleAssignment::Named {
                active_mode: RhythmMode::Sleep,
                ..
            })
        ));

        let explicit = RoomProfileSettings {
            light_schedule: Some(LightScheduleAssignment::Named {
                schedule_id: "outdoor".into(),
                active_mode: RhythmMode::Day,
            }),
            light_schedule_modes: dormant_modes,
            ..Default::default()
        }
        .merged_with_parent(&parent);
        assert!(matches!(
            explicit.light_schedule,
            Some(LightScheduleAssignment::Named {
                active_mode: RhythmMode::Day,
                ..
            })
        ));
    }

    #[test]
    fn solar_trigger_offset_round_trips_and_rejects_invalid_shapes() {
        let trigger = ModeTransitionTrigger::Sunrise
            .with_solar_offset(-45)
            .unwrap();
        let json = serde_json::to_value(trigger).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "solar",
                "event": "sunrise",
                "offset_minutes": -45,
            })
        );
        assert_eq!(
            serde_json::from_value::<ModeTransitionTrigger>(json).unwrap(),
            trigger
        );

        for invalid in [
            serde_json::json!({"kind": "manual", "offset_minutes": 1}),
            serde_json::json!({"kind": "scheduled", "time": "08:00", "offset_minutes": 1}),
            serde_json::json!({"kind": "solar", "event": "sunrise", "offset_minutes": 721}),
            serde_json::json!({"kind": "solar", "event": "sunrise", "offset_minutes": 1.5}),
        ] {
            assert!(serde_json::from_value::<ModeTransitionTrigger>(invalid).is_err());
        }
    }

    #[test]
    fn sparse_schedule_overrides_merge_field_by_field_and_follow_base_edits() {
        let parent = LightScheduleOverride {
            transitions: BTreeMap::from([(
                "wake".to_string(),
                ModeTransitionOverride {
                    trigger: ModeTransitionTriggerOverride {
                        offset_minutes: Some(-30),
                        ..Default::default()
                    },
                    trigger_enabled: Some(false),
                    ..Default::default()
                },
            )]),
        };
        let local = LightScheduleOverride {
            transitions: BTreeMap::from([(
                "wake".to_string(),
                ModeTransitionOverride {
                    trigger: ModeTransitionTriggerOverride {
                        event: Some(SolarEvent::CivilTwilight),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )]),
        };
        let merged = local.merged_with_parent(&parent);
        let mut base = LightScheduleConfig {
            id: "outdoor".into(),
            name: "Outdoor".into(),
            enabled: true,
            active_mode: RhythmMode::Sleep,
            transitions: vec![
                ModeTransitionConfig::new(RhythmMode::Sleep, RhythmMode::Day, 1_000)
                    .with_id("wake")
                    .with_trigger(ModeTransitionTrigger::Sunrise),
            ],
        };

        let effective = merged.apply_to(&base).unwrap();
        assert_eq!(
            effective.transitions[0].trigger.solar_event(),
            Some(SolarEvent::CivilTwilight)
        );
        assert_eq!(effective.transitions[0].trigger.offset_minutes(), -30);
        assert!(!effective.transitions[0].trigger_enabled);

        base.transitions[0].duration_ms = TimerSetting::Fixed { value: 9_000 };
        let edited = merged.apply_to(&base).unwrap();
        assert_eq!(
            edited.transitions[0].duration_ms,
            TimerSetting::Fixed { value: 9_000 }
        );
    }

    #[test]
    fn trigger_kind_switches_clear_incompatible_inherited_fields() {
        let inherited_solar = ModeTransitionTriggerOverride {
            event: Some(SolarEvent::CivilTwilight),
            offset_minutes: Some(-30),
            ..Default::default()
        };
        let scheduled = ModeTransitionTriggerOverride {
            kind: Some(ModeTransitionTriggerType::Scheduled),
            time: Some(ModeTransitionTime::from_hour_minute(7, 15).unwrap()),
            ..Default::default()
        }
        .merged_with_parent(&inherited_solar);
        assert_eq!(scheduled.event, None);
        assert_eq!(scheduled.offset_minutes, None);
        assert_eq!(
            scheduled.time,
            Some(ModeTransitionTime::from_hour_minute(7, 15).unwrap())
        );
        assert_eq!(
            scheduled.apply_to(ModeTransitionTrigger::Sunrise).unwrap(),
            ModeTransitionTrigger::Scheduled(ModeTransitionTime::from_hour_minute(7, 15).unwrap())
        );

        let inherited_scheduled = ModeTransitionTriggerOverride {
            time: Some(ModeTransitionTime::from_hour_minute(6, 45).unwrap()),
            ..Default::default()
        };
        let solar = ModeTransitionTriggerOverride {
            kind: Some(ModeTransitionTriggerType::Solar),
            event: Some(SolarEvent::Sunset),
            offset_minutes: Some(20),
            ..Default::default()
        }
        .merged_with_parent(&inherited_scheduled);
        assert_eq!(solar.time, None);
        assert_eq!(
            solar
                .apply_to(ModeTransitionTrigger::Scheduled(
                    ModeTransitionTime::from_hour_minute(6, 45).unwrap(),
                ))
                .unwrap(),
            ModeTransitionTrigger::Sunset.with_solar_offset(20).unwrap()
        );

        let sparse_solar = ModeTransitionTriggerOverride {
            kind: Some(ModeTransitionTriggerType::Solar),
            offset_minutes: Some(10),
            ..Default::default()
        }
        .merged_with_parent(&inherited_solar);
        assert_eq!(sparse_solar.event, Some(SolarEvent::CivilTwilight));
        assert_eq!(sparse_solar.offset_minutes, Some(10));

        let sparse_scheduled = ModeTransitionTriggerOverride {
            kind: Some(ModeTransitionTriggerType::Scheduled),
            ..Default::default()
        }
        .merged_with_parent(&inherited_scheduled);
        assert_eq!(
            sparse_scheduled.time,
            Some(ModeTransitionTime::from_hour_minute(6, 45).unwrap())
        );
    }

    #[test]
    fn trigger_kind_switch_requires_the_new_kind_fields() {
        assert!(ModeTransitionTriggerOverride {
            kind: Some(ModeTransitionTriggerType::Scheduled),
            ..Default::default()
        }
        .apply_to(ModeTransitionTrigger::Sunrise)
        .unwrap_err()
        .contains("requires time"));

        assert!(ModeTransitionTriggerOverride {
            kind: Some(ModeTransitionTriggerType::Solar),
            ..Default::default()
        }
        .apply_to(ModeTransitionTrigger::Scheduled(
            ModeTransitionTime::from_hour_minute(6, 45).unwrap(),
        ))
        .unwrap_err()
        .contains("requires event"));
    }
}
