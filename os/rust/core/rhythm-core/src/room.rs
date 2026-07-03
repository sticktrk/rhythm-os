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

/// Trigger that initiates a configured mode transition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ModeTransitionTrigger {
    #[default]
    Manual,
    Sunrise,
    Sunset,
    CivilTwilight,
    NauticalTwilight,
    AstronomicalTwilight,
    Scheduled(ModeTransitionTime),
}

impl ModeTransitionTrigger {
    pub const fn is_manual(self) -> bool {
        matches!(self, Self::Manual)
    }

    pub const fn kind(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Scheduled(_) => "scheduled",
            _ => "solar",
        }
    }

    pub const fn event(self) -> Option<&'static str> {
        match self {
            Self::Manual => None,
            Self::Sunrise => Some("sunrise"),
            Self::Sunset => Some("sunset"),
            Self::CivilTwilight => Some("civil_twilight"),
            Self::NauticalTwilight => Some("nautical_twilight"),
            Self::AstronomicalTwilight => Some("astronomical_twilight"),
            Self::Scheduled(_) => None,
        }
    }

    pub const fn scheduled_time(self) -> Option<ModeTransitionTime> {
        match self {
            Self::Scheduled(time) => Some(time),
            _ => None,
        }
    }

    pub fn id_suffix(self) -> String {
        match self {
            Self::Manual => "manual".to_string(),
            Self::Sunrise => "sunrise".to_string(),
            Self::Sunset => "sunset".to_string(),
            Self::CivilTwilight => "civil_twilight".to_string(),
            Self::NauticalTwilight => "nautical_twilight".to_string(),
            Self::AstronomicalTwilight => "astronomical_twilight".to_string(),
            Self::Scheduled(time) => time.id_suffix(),
        }
    }

    pub fn label_suffix(self) -> Option<String> {
        match self {
            Self::Manual => None,
            Self::Sunrise => Some("Sunrise".to_string()),
            Self::Sunset => Some("Sunset".to_string()),
            Self::CivilTwilight => Some("Civil Twilight".to_string()),
            Self::NauticalTwilight => Some("Nautical Twilight".to_string()),
            Self::AstronomicalTwilight => Some("Astronomical Twilight".to_string()),
            Self::Scheduled(time) => Some(time.display()),
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
        }

        TriggerRepr {
            kind: self.kind(),
            event: self.event(),
            time: self.scheduled_time(),
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
            TriggerRepr::Object(TriggerObject { kind, event, time }) => match kind {
                TriggerKind::Manual => Ok(Self::Manual),
                TriggerKind::Solar => match event {
                    Some(LegacyTrigger::Sunrise) => Ok(Self::Sunrise),
                    Some(LegacyTrigger::Sunset) => Ok(Self::Sunset),
                    Some(LegacyTrigger::CivilTwilight) => Ok(Self::CivilTwilight),
                    Some(LegacyTrigger::NauticalTwilight) => Ok(Self::NauticalTwilight),
                    Some(LegacyTrigger::AstronomicalTwilight) => Ok(Self::AstronomicalTwilight),
                    Some(LegacyTrigger::Manual) | None => Err(serde::de::Error::custom(
                        "solar trigger requires a solar event",
                    )),
                },
                TriggerKind::Scheduled => time
                    .map(Self::Scheduled)
                    .ok_or_else(|| serde::de::Error::custom("scheduled trigger requires time")),
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

fn default_preserve_hard_off() -> bool {
    true
}

fn default_trigger_enabled() -> bool {
    true
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
    #[cfg_attr(feature = "serde", serde(default = "default_preserve_hard_off"))]
    pub preserve_hard_off: bool,
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
            preserve_hard_off: true,
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

/// Per-node timer overrides for one resolved light profile.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightProfileNodeOverride {
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
}

impl LightProfileNodeOverride {
    pub fn is_empty(&self) -> bool {
        self.fade_ms.is_none() && self.motion_timeout_secs.is_none()
    }

    fn apply_to_config(&self, config: &mut LightProfileConfig) {
        if let Some(fade_ms) = &self.fade_ms {
            config.fade_ms = fade_ms.clone();
        }
        if let Some(motion_timeout_secs) = &self.motion_timeout_secs {
            config.motion_timeout_secs = motion_timeout_secs.clone();
        }
    }
}

/// Per-room light profile selection and timer overrides.
///
/// This layer sits on top of the globally active profile:
/// - `profile_id`: optionally selects a different stored base profile for this room
/// - `mood_enabled`: legacy compatibility field, currently not runtime-active
/// - `mood_profile_id`: legacy compatibility field for stored profile payloads
/// - timer fields: optionally override the selected profile's timer settings
/// - `profile_overrides`: optionally override timers for specific resolved profiles
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
            && self.fade_ms.is_none()
            && self.motion_timeout_secs.is_none()
            && self.profile_overrides.is_empty()
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

    /// Apply per-node timer overrides for a specific resolved profile.
    pub fn apply_to_config_for_profile(&self, profile_id: &str, config: &mut LightProfileConfig) {
        self.apply_to_config(config);
        if let Some(profile_override) = self.profile_overrides.get(profile_id) {
            profile_override.apply_to_config(config);
        }
    }

    /// Merge this node-local override on top of a parent's effective settings.
    pub fn merged_with_parent(&self, parent: &Self) -> Self {
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
            fade_ms: self.fade_ms.clone().or_else(|| parent.fade_ms.clone()),
            motion_timeout_secs: self
                .motion_timeout_secs
                .clone()
                .or_else(|| parent.motion_timeout_secs.clone()),
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
            fade_ms: Some(TimerSetting::Fixed { value: 250 }),
            motion_timeout_secs: Some(TimerSetting::Fixed { value: 42 }),
            profile_overrides: BTreeMap::new(),
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
        fn test_mode_transition_deserialize_defaults_duration_and_preserve_hard_off() {
            let json = r#"{
                "from_mode": "sleep",
                "to_mode": "day",
                "trigger": "sunrise"
            }"#;

            let config: ModeTransitionConfig = serde_json::from_str(json).unwrap();
            assert_eq!(config.duration_ms, TimerSetting::Auto);
            assert!(config.preserve_hard_off);
            assert!(config.trigger_enabled);
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
                preserve_hard_off: true,
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
                    preserve_hard_off: true,
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
                    preserve_hard_off: true,
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
                    preserve_hard_off: true,
                },
                ModeTransitionConfig {
                    id: "day_to_sleep_nautical_twilight".into(),
                    label: "Night".into(),
                    from_mode: RhythmMode::Day,
                    to_mode: RhythmMode::Sleep,
                    trigger: ModeTransitionTrigger::NauticalTwilight,
                    trigger_enabled: true,
                    duration_ms: TimerSetting::Fixed { value: 2_000 },
                    preserve_hard_off: true,
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
}
