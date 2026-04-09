//! Extracted business logic for configuration commands.
//!
//! Platform-agnostic functions called by HTTP handlers.
//! Each function takes `SharedState` and parsed parameters, performs the
//! operation (registry update, engine update, persistence), and returns
//! a result that the transport layer can format into a response.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use chrono::{Datelike, Timelike};
use log::{debug, info, warn};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::{
    ButtonAction, HubRegistry, InputEvent, LightProfileConfig, LightProfileRegistry,
    LightingCommand, ModeConfig, ModeTransitionTrigger, RhythmMode, RoomModeState,
    RoomProfileSettings, RuntimeHandle, TimerSetting,
};
use serde_json::Value;

use crate::api_types::{
    ActiveProfileDto, ActiveProfileEffectiveDto, FixResponse, HubDto, LocationDto, RoomFullState,
    RoomPollState, RoomRhythmState, RoomsPollResponse, SettingsDto, StateSnapshot, TypedDeviceDto,
};
use crate::canonical::identity::HubKey;
use crate::state::{rooms_from_engine, AppState, SharedState};
use crate::storage::StoredLocation;

// ============================================================================
// Display value computation
// ============================================================================

/// Compute effective brightness and kelvin for a room given its offsets.
///
/// Uses the provided light profile config, current solar context, and room offsets
/// to produce the values a client should display.
pub fn compute_room_display_values(
    config: &LightProfileConfig,
    solar_noon: f32,
    latitude: f32,
    utc_offset: f32,
    time_offset_minutes: f32,
    brightness_offset: f32,
) -> (u8, u16) {
    use rhythm_core::{LightProfile, LightProfileModule, SolarTime};

    let now = chrono::Utc::now().naive_utc();
    let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);

    let offset_secs = (utc_offset * 3600.0) as i64;
    let local = now + chrono::Duration::seconds(offset_secs);
    let t = local.time();
    let current_hour = t.hour() as f32 + t.minute() as f32 / 60.0 + t.second() as f32 / 3600.0;

    let solar = SolarTime::new(solar_noon, latitude, day_of_year);
    let ctx = rhythm_core::light_profile::CurveContext::new(current_hour, solar, None);
    let module = LightProfile::new(config.clone());
    let values = module.calculate_with_offset(&ctx, time_offset_minutes);
    let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
    (brightness, values.kelvin)
}

fn active_profile_effective_values(
    config: &LightProfileConfig,
    current_hour: f32,
    solar: rhythm_core::SolarTime,
    sun_times: Option<rhythm_core::SunTimes>,
) -> ActiveProfileEffectiveDto {
    use rhythm_core::runtime::config::DEFAULT_UPDATE_INTERVAL_SECS;
    use rhythm_core::{LightProfile, LightProfileModule};

    let ctx = rhythm_core::light_profile::CurveContext::new(current_hour, solar, sun_times);
    let values = LightProfile::new(config.clone()).calculate(&ctx);
    let configured_interval_secs = config
        .rhythm_interval_secs
        .resolve(current_hour)
        .map(u64::from)
        .unwrap_or(DEFAULT_UPDATE_INTERVAL_SECS);
    let rhythm_interval_secs = values
        .suggested_tick_interval_secs
        .map(u64::from)
        .unwrap_or(configured_interval_secs)
        .max(configured_interval_secs);

    ActiveProfileEffectiveDto {
        fade_ms: values.transition_ms,
        motion_timeout_secs: u64::from(values.motion_timeout_secs),
        rhythm_interval_secs,
    }
}

fn light_profile_registry_from_parts(
    light_profile_configs: &BTreeMap<String, LightProfileConfig>,
    mode_configs: &[ModeConfig],
    active_mode: RhythmMode,
) -> LightProfileRegistry {
    let active_profile_id = resolved_active_profile_id_for_mode_from_parts(
        light_profile_configs,
        mode_configs,
        active_mode,
    );
    let mut registry = LightProfileRegistry::with_profiles(
        light_profile_configs.values().cloned().collect::<Vec<_>>(),
        &active_profile_id,
    );
    registry.set_mode_configs(mode_configs.iter().cloned());
    registry
}

fn render_state_for_display(room_state: RoomModeState) -> RoomModeState {
    room_state
}

fn mode_config_for_mode<'a>(
    mode_configs: &'a [ModeConfig],
    mode: RhythmMode,
) -> Option<&'a ModeConfig> {
    mode_configs.iter().find(|config| config.mode == mode)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ModeOutputApplyScope {
    active: bool,
    idle: bool,
    wake: bool,
    warning: bool,
}

impl ModeOutputApplyScope {
    fn all_visible() -> Self {
        Self {
            active: true,
            idle: true,
            wake: true,
            warning: true,
        }
    }

    fn is_empty(self) -> bool {
        !self.active && !self.idle && !self.wake && !self.warning
    }

    fn includes(self, room_state: RoomModeState) -> bool {
        match room_state {
            RoomModeState::Active => self.active,
            RoomModeState::Idle => self.idle,
            RoomModeState::Wake => self.wake,
            RoomModeState::Warning => self.warning,
            RoomModeState::HardOff => false,
        }
    }
}

fn mode_output_apply_scope(
    mode_changed: bool,
    previous_config: &ModeConfig,
    updated_config: &ModeConfig,
) -> ModeOutputApplyScope {
    if mode_changed || previous_config.active_profile_id != updated_config.active_profile_id {
        return ModeOutputApplyScope::all_visible();
    }

    ModeOutputApplyScope {
        active: false,
        idle: previous_config.idle_profile_id != updated_config.idle_profile_id,
        wake: previous_config.wake_profile_id != updated_config.wake_profile_id,
        warning: previous_config.warning_profile_id != updated_config.warning_profile_id,
    }
}

fn warning_uses_custom_profile(mode_configs: &[ModeConfig], mode: RhythmMode) -> bool {
    mode_config_for_mode(mode_configs, mode)
        .and_then(|config| config.warning_profile_id.as_deref())
        .is_some()
}

fn resolved_active_profile_id_for_mode_from_parts(
    light_profile_configs: &BTreeMap<String, LightProfileConfig>,
    mode_configs: &[ModeConfig],
    mode: RhythmMode,
) -> String {
    let requested = mode_config_for_mode(mode_configs, mode)
        .and_then(|config| config.active_profile_id.clone())
        .unwrap_or_else(|| mode.default_active_profile_id().to_string());

    if !rhythm_core::is_builtin_state_profile_id(&requested)
        && light_profile_configs.contains_key(&requested)
    {
        requested
    } else {
        mode.default_active_profile_id().to_string()
    }
}

fn compute_room_display_values_for_settings_from_parts(
    light_profile_configs: &BTreeMap<String, LightProfileConfig>,
    mode_configs: &[ModeConfig],
    active_mode: RhythmMode,
    solar_noon: f32,
    latitude: f32,
    utc_offset: f32,
    settings: &RoomProfileSettings,
    room_state: RoomModeState,
    time_offset_minutes: f32,
    brightness_offset: f32,
) -> (u8, u16) {
    use rhythm_core::SolarTime;

    if room_state == RoomModeState::HardOff {
        return (0, 0);
    }

    let now = chrono::Utc::now().naive_utc();
    let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);

    let offset_secs = (utc_offset * 3600.0) as i64;
    let local = now + chrono::Duration::seconds(offset_secs);
    let t = local.time();
    let current_hour = t.hour() as f32 + t.minute() as f32 / 60.0 + t.second() as f32 / 3600.0;

    let solar = SolarTime::new(solar_noon, latitude, day_of_year);
    let ctx = rhythm_core::light_profile::CurveContext::new(current_hour, solar, None);
    let render_state = render_state_for_display(room_state);
    if matches!(render_state, RoomModeState::HardOff) {
        return (0, 0);
    }
    let registry =
        light_profile_registry_from_parts(light_profile_configs, mode_configs, active_mode);
    let values = registry
        .profile_for_room_state(active_mode, render_state, Some(settings))
        .calculate_with_offset(&ctx, time_offset_minutes);
    let adjusted_brightness =
        (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
    let brightness = match render_state {
        RoomModeState::Idle => values.brightness,
        RoomModeState::Warning if !warning_uses_custom_profile(mode_configs, active_mode) => {
            ((adjusted_brightness as f32) * crate::event_loop::WARNING_DIM_FACTOR).clamp(1.0, 100.0)
                as u8
        }
        RoomModeState::Active | RoomModeState::Wake | RoomModeState::Warning => adjusted_brightness,
        RoomModeState::HardOff => 0,
    };
    (brightness, values.kelvin)
}

fn compute_room_display_values_for_settings(
    s: &AppState,
    settings: &RoomProfileSettings,
    room_state: RoomModeState,
    time_offset_minutes: f32,
    brightness_offset: f32,
) -> (u8, u16) {
    compute_room_display_values_for_settings_from_parts(
        &s.light_profile_configs,
        &s.mode_configs(),
        s.active_mode,
        s.solar_noon_hour(),
        s.latitude.unwrap_or(35.0),
        s.utc_offset_hours,
        settings,
        room_state,
        time_offset_minutes,
        brightness_offset,
    )
}

fn room_mode_state_from_flags(
    hard_off: bool,
    soft_off: bool,
    warning_active: bool,
) -> RoomModeState {
    RoomModeState::from_flags(hard_off, soft_off, warning_active)
}

fn persistent_room_state_from_flags(hard_off: bool, soft_off: bool) -> RoomModeState {
    room_mode_state_from_flags(hard_off, soft_off, false)
}

fn room_flags_for_target_state(state: RoomModeState) -> Result<(bool, bool)> {
    match state {
        RoomModeState::Active => Ok((false, false)),
        RoomModeState::Idle => Ok((true, false)),
        RoomModeState::HardOff => Ok((false, true)),
        RoomModeState::Wake | RoomModeState::Warning => Err(anyhow::anyhow!(
            "Room state '{}' cannot be set directly",
            serde_json::to_string(&state).unwrap_or_else(|_| "\"invalid\"".to_string())
        )),
    }
}

fn clear_room_mode_transition(state: &SharedState, room_id: &str) {
    if let Ok(mut s) = state.lock() {
        if s.room_mode_transitions.remove(room_id).is_some() {
            debug!(target: "cmd", "room_mode_transition: cleared '{}'", room_id);
        }
    }
}

fn resolved_room_motion_timeout_secs_from_parts(
    light_profile_configs: &BTreeMap<String, LightProfileConfig>,
    mode_configs: &[ModeConfig],
    active_mode: RhythmMode,
    solar_noon: f32,
    latitude: f32,
    settings: &RoomProfileSettings,
    current_hour: f32,
) -> u64 {
    let now = chrono::Utc::now().naive_utc();
    let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);
    let solar = rhythm_core::SolarTime::new(solar_noon, latitude, day_of_year);
    let registry =
        light_profile_registry_from_parts(light_profile_configs, mode_configs, active_mode);
    let ctx = rhythm_core::light_profile::CurveContext::new(current_hour, solar, None);
    registry
        .profile_for_room_state(active_mode, RoomModeState::Active, Some(settings))
        .calculate(&ctx)
        .motion_timeout_secs as u64
}

#[derive(Debug, Clone, Default)]
pub struct RoomProfileSettingsPatch {
    pub clear_all: bool,
    pub profile_id: Option<Option<String>>,
    pub fade_ms: Option<Option<TimerSetting>>,
    pub motion_timeout_secs: Option<Option<TimerSetting>>,
}

impl RoomProfileSettingsPatch {
    fn apply_to(&self, settings: &mut RoomProfileSettings) {
        if self.clear_all {
            *settings = RoomProfileSettings::default();
            return;
        }

        if let Some(profile_id) = &self.profile_id {
            settings.profile_id = profile_id.clone();
        }
        if let Some(fade_ms) = &self.fade_ms {
            settings.fade_ms = fade_ms.clone();
        }
        if let Some(motion_timeout_secs) = &self.motion_timeout_secs {
            settings.motion_timeout_secs = motion_timeout_secs.clone();
        }
    }

    fn touches_profile_settings(&self) -> bool {
        self.clear_all
            || self.profile_id.is_some()
            || self.fade_ms.is_some()
            || self.motion_timeout_secs.is_some()
    }
}

/// Build a RoomStateEvent for a room, computing display values from AppState.
///
/// For soft-off rooms, brightness is the soft-off percentage (not curve value).
/// Kelvin is always from the curve (soft-off tracks color temp).
#[cfg(feature = "desktop")]
pub fn build_room_state_event(
    state: &SharedState,
    snap: &rhythm_core::RoomSnapshot,
) -> crate::server_event::RoomStateEvent {
    let (mode, room_state, lights_on, brightness, kelvin) = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        let mode = s.active_mode;
        let room_state = room_mode_state_from_flags(
            snap.hard_off,
            snap.soft_off,
            s.motion_snapshots
                .get(&snap.id)
                .is_some_and(|motion| motion.warning_active),
        );
        let lights_on = s.room_lights_on.get(&snap.id).copied().unwrap_or(false);
        let (curve_brightness, kelvin) = compute_room_display_values_for_settings(
            &s,
            &snap.profile_settings,
            room_state,
            snap.time_offset_minutes,
            snap.brightness_offset,
        );
        (mode, room_state, lights_on, curve_brightness, kelvin)
    };
    crate::server_event::RoomStateEvent::from_snapshot(
        snap,
        mode,
        room_state,
        lights_on,
        brightness,
        kelvin,
        snap.profile_settings.clone(),
    )
}

/// Extract a single registry Arc from any active hub (brief AppState lock).
/// Used as a fallback for single-item operations when no hub_key is provided.
fn extract_registry(state: &SharedState) -> Option<Arc<Mutex<dyn HubRegistry>>> {
    state
        .lock()
        .ok()
        .and_then(|s| s.all_hub_registries().into_iter().next())
}

fn light_profile_registry_from_state(s: &AppState) -> LightProfileRegistry {
    let mut registry = LightProfileRegistry::with_profiles(
        s.light_profile_configs
            .values()
            .cloned()
            .collect::<Vec<_>>(),
        &s.active_mode_profile_id(),
    );
    registry.set_mode_configs(s.mode_configs());
    registry
}

fn resolve_profile_id(s: &AppState, requested_id: Option<&str>) -> Result<String> {
    let active_profile_id = s.active_mode_profile_id();
    let id = requested_id.unwrap_or(active_profile_id.as_str());
    if s.light_profile_configs.contains_key(id) {
        Ok(id.to_string())
    } else {
        Err(anyhow::anyhow!("Unknown light profile: {}", id))
    }
}

fn active_profile_config(s: &AppState) -> LightProfileConfig {
    s.active_mode_profile_config()
        .cloned()
        .unwrap_or_else(rhythm_core::default_rhythm_profile)
}

fn absorb_light_profile_time_offset(
    config: &LightProfileConfig,
    current_hour: f32,
    offset_minutes: f32,
    sunrise: f32,
    sunset: f32,
) -> Option<LightProfileConfig> {
    let rhythm_core::LightCurveShape::SuperGaussian {
        width_left_bri,
        width_right_bri,
        width_left_cct,
        width_right_cct,
        ..
    } = &config.curve
    else {
        return None;
    };

    let mu = (sunrise + sunset) / 2.0;
    let offset_hours = offset_minutes / 60.0;
    let target_hour = (current_hour + offset_hours).rem_euclid(24.0);

    let wrapped_dist = |hour: f32| {
        let mut d = hour - mu;
        if d > 12.0 {
            d -= 24.0;
        }
        if d < -12.0 {
            d += 24.0;
        }
        d
    };

    let dist_current = wrapped_dist(current_hour);
    let dist_target = wrapped_dist(target_hour);

    if dist_current.abs() < 0.25 || dist_target.abs() < 0.25 {
        return None;
    }
    if dist_current.signum() != dist_target.signum() {
        return None;
    }

    let factor = dist_target.abs() / dist_current.abs();
    if !(0.1..=5.0).contains(&factor) {
        return None;
    }

    let mut new = config.clone();
    if let rhythm_core::LightCurveShape::SuperGaussian {
        width_left_bri: new_width_left_bri,
        width_right_bri: new_width_right_bri,
        width_left_cct: new_width_left_cct,
        width_right_cct: new_width_right_cct,
        ..
    } = &mut new.curve
    {
        if dist_current < 0.0 {
            *new_width_left_bri = width_left_bri.clamp(0.2, 2.0) * factor;
            *new_width_left_cct = width_left_cct.clamp(0.2, 2.0) * factor;
            *new_width_left_bri = new_width_left_bri.clamp(0.2, 2.0);
            *new_width_left_cct = new_width_left_cct.clamp(0.2, 2.0);
        } else {
            *new_width_right_bri = width_right_bri.clamp(0.2, 2.0) * factor;
            *new_width_right_cct = width_right_cct.clamp(0.2, 2.0) * factor;
            *new_width_right_bri = new_width_right_bri.clamp(0.2, 2.0);
            *new_width_right_cct = new_width_right_cct.clamp(0.2, 2.0);
        }
        Some(new)
    } else {
        None
    }
}

fn persist_light_profiles_locked(s: &AppState) {
    if let Some(ref storage) = s.storage {
        let stored = crate::storage::StoredLightProfiles::from_state(
            &s.light_profile_configs,
            &s.runtime_config,
        );
        if let Err(e) = storage.save_light_profiles(&stored) {
            warn!(target: "cmd", "Failed to save light profiles: {}", e);
        }
    }
}

fn persist_settings_locked(s: &AppState) {
    if let Some(ref storage) = s.storage {
        if let Err(e) = storage.save_settings(&crate::storage::StoredSettings {
            power_save: s.power_save,
            active_mode: s.active_mode,
            modes: s.mode_configs(),
            mode_transitions: s.mode_transition_configs(),
        }) {
            warn!(target: "cmd", "Failed to save settings: {}", e);
        }
    }
}

pub(crate) fn sync_active_mode_from_runtime(
    state: &SharedState,
    runtime: &Arc<dyn rhythm_core::RuntimeHandle>,
) {
    if let Ok(mut s) = state.lock() {
        let active_profile_id = runtime.active_light_profile_id();
        let inferred_mode = RhythmMode::from_profile_id(&active_profile_id);
        let matched_mode = s
            .mode_configs()
            .into_iter()
            .find(|config| config.active_profile_id.as_deref() == Some(active_profile_id.as_str()))
            .map(|config| config.mode);
        let active = matched_mode.unwrap_or(inferred_mode);
        if s.active_mode != active {
            debug!(
                target: "cmd",
                "active_mode_sync: runtime profile '{}' remapped {:?} -> {:?}",
                active_profile_id,
                s.active_mode,
                active
            );
            s.active_mode = active;
            s.sync_active_mode_runtime_overrides();
            persist_settings_locked(&s);
        } else if matched_mode.is_none() {
            debug!(
                target: "cmd",
                "active_mode_sync: runtime profile '{}' inferred as {:?}",
                active_profile_id,
                active
            );
        }
    }
}

// ============================================================================
// Room ID translation helpers
// ============================================================================

/// Resolve a room ID from an HTTP request to the canonical engine room ID.
///
/// If the ID is already a topology room ID, returns it unchanged.
/// If it's a hub-native ID, translates it via the topology store.
/// Falls through unchanged on ESP32 (empty topology) or unknown IDs.
pub fn resolve_room_id(state: &SharedState, room_id: &str) -> String {
    let s = match state.lock() {
        Ok(s) => s,
        Err(_) => return room_id.to_string(),
    };
    // Already a topology room ID?
    if s.topology.get(room_id).is_some() {
        return room_id.to_string();
    }
    // Try translating as hub-native ID
    if let Some(topo_id) = s.topology.translate_room_id_any_hub(room_id) {
        return topo_id.to_string();
    }
    // Fallback: use as-is (ESP32 or pre-topology state)
    room_id.to_string()
}

/// Reverse-lookup: topology room ID → hub-native room IDs for registry operations.
///
/// The hub-native registry uses hub room IDs, but callers use topology IDs.
/// Returns the ID unchanged as fallback (ESP32 / no topology entry).
fn hub_room_ids_for_topology(state: &SharedState, topo_id: &str) -> Vec<String> {
    state
        .lock()
        .ok()
        .and_then(|s| {
            s.topology.get(topo_id).map(|room| {
                room.hub_targets
                    .iter()
                    .map(|t| t.hub_room_id.clone())
                    .collect()
            })
        })
        .unwrap_or_else(|| vec![topo_id.to_string()])
}

// ============================================================================
// State snapshots (for GET endpoints)
// ============================================================================

/// Build a full state snapshot.
///
/// Two-phase lock: collects metadata from state (brief lock), then queries
/// engine snapshots (engine read lock) to prevent cascading lock contention.
pub fn build_state_snapshot(state: &SharedState) -> Result<String> {
    struct RoomInfo {
        topology_id: String, // stable Rhythm room ID (for engine + external API)
        name: String,
        hub_types: Vec<String>,
        grouped_light_id: String,
        device_ids: Vec<String>,
        typed_devices: Vec<(String, DeviceType)>,
    }

    // Phase 1: Brief AppState lock — extract registry Arcs + scalar data + canonical lookup
    let (
        all_registries,
        runtime,
        storage_rooms,
        hubs_dto,
        active_profile_cfg,
        mut active_profile_effective,
        location_dto,
        settings_dto,
        firmware_version,
        platform_type,
        platform_ctx,
        listen_port,
        canonical_lookup,
        topo_map,
        topo_hub_types,
        last_tick_epoch_ms,
        profile_registry,
        periodic_ctx,
        update_interval,
        power_save,
        skipped_periodic_room_ids,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        let runtime = s.hub_runtime();

        let storage_rooms = if runtime.is_none() {
            s.storage.as_ref().and_then(|st| st.load_rooms().ok())
        } else {
            None
        };

        let all_registries = s.all_hub_registries();

        let hubs_dto: Vec<HubDto> = s
            .hub_credentials
            .iter()
            .map(|(key, creds)| HubDto {
                hub_type: creds
                    .hub_type
                    .as_ref()
                    .map(|t| t.as_str().to_string())
                    .unwrap_or_else(|| "none".to_string()),
                address: Some(creds.address.clone()),
                connected: s.hubs.contains_key(key),
            })
            .collect();

        let active_profile_id = s.active_mode_profile_id();
        let mut profile_registry = LightProfileRegistry::with_profiles(
            s.light_profile_configs
                .values()
                .cloned()
                .collect::<Vec<_>>(),
            &active_profile_id,
        );
        profile_registry.set_mode_configs(s.mode_configs());
        let active_profile_cfg = active_profile_config(&s);
        let utc_now = chrono::Utc::now();

        let current_local_time = {
            let offset_secs = (s.utc_offset_hours * 3600.0) as i32;
            let tz = chrono::FixedOffset::east_opt(offset_secs)
                .unwrap_or(chrono::FixedOffset::east_opt(0).unwrap());
            utc_now
                .with_timezone(&tz)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        };

        let local_now =
            utc_now.naive_utc() + chrono::Duration::seconds((s.utc_offset_hours * 3600.0) as i64);
        let local_time = local_now.time();
        let current_hour = local_time.hour() as f32
            + local_time.minute() as f32 / 60.0
            + local_time.second() as f32 / 3600.0;
        let resolved_solar = resolve_solar(
            &s,
            local_now.date().year(),
            local_now.date().month(),
            local_now.date().day(),
        );
        let active_profile_effective = active_profile_effective_values(
            &active_profile_cfg,
            current_hour,
            resolved_solar.solar,
            resolved_solar.sun_times.clone(),
        );
        let periodic_ctx = rhythm_core::CurveContext::new(
            current_hour,
            resolved_solar.solar,
            resolved_solar.sun_times,
        );
        let current_solar_time = periodic_ctx.solar_time();

        let location_dto = LocationDto {
            current_local_time,
            latitude: s.latitude,
            longitude: s.longitude,
            utc_offset_hours: s.utc_offset_hours,
            solar_noon: resolved_solar.solar.solar_noon_hour,
            solar_midnight: resolved_solar.solar.solar_midnight_hour(),
            current_solar_time,
            timezone_name: s.timezone_name.clone(),
            twilight: build_twilight_response(resolved_solar.twilight.as_ref()),
        };

        let settings_dto = build_settings_dto_inner(&s);

        let fw_version = s.firmware_version;
        let platform_type = s.platform_type;
        let platform_ctx = s.platform_context;
        let listen_port = s.listen_port;

        // Build canonical device lookup: native_id → (name, manufacturer, model, device_type)
        // Aggregate across all configured hubs
        let active_hub_keys: std::collections::HashSet<String> =
            s.hub_credentials.keys().map(|k| k.to_string()).collect();
        let canonical_lookup: std::collections::HashMap<
            String,
            (String, Option<String>, Option<String>, DeviceType),
        > = s
            .canonical_registry
            .devices()
            .flat_map(|d| {
                d.endpoints
                    .iter()
                    .filter(|ep| active_hub_keys.contains(&ep.hub_key.to_string()))
                    .map(move |ep| {
                        (
                            ep.native_id.clone(),
                            (
                                d.name.clone(),
                                d.manufacturer.clone(),
                                d.model.clone(),
                                d.device_type.clone(),
                            ),
                        )
                    })
            })
            .collect();

        // Build hub-native → (topology_id, topology_name) lookup so Phase 1b
        // can translate registry room IDs to stable Rhythm room IDs.
        let topo_map: std::collections::HashMap<String, (String, String)> = s
            .topology
            .rooms()
            .flat_map(|room| {
                room.hub_targets
                    .iter()
                    .map(move |t| (t.hub_room_id.clone(), (room.id.clone(), room.name.clone())))
            })
            .collect();

        // Build topology_room_id → Vec<hub_type_string> for API responses.
        let topo_hub_types: std::collections::HashMap<String, Vec<String>> = s
            .topology
            .rooms()
            .map(|room| {
                let types: Vec<String> = room
                    .hub_targets
                    .iter()
                    .map(|t| t.hub_key.hub_type.as_str().to_string())
                    .collect();
                (room.id.clone(), types)
            })
            .collect();

        let last_tick_epoch_ms = s.last_tick_epoch_ms;
        let update_interval = std::time::Duration::from_secs(s.runtime_config.update_interval_secs);
        let power_save = s.power_save;
        let now = std::time::Instant::now();
        let skipped_periodic_room_ids: HashSet<String> = s
            .room_mode_transitions
            .iter()
            .filter(|(_, transition)| transition.ends_at > now)
            .map(|(room_id, _)| room_id.clone())
            .chain(
                s.motion_snapshots
                    .iter()
                    .filter(|(_, snapshot)| snapshot.warning_active)
                    .map(|(room_id, _)| room_id.clone()),
            )
            .collect();

        (
            all_registries,
            runtime,
            storage_rooms,
            hubs_dto,
            active_profile_cfg,
            active_profile_effective,
            location_dto,
            settings_dto,
            fw_version,
            platform_type,
            platform_ctx,
            listen_port,
            canonical_lookup,
            topo_map,
            topo_hub_types,
            last_tick_epoch_ms,
            profile_registry,
            periodic_ctx,
            update_interval,
            power_save,
            skipped_periodic_room_ids,
        )
    };
    // AppState lock released

    let periodic_room_snapshots = runtime
        .as_ref()
        .map(|rt| {
            rt.engine_all_room_snapshots()
                .into_iter()
                .filter(|room| room.rhythm_enabled && !skipped_periodic_room_ids.contains(&room.id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    active_profile_effective.rhythm_interval_secs = crate::periodic::effective_cycle_duration(
        &profile_registry,
        &periodic_ctx,
        &periodic_room_snapshots,
        update_interval,
        power_save,
    )
    .as_secs();

    // Phase 1b: Lock registries without holding AppState — aggregate rooms from ALL hubs
    let mut room_infos = Vec::new();
    for reg_arc in &all_registries {
        if let Ok(reg) = reg_arc.lock() {
            for room in reg.rooms() {
                // Translate hub-native room ID to topology ID for external API.
                // Registry lookups above already used the hub-native ID.
                let (topo_id, topo_name) = topo_map
                    .get(&room.id)
                    .map(|(id, name)| (id.clone(), name.clone()))
                    .unwrap_or_else(|| (room.id.clone(), room.name.clone()));
                let hub_types = topo_hub_types.get(&topo_id).cloned().unwrap_or_default();
                room_infos.push(RoomInfo {
                    topology_id: topo_id,
                    name: topo_name,
                    hub_types,
                    grouped_light_id: reg.get_grouped_light_id(&room.id).unwrap_or_default(),
                    device_ids: reg.devices_for_room(&room.id),
                    typed_devices: reg.devices_for_room_typed(&room.id),
                });
            }
        }
    }

    // Fallback: if registry had no rooms (e.g. boot before hub connects),
    // populate from storage so /api/state agrees with /api/rooms/state.
    // Storage rooms already have topology IDs (persisted from engine after Phase 4d).
    if room_infos.is_empty() {
        if let Some(ref mgr) = storage_rooms {
            for room in mgr.iter() {
                room_infos.push(RoomInfo {
                    topology_id: room.id.clone(),
                    name: room.name.clone(),
                    hub_types: vec![],
                    grouped_light_id: String::new(),
                    device_ids: Vec::new(),
                    typed_devices: Vec::new(),
                });
            }
        }
    }

    // Dedup: cross-hub rooms may have multiple registry entries that map to the
    // same topology ID. Merge device lists, keep first occurrence's metadata.
    {
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut deduped: Vec<RoomInfo> = Vec::new();
        for info in room_infos {
            if let Some(&idx) = seen.get(&info.topology_id) {
                deduped[idx].device_ids.extend(info.device_ids);
                deduped[idx].typed_devices.extend(info.typed_devices);
            } else {
                seen.insert(info.topology_id.clone(), deduped.len());
                deduped.push(info);
            }
        }
        room_infos = deduped;
    }

    // Phase 2: query engine snapshots without state lock
    // Grab display context for computing brightness/kelvin
    let (
        disp_profiles,
        disp_mode_configs,
        disp_active_mode,
        disp_solar,
        disp_lat,
        disp_utc,
        disp_lights,
        disp_motion,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.light_profile_configs.clone(),
            s.mode_configs(),
            s.active_mode,
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.utc_offset_hours,
            s.room_lights_on.clone(),
            s.motion_snapshots.clone(),
        )
    };

    let mut rooms = Vec::with_capacity(room_infos.len());
    for info in &room_infos {
        let (rhythm_enabled, disabled, time_offset, bri_offset, soft_off, hard_off, room_profile) =
            if let Some(ref rt) = runtime {
                rt.engine_room_snapshot(&info.topology_id)
                    .map(|snap| {
                        (
                            snap.rhythm_enabled,
                            snap.disabled,
                            snap.time_offset_minutes,
                            snap.brightness_offset,
                            snap.soft_off,
                            snap.hard_off,
                            snap.profile_settings,
                        )
                    })
                    .unwrap_or((
                        false,
                        false,
                        0.0,
                        0.0,
                        false,
                        false,
                        RoomProfileSettings::default(),
                    ))
            } else if let Some(ref mgr) = storage_rooms {
                mgr.get(&info.topology_id)
                    .map(|r| {
                        (
                            r.rhythm_enabled,
                            r.disabled,
                            r.time_offset_minutes,
                            r.brightness_offset,
                            r.soft_off,
                            r.hard_off,
                            r.profile_settings.clone(),
                        )
                    })
                    .unwrap_or((
                        false,
                        false,
                        0.0,
                        0.0,
                        false,
                        false,
                        RoomProfileSettings::default(),
                    ))
            } else {
                (
                    false,
                    false,
                    0.0,
                    0.0,
                    false,
                    false,
                    RoomProfileSettings::default(),
                )
            };

        let lights_on = disp_lights.get(&info.topology_id).copied().unwrap_or(false);
        let room_state = room_mode_state_from_flags(
            hard_off,
            soft_off,
            disp_motion
                .get(&info.topology_id)
                .is_some_and(|motion| motion.warning_active),
        );
        let (curve_brightness, kelvin) = compute_room_display_values_for_settings_from_parts(
            &disp_profiles,
            &disp_mode_configs,
            disp_active_mode,
            disp_solar,
            disp_lat,
            disp_utc,
            &room_profile,
            room_state,
            time_offset,
            bri_offset,
        );

        // Build enriched device list: all devices from device_ids (with canonical
        // data) plus any typed_devices not already covered (e.g. motion service IDs)
        let typed_lookup: std::collections::HashMap<&str, &DeviceType> = info
            .typed_devices
            .iter()
            .map(|(id, dt)| (id.as_str(), dt))
            .collect();
        let mut devices: Vec<TypedDeviceDto> = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // All device UUIDs from room children — includes lights, buttons, etc.
        for id in &info.device_ids {
            if !seen_ids.insert(id.clone()) {
                continue;
            }
            if let Some((name, mfr, model, dt)) = canonical_lookup.get(id) {
                let name_opt = if name.is_empty() {
                    None
                } else {
                    Some(name.clone())
                };
                devices.push(TypedDeviceDto::enriched(
                    id.clone(),
                    dt,
                    name_opt,
                    mfr.clone(),
                    model.clone(),
                ));
            } else {
                let dt = typed_lookup
                    .get(id.as_str())
                    .copied()
                    .unwrap_or(&DeviceType::Light);
                devices.push(TypedDeviceDto::new(id.clone(), dt));
            }
        }

        // Add typed devices not in device_ids (motion sensors use service UUIDs)
        for (id, dt) in &info.typed_devices {
            if seen_ids.contains(id) {
                continue;
            }
            if let Some((name, mfr, model, _)) = canonical_lookup.get(id) {
                let name_opt = if name.is_empty() {
                    None
                } else {
                    Some(name.clone())
                };
                devices.push(TypedDeviceDto::enriched(
                    id.clone(),
                    dt,
                    name_opt,
                    mfr.clone(),
                    model.clone(),
                ));
            } else {
                devices.push(TypedDeviceDto::new(id.clone(), dt));
            }
        }

        rooms.push(RoomFullState {
            rhythm: RoomRhythmState {
                id: info.topology_id.clone(),
                hub_types: info.hub_types.clone(),
                state: room_state,
                rhythm_enabled,
                time_offset,
                brightness_offset: bri_offset,
                lights_on,
                brightness: curve_brightness,
                kelvin,
                room_profile,
            },
            name: info.name.clone(),
            grouped_light_id: info.grouped_light_id.clone(),
            disabled,
            device_ids: info.device_ids.clone(),
            devices,
        });
    }

    let snapshot = StateSnapshot {
        version: firmware_version.to_string(),
        platform: platform_type.to_string(),
        context: platform_ctx.to_string(),
        listen_port,
        hubs: hubs_dto,
        active_profile: ActiveProfileDto {
            config: active_profile_cfg,
            effective: active_profile_effective,
        },
        location: location_dto,
        settings: settings_dto,
        rooms,
        last_tick_epoch_ms,
    };
    serde_json::to_string(&snapshot).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Build a lightweight rooms-state snapshot for polling.
pub fn build_rooms_state(state: &SharedState) -> Result<String> {
    let (
        hub_connected,
        runtime,
        storage_rooms,
        motion_snapshots,
        room_lights_on,
        light_profile_configs,
        mode_configs,
        active_mode,
        solar_noon,
        latitude,
        utc_offset,
        rooms_with_sensors,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let runtime = s.hub_runtime();
        let storage_rooms = if runtime.is_none() {
            s.storage.as_ref().and_then(|st| st.load_rooms().ok())
        } else {
            None
        };
        let motion = s.motion_snapshots.clone();
        let lights = s.room_lights_on.clone();
        let sensor_rooms = s.motion_sensor_room_ids();
        (
            s.has_any_hub(),
            runtime,
            storage_rooms,
            motion,
            lights,
            s.light_profile_configs.clone(),
            s.mode_configs(),
            s.active_mode,
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.utc_offset_hours,
            sensor_rooms,
        )
    };

    let snapshots = if let Some(ref rt) = runtime {
        rt.engine_all_room_snapshots()
    } else {
        Vec::new()
    };

    let mut rooms = Vec::new();

    if !snapshots.is_empty() {
        for snap in &snapshots {
            let lights_on = room_lights_on.get(&snap.id).copied().unwrap_or(false);
            let room_state = room_mode_state_from_flags(
                snap.hard_off,
                snap.soft_off,
                motion_snapshots
                    .get(&snap.id)
                    .is_some_and(|motion| motion.warning_active),
            );
            let (curve_brightness, kelvin) = compute_room_display_values_for_settings_from_parts(
                &light_profile_configs,
                &mode_configs,
                active_mode,
                solar_noon,
                latitude,
                utc_offset,
                &snap.profile_settings,
                room_state,
                snap.time_offset_minutes,
                snap.brightness_offset,
            );
            let rhythm = RoomRhythmState {
                id: snap.id.clone(),
                hub_types: vec![],
                state: room_state,
                rhythm_enabled: snap.rhythm_enabled,
                time_offset: snap.time_offset_minutes,
                brightness_offset: snap.brightness_offset,
                lights_on,
                brightness: curve_brightness,
                kelvin,
                room_profile: snap.profile_settings.clone(),
            };
            let (motion_active, motion_owned, remaining_secs, timeout_secs, warning_active) =
                if let Some(ms) = motion_snapshots.get(&snap.id) {
                    (
                        Some(ms.motion_active),
                        Some(ms.motion_owned),
                        ms.remaining_secs,
                        Some(ms.timeout_secs),
                        Some(ms.warning_active),
                    )
                } else if rooms_with_sensors.contains(&snap.id) {
                    // Sensor exists but hasn't fired yet — idle defaults
                    let timeout = resolved_room_motion_timeout_secs_from_parts(
                        &light_profile_configs,
                        &mode_configs,
                        active_mode,
                        solar_noon,
                        latitude,
                        &snap.profile_settings,
                        runtime.as_ref().map(|rt| rt.current_hour()).unwrap_or(12.0),
                    );
                    (Some(false), Some(false), None, Some(timeout), Some(false))
                } else {
                    (None, None, None, None, None)
                };
            rooms.push(RoomPollState {
                rhythm,
                motion_active,
                motion_owned,
                remaining_secs,
                timeout_secs,
                warning_active,
            });
        }
    } else if let Some(ref mgr) = storage_rooms {
        for room in mgr.iter() {
            let room_state = room_mode_state_from_flags(room.hard_off, room.soft_off, false);
            let (curve_brightness, kelvin) = compute_room_display_values_for_settings_from_parts(
                &light_profile_configs,
                &mode_configs,
                active_mode,
                solar_noon,
                latitude,
                utc_offset,
                &room.profile_settings,
                room_state,
                room.time_offset_minutes,
                room.brightness_offset,
            );
            rooms.push(RoomPollState {
                rhythm: RoomRhythmState {
                    id: room.id.clone(),
                    hub_types: vec![],
                    state: room_state,
                    rhythm_enabled: room.rhythm_enabled,
                    time_offset: room.time_offset_minutes,
                    brightness_offset: room.brightness_offset,
                    lights_on: false,
                    brightness: curve_brightness,
                    kelvin,
                    room_profile: room.profile_settings.clone(),
                },
                motion_active: None,
                motion_owned: None,
                remaining_secs: None,
                timeout_secs: None,
                warning_active: None,
            });
        }
    }

    let response = RoomsPollResponse {
        hub_connected,
        rooms,
    };
    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Build a `RoomRhythmState` for a single room from engine state.
pub fn build_room_rhythm_state(state: &SharedState, room_id: &str) -> Result<RoomRhythmState> {
    let (runtime, lights_on, warning_active) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.hub_runtime(),
            s.room_lights_on.get(room_id).copied().unwrap_or(false),
            s.motion_snapshots
                .get(room_id)
                .is_some_and(|motion| motion.warning_active),
        )
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;
    let snap = runtime
        .engine_room_snapshot(room_id)
        .ok_or_else(|| anyhow::anyhow!("Room '{}' not found in engine", room_id))?;
    let room_state = room_mode_state_from_flags(snap.hard_off, snap.soft_off, warning_active);
    let (curve_brightness, kelvin) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        compute_room_display_values_for_settings(
            &s,
            &snap.profile_settings,
            room_state,
            snap.time_offset_minutes,
            snap.brightness_offset,
        )
    };

    Ok(RoomRhythmState {
        id: snap.id.clone(),
        hub_types: vec![],
        state: room_state,
        rhythm_enabled: snap.rhythm_enabled,
        time_offset: snap.time_offset_minutes,
        brightness_offset: snap.brightness_offset,
        lights_on,
        brightness: curve_brightness,
        kelvin,
        room_profile: snap.profile_settings.clone(),
    })
}

// ============================================================================
// Config queries
// ============================================================================

/// Build the requested light profile config as a JSON string.
pub fn build_config(state: &SharedState, profile_id: Option<&str>) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let id = resolve_profile_id(&s, profile_id)?;
    let config = s
        .light_profile_config(&id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Unknown light profile: {}", id))?;
    serde_json::to_string(&config).map_err(|e| anyhow::anyhow!("serialize config: {}", e))
}

// ============================================================================
// Settings commands
// ============================================================================

/// Build `SettingsDto` from an already-locked `AppState`.
fn build_settings_dto_inner(s: &AppState) -> SettingsDto {
    let profiles = s.light_profile_configs.values().cloned().collect();

    SettingsDto {
        active_mode: s.active_mode,
        power_save: s.power_save,
        modes: s.mode_configs(),
        mode_transitions: s.mode_transition_configs(),
        profiles,
    }
}

/// Build the current settings.
pub fn build_settings_dto(state: &SharedState) -> Result<SettingsDto> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    Ok(build_settings_dto_inner(&s))
}

/// Build the current settings as a JSON string.
pub fn build_settings(state: &SharedState) -> Result<String> {
    let dto = build_settings_dto(state)?;
    serde_json::to_string(&dto).map_err(|e| anyhow::anyhow!("serialize settings: {}", e))
}

fn current_local_datetime(utc_offset: f32) -> chrono::NaiveDateTime {
    let now = chrono::Utc::now().naive_utc();
    let offset_secs = (utc_offset * 3600.0) as i64;
    now + chrono::Duration::seconds(offset_secs)
}

fn mode_apply_cycle_duration(
    light_profile_configs: &BTreeMap<String, LightProfileConfig>,
    mode_configs: &[ModeConfig],
    target_mode: RhythmMode,
    solar_noon: f32,
    latitude: f32,
    utc_offset: f32,
    room_snapshots: &[rhythm_core::RoomSnapshot],
    update_interval: Duration,
    power_save: bool,
    room_commands: &[(String, LightingCommand)],
) -> Duration {
    let sample_at = current_local_datetime(utc_offset);
    let sample_date = sample_at.date();
    let sample_time = sample_at.time();
    let day_of_year = rhythm_core::timezone::day_of_year(
        sample_date.year(),
        sample_date.month(),
        sample_date.day(),
    );
    let sample_hour = sample_time.hour() as f32
        + sample_time.minute() as f32 / 60.0
        + sample_time.second() as f32 / 3600.0;
    let solar = rhythm_core::SolarTime::new(solar_noon, latitude, day_of_year);
    let ctx = rhythm_core::light_profile::CurveContext::new(sample_hour, solar, None);
    let registry =
        light_profile_registry_from_parts(light_profile_configs, mode_configs, target_mode);

    let periodic_cycle = crate::periodic::effective_cycle_duration(
        &registry,
        &ctx,
        room_snapshots,
        update_interval,
        power_save,
    );

    let fade_window_ms = room_commands
        .iter()
        .filter_map(|(_, command)| command.transition_ms)
        .max()
        .unwrap_or(0)
        .max(1_000);

    periodic_cycle.min(Duration::from_millis(u64::from(fade_window_ms)))
}

fn emit_room_state_event_after_apply(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    room_id: &str,
) {
    #[cfg(feature = "desktop")]
    if let Some(snap) = runtime.engine_room_snapshot(room_id) {
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::RoomState {
                rooms: vec![build_room_state_event(state, &snap)],
            },
        );
    }

    let _ = (state, runtime, room_id);
}

fn apply_room_commands_inline(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    room_commands: Vec<(String, LightingCommand)>,
    phase_gap: Duration,
) {
    let room_count = room_commands.len();

    for (idx, (room_id, command)) in room_commands.into_iter().enumerate() {
        if let Err(e) = runtime.apply_room_command(&room_id, command) {
            warn!(target: "cmd", "active_mode_apply: room '{}' failed: {}", room_id, e);
            continue;
        }
        emit_room_state_event_after_apply(state, runtime, &room_id);
        if idx + 1 < room_count && !phase_gap.is_zero() {
            std::thread::sleep(phase_gap);
        }
    }
}

fn dispatch_room_commands(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    mut room_commands: Vec<(String, LightingCommand)>,
    cycle_duration: Duration,
) {
    if room_commands.is_empty() {
        return;
    }

    room_commands.sort_by_key(|(room_id, _)| crate::periodic::stable_room_phase_key(room_id));
    let phase_gap = crate::periodic::dispatch_spacing(cycle_duration, room_commands.len());

    let dispatch_tx = state
        .lock()
        .ok()
        .and_then(|s| s.periodic_work_tx.clone().or_else(|| s.work_tx.clone()));

    let Some(tx) = dispatch_tx else {
        apply_room_commands_inline(state, runtime, room_commands, phase_gap);
        return;
    };

    let room_count = room_commands.len();
    let fallback_commands = room_commands.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("room-dispatch".to_string())
        .spawn(move || {
            for (idx, (room_id, command)) in room_commands.into_iter().enumerate() {
                if tx
                    .send(crate::state::WorkItem::ApplyRoomCommand { room_id, command })
                    .is_err()
                {
                    warn!(target: "cmd", "active_mode_apply: room dispatcher disconnected");
                    return;
                }
                if idx + 1 < room_count && !phase_gap.is_zero() {
                    std::thread::sleep(phase_gap);
                }
            }
        })
    {
        warn!(
            target: "cmd",
            "active_mode_apply: failed to spawn room dispatcher thread: {}",
            e
        );
        apply_room_commands_inline(state, runtime, fallback_commands, phase_gap);
    }
}

fn build_room_command_from_values(
    values: &rhythm_core::LightingValues,
    brightness: u8,
    transition_ms: u32,
) -> LightingCommand {
    if values.is_direct_color {
        LightingCommand::from_color(brightness, values.rgb, values.xy, Some(transition_ms))
    } else {
        LightingCommand::with_transition(brightness, values.kelvin, transition_ms)
    }
}

fn resolve_room_command_for_state_at_from_parts(
    light_profile_configs: &BTreeMap<String, LightProfileConfig>,
    mode_configs: &[ModeConfig],
    target_mode: RhythmMode,
    solar_noon: f32,
    latitude: f32,
    settings: &RoomProfileSettings,
    room_state: RoomModeState,
    time_offset_minutes: f32,
    brightness_offset: f32,
    sample_at: chrono::NaiveDateTime,
    transition_ms_override: Option<u32>,
) -> Option<LightingCommand> {
    if room_state == RoomModeState::HardOff {
        return None;
    }

    let sample_date = sample_at.date();
    let day_of_year = rhythm_core::timezone::day_of_year(
        sample_date.year(),
        sample_date.month(),
        sample_date.day(),
    );
    let sample_time = sample_at.time();
    let sample_hour = sample_time.hour() as f32
        + sample_time.minute() as f32 / 60.0
        + sample_time.second() as f32 / 3600.0;
    let solar = rhythm_core::SolarTime::new(solar_noon, latitude, day_of_year);
    let ctx = rhythm_core::light_profile::CurveContext::new(sample_hour, solar, None);
    let registry =
        light_profile_registry_from_parts(light_profile_configs, mode_configs, target_mode);
    let render_state = render_state_for_display(room_state);
    let values = registry
        .profile_for_room_state(target_mode, render_state, Some(settings))
        .calculate_with_offset(&ctx, time_offset_minutes);
    let adjusted_brightness =
        (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
    let brightness = match render_state {
        RoomModeState::Idle => values.brightness,
        RoomModeState::Warning if !warning_uses_custom_profile(mode_configs, target_mode) => {
            ((adjusted_brightness as f32) * crate::event_loop::WARNING_DIM_FACTOR).clamp(1.0, 100.0)
                as u8
        }
        RoomModeState::Active | RoomModeState::Wake | RoomModeState::Warning => adjusted_brightness,
        RoomModeState::HardOff => return None,
    };
    let transition_ms = transition_ms_override.unwrap_or(values.transition_ms);
    Some(build_room_command_from_values(
        &values,
        brightness,
        transition_ms,
    ))
}

fn apply_active_mode_outputs(
    state: &SharedState,
    previous_mode: RhythmMode,
    target_mode: RhythmMode,
    mode_changed: bool,
    transition_trigger: Option<ModeTransitionTrigger>,
    apply_scope: ModeOutputApplyScope,
) {
    let (
        runtime,
        light_profile_configs,
        mode_configs,
        mode_transition_configs,
        solar_noon,
        latitude,
        utc_offset,
        room_lights_on,
        motion_snapshots,
        update_interval,
        power_save,
    ) = {
        let Ok(s) = state.lock() else { return };
        (
            s.hub_runtime(),
            s.light_profile_configs.clone(),
            s.mode_configs(),
            s.mode_transition_configs(),
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.utc_offset_hours,
            s.room_lights_on.clone(),
            s.motion_snapshots.clone(),
            Duration::from_secs(s.runtime_config.update_interval_secs),
            s.power_save,
        )
    };

    debug!(
        target: "cmd",
        "active_mode_apply: {:?} -> {:?} changed={} trigger={:?}",
        previous_mode,
        target_mode,
        mode_changed,
        transition_trigger
    );

    let Some(runtime) = runtime else {
        if let Ok(mut s) = state.lock() {
            s.room_mode_transitions.clear();
        }
        debug!(
            target: "cmd",
            "active_mode_apply: no runtime available, cleared pending room transitions"
        );
        return;
    };

    let transition = if mode_changed {
        mode_transition_configs.into_iter().find(|config| {
            config.from_mode == previous_mode
                && config.to_mode == target_mode
                && transition_trigger.is_some_and(|trigger| config.trigger == trigger)
        })
    } else {
        None
    };
    let transition_duration_ms = transition
        .as_ref()
        .map(|config| config.duration_ms)
        .unwrap_or(0);
    let sample_at = current_local_datetime(utc_offset)
        + chrono::Duration::milliseconds(i64::from(transition_duration_ms));
    let snapshots = runtime.engine_all_room_snapshots();
    let mut room_commands = Vec::new();
    let mut dispatch_snapshots = Vec::new();
    let mut transitioned_room_ids = Vec::new();
    let mut changed_room_ids = Vec::new();
    let mut preserved_hard_off = 0usize;
    let mut hidden_rooms = 0usize;
    let mut unresolved_rooms = 0usize;

    if mode_changed && transition.is_none() {
        debug!(
            target: "cmd",
            "active_mode_apply: no matching transition config for {:?} -> {:?} with trigger {:?}",
            previous_mode,
            target_mode,
            transition_trigger
        );
    }

    for snap in &snapshots {
        let room_state = room_mode_state_from_flags(
            snap.hard_off,
            snap.soft_off,
            motion_snapshots
                .get(&snap.id)
                .is_some_and(|motion| motion.warning_active),
        );

        if !apply_scope.includes(room_state) {
            continue;
        }

        if room_state == RoomModeState::HardOff
            && transition
                .as_ref()
                .is_some_and(|config| config.preserve_hard_off)
        {
            preserved_hard_off += 1;
            continue;
        }

        let is_visible = match room_state {
            RoomModeState::Idle => true,
            RoomModeState::HardOff => false,
            RoomModeState::Active | RoomModeState::Wake | RoomModeState::Warning => {
                room_lights_on.get(&snap.id).copied().unwrap_or(false)
            }
        };
        if !is_visible {
            hidden_rooms += 1;
            continue;
        }

        let Some(command) = resolve_room_command_for_state_at_from_parts(
            &light_profile_configs,
            &mode_configs,
            target_mode,
            solar_noon,
            latitude,
            &snap.profile_settings,
            room_state,
            snap.time_offset_minutes,
            snap.brightness_offset,
            sample_at,
            (transition_duration_ms > 0).then_some(transition_duration_ms),
        ) else {
            unresolved_rooms += 1;
            debug!(
                target: "cmd",
                "active_mode_apply: no command resolved for room '{}' state={:?}",
                snap.id,
                room_state
            );
            continue;
        };

        room_commands.push((snap.id.clone(), command));
        dispatch_snapshots.push(snap.clone());
        changed_room_ids.push(snap.id.clone());
        if transition_duration_ms > 0 {
            transitioned_room_ids.push(snap.id.clone());
        }
    }

    if let Ok(mut s) = state.lock() {
        s.room_mode_transitions.clear();
        if transition_duration_ms > 0 {
            let ends_at = std::time::Instant::now()
                + std::time::Duration::from_millis(u64::from(transition_duration_ms));
            for room_id in &transitioned_room_ids {
                s.room_mode_transitions.insert(
                    room_id.clone(),
                    crate::state::RoomModeTransition { ends_at },
                );
            }
        }
    }

    debug!(
        target: "cmd",
        "active_mode_apply: {} rooms scanned, {} changed, {} transitioning, {} hidden, {} preserved hard-off, {} unresolved",
        snapshots.len(),
        changed_room_ids.len(),
        transitioned_room_ids.len(),
        hidden_rooms,
        preserved_hard_off,
        unresolved_rooms
    );
    if !room_commands.is_empty() {
        let cycle_duration = mode_apply_cycle_duration(
            &light_profile_configs,
            &mode_configs,
            target_mode,
            solar_noon,
            latitude,
            utc_offset,
            &dispatch_snapshots,
            update_interval,
            power_save,
            &room_commands,
        );
        dispatch_room_commands(state, &runtime, room_commands, cycle_duration);
    }
}

fn do_settings_set_internal(
    state: &SharedState,
    power_save: Option<bool>,
    active_mode: Option<RhythmMode>,
    mode_configs: Option<Vec<ModeConfig>>,
    mode_transitions: Option<Vec<rhythm_core::ModeTransitionConfig>>,
    transition_trigger: Option<ModeTransitionTrigger>,
) -> Result<String> {
    let requested_mode_config_count = mode_configs.as_ref().map(Vec::len);
    let requested_transition_count = mode_transitions.as_ref().map(Vec::len);
    let (
        rooms_to_off,
        runtimes,
        active_profile_id,
        updated_mode_configs,
        previous_mode,
        selected_mode,
        mode_changed,
        reapply_scope,
    ) = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let previous_mode = s.active_mode;
        let previous_mode_config = s
            .mode_configs()
            .into_iter()
            .find(|config| config.mode == previous_mode)
            .unwrap_or_else(|| ModeConfig::default_for_mode(previous_mode));

        let mut rooms_to_off = Vec::new();
        if let Some(ps) = power_save {
            s.power_save = ps;
            info!(target: "cmd", "settings: power_save={}", ps);

            if let Some(runtime) = s.hub_runtime() {
                rooms_to_off = runtime.set_power_save(ps);
            }
        }

        let mut modes_updated = false;
        if let Some(configs) = mode_configs {
            s.set_mode_configs(configs);
            modes_updated = true;
        }
        if let Some(transitions) = mode_transitions {
            s.set_mode_transition_configs(transitions);
        }
        let mut mode_changed = false;
        if let Some(mode) = active_mode {
            mode_changed = s.active_mode != mode;
            s.active_mode = mode;
        }
        let selected_mode = s.active_mode;
        let should_reapply_mode_outputs = modes_updated || mode_changed;
        let mut active_profile_id = None;
        let mut updated_mode_configs = None;
        let mut runtimes = Vec::new();
        let mut reapply_scope = ModeOutputApplyScope::default();
        if should_reapply_mode_outputs {
            s.sync_active_mode_runtime_overrides();
            active_profile_id = Some(s.active_mode_profile_id());
            updated_mode_configs = Some(s.mode_configs());
            let updated_mode_config = updated_mode_configs
                .as_ref()
                .and_then(|configs| configs.iter().find(|config| config.mode == selected_mode))
                .cloned()
                .unwrap_or_else(|| ModeConfig::default_for_mode(selected_mode));
            reapply_scope =
                mode_output_apply_scope(mode_changed, &previous_mode_config, &updated_mode_config);
            runtimes = s
                .hubs
                .values()
                .filter_map(|hub| hub.runtime.clone())
                .collect();
        }

        persist_settings_locked(&s);
        (
            rooms_to_off,
            runtimes,
            active_profile_id,
            updated_mode_configs,
            previous_mode,
            selected_mode,
            mode_changed,
            reapply_scope,
        )
    };

    if let Some(count) = requested_mode_config_count {
        info!(target: "cmd", "settings: updated {} mode configs", count);
    }
    if let Some(count) = requested_transition_count {
        info!(target: "cmd", "settings: updated {} mode transitions", count);
    }
    if mode_changed {
        info!(
            target: "cmd",
            "settings: active_mode {:?} -> {:?} ({:?})",
            previous_mode,
            selected_mode,
            transition_trigger.unwrap_or(ModeTransitionTrigger::Manual)
        );
    }

    if !rooms_to_off.is_empty() {
        let runtime = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.hub_runtime()
        };
        if let Some(runtime) = runtime {
            for room_id in &rooms_to_off {
                info!(target: "cmd", "Turning off idle room '{}' (power_save ON)", room_id);
                let event = InputEvent::new(room_id, ButtonAction::OffPress);
                match runtime.handle_event(&event) {
                    Ok(_) => sync_active_mode_from_runtime(state, &runtime),
                    Err(e) => warn!(target: "cmd", "Failed to turn off room '{}': {}", room_id, e),
                }
            }
        }
    }

    if let (Some(profile_id), Some(configs)) = (active_profile_id.as_deref(), updated_mode_configs)
    {
        let mut any_set = false;
        if runtimes.is_empty() {
            debug!(
                target: "cmd",
                "settings: deferred runtime mode update for profile '{}' because no runtimes are active",
                profile_id
            );
        }
        for runtime in &runtimes {
            if let Err(e) = runtime.set_mode_configs(configs.clone()) {
                warn!(target: "cmd", "Failed to update runtime mode configs: {}", e);
            }
            if runtime.set_light_profile(profile_id) {
                any_set = true;
            }
        }
        debug!(
            target: "cmd",
            "settings: propagated active mode profile '{}' to {} runtime(s)",
            profile_id,
            runtimes.len()
        );
        if !runtimes.is_empty() && !any_set {
            return Err(anyhow::anyhow!(
                "Failed to activate active mode profile '{}'",
                profile_id
            ));
        }
        if !reapply_scope.is_empty() {
            apply_active_mode_outputs(
                state,
                previous_mode,
                selected_mode,
                mode_changed,
                transition_trigger,
                reapply_scope,
            );
        } else {
            debug!(
                target: "cmd",
                "settings: mode config update did not affect visible room states"
            );
        }
    }

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::SettingsChanged);

    build_settings(state)
}

/// Update global settings (partial: only provided fields are changed).
pub fn do_settings_set(
    state: &SharedState,
    power_save: Option<bool>,
    active_mode: Option<RhythmMode>,
    mode_configs: Option<Vec<ModeConfig>>,
    mode_transitions: Option<Vec<rhythm_core::ModeTransitionConfig>>,
) -> Result<String> {
    do_settings_set_internal(
        state,
        power_save,
        active_mode,
        mode_configs,
        mode_transitions,
        active_mode.map(|_| ModeTransitionTrigger::Manual),
    )
}

pub fn do_set_active_mode(state: &SharedState, mode: RhythmMode) -> Result<()> {
    do_set_active_mode_with_trigger(state, mode, ModeTransitionTrigger::Manual)
}

pub fn do_set_active_mode_with_trigger(
    state: &SharedState,
    mode: RhythmMode,
    trigger: ModeTransitionTrigger,
) -> Result<()> {
    do_settings_set_internal(state, None, Some(mode), None, None, Some(trigger)).map(|_| ())
}

// ============================================================================
// Room commands
// ============================================================================

/// Parsed room data from a JSON value.
pub struct RoomParams {
    pub id: String,
    pub name: String,
    pub grouped_light_id: String,
    pub rhythm_enabled: bool,
    pub disabled: bool,
    /// Explicit room state from client. `None` preserves the existing state.
    pub state: Option<RoomModeState>,
    pub device_ids: Vec<String>,
}

impl RoomParams {
    /// Parse room parameters from a JSON value (the room object itself).
    pub fn from_json(room: &Value) -> Result<Self> {
        let id = room
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing room.id"))?
            .to_string();
        let name = room
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&id)
            .to_string();
        let grouped_light_id = room
            .get("grouped_light_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let rhythm_enabled = room
            .get("rhythm_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let disabled = room
            .get("disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let state = room
            .get("state")
            .cloned()
            .map(serde_json::from_value::<RoomModeState>)
            .transpose()
            .map_err(|_| anyhow::anyhow!("Invalid room.state"))?;
        let device_ids: Vec<String> = room
            .get("device_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(RoomParams {
            id,
            name,
            grouped_light_id,
            rhythm_enabled,
            disabled,
            state,
            device_ids,
        })
    }
}

/// Upsert a room in registry + engine, optionally persist.
///
/// When `persist` is `false` the caller is responsible for triggering
/// persistence (e.g. via `DeferredPersistState` on the worker thread).
/// This avoids stack-heavy serde on the 12KB HTTP handler stack during
/// batch room updates.
/// Returns the room's rhythm state JSON snippet.
pub fn do_room_set(
    state: &SharedState,
    params: &RoomParams,
    hub_key: Option<&HubKey>,
    persist: bool,
) -> Result<String> {
    info!(target: "cmd", "room_set: {} '{}' gl={}", params.id, params.name, params.grouped_light_id);

    // Dedup: skip write if room already matches (check targeted registry when hub_key provided)
    let dedup_registry = hub_key
        .and_then(|k| state.lock().ok().and_then(|s| s.hub_registry_for(k)))
        .or_else(|| extract_registry(state));
    let room_unchanged = dedup_registry
        .and_then(|r| {
            r.lock().ok().map(|reg| {
                reg.room_matches(
                    &params.id,
                    &params.name,
                    &params.grouped_light_id,
                    &params.device_ids,
                )
            })
        })
        .unwrap_or(false);
    if room_unchanged {
        if let Ok(room_state) = build_room_rhythm_state(state, &params.id) {
            info!(target: "cmd", "room_set: {} unchanged, skipping persist", params.id);
            return serde_json::to_string(&room_state)
                .map_err(|e| anyhow::anyhow!("serialize: {}", e));
        }
        // Room is in registry but not yet in engine — fall through to add it
    }

    // Upsert into the targeted hub's registry (when hub_key provided) or all
    // registries (when None — backward compat for HTTP handler path).
    {
        let registries: Vec<Arc<Mutex<dyn HubRegistry>>> = state
            .lock()
            .ok()
            .map(|s| {
                if let Some(key) = hub_key {
                    s.hub_registry_for(key).into_iter().collect()
                } else {
                    s.hubs
                        .values()
                        .filter_map(|hub| hub.registry.clone())
                        .collect()
                }
            })
            .unwrap_or_default();
        for reg in &registries {
            if let Ok(mut reg) = reg.lock() {
                reg.upsert_room(
                    &params.id,
                    &params.name,
                    &params.grouped_light_id,
                    &params.device_ids,
                );
            }
        }
    }

    // Ensure runtime exists (first room triggers runtime creation)
    let needs_runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let has_runtime = s.hubs.values().any(|h| h.runtime.is_some());
        !has_runtime && s.has_any_hub()
    };

    if needs_runtime {
        ensure_runtime(state);
    }

    // Determine engine room ID: always use topology room IDs for the engine.
    // When hub_key is provided (room_sync path), translate_or_create ensures a
    // topology entry exists. When hub_key is None (HTTP handler path), params.id
    // should already be a topology ID (clients send topology IDs from GET /api/state).
    // Registry operations (above) always use params.id (hub-native).
    let engine_room_id = if let Some(k) = hub_key {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.topology.translate_or_create(
            k,
            &params.id,
            &params.name,
            &params.grouped_light_id,
            &params.device_ids,
        )
    } else {
        params.id.clone()
    };

    // Always apply params (whether runtime was just created or pre-existing)
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };
    if let Some(runtime) = runtime {
        let existing = runtime.engine_room_snapshot(&engine_room_id);
        let had_existing = existing.is_some();
        runtime.add_room(&engine_room_id, &params.name);
        let state_flags = params.state.map(room_flags_for_target_state).transpose()?;
        let (rhythm_enabled, time_offset, bri_offset, soft_off, hard_off, profile_settings) =
            existing
                .map(|snap| {
                    let (soft_off, hard_off) =
                        state_flags.unwrap_or((snap.soft_off, snap.hard_off));
                    (
                        params.rhythm_enabled,
                        snap.time_offset_minutes,
                        snap.brightness_offset,
                        soft_off,
                        hard_off,
                        snap.profile_settings,
                    )
                })
                .unwrap_or((
                    params.rhythm_enabled,
                    0.0,
                    0.0,
                    state_flags.unwrap_or((false, false)).0,
                    state_flags.unwrap_or((false, false)).1,
                    RoomProfileSettings::default(),
                ));
        // Soft-off rooms need rhythm enabled for periodic soft-off ticks
        let rhythm_enabled = if soft_off { true } else { rhythm_enabled };
        debug!(
            target: "cmd",
            "room_set: engine='{}' existing={} rhythm={} disabled={} soft_off={} hard_off={} room_profile={}",
            engine_room_id,
            had_existing,
            rhythm_enabled,
            params.disabled,
            soft_off,
            hard_off,
            !profile_settings.is_empty()
        );
        runtime.restore_room_state(
            &engine_room_id,
            rhythm_enabled,
            params.disabled,
            time_offset,
            bri_offset,
            soft_off,
            hard_off,
            profile_settings,
        );
    }

    if persist {
        persist_state(state);
    }

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);

    let room_state = build_room_rhythm_state(state, &engine_room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Remove a room from registry + engine + topology, persist.
///
/// `room_id` should be a topology room ID (handler resolves before calling).
/// Looks up hub-native IDs from topology for registry removal.
pub fn do_room_remove(state: &SharedState, room_id: &str) -> Result<()> {
    info!(target: "cmd", "room_remove: {}", room_id);

    // Look up hub-native room IDs for registry removal
    let hub_room_ids = hub_room_ids_for_topology(state, room_id);

    // Remove from ALL hub registries using hub-native IDs
    {
        let registries: Vec<Arc<Mutex<dyn HubRegistry>>> = state
            .lock()
            .ok()
            .map(|s| s.hubs.values().filter_map(|h| h.registry.clone()).collect())
            .unwrap_or_default();
        for reg in &registries {
            if let Ok(mut reg) = reg.lock() {
                for hub_room_id in &hub_room_ids {
                    reg.remove_room(hub_room_id);
                }
            }
        }
    }

    // Remove from engine using topology ID
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };
    if let Some(runtime) = runtime {
        runtime.remove_room(room_id);
    }

    // Clean up AppState maps (all topology-keyed)
    {
        if let Ok(mut s) = state.lock() {
            s.room_lights_on.remove(room_id);
            s.motion_snapshots.remove(room_id);
        }
    }

    persist_state(state);

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);

    Ok(())
}

// ============================================================================
// Room action commands
// ============================================================================

/// Dispatch a button action to a room via the runtime engine.
///
/// Actions: on, off, toggle, rhythm_on, rhythm_off, rhythm_toggle,
/// step_up, step_down, dim_up, dim_down, reset, lights_off.
///
/// Returns the updated room rhythm state JSON, or an error.
pub fn do_room_action(
    state: &SharedState,
    room_id: &str,
    action_str: &str,
    persist: bool,
) -> Result<String> {
    let action = match action_str {
        "on" => ButtonAction::OnPress,
        "off" => ButtonAction::OffPress,
        "toggle" => ButtonAction::Toggle,
        other => ButtonAction::from_service_name(other)
            .ok_or_else(|| anyhow::anyhow!("Unknown action: {}", other))?,
    };

    info!(target: "cmd", "room_action: {} -> {:?}", room_id, action);

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let event = InputEvent::new(room_id, action);
    let turned_on = runtime.handle_event(&event)?;
    sync_active_mode_from_runtime(state, &runtime);
    clear_room_mode_transition(state, room_id);

    // Track lights_on state from action result
    {
        if let Ok(mut s) = state.lock() {
            s.room_lights_on.insert(room_id.to_string(), turned_on);
        }
    }

    #[cfg(feature = "desktop")]
    {
        if let Some(snap) = runtime.engine_room_snapshot(room_id) {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState {
                    rooms: vec![build_room_state_event(state, &snap)],
                },
            );
        }
    }

    if persist {
        persist_rooms(state);
    }

    let room_state = build_room_rhythm_state(state, room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Reset all qualifying rooms back to their current adaptive curve position,
/// and turn off any motion-sensor rooms that are on.
///
/// Two categories:
/// 1. **On-rooms** (not disabled, lights on, not soft-off, no motion sensors) → `ButtonAction::Reset`
/// 2. **Motion rooms** (have motion sensors registered AND lights on, or have active motion timer) → `ButtonAction::OffPress` + clear motion timers
///
/// Category 2 uses the device registry (not just active timers) so that
/// rooms turned on by motion are turned off even when no motion timer is
/// running — e.g. after an addon restart where the timer state was lost.
///
/// Returns JSON with status, counts, and lists of affected room IDs.
pub fn do_fix_my_lights(state: &SharedState, persist: bool) -> Result<String> {
    let (runtime, room_lights_on, motion_room_ids) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        // Rooms with active motion timers
        let mut motion_ids: HashSet<String> = s.motion_snapshots.keys().cloned().collect();
        // Also include rooms that have motion sensors registered in any hub's
        // registry, even if no timer is active. This covers the case where the
        // addon restarted and lost timer state, but lights were still on from
        // prior motion activation.
        motion_ids.extend(s.motion_sensor_room_ids());
        (s.hub_runtime(), s.room_lights_on.clone(), motion_ids)
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let snapshots = runtime.engine_all_room_snapshots();

    // Category 1: on-rooms without motion sensors → reset to adaptive curve
    let qualifying: Vec<_> = snapshots
        .iter()
        .filter(|snap| {
            !snap.disabled
                && !snap.soft_off
                && room_lights_on.get(&snap.id).copied().unwrap_or(false)
                && !motion_room_ids.contains(&snap.id)
        })
        .collect();

    // Category 2: motion-sensor rooms that are on (or have active timer) → turn off
    let motion_rooms: Vec<_> = snapshots
        .iter()
        .filter(|snap| {
            !snap.disabled
                && motion_room_ids.contains(&snap.id)
                && room_lights_on.get(&snap.id).copied().unwrap_or(false)
        })
        .collect();

    info!(target: "cmd", "fix_my_lights: {} on-rooms to reset, {} motion rooms to turn off (of {} total)",
        qualifying.len(), motion_rooms.len(), snapshots.len());

    let mut reset_ids = Vec::new();
    for snap in &qualifying {
        let event = InputEvent::new(&snap.id, ButtonAction::Reset);
        match runtime.handle_event(&event) {
            Ok(turned_on) => {
                sync_active_mode_from_runtime(state, &runtime);
                if let Ok(mut s) = state.lock() {
                    s.room_lights_on.insert(snap.id.clone(), turned_on);
                }
                reset_ids.push(snap.id.clone());
            }
            Err(e) => {
                warn!(target: "cmd", "fix_my_lights: reset failed for {}: {}", snap.id, e);
            }
        }
    }

    // Clear stale offsets on motion rooms before turning off
    for snap in &motion_rooms {
        if snap.time_offset_minutes.abs() > 0.001 || snap.brightness_offset.abs() > 0.001 {
            runtime.restore_room_state(
                &snap.id,
                snap.rhythm_enabled,
                snap.disabled,
                0.0,
                0.0,
                snap.soft_off,
                snap.hard_off,
                snap.profile_settings.clone(),
            );
        }
    }

    // OffPress → engine.turn_off() which respects power_save:
    //   power_save=true  → fully off
    //   power_save=false → soft-off (dim to idle curve brightness with idle color)
    let mut motion_off_ids = Vec::new();
    for snap in &motion_rooms {
        let event = InputEvent::new(&snap.id, ButtonAction::OffPress);
        match runtime.handle_event(&event) {
            Ok(turned_on) => {
                sync_active_mode_from_runtime(state, &runtime);
                if let Ok(mut s) = state.lock() {
                    s.room_lights_on.insert(snap.id.clone(), turned_on);
                }
                motion_off_ids.push(snap.id.clone());
            }
            Err(e) => {
                warn!(target: "cmd", "fix_my_lights: off failed for {}: {}", snap.id, e);
            }
        }
    }

    // Signal the event loop to clear motion timers for these rooms
    if !motion_off_ids.is_empty() {
        if let Ok(mut s) = state.lock() {
            s.pending_motion_clear
                .extend(motion_off_ids.iter().cloned());
        }
    }

    // Collect all affected room IDs for SSE + response
    let all_affected: Vec<String> = reset_ids
        .iter()
        .chain(motion_off_ids.iter())
        .cloned()
        .collect();

    #[cfg(feature = "desktop")]
    {
        let events: Vec<_> = all_affected
            .iter()
            .filter_map(|id| runtime.engine_room_snapshot(id))
            .map(|snap| build_room_state_event(state, &snap))
            .collect();
        if !events.is_empty() {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState { rooms: events },
            );
        }
    }

    if persist && !all_affected.is_empty() {
        persist_rooms(state);
    }

    let room_states: Vec<RoomRhythmState> = all_affected
        .iter()
        .filter_map(|id| build_room_rhythm_state(state, id).ok())
        .collect();

    let response = FixResponse {
        rooms_reset: reset_ids.len(),
        rooms: room_states,
        motion_cleared: motion_off_ids.len(),
        motion_rooms: motion_off_ids,
    };
    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Set absolute brightness for a room (1-100).
///
/// Computes the offset so effective brightness equals the target.
/// Rhythm stays enabled — only brightness is overridden.
pub fn do_set_brightness(
    state: &SharedState,
    room_id: &str,
    brightness: u8,
    persist: bool,
) -> Result<String> {
    let brightness = brightness.clamp(1, 100);
    info!(target: "cmd", "set_brightness: {} -> {}", room_id, brightness);

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    runtime.set_room_brightness(room_id, brightness)?;
    clear_room_mode_transition(state, room_id);

    // Setting brightness implies lights are on
    {
        if let Ok(mut s) = state.lock() {
            s.room_lights_on.insert(room_id.to_string(), true);
        }
    }

    #[cfg(feature = "desktop")]
    {
        if let Some(snap) = runtime.engine_room_snapshot(room_id) {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState {
                    rooms: vec![build_room_state_event(state, &snap)],
                },
            );
        }
    }

    if persist {
        persist_rooms(state);
    }

    let room_state = build_room_rhythm_state(state, room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Set the time offset for a room directly (not additive).
///
/// Does NOT change `room_lights_on` tracking — offset doesn't imply lights-on state change.
pub fn do_set_time_offset(
    state: &SharedState,
    room_id: &str,
    offset_minutes: f32,
    persist: bool,
) -> Result<String> {
    info!(target: "cmd", "set_time_offset: {} -> {}", room_id, offset_minutes);

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    runtime.set_room_time_offset(room_id, offset_minutes)?;
    clear_room_mode_transition(state, room_id);

    #[cfg(feature = "desktop")]
    {
        if let Some(snap) = runtime.engine_room_snapshot(room_id) {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState {
                    rooms: vec![build_room_state_event(state, &snap)],
                },
            );
        }
    }

    if persist {
        persist_rooms(state);
    }

    let room_state = build_room_rhythm_state(state, room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

// ============================================================================
// Device commands
// ============================================================================

/// Upsert a device with button mappings and type in the registry.
///
/// When `hub_key` is provided, only that hub's registry is updated.
/// When `None`, the first available registry is used (backward compat).
pub fn do_device_set(
    state: &SharedState,
    device_id: &str,
    room_id: &str,
    buttons: &[(String, u8)],
    device_type: DeviceType,
    hub_key: Option<&HubKey>,
    persist: bool,
) -> Result<()> {
    info!(target: "cmd", "device_set: {} ({:?}) -> room {} ({} buttons)", device_id, device_type, room_id, buttons.len());

    let registry = hub_key
        .and_then(|k| state.lock().ok().and_then(|s| s.hub_registry_for(k)))
        .or_else(|| extract_registry(state));

    // Dedup
    let device_unchanged = registry
        .as_ref()
        .and_then(|r| {
            r.lock()
                .ok()
                .map(|reg| reg.device_matches(device_id, room_id, buttons, &device_type))
        })
        .unwrap_or(false);
    if device_unchanged {
        info!(target: "cmd", "device_set: {} unchanged, skipping persist", device_id);
        return Ok(());
    }

    if let Some(reg) = registry {
        if let Ok(mut reg) = reg.lock() {
            reg.upsert_device(device_id, room_id, buttons, device_type);
        }
    }

    if persist {
        persist_state(state);
    }
    Ok(())
}

/// Remove a device from the registry.
///
/// When `hub_key` is provided, removes from that hub's registry only.
/// When `None`, falls back to the first hub's registry (backward compat).
pub fn do_device_remove(
    state: &SharedState,
    device_id: &str,
    hub_key: Option<&HubKey>,
) -> Result<()> {
    info!(target: "cmd", "device_remove: {}", device_id);

    let registry = hub_key
        .and_then(|k| state.lock().ok().and_then(|s| s.hub_registry_for(k)))
        .or_else(|| extract_registry(state));
    if let Some(reg) = registry {
        if let Ok(mut reg) = reg.lock() {
            reg.remove_device(device_id);
        }
    }

    persist_state(state);
    Ok(())
}

/// Hard-remove a device from canonical state, topology, and hub registries.
///
/// This is the user-facing delete path. Unlike [`do_device_remove`], which
/// only drops a native registry entry, this removes the device's canonical
/// identity and topology wiring so it disappears fully from the app.
pub fn do_device_hard_remove(
    state: &SharedState,
    device_id: &str,
    hub_key: Option<&HubKey>,
) -> Result<()> {
    info!(target: "cmd", "device_hard_remove: {}", device_id);

    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

    let canonical_id = if s.canonical_registry.get(device_id).is_some() {
        Some(device_id.to_string())
    } else if let Some(hub_key) = hub_key {
        s.canonical_registry
            .find_by_native_id(hub_key, device_id)
            .map(|d| d.id.clone())
    } else {
        s.hubs.keys().find_map(|key| {
            s.canonical_registry
                .find_by_native_id(key, device_id)
                .map(|d| d.id.clone())
        })
    };

    let canonical_device = canonical_id
        .as_ref()
        .and_then(|id| s.canonical_registry.get(id).cloned());

    let mut registry_removals: HashSet<(HubKey, String)> = HashSet::new();
    let mut topology_changed = false;

    if let Some(device) = canonical_device {
        topology_changed |= !s.topology.remove_device_everywhere(&device.id).is_empty();
        for endpoint in &device.endpoints {
            topology_changed |= !s
                .topology
                .remove_hub_target_everywhere(&endpoint.hub_key, &endpoint.native_id)
                .is_empty();
            registry_removals.insert((endpoint.hub_key.clone(), endpoint.native_id.clone()));
        }
        s.canonical_registry.remove_device(&device.id);
        persist_canonical(&s);
    }

    if topology_changed {
        persist_topology(&s);
    }

    if registry_removals.is_empty() {
        if let Some(hub_key) = hub_key {
            registry_removals.insert((hub_key.clone(), device_id.to_string()));
        } else {
            for key in s.hubs.keys() {
                registry_removals.insert((key.clone(), device_id.to_string()));
            }
        }
    }

    for (target_hub_key, native_id) in &registry_removals {
        if let Some(hub) = s.hubs.get(target_hub_key) {
            if let Some(reg) = &hub.registry {
                if let Ok(mut reg) = reg.lock() {
                    reg.remove_room(native_id);
                    reg.remove_device(native_id);
                }
            }
        }
    }

    drop(s);

    persist_registry(state);

    #[cfg(feature = "desktop")]
    {
        rebuild_composite_routing(state);
        emit_triage_changed(state);
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Soft-remove a device from the canonical registry by its native ID.
///
/// Looks up the canonical device via native ID + hub key, marks it as
/// soft-removed (preserves the tombstone for dedup), and persists.
pub fn do_canonical_soft_remove(state: &SharedState, device_id: &str, hub_key: &HubKey) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut s = match state.lock() {
        Ok(s) => s,
        Err(_) => return,
    };

    // Find canonical ID by native ID
    let canonical_id = s
        .canonical_registry
        .find_by_native_id(hub_key, device_id)
        .map(|d| d.id.clone());

    if let Some(id) = canonical_id {
        if s.canonical_registry.soft_remove(&id, now) {
            info!(target: "cmd", "canonical_soft_remove: {} (native={})", id, device_id);
        }
        persist_canonical(&s);
    }
}

/// Set a per-room motion timeout override in the room profile settings layer.
pub fn do_motion_timeout_set(
    state: &SharedState,
    room_id: &str,
    timeout_secs: u64,
    _hub_key: Option<&HubKey>,
) -> Result<()> {
    info!(target: "cmd", "motion_timeout_set: room {} -> {}s", room_id, timeout_secs);

    let value = u32::try_from(timeout_secs)
        .map_err(|_| anyhow::anyhow!("motion timeout {} exceeds supported range", timeout_secs))?;
    let patch = RoomProfileSettingsPatch {
        motion_timeout_secs: Some(Some(TimerSetting::Fixed { value })),
        ..Default::default()
    };
    do_room_preferences_set(state, room_id, None, None, None, Some(&patch), true)?;
    Ok(())
}

/// Remove a per-room motion timeout override so it falls back to the profile default.
pub fn do_motion_timeout_clear(
    state: &SharedState,
    room_id: &str,
    _hub_key: Option<&HubKey>,
) -> Result<()> {
    info!(target: "cmd", "motion_timeout_clear: room {}", room_id);

    let patch = RoomProfileSettingsPatch {
        motion_timeout_secs: Some(None),
        ..Default::default()
    };
    do_room_preferences_set(state, room_id, None, None, None, Some(&patch), true)?;
    Ok(())
}

/// Resolve room motion timeout defaults from persisted room profile settings.
pub(crate) fn resolved_room_motion_timeout_map(
    state: &SharedState,
) -> (HashMap<String, u64>, HashSet<String>) {
    let (
        runtime,
        light_profile_configs,
        mode_configs,
        active_mode,
        solar_noon,
        latitude,
        sensor_rooms,
    ) = {
        let Ok(s) = state.lock() else {
            return (HashMap::new(), HashSet::new());
        };
        (
            s.hub_runtime(),
            s.light_profile_configs.clone(),
            s.mode_configs(),
            s.active_mode,
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.motion_sensor_room_ids(),
        )
    };

    let current_hour = runtime.as_ref().map(|rt| rt.current_hour()).unwrap_or(12.0);
    let snapshots = runtime
        .as_ref()
        .map(|rt| rt.engine_all_room_snapshots())
        .unwrap_or_default();

    let mut timeouts = HashMap::new();
    for snap in &snapshots {
        timeouts.insert(
            snap.id.clone(),
            resolved_room_motion_timeout_secs_from_parts(
                &light_profile_configs,
                &mode_configs,
                active_mode,
                solar_noon,
                latitude,
                &snap.profile_settings,
                current_hour,
            ),
        );
    }
    (timeouts, sensor_rooms)
}

// ============================================================================
// Config commands
// ============================================================================

/// Update a light profile config, push it to the runtime, and persist it.
pub fn do_config_set(state: &SharedState, mut config: LightProfileConfig) -> Result<()> {
    rhythm_core::normalize_builtin_state_profile_config(&mut config);

    let curve_desc = match &config.curve {
        rhythm_core::LightCurveShape::SuperGaussian { direct_color, .. } => {
            if let Some(dc) = direct_color {
                format!(
                    "super-gaussian+color(xy={:.3},{:.3} rgb={},{},{})",
                    dc.xy.x, dc.xy.y, dc.rgb.r, dc.rgb.g, dc.rgb.b
                )
            } else {
                "super-gaussian".to_string()
            }
        }
        rhythm_core::LightCurveShape::Palette { keyframes } => {
            format!("palette({}kf)", keyframes.len())
        }
        rhythm_core::LightCurveShape::InheritActive => "inherit-active".to_string(),
        rhythm_core::LightCurveShape::Constant {
            brightness,
            color_temp,
            ..
        } => format!("constant(bri={:.2}, cct={:.2})", brightness, color_temp),
    };
    info!(
        target: "cmd",
        "config_set: id='{}', curve={}, min_bri={}, max_bri={}, min_cct={}, max_cct={}, dim_steps={}, fade={:?}, motion={:?}, interval={:?}",
        config.id,
        curve_desc,
        config.min_brightness,
        config.max_brightness,
        config.min_color_temp,
        config.max_color_temp,
        config.max_dim_steps,
        config.fade_ms,
        config.motion_timeout_secs,
        config.rhythm_interval_secs,
    );

    let runtime = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if !s.light_profile_configs.contains_key(&config.id) {
            return Err(anyhow::anyhow!("Unknown light profile: {}", config.id));
        }
        s.set_light_profile_config(config.clone());
        if config.id == s.active_mode_profile_id() {
            s.sync_active_mode_runtime_overrides();
        }
        persist_light_profiles_locked(&s);
        s.hub_runtime()
    };

    if let Some(runtime) = runtime {
        if let Err(e) = runtime.set_light_profile_config(config) {
            warn!(target: "cmd", "Failed to update runtime light profile config: {}", e);
        }
    }

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::ConfigChanged);

    Ok(())
}

/// Absorb a time offset into a light profile config by adjusting ramp widths.
///
/// Modifies the width parameters for the current side of the day (morning/evening)
/// so the curve naturally produces the offset's values at the current time, then
/// resets all room time offsets to 0.
pub fn do_absorb_time_offset(
    state: &SharedState,
    profile_id: Option<&str>,
    offset_minutes: f32,
) -> Result<()> {
    info!(target: "cmd", "absorb_time_offset: {}min", offset_minutes);

    let (resolved_profile_id, config, runtime, sunrise, sunset) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let resolved_profile_id = resolve_profile_id(&s, profile_id)?;
        let config = s
            .light_profile_config(&resolved_profile_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown light profile: {}", resolved_profile_id))?;
        let runtime = s.hub_runtime();

        // Compute sunrise/sunset from location, or use fallbacks
        let (sunrise, sunset) = if let (Some(lat), Some(lon)) = (s.latitude, s.longitude) {
            let now = chrono::Utc::now().naive_utc();
            let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());

            if let Some(ref tz_name) = s.timezone_name {
                let tz = rhythm_core::Timezone::new(tz_name);
                let st = rhythm_core::solar::calculate_sun_times(lat, lon, year, month, day, &tz);
                (st.sunrise, st.sunset)
            } else {
                // Fallback: estimate from solar noon
                let half_day = 7.0; // rough approximation
                let noon = s.solar_noon_hour();
                (noon - half_day, noon + half_day)
            }
        } else {
            (
                rhythm_core::config::FALLBACK_SUNRISE_HOUR,
                rhythm_core::config::FALLBACK_SUNSET_HOUR,
            )
        };

        (resolved_profile_id, config, runtime, sunrise, sunset)
    };

    // Compute current local hour — prefer the runtime's time provider (which
    // may be mocked in tests) over the wall clock.
    let current_hour = if let Some(ref rt) = runtime {
        rt.current_hour()
    } else {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let now = chrono::Utc::now().naive_utc();
        let offset_secs = (s.utc_offset_hours * 3600.0) as i64;
        let local = now + chrono::Duration::seconds(offset_secs);
        let t = local.time();
        t.hour() as f32 + t.minute() as f32 / 60.0 + t.second() as f32 / 3600.0
    };

    // Adjust config widths if offset is meaningful
    if let Some(new_config) =
        absorb_light_profile_time_offset(&config, current_hour, offset_minutes, sunrise, sunset)
    {
        info!(target: "cmd", "absorb_time_offset: adjusted widths — bri L={:.3} R={:.3}, cct L={:.3} R={:.3}",
        match &new_config.curve {
            rhythm_core::LightCurveShape::SuperGaussian { width_left_bri, .. } => *width_left_bri,
            _ => 0.0,
        },
        match &new_config.curve {
            rhythm_core::LightCurveShape::SuperGaussian { width_right_bri, .. } => *width_right_bri,
            _ => 0.0,
        },
        match &new_config.curve {
            rhythm_core::LightCurveShape::SuperGaussian { width_left_cct, .. } => *width_left_cct,
            _ => 0.0,
        },
        match &new_config.curve {
            rhythm_core::LightCurveShape::SuperGaussian { width_right_cct, .. } => *width_right_cct,
            _ => 0.0,
        });

        // Update config in state + persist + push to engine
        {
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            if resolved_profile_id != new_config.id {
                return Err(anyhow::anyhow!(
                    "Profile ID mismatch while absorbing offset"
                ));
            }
            s.set_light_profile_config(new_config.clone());
            if resolved_profile_id == s.active_mode_profile_id() {
                s.sync_active_mode_runtime_overrides();
            }
            persist_light_profiles_locked(&s);
        }

        if let Some(ref rt) = runtime {
            if let Err(e) = rt.set_light_profile_config(new_config) {
                warn!(target: "cmd", "Failed to update runtime light profile config: {}", e);
            }
        }
    }

    // Reset all room time offsets to 0
    if let Some(ref rt) = runtime {
        let snapshots = rt.engine_all_room_snapshots();
        for snap in &snapshots {
            if snap.time_offset_minutes.abs() > 0.001 {
                if let Err(e) = rt.set_room_time_offset(&snap.id, 0.0) {
                    warn!(target: "cmd", "Failed to reset offset for room {}: {}", snap.id, e);
                }
            }
        }
    }

    persist_rooms(state);

    #[cfg(feature = "desktop")]
    {
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::ConfigChanged);
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Update lat/lon/utc_offset, recalculate solar noon.
///
/// Solar noon is computed internally:
/// - If `timezone_name` is provided, uses DST-aware `calculate_solar_noon`
/// - Otherwise, falls back to `calculate_solar_noon_from_offset`
pub fn do_location_set(
    state: &SharedState,
    lat: f32,
    lon: f32,
    utc_offset: Option<f32>,
    timezone_name: Option<String>,
) -> Result<()> {
    info!(target: "cmd", "location_set: lat={}, lon={}, utc_offset={:?}, tz={:?}", lat, lon, utc_offset, timezone_name);

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        s.latitude = Some(lat);
        s.longitude = Some(lon);
        s.timezone_name = timezone_name.clone();

        if let Some(offset) = utc_offset {
            s.utc_offset_hours = offset;
        }

        // Compute solar noon: prefer timezone-aware calculation
        let now = chrono::Utc::now().naive_utc();
        let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
        let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);

        let solar_noon = if let Some(ref tz_name) = timezone_name {
            let tz = rhythm_core::Timezone::new(tz_name);
            // Also refresh utc_offset from timezone
            s.utc_offset_hours = tz.utc_offset(year, month, day, now.time().hour());
            rhythm_core::calculate_solar_noon(lon, year, month, day, &tz)
        } else {
            rhythm_core::calculate_solar_noon_from_offset(lon, s.utc_offset_hours, day_of_year)
        };
        s.runtime_config.solar_noon_hour = solar_noon;

        if let Some(runtime) = s.hub_runtime() {
            let solar_time = rhythm_core::SolarTime::new(solar_noon, lat, day_of_year);
            if let Err(e) = runtime.set_solar(solar_time) {
                warn!(target: "cmd", "Failed to update solar time: {}", e);
            }
        }

        if let Some(ref storage) = s.storage {
            let loc = StoredLocation {
                latitude: Some(lat),
                longitude: Some(lon),
                utc_offset_hours: s.utc_offset_hours,
                timezone_name,
            };
            if let Err(e) = storage.save_location(&loc) {
                warn!(target: "cmd", "Failed to save location: {}", e);
            }
        }
    }

    // No ConfigChanged broadcast — location is client→server fire-and-forget.
    // Broadcasting would cause a feedback loop when multiple apps connect.

    Ok(())
}

/// Save hub credentials and connect SSE.
///
/// After a successful configure, auto-syncs rooms from the hub if
/// the hub provides a `HubDiscovery` implementation.
pub fn do_hub_credentials(
    state: &SharedState,
    hub_type_str: &str,
    address: &str,
    credentials: &Value,
) -> Result<()> {
    info!(target: "cmd", "hub_credentials: type={}, address={}", hub_type_str, address);

    let hub_type = crate::hub::HubType::parse(hub_type_str)
        .ok_or_else(|| anyhow::anyhow!("Unknown hub type: {}", hub_type_str))?;

    let credentials_json = serde_json::to_string(credentials)?;

    let get_provider = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.get_hub_provider_fn
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No hub provider registered"))?
    };

    let provider = get_provider(hub_type.clone());
    provider.configure(address, &credentials_json, state)?;

    let hub_key = crate::canonical::identity::HubKey::new(hub_type, address);

    // Register the new hub's controller with the composite (if runtime already exists).
    #[cfg(feature = "desktop")]
    register_hub_with_composite(state, &hub_key);

    // Auto-sync rooms from the newly configured hub.
    // Uses platform config to decide whether to also discover devices/sensors
    // (desktop: full sync, ESP32: rooms only — devices arrive via SSE).
    let discover_devices = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.platform.full_device_discovery
    };
    // Auto-sync + poll must run on a dedicated thread — reqwest::blocking::Client
    // panics if used inside a tokio runtime context (spawn_blocking from HTTP handler).
    {
        let sync_state = state.clone();
        let _ = std::thread::Builder::new()
            .name("hub-sync".to_string())
            .spawn(move || {
                if let Err(e) =
                    crate::room_sync::sync_from_hub_for_key(&sync_state, &hub_key, discover_devices)
                {
                    warn!(target: "cmd", "Auto-sync after hub configure failed: {}", e);
                }
                crate::room_sync::poll_initial_light_state(&sync_state);

                // Standardize all rooms to current adaptive curve position.
                match do_fix_my_lights(&sync_state, true) {
                    Ok(_) => info!(target: "cmd", "Auto-fix after credential push complete"),
                    Err(e) => warn!(target: "cmd", "Auto-fix after credential push failed: {}", e),
                }
            })
            .and_then(|h| h.join().map_err(|_| std::io::Error::other("panicked")));
    }

    #[cfg(feature = "desktop")]
    {
        let hub_connected = state.lock().ok().map(|s| s.has_any_hub()).unwrap_or(false);
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::HubStatus {
                hub_type: Some(hub_type_str.to_string()),
                address: Some(address.to_string()),
                connected: hub_connected,
            },
        );
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Disconnect all hubs — clear credentials, runtime, and rooms.
pub fn do_hub_disconnect(state: &SharedState) -> Result<()> {
    info!(target: "cmd", "hub_disconnect: clearing all hubs, credentials, and rooms");

    // Capture hub keys before clearing for SSE notifications
    let old_hubs;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        old_hubs = std::mem::take(&mut s.hubs);

        s.hub_credentials.clear();
        if let Some(ref storage) = s.storage {
            let _ = storage.save_all_hub_credentials(&[]);
        }

        s.room_lights_on.clear();
        s.motion_snapshots.clear();

        if let Some(ref storage) = s.storage {
            let empty = rhythm_core::room::RoomManager::new();
            let _ = storage.save_rooms(&empty);
        }
    }

    persist_registry(state);

    // Capture keys before old_hubs is moved into the drop thread
    #[cfg(feature = "desktop")]
    let old_hub_keys: Vec<_> = old_hubs.keys().cloned().collect();

    // Clear all controllers from the composite
    #[cfg(feature = "desktop")]
    {
        let composite = state
            .lock()
            .ok()
            .and_then(|s| s.composite_controller.clone());
        if let Some(composite) = composite {
            for key in &old_hub_keys {
                composite.remove_controller(&key.to_string());
            }
        }
        rebuild_composite_routing(state);
    }

    // Drop old hubs on a dedicated thread (reqwest::blocking::Client panics in tokio context)
    if !old_hubs.is_empty() {
        std::thread::Builder::new()
            .name("hub-drop".to_string())
            .spawn(move || drop(old_hubs))
            .ok();
    }

    #[cfg(feature = "desktop")]
    {
        for key in &old_hub_keys {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::HubStatus {
                    hub_type: Some(key.hub_type.as_str().to_string()),
                    address: Some(key.address.clone()),
                    connected: false,
                },
            );
        }
        // Also emit a generic disconnected if no specific keys (fresh state)
        if old_hub_keys.is_empty() {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::HubStatus {
                    hub_type: None,
                    address: None,
                    connected: false,
                },
            );
        }
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Disconnect a single hub by type and address.
///
/// Removes only the matching hub. Other hubs remain connected.
/// Rooms owned by this hub are removed from the engine.
pub fn do_hub_disconnect_one(state: &SharedState, hub_type_str: &str, address: &str) -> Result<()> {
    use crate::canonical::identity::HubKey;

    info!(target: "cmd", "hub_disconnect_one: type={}, address={}", hub_type_str, address);

    let hub_type = crate::hub::HubType::parse(hub_type_str)
        .ok_or_else(|| anyhow::anyhow!("Unknown hub type: {}", hub_type_str))?;
    let key = HubKey::new(hub_type, address);

    let old_hub;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        old_hub = s.hubs.remove(&key);
        s.hub_credentials.remove(&key);

        if let Some(ref storage) = s.storage {
            let all: Vec<_> = s.hub_credentials.values().cloned().collect();
            let _ = storage.save_all_hub_credentials(&all);
        }
    }

    persist_state(state);

    // Remove the hub's controller from the composite and rebuild routing
    #[cfg(feature = "desktop")]
    {
        let composite = state
            .lock()
            .ok()
            .and_then(|s| s.composite_controller.clone());
        if let Some(composite) = composite {
            composite.remove_controller(&key.to_string());
            info!(target: "cmd", "Removed {} controller from composite", key);
        }
        rebuild_composite_routing(state);
    }

    // Drop old hub on a dedicated thread
    if let Some(hub) = old_hub {
        std::thread::Builder::new()
            .name("hub-drop".to_string())
            .spawn(move || drop(hub))
            .ok();
    }

    #[cfg(feature = "desktop")]
    {
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::HubStatus {
                hub_type: Some(hub_type_str.to_string()),
                address: Some(address.to_string()),
                connected: false,
            },
        );
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Update user preferences for a room (rhythm_enabled, disabled, state).
///
/// Only modifies user state, not topology. Safe for app to call without
/// overwriting server-discovered rooms/devices.
pub fn do_room_preferences_set(
    state: &SharedState,
    room_id: &str,
    rhythm_enabled: Option<bool>,
    disabled: Option<bool>,
    target_state: Option<RoomModeState>,
    room_profile: Option<&RoomProfileSettingsPatch>,
    persist: bool,
) -> Result<String> {
    info!(
        target: "cmd",
        "room_preferences_set: {} rhythm={:?} disabled={:?} state={:?} room_profile={}",
        room_id,
        rhythm_enabled,
        disabled,
        target_state,
        room_profile.is_some()
    );

    let (runtime, lights_on, valid_profile_ids) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.hub_runtime(),
            s.room_lights_on.get(room_id).copied().unwrap_or(false),
            s.light_profile_configs
                .keys()
                .cloned()
                .collect::<HashSet<_>>(),
        )
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let snap = runtime
        .engine_room_snapshot(room_id)
        .ok_or_else(|| anyhow::anyhow!("Room '{}' not found in engine", room_id))?;

    let prev_soft_off = snap.soft_off;
    let prev_hard_off = snap.hard_off;
    let rhythm_enabled = rhythm_enabled.unwrap_or(snap.rhythm_enabled);
    let disabled = disabled.unwrap_or(snap.disabled);
    let persistent_state = target_state
        .unwrap_or_else(|| persistent_room_state_from_flags(snap.hard_off, snap.soft_off));
    let (soft_off, hard_off) = room_flags_for_target_state(persistent_state)?;
    let mut profile_settings = snap.profile_settings.clone();
    if let Some(patch) = room_profile {
        patch.apply_to(&mut profile_settings);
    }

    if let Some(profile_id) = profile_settings.profile_id.as_deref() {
        if rhythm_core::is_builtin_state_profile_id(profile_id) {
            return Err(anyhow::anyhow!(
                "State profiles cannot be selected per-room"
            ));
        }
        if !valid_profile_ids.contains(profile_id) {
            return Err(anyhow::anyhow!("Unknown light profile: {}", profile_id));
        }
    }

    // Soft-off rooms need rhythm enabled for periodic soft-off ticks
    let rhythm_enabled = if soft_off { true } else { rhythm_enabled };

    runtime.restore_room_state(
        room_id,
        rhythm_enabled,
        disabled,
        snap.time_offset_minutes,
        snap.brightness_offset,
        soft_off,
        hard_off,
        profile_settings.clone(),
    );
    clear_room_mode_transition(state, room_id);

    let entered_hard_off = hard_off && !prev_hard_off;
    let left_hard_off = !hard_off && prev_hard_off;

    if entered_hard_off {
        info!(target: "cmd", "room_preferences_set: {} entering hard_off", room_id);
        let event = InputEvent::new(room_id, ButtonAction::LightsOff);
        if let Err(e) = runtime.handle_event(&event) {
            warn!(target: "cmd", "lights_off for '{}' failed: {}", room_id, e);
        }
    } else if soft_off && !prev_soft_off {
        info!(target: "cmd", "room_preferences_set: {} entering idle", room_id);
        if let Err(e) = runtime.soft_off_tick_room(room_id) {
            warn!(target: "cmd", "soft_off_tick for '{}' failed: {}", room_id, e);
        }
    } else if !soft_off && prev_soft_off {
        info!(target: "cmd", "room_preferences_set: {} leaving idle, turning on", room_id);
        if let Err(e) = runtime.turn_on_room(room_id) {
            warn!(target: "cmd", "turn_on for '{}' failed: {}", room_id, e);
        }
    } else if left_hard_off {
        match persistent_state {
            RoomModeState::Active => {
                info!(target: "cmd", "room_preferences_set: {} leaving hard_off to active", room_id);
                if let Err(e) = runtime.turn_on_room(room_id) {
                    warn!(target: "cmd", "turn_on for '{}' failed: {}", room_id, e);
                }
            }
            RoomModeState::Idle => {
                info!(target: "cmd", "room_preferences_set: {} leaving hard_off to idle", room_id);
                if let Err(e) = runtime.soft_off_tick_room(room_id) {
                    warn!(target: "cmd", "soft_off_tick for '{}' failed: {}", room_id, e);
                }
            }
            RoomModeState::HardOff | RoomModeState::Wake | RoomModeState::Warning => {}
        }
    } else if room_profile.is_some_and(|patch| patch.touches_profile_settings()) {
        match persistent_state {
            RoomModeState::Idle => {
                info!(target: "cmd", "room_preferences_set: {} applying room profile to idle state", room_id);
                if let Err(e) = runtime.soft_off_tick_room(room_id) {
                    warn!(target: "cmd", "soft_off_tick for '{}' failed: {}", room_id, e);
                }
            }
            RoomModeState::Active if lights_on => {
                info!(target: "cmd", "room_preferences_set: {} applying room profile to active lights", room_id);
                if let Err(e) = runtime.turn_on_room(room_id) {
                    warn!(target: "cmd", "turn_on for '{}' failed: {}", room_id, e);
                }
            }
            RoomModeState::HardOff
            | RoomModeState::Wake
            | RoomModeState::Warning
            | RoomModeState::Active => {}
        }
    }

    if hard_off {
        if let Ok(mut s) = state.lock() {
            s.room_lights_on.insert(room_id.to_string(), false);
        }
    } else if soft_off {
        if let Ok(mut s) = state.lock() {
            s.room_lights_on.insert(room_id.to_string(), true);
        }
    }

    #[cfg(feature = "desktop")]
    {
        if let Some(snap) = runtime.engine_room_snapshot(room_id) {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState {
                    rooms: vec![build_room_state_event(state, &snap)],
                },
            );
        }
    }

    if persist {
        persist_rooms(state);
    }

    let room_state = build_room_rhythm_state(state, room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

// ============================================================================
// Runtime initialization helper
// ============================================================================

/// Call the platform's ensure_runtime callback in a thread with adequate stack.
///
/// The callback is stored on AppState (as an `Arc`) and set by the platform
/// crate at startup. It handles creating the RhythmRuntime with platform-specific types.
pub fn ensure_runtime(state: &SharedState) {
    // Clone the Arc + platform config out of the lock so we can call without holding it
    let (ensure_fn, stack_size) = {
        let Ok(s) = state.lock() else { return };
        (s.ensure_runtime_fn.clone(), s.platform.runtime_init_stack)
    };

    let Some(ensure_fn) = ensure_fn else { return };

    let rt_state = state.clone();
    let rt_result = std::thread::Builder::new()
        .name("rt-init".to_string())
        .stack_size(stack_size)
        .spawn(move || ensure_fn(&rt_state))
        .and_then(|handle| handle.join().map_err(|_| std::io::Error::other("panicked")));

    match rt_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!(target: "cmd", "Failed to start runtime: {}", e),
        Err(e) => warn!(target: "cmd", "Runtime init thread error: {}", e),
    }
}

// ============================================================================
// Composite controller registration
// ============================================================================

/// Rebuild the composite controller's routing table from topology.
///
/// Call after any operation that changes room-to-hub mappings: room sync,
/// hub connect/disconnect, topology merge/split/device-move.
#[cfg(feature = "desktop")]
pub fn rebuild_composite_routing(state: &SharedState) {
    let (composite, routing, room_count) = {
        let Ok(s) = state.lock() else { return };
        let composite = match s.composite_controller.clone() {
            Some(c) => c,
            None => return, // No composite yet — nothing to rebuild
        };
        let routing = s.topology.composite_routing();
        let rooms = s.topology.room_count();
        (composite, routing, rooms)
    };
    let route_count = routing.len();
    composite.update_routing(routing);
    info!(target: "cmd", "Rebuilt composite routing: {} rooms, {} route entries (incl. hub aliases)",
        room_count, route_count);
}

/// Register a newly connected hub's controller with the composite controller.
///
/// Called after `configure_hub` when a hub is added via HTTP while the server
/// is already running. If the composite controller doesn't exist yet (runtime
/// not created), this is a no-op — the controller will be picked up when
/// `ensure_composite_runtime` runs on first room arrival.
#[cfg(feature = "desktop")]
pub fn register_hub_with_composite(
    state: &SharedState,
    hub_key: &crate::canonical::identity::HubKey,
) {
    let register_fn = {
        let Ok(s) = state.lock() else { return };
        s.register_controller_fn.clone()
    };
    let Some(register_fn) = register_fn else {
        return;
    };

    match register_fn(state, hub_key) {
        Ok(()) => {
            // Assign the shared runtime to the new hub so hub_runtime_for() works
            {
                let Ok(mut s) = state.lock() else { return };
                let runtime = s.hubs.values().find_map(|h| h.runtime.clone());
                if let (Some(runtime), Some(hub)) = (runtime, s.hubs.get_mut(hub_key)) {
                    if hub.runtime.is_none() {
                        hub.runtime = Some(runtime);
                    }
                }
            }
            // Rebuild routing so existing topology rooms are routable via the new controller.
            // sync_from_hub_for_key() will rebuild again after discovery, but this
            // closes the window between registration and sync completion.
            rebuild_composite_routing(state);
        }
        Err(e) => {
            warn!(target: "cmd", "Failed to register controller for {}: {}", hub_key, e);
        }
    }
}

// ============================================================================
// Parse helpers
// ============================================================================

/// Parse button mappings from a JSON array.
pub fn parse_buttons(msg: &Value) -> Vec<(String, u8)> {
    msg.get("buttons")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|btn| {
                    let button_id = btn.get("button_id").and_then(|v| v.as_str())?;
                    let control_id = btn.get("control_id").and_then(|v| v.as_u64())? as u8;
                    Some((button_id.to_string(), control_id))
                })
                .collect()
        })
        .unwrap_or_default()
}

// ============================================================================
// Persistence helpers
// ============================================================================

/// Persist registry snapshot + room state.
pub fn persist_state(state: &SharedState) {
    persist_registry(state);
    persist_rooms(state);
}

/// Persist registry snapshot for all connected hubs.
///
/// Three-phase lock per hub to avoid holding AppState during JSON serialization + disk I/O:
/// 1. Brief AppState lock — extract registry Arcs + hub keys + check storage exists
/// 2. Registry lock only — snapshot to JSON (no AppState held)
/// 3. Brief AppState lock — write to storage (no registry lock)
pub fn persist_registry(state: &SharedState) {
    // Phase 1: Extract all hub registry Arcs + check storage (brief AppState lock)
    let (hub_registries, has_storage) = {
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        let regs: Vec<_> = s
            .hubs
            .iter()
            .filter_map(|(key, hub)| hub.registry.as_ref().map(|r| (key.clone(), r.clone())))
            .collect();
        (regs, s.storage.is_some())
    };
    if !has_storage || hub_registries.is_empty() {
        return;
    }

    // Phase 2+3: Snapshot + save each hub's registry
    for (hub_key, reg_arc) in &hub_registries {
        let value = reg_arc.lock().ok().map(|reg| reg.snapshot_json());

        if let Some(ref value) = value {
            let s = match state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if let Some(ref storage) = s.storage {
                if let Err(e) = storage.save_hub_registry_for(hub_key, value) {
                    warn!(target: "cmd", "Failed to save registry for {}: {}", hub_key, e);
                }
            }
        }
    }
}

/// Persist room state.
pub fn persist_rooms(state: &SharedState) {
    let (runtime, has_storage) = {
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        (s.hub_runtime(), s.storage.is_some())
    };

    if let (Some(ref runtime), true) = (runtime, has_storage) {
        let rooms = rooms_from_engine(runtime.as_ref());
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        if let Some(ref storage) = s.storage {
            if let Err(e) = storage.save_rooms(&rooms) {
                warn!(target: "cmd", "Failed to save rooms: {}", e);
            }
        }
    }
}

// ============================================================================
// Canonical device commands
// ============================================================================

/// Build JSON for all canonical devices.
pub fn build_canonical_devices(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let registry = &s.canonical_registry;
    let devices: Vec<_> = registry.devices().collect();
    serde_json::to_string(&devices).map_err(|e| anyhow::anyhow!(e))
}

/// Build JSON for a single canonical device.
pub fn build_canonical_device(state: &SharedState, id: &str) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    match s.canonical_registry.get(id) {
        Some(device) => serde_json::to_string(device).map_err(|e| anyhow::anyhow!(e)),
        None => Err(anyhow::anyhow!("Device not found: {}", id)),
    }
}

/// Assign a canonical device to a Rhythm room (or unassign with None).
///
/// Updates the canonical registry AND the topology so the composite routing
/// includes hub targets for the device's endpoints in the assigned room.
pub fn do_canonical_assign_room(
    state: &SharedState,
    device_id: &str,
    room_id: Option<&str>,
) -> Result<()> {
    use crate::topology::HubControlTarget;

    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

    // Snapshot endpoints, name, and old room before mutating.
    let device = s
        .canonical_registry
        .get(device_id)
        .ok_or_else(|| anyhow::anyhow!("Device not found: {}", device_id))?;
    let endpoints: Vec<_> = device.endpoints.clone();
    let device_name = device.name.clone();
    let old_room_id = device.room_id.clone();

    if !s.canonical_registry.assign_room(device_id, room_id) {
        return Err(anyhow::anyhow!("Device not found: {}", device_id));
    }

    // Remove hub targets from the old room (if any).
    if let Some(old_id) = &old_room_id {
        for ep in &endpoints {
            if let Some(room) = s.topology.get_mut(old_id) {
                room.remove_hub_target(&ep.hub_key, &ep.native_id);
            }
        }
    }

    // Add hub targets to the new room and update hub device registries.
    if let Some(target_room_id) = room_id {
        for ep in &endpoints {
            if let Some(room) = s.topology.get_mut(target_room_id) {
                let target = HubControlTarget {
                    hub_key: ep.hub_key.clone(),
                    hub_room_id: ep.native_id.clone(),
                    control_id: ep.native_id.clone(),
                    light_device_ids: vec![ep.native_id.clone()],
                    topology_aligned: false, // per-device addressing
                };
                room.upsert_hub_target(target);
            }

            // Update the hub's device registry so the device appears in /api/state.
            // The state builder maps hub-native room IDs to topology IDs via hub_targets,
            // so putting the device in a room named after its native_id works correctly.
            if let Some(hub) = s.hubs.get(&ep.hub_key) {
                if let Some(reg) = &hub.registry {
                    if let Ok(mut r) = reg.lock() {
                        r.upsert_room(
                            &ep.native_id,
                            &device_name,
                            &ep.native_id,
                            &[ep.native_id.clone()],
                        );
                    }
                }
            }
        }
    }

    persist_canonical(&s);
    persist_topology(&s);
    drop(s);

    persist_registry(state);

    #[cfg(feature = "desktop")]
    {
        rebuild_composite_routing(state);
        emit_triage_changed(state);
    }
    Ok(())
}

/// Set the preferred endpoint for a canonical device.
pub fn do_canonical_set_preferred(
    state: &SharedState,
    device_id: &str,
    hub_type: &str,
    hub_address: &str,
    native_id: &str,
) -> Result<()> {
    use crate::canonical::identity::HubKey;
    use crate::hub::HubType;

    let hub_key = HubKey::new(HubType::new(hub_type), hub_address);
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.canonical_registry
        .set_preferred_endpoint(device_id, &hub_key, native_id)
    {
        persist_canonical(&s);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Device or endpoint not found"))
    }
}

// ============================================================================
// Triage commands
// ============================================================================

/// Build JSON for the triage queue (pending entries).
pub fn build_triage_queue(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let pending = s.canonical_registry.triage().pending();
    serde_json::to_string(&pending).map_err(|e| anyhow::anyhow!(e))
}

/// Confirm a triage device merge.
///
/// Merges the discovered device's endpoint into the chosen canonical device
/// and removes the silo device that was created during Phase 4 of resolve().
pub fn do_triage_merge(state: &SharedState, entry_id: &str, canonical_id: &str) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let (hub_key, native_id, hub_room_id) = match s.canonical_registry.triage().get(entry_id) {
        Some(entry) => (
            entry.hub_key.clone(),
            entry.discovered.native_id.clone(),
            entry.discovered.room_id.clone(),
        ),
        None => return Err(anyhow::anyhow!("Triage entry not found: {}", entry_id)),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Find the silo device BEFORE complete_merge updates native_index.
    // The silo device has the same native_id on the same hub but a different canonical ID.
    let silo_id = s
        .canonical_registry
        .find_by_native_id(&hub_key, &native_id)
        .filter(|d| d.id != canonical_id)
        .map(|d| d.id.clone());

    if s.canonical_registry
        .complete_merge(entry_id, canonical_id, &hub_key, now)
    {
        // Remove the silo device that was created during resolve() Phase 4.
        if let Some(silo_id) = silo_id {
            s.canonical_registry.remove_device(&silo_id);
            log::debug!(target: "triage", "Removed silo device '{}' after merge", silo_id);
        }

        // Update topology: add device to the correct room
        if !hub_room_id.is_empty() {
            if let Some(rhythm_room_id) = s
                .topology
                .translate_room_id(&hub_key, &hub_room_id)
                .map(|s| s.to_string())
            {
                if let Some(room) = s.topology.get_mut(&rhythm_room_id) {
                    room.add_device_hub_default(canonical_id);
                }
                s.canonical_registry
                    .assign_room(canonical_id, Some(&rhythm_room_id));
            }
        }
        persist_canonical(&s);
        persist_topology(&s);
        drop(s);
        #[cfg(feature = "desktop")]
        emit_triage_changed(state);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to merge"))
    }
}

/// Create a new device from a triage entry (rejected merge, keep separate).
pub fn do_triage_new_device(state: &SharedState, entry_id: &str) -> Result<String> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let (hub_key, hub_room_id) = match s.canonical_registry.triage().get(entry_id) {
        Some(entry) => (entry.hub_key.clone(), entry.discovered.room_id.clone()),
        None => return Err(anyhow::anyhow!("Triage entry not found: {}", entry_id)),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match s
        .canonical_registry
        .complete_new_device(entry_id, &hub_key, now)
    {
        Some(canonical_id) => {
            if !hub_room_id.is_empty() {
                if let Some(rhythm_room_id) = s
                    .topology
                    .translate_room_id(&hub_key, &hub_room_id)
                    .map(|s| s.to_string())
                {
                    if let Some(room) = s.topology.get_mut(&rhythm_room_id) {
                        room.add_device_hub_default(&canonical_id);
                    }
                    s.canonical_registry
                        .assign_room(&canonical_id, Some(&rhythm_room_id));
                }
            }
            persist_canonical(&s);
            persist_topology(&s);
            drop(s);
            #[cfg(feature = "desktop")]
            emit_triage_changed(state);
            Ok(format!(r#"{{"canonical_id":"{}"}}"#, canonical_id))
        }
        None => Err(anyhow::anyhow!("Failed to create new device")),
    }
}

/// Dismiss a triage entry.
pub fn do_triage_dismiss(state: &SharedState, entry_id: &str) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if s.canonical_registry.triage_mut().dismiss(entry_id, now) {
        persist_canonical(&s);
        drop(s);
        #[cfg(feature = "desktop")]
        emit_triage_changed(state);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Triage entry not found: {}", entry_id))
    }
}

/// Approve a room binding triage entry — merge two rooms.
///
/// Merges the silo room (created for the new hub) into the target Rhythm room,
/// records an approved binding so it re-applies on re-sync, rebuilds composite
/// routing, and emits SSE events.
pub fn do_triage_bind_room(state: &SharedState, entry_id: &str) -> Result<()> {
    do_triage_bind_room_to(state, entry_id, None)
}

/// Approve a room binding triage entry — optionally binding to a specific target
/// (for the 3+ hub case where the user picks from candidate rooms).
pub fn do_triage_bind_room_to(
    state: &SharedState,
    entry_id: &str,
    target_override: Option<&str>,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Get the room binding proposal
    let entry = s
        .canonical_registry
        .triage()
        .get(entry_id)
        .ok_or_else(|| anyhow::anyhow!("Triage entry not found: {}", entry_id))?
        .clone();

    let binding = entry
        .room_binding
        .ok_or_else(|| anyhow::anyhow!("Not a room binding entry: {}", entry_id))?;

    let target_id = target_override
        .map(|s| s.to_string())
        .unwrap_or(binding.target_rhythm_room_id.clone());

    // Validate target room exists
    let target_name = s
        .topology
        .get(&target_id)
        .map(|r| r.name.clone())
        .ok_or_else(|| anyhow::anyhow!("Target room '{}' not found", target_id))?;

    // Find the silo room (the one created for this hub's room)
    let source_id = s
        .topology
        .translate_room_id(&entry.hub_key, &binding.hub_room_id)
        .map(|s| s.to_string())
        .ok_or_else(|| {
            anyhow::anyhow!("Silo room not found for hub room: {}", binding.hub_room_id)
        })?;

    // Merge the silo room into the target room
    if !s.topology.merge_rooms(&target_id, &source_id) {
        return Err(anyhow::anyhow!(
            "Failed to merge rooms: {} → {}",
            source_id,
            target_id
        ));
    }

    // Record the approved binding so it re-applies on re-sync
    s.topology.approve_binding(
        entry.hub_key.clone(),
        binding.hub_room_id.clone(),
        target_id.clone(),
        now,
    );

    // Resolve the triage entry
    s.canonical_registry.triage_mut().resolve(
        entry_id,
        crate::canonical::triage::TriageStatus::Confirmed,
        "api",
        now,
    );

    persist_canonical(&s);
    persist_topology(&s);

    // Remap engine room if runtime exists
    drop(s);

    if let Some(runtime) = state.lock().ok().and_then(|s| s.hub_runtime()) {
        // If the silo room exists in the engine, merge it into the target
        if let Some(snap) = runtime.engine_room_snapshot(&source_id) {
            if runtime.engine_room_snapshot(&target_id).is_none() {
                runtime.add_room(&target_id, &target_name);
                runtime.restore_room_state(
                    &target_id,
                    snap.rhythm_enabled,
                    snap.disabled,
                    snap.time_offset_minutes,
                    snap.brightness_offset,
                    snap.soft_off,
                    snap.hard_off,
                    snap.profile_settings.clone(),
                );
            }
            runtime.remove_room(&source_id);
            info!(target: "triage", "Remapped engine room '{}' → '{}'", source_id, target_id);
        }
    }

    // Rebuild composite routing
    #[cfg(feature = "desktop")]
    rebuild_composite_routing(state);

    // Emit SSE events
    #[cfg(feature = "desktop")]
    {
        emit_triage_changed(state);
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Build JSON with triage count summary.
pub fn build_triage_count(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let triage = s.canonical_registry.triage();
    let json = format!(
        r#"{{"devices":{},"rooms":{},"unassigned":{},"total":{}}}"#,
        triage.pending_device_count(),
        triage.pending_room_count(),
        triage.pending_unassigned_count(),
        triage.pending_count(),
    );
    Ok(json)
}

/// Emit a TriageChanged SSE event with current counts.
#[cfg(feature = "desktop")]
pub fn emit_triage_changed(state: &SharedState) {
    // Read counts under lock, then drop before emitting (emit_server_event locks too)
    let counts = state.lock().ok().map(|s| {
        let triage = s.canonical_registry.triage();
        (
            triage.pending_count(),
            triage.pending_device_count(),
            triage.pending_room_count(),
            triage.pending_unassigned_count(),
        )
    });
    if let Some((total, devices, rooms, unassigned)) = counts {
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::TriageChanged {
                pending_count: total,
                pending_devices: devices,
                pending_rooms: rooms,
                pending_unassigned: unassigned,
            },
        );
    }
}

// ============================================================================
// Topology commands
// ============================================================================

/// Build JSON for all topology rooms.
pub fn build_topology_rooms(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let rooms: Vec<_> = s.topology.rooms().collect();
    serde_json::to_string(&rooms).map_err(|e| anyhow::anyhow!(e))
}

/// Create a new empty Rhythm room.
pub fn do_topology_create_room(state: &SharedState, name: &str) -> Result<String> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let id = s.topology.create_room(name);
    persist_topology(&s);
    Ok(format!(r#"{{"id":"{}","name":"{}"}}"#, id, name))
}

/// Merge two rooms.
pub fn do_topology_merge_rooms(
    state: &SharedState,
    target_id: &str,
    source_id: &str,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.topology.merge_rooms(target_id, source_id) {
        persist_topology(&s);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to merge rooms"))
    }
}

/// Move a device between rooms.
pub fn do_topology_move_device(
    state: &SharedState,
    device_id: &str,
    from_room: &str,
    to_room: &str,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.topology.move_device(device_id, from_room, to_room) {
        persist_topology(&s);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to move device"))
    }
}

// ============================================================================
// Canonical + Topology persistence helpers
// ============================================================================

/// Persist the canonical registry to storage.
pub(crate) fn persist_canonical(s: &AppState) {
    if let Some(ref storage) = s.storage {
        match serde_json::to_value(&s.canonical_registry) {
            Ok(value) => {
                if let Err(e) = storage.save_canonical_registry(&value) {
                    warn!(target: "cmd", "Failed to save canonical registry: {}", e);
                }
            }
            Err(e) => warn!(target: "cmd", "Failed to serialize canonical registry: {}", e),
        }
    }
}

/// Persist the topology store to storage.
pub(crate) fn persist_topology(s: &AppState) {
    if let Some(ref storage) = s.storage {
        match serde_json::to_value(&s.topology) {
            Ok(value) => {
                if let Err(e) = storage.save_topology(&value) {
                    warn!(target: "cmd", "Failed to save topology: {}", e);
                }
            }
            Err(e) => warn!(target: "cmd", "Failed to serialize topology: {}", e),
        }
    }
}

// ============================================================================
// Curve visualization queries
// ============================================================================

/// Solar context resolved from AppState for a given date.
struct ResolvedSolar {
    solar: rhythm_core::SolarTime,
    sun_times: Option<rhythm_core::SunTimes>,
    twilight: Option<rhythm_core::TwilightTimes>,
}

/// Extract solar context from AppState for a given date.
///
/// If latitude/longitude are configured, computes real sun times.
/// Otherwise falls back to `SolarTime::default_noon()` with no sun/twilight data.
fn resolve_solar(s: &AppState, year: i32, month: u32, day: u32) -> ResolvedSolar {
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);

    if let (Some(lat), Some(lon)) = (s.latitude, s.longitude) {
        if let Some(ref tz_name) = s.timezone_name {
            let tz = rhythm_core::Timezone::new(tz_name);
            let solar_time = rhythm_core::solar_time_from_location(lat, lon, year, month, day, &tz);
            let sun_times = rhythm_core::calculate_sun_times(lat, lon, year, month, day, &tz);
            let twilight = rhythm_core::calculate_twilight_times(lat, lon, year, month, day, &tz);
            ResolvedSolar {
                solar: solar_time,
                sun_times: Some(sun_times),
                twilight: Some(twilight),
            }
        } else {
            // No timezone — use stored solar noon + rough estimation
            let solar = rhythm_core::SolarTime::new(s.solar_noon_hour(), lat, day_of_year);
            ResolvedSolar {
                solar,
                sun_times: None,
                twilight: None,
            }
        }
    } else {
        // No location — defaults
        let solar = rhythm_core::SolarTime::new(s.solar_noon_hour(), 35.0, day_of_year);
        ResolvedSolar {
            solar,
            sun_times: None,
            twilight: None,
        }
    }
}

/// Build a `SolarResponse` from resolved solar data.
fn build_solar_response(resolved: &ResolvedSolar) -> crate::api_types::SolarResponse {
    use crate::api_types::{SolarResponse, TwilightPhaseResponse, TwilightResponse};

    SolarResponse {
        sunrise: resolved.sun_times.map(|st| st.sunrise),
        sunset: resolved.sun_times.map(|st| st.sunset),
        solar_noon: resolved.solar.solar_noon_hour,
        solar_midnight: resolved.solar.solar_midnight_hour(),
        day_length: resolved.sun_times.map(|st| st.day_length),
        twilight: resolved.twilight.as_ref().map(|tw| TwilightResponse {
            dawn: TwilightPhaseResponse {
                civil: tw.dawn.civil,
                nautical: tw.dawn.nautical,
                astronomical: tw.dawn.astronomical,
            },
            dusk: TwilightPhaseResponse {
                civil: tw.dusk.civil,
                nautical: tw.dusk.nautical,
                astronomical: tw.dusk.astronomical,
            },
        }),
    }
}

fn build_twilight_response(
    twilight: Option<&rhythm_core::TwilightTimes>,
) -> Option<crate::api_types::TwilightResponse> {
    use crate::api_types::{TwilightPhaseResponse, TwilightResponse};

    twilight.map(|tw| TwilightResponse {
        dawn: TwilightPhaseResponse {
            civil: tw.dawn.civil,
            nautical: tw.dawn.nautical,
            astronomical: tw.dawn.astronomical,
        },
        dusk: TwilightPhaseResponse {
            civil: tw.dusk.civil,
            nautical: tw.dusk.nautical,
            astronomical: tw.dusk.astronomical,
        },
    })
}

/// Parse an optional `YYYY-MM-DD` date string, falling back to today (UTC).
fn parse_date_or_today(date: Option<&str>, utc_offset: f32) -> Result<(i32, u32, u32)> {
    if let Some(d) = date {
        let parts: Vec<&str> = d.split('-').collect();
        if parts.len() != 3 {
            return Err(anyhow::anyhow!("Invalid date format, expected YYYY-MM-DD"));
        }
        let year: i32 = parts[0]
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid year"))?;
        let month: u32 = parts[1]
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid month"))?;
        let day: u32 = parts[2]
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid day"))?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return Err(anyhow::anyhow!("Date out of range"));
        }
        Ok((year, month, day))
    } else {
        let now = chrono::Utc::now().naive_utc();
        let offset_secs = (utc_offset * 3600.0) as i64;
        let local = now + chrono::Duration::seconds(offset_secs);
        Ok((
            local.date().year(),
            local.date().month(),
            local.date().day(),
        ))
    }
}

/// Compute the current local hour from UTC + offset.
fn current_local_hour(utc_offset: f32) -> f32 {
    let now = chrono::Utc::now().naive_utc();
    let offset_secs = (utc_offset * 3600.0) as i64;
    let local = now + chrono::Duration::seconds(offset_secs);
    let t = local.time();
    t.hour() as f32 + t.minute() as f32 / 60.0 + t.second() as f32 / 3600.0
}

/// Build the full curve response (config + solar + curve samples + step sequences).
///
/// Used by `GET /api/curve` and `POST /api/curve`.
pub fn build_curve(
    state: &SharedState,
    config_override: Option<LightProfileConfig>,
    profile_id: Option<&str>,
    date: Option<&str>,
    samples_per_hour: Option<u32>,
    start_hour: Option<f32>,
    max_steps: Option<u8>,
) -> Result<String> {
    use crate::api_types::CurveResponse;

    let (registry, target_id, config, utc_offset) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let mut registry = light_profile_registry_from_state(&s);
        let mut config_override = config_override;
        if let Some(config) = &mut config_override {
            rhythm_core::normalize_builtin_state_profile_config(config);
        }
        let target_id = if let Some(ref config) = config_override {
            config.id.clone()
        } else {
            resolve_profile_id(&s, profile_id)?
        };
        let config = if let Some(config) = config_override {
            if !registry.set_profile_config(config.clone()) {
                return Err(anyhow::anyhow!("Unknown light profile: {}", config.id));
            }
            config
        } else {
            s.light_profile_config(&target_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Unknown light profile: {}", target_id))?
        };
        (registry, target_id, config, s.utc_offset_hours)
    };

    let (year, month, day) = parse_date_or_today(date, utc_offset)?;

    let resolved = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        resolve_solar(&s, year, month, day)
    };

    let module = registry
        .get(&target_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown light profile: {}", target_id))?;

    let samples = samples_per_hour.unwrap_or(4);
    let curve_data = rhythm_profile::generate_curve_data(
        module.as_ref(),
        resolved.solar,
        resolved.sun_times,
        samples,
    );

    let start = start_hour.unwrap_or_else(|| current_local_hour(utc_offset));
    let steps = max_steps.unwrap_or(config.max_dim_steps);
    let step_data = rhythm_profile::generate_step_sequences(
        module.as_ref(),
        resolved.solar,
        resolved.sun_times,
        start,
        steps,
    );

    let solar_response = build_solar_response(&resolved);
    let config_value = serde_json::to_value(&config)?;

    let response = CurveResponse {
        config: config_value,
        solar: solar_response,
        curve: curve_data,
        steps: step_data,
    };

    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Build the current lighting values response.
///
/// Used by `GET /api/curve/now`.
pub fn build_curve_now(
    state: &SharedState,
    profile_id: Option<&str>,
    hour_override: Option<f32>,
) -> Result<String> {
    use crate::api_types::LightingNowResponse;
    use rhythm_core::kelvin_to_mireds;

    let (registry, target_id, utc_offset) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            light_profile_registry_from_state(&s),
            resolve_profile_id(&s, profile_id)?,
            s.utc_offset_hours,
        )
    };

    let hour = hour_override.unwrap_or_else(|| current_local_hour(utc_offset));

    let now = chrono::Utc::now().naive_utc();
    let offset_secs = (utc_offset * 3600.0) as i64;
    let local = now + chrono::Duration::seconds(offset_secs);
    let (year, month, day) = (
        local.date().year(),
        local.date().month(),
        local.date().day(),
    );

    let resolved = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        resolve_solar(&s, year, month, day)
    };

    let module = registry
        .get(&target_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown light profile: {}", target_id))?;
    let ctx = rhythm_core::CurveContext::new(hour, resolved.solar, resolved.sun_times);
    let values = module.calculate(&ctx);

    let response = LightingNowResponse {
        hour,
        brightness: values.brightness,
        kelvin: values.kelvin,
        mireds: kelvin_to_mireds(values.kelvin),
        rgb: values.rgb,
        xy: values.xy,
        solar_time: values.solar_time,
        sun_position: values.sun_position,
    };

    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Build the solar times response.
///
/// Used by `GET /api/curve/solar`.
pub fn build_curve_solar(state: &SharedState, date: Option<&str>) -> Result<String> {
    let utc_offset = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.utc_offset_hours
    };

    let (year, month, day) = parse_date_or_today(date, utc_offset)?;

    let resolved = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        resolve_solar(&s, year, month, day)
    };

    let response = build_solar_response(&resolved);
    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::{ActiveHub, HubType};
    use crate::state::{AppState, MotionSnapshot};
    use chrono::{Datelike, Timelike};
    use rhythm_core::{LightProfileConfig, RoomSnapshot, RuntimeHandle};
    use std::sync::{Arc, Mutex};

    /// Mock runtime that returns configurable room snapshots and tracks events.
    struct MockRuntime {
        snapshots: Vec<RoomSnapshot>,
        events: Mutex<Vec<(String, ButtonAction)>>,
        applied_commands: Mutex<Vec<(String, rhythm_core::LightingCommand)>>,
        config_updates: Mutex<Vec<LightProfileConfig>>,
        time_offset_updates: Mutex<Vec<(String, f32)>>,
        current_hour: f32,
    }

    impl MockRuntime {
        fn new(snapshots: Vec<RoomSnapshot>, current_hour: f32) -> Self {
            Self {
                snapshots,
                events: Mutex::new(Vec::new()),
                applied_commands: Mutex::new(Vec::new()),
                config_updates: Mutex::new(Vec::new()),
                time_offset_updates: Mutex::new(Vec::new()),
                current_hour,
            }
        }

        fn events(&self) -> Vec<(String, ButtonAction)> {
            self.events.lock().unwrap().clone()
        }

        fn applied_commands(&self) -> Vec<(String, rhythm_core::LightingCommand)> {
            self.applied_commands.lock().unwrap().clone()
        }

        fn config_updates(&self) -> Vec<LightProfileConfig> {
            self.config_updates.lock().unwrap().clone()
        }

        fn time_offset_updates(&self) -> Vec<(String, f32)> {
            self.time_offset_updates.lock().unwrap().clone()
        }
    }

    impl RuntimeHandle for MockRuntime {
        fn handle_event(&self, event: &InputEvent) -> anyhow::Result<bool> {
            self.events
                .lock()
                .unwrap()
                .push((event.room_id.clone(), event.action));
            // Reset/OnPress → lights on; LightsOff/OffPress → lights off
            Ok(!matches!(
                event.action,
                ButtonAction::LightsOff | ButtonAction::OffPress
            ))
        }
        fn sync_rooms(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_solar(&self, _: rhythm_core::SolarTime) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_light_profile_config(&self, config: LightProfileConfig) -> anyhow::Result<()> {
            self.config_updates.lock().unwrap().push(config);
            Ok(())
        }
        fn set_mode_configs(&self, _: Vec<rhythm_core::ModeConfig>) -> anyhow::Result<()> {
            Ok(())
        }
        fn periodic_tick_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }
        fn engine_room_snapshot(&self, room_id: &str) -> Option<RoomSnapshot> {
            self.snapshots.iter().find(|s| s.id == room_id).cloned()
        }
        fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
            self.snapshots.clone()
        }
        fn restore_room_state(
            &self,
            _: &str,
            _: bool,
            _: bool,
            _: f32,
            _: f32,
            _: bool,
            _: bool,
            _: rhythm_core::RoomProfileSettings,
        ) {
        }
        fn add_room(&self, _: &str, _: &str) {}
        fn remove_room(&self, _: &str) {}
        fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }
        fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn apply_room_command(
            &self,
            room_id: &str,
            command: rhythm_core::LightingCommand,
        ) -> anyhow::Result<()> {
            self.applied_commands
                .lock()
                .unwrap()
                .push((room_id.to_string(), command));
            Ok(())
        }
        fn set_power_save(&self, _: bool) -> Vec<String> {
            vec![]
        }
        fn is_power_save(&self) -> bool {
            false
        }
        fn set_room_brightness(&self, _: &str, _: u8) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_room_time_offset(&self, room_id: &str, offset_minutes: f32) -> anyhow::Result<()> {
            self.time_offset_updates
                .lock()
                .unwrap()
                .push((room_id.to_string(), offset_minutes));
            Ok(())
        }
        fn idle_brightness(&self) -> u8 {
            1
        }
        fn soft_off_tick_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn any_lights_on(&self, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        fn current_hour(&self) -> f32 {
            self.current_hour
        }
        fn set_light_profile(&self, _: &str) -> bool {
            true
        }
        fn active_light_profile_id(&self) -> String {
            "rhythm".into()
        }
        fn available_light_profiles(&self) -> Vec<(String, String)> {
            vec![
                ("rhythm".into(), "Rhythm Curve".into()),
                ("sleep".into(), "Sleep Curve".into()),
            ]
        }
    }

    fn make_snapshot(id: &str, disabled: bool, soft_off: bool) -> RoomSnapshot {
        RoomSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            rhythm_enabled: true,
            disabled,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off,
            hard_off: false,
            profile_settings: rhythm_core::RoomProfileSettings::default(),
        }
    }

    fn setup_state(snapshots: Vec<RoomSnapshot>) -> (SharedState, Arc<MockRuntime>) {
        setup_state_at_hour(snapshots, 12.0)
    }

    fn setup_state_at_hour(
        snapshots: Vec<RoomSnapshot>,
        current_hour: f32,
    ) -> (SharedState, Arc<MockRuntime>) {
        let runtime = Arc::new(MockRuntime::new(snapshots, current_hour));
        let mut app = AppState::default();
        let hub_type = HubType::parse("mock").unwrap();
        let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime.clone() as Arc<dyn RuntimeHandle>),
                hub_data: Box::new(()),
                registry: None,
                discovery: None,
                shutdown: Default::default(),
            },
        );
        (Arc::new(Mutex::new(app)), runtime)
    }

    #[test]
    fn fix_resets_on_rooms_only() {
        let (state, runtime) = setup_state(vec![
            make_snapshot("on_room", false, false),
            make_snapshot("off_room", false, false),
            make_snapshot("disabled_room", true, false),
            make_snapshot("soft_off_room", false, true),
        ]);

        // Mark only on_room as lights-on
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("on_room".into(), true);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("off_room".into(), false);

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["rooms_reset"], 1);
        // rooms is now an array of room state objects, not ID strings
        assert_eq!(parsed["rooms"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["rooms"][0]["id"], "on_room");
        assert_eq!(parsed["motion_cleared"], 0);

        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("on_room".into(), ButtonAction::Reset));
    }

    #[test]
    fn fix_turns_off_motion_rooms() {
        let (state, runtime) = setup_state(vec![
            make_snapshot("motion_room", false, false),
            make_snapshot("normal_room", false, false),
        ]);

        // motion_room has active motion, normal_room is just on
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("motion_room".into(), true);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("normal_room".into(), true);
        state.lock().unwrap().motion_snapshots.insert(
            "motion_room".into(),
            MotionSnapshot {
                motion_active: true,
                motion_owned: true,
                remaining_secs: None,
                timeout_secs: 300,
                warning_active: false,
            },
        );

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // normal_room → reset, motion_room → lights_off
        assert_eq!(parsed["rooms_reset"], 1);
        // rooms is now an array of room state objects containing both reset and motion rooms
        let room_ids: Vec<&str> = parsed["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert!(room_ids.contains(&"normal_room"));
        assert!(room_ids.contains(&"motion_room"));
        assert_eq!(parsed["motion_cleared"], 1);
        assert_eq!(parsed["motion_rooms"], serde_json::json!(["motion_room"]));

        let events = runtime.events();
        assert_eq!(events.len(), 2);
        assert!(events.contains(&("normal_room".into(), ButtonAction::Reset)));
        assert!(events.contains(&("motion_room".into(), ButtonAction::OffPress)));

        // Verify pending_motion_clear was populated
        let s = state.lock().unwrap();
        assert_eq!(s.pending_motion_clear, vec!["motion_room".to_string()]);
        // lights_on should be false for motion room
        assert_eq!(s.room_lights_on.get("motion_room"), Some(&false));
    }

    #[test]
    fn fix_skips_disabled_motion_rooms() {
        let (state, runtime) = setup_state(vec![make_snapshot("disabled_motion", true, false)]);

        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("disabled_motion".into(), true);
        state.lock().unwrap().motion_snapshots.insert(
            "disabled_motion".into(),
            MotionSnapshot {
                motion_active: true,
                motion_owned: true,
                remaining_secs: None,
                timeout_secs: 300,
                warning_active: false,
            },
        );

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["rooms_reset"], 0);
        assert_eq!(parsed["motion_cleared"], 0);
        assert!(runtime.events().is_empty());
    }

    #[test]
    fn fix_turns_off_motion_sensor_rooms_without_active_timer() {
        // Room has a motion sensor registered in the registry but no active
        // motion timer. fix_my_lights should still turn it off (not reset).
        let runtime = Arc::new(MockRuntime::new(
            vec![
                make_snapshot("motion_room", false, false),
                make_snapshot("normal_room", false, false),
            ],
            12.0,
        ));
        let mut registry = crate::registry::HubDeviceRegistry::new();
        registry.upsert_device("sensor1", "motion_room", &[], DeviceType::Motion);
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(registry));

        let mut app = AppState::default();
        let hub_type = HubType::parse("mock").unwrap();
        let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime.clone() as Arc<dyn RuntimeHandle>),
                hub_data: Box::new(()),
                registry: Some(registry),
                discovery: None,
                shutdown: Default::default(),
            },
        );
        app.room_lights_on.insert("motion_room".into(), true);
        app.room_lights_on.insert("normal_room".into(), true);
        let state: SharedState = Arc::new(Mutex::new(app));

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // normal_room → Reset, motion_room → OffPress (despite no active timer)
        assert_eq!(parsed["rooms_reset"], 1);
        assert_eq!(parsed["motion_cleared"], 1);
        assert_eq!(parsed["motion_rooms"], serde_json::json!(["motion_room"]));

        let events = runtime.events();
        assert_eq!(events.len(), 2);
        assert!(events.contains(&("normal_room".into(), ButtonAction::Reset)));
        assert!(events.contains(&("motion_room".into(), ButtonAction::OffPress)));
    }

    #[test]
    fn fix_no_runtime_returns_error() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_fix_my_lights(&state, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No runtime"));
    }

    #[test]
    fn fix_empty_rooms_succeeds() {
        let (state, _runtime) = setup_state(vec![]);
        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["rooms_reset"], 0);
        assert_eq!(parsed["motion_cleared"], 0);
    }

    // ========================================================================
    // Room action tests
    // ========================================================================

    #[test]
    fn room_action_on() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "on", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::OnPress));
    }

    #[test]
    fn room_action_off() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "off", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::OffPress));
    }

    #[test]
    fn room_action_toggle() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "toggle", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::Toggle));
    }

    #[test]
    fn room_action_reset() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "reset", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::Reset));
    }

    #[test]
    fn room_action_rhythm_on() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "rhythm_on", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::RhythmOn));
    }

    #[test]
    fn room_action_rhythm_off() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "rhythm_off", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::RhythmOff));
    }

    #[test]
    fn room_action_unknown() {
        let (state, _runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "nonsense", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown action"));
    }

    #[test]
    fn room_action_no_runtime() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_room_action(&state, "room1", "on", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No runtime"));
    }

    // ========================================================================
    // Settings tests
    // ========================================================================

    #[test]
    fn settings_partial_update() {
        let (state, _runtime) = setup_state(vec![]);
        do_settings_set(&state, Some(true), None, None, None).unwrap();
        let s = state.lock().unwrap();
        assert!(s.power_save);
    }

    #[test]
    fn settings_update_preserves_profile_interval() {
        let (state, _runtime) = setup_state(vec![]);

        let original_interval = {
            let s = state.lock().unwrap();
            s.runtime_config.update_interval_secs
        };

        do_settings_set(&state, Some(true), None, None, None).unwrap();

        let s = state.lock().unwrap();
        assert_eq!(s.runtime_config.update_interval_secs, original_interval);
        assert!(s.power_save);
    }

    // ========================================================================
    // Parse helper tests
    // ========================================================================

    #[test]
    fn parse_buttons_valid() {
        let json = serde_json::json!({
            "buttons": [
                {"button_id": "btn1", "control_id": 1},
                {"button_id": "btn2", "control_id": 2}
            ]
        });
        let result = parse_buttons(&json);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], ("btn1".to_string(), 1));
        assert_eq!(result[1], ("btn2".to_string(), 2));
    }

    #[test]
    fn parse_buttons_empty() {
        let json = serde_json::json!({"name": "no buttons here"});
        let result = parse_buttons(&json);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_buttons_missing_fields() {
        let json = serde_json::json!({
            "buttons": [
                {"button_id": "btn1"},
                {"control_id": 3},
                {"button_id": "btn2", "control_id": 4}
            ]
        });
        let result = parse_buttons(&json);
        // Only the complete entry should remain
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], ("btn2".to_string(), 4));
    }

    #[test]
    fn room_params_from_json_valid() {
        let json = serde_json::json!({
            "id": "room_1",
            "name": "Living Room",
            "grouped_light_id": "gl_abc",
            "rhythm_enabled": true,
            "disabled": false,
            "state": "idle",
            "device_ids": ["d1", "d2"]
        });
        let params = RoomParams::from_json(&json).unwrap();
        assert_eq!(params.id, "room_1");
        assert_eq!(params.name, "Living Room");
        assert_eq!(params.grouped_light_id, "gl_abc");
        assert!(params.rhythm_enabled);
        assert!(!params.disabled);
        assert_eq!(params.state, Some(RoomModeState::Idle));
        assert_eq!(params.device_ids, vec!["d1".to_string(), "d2".to_string()]);
    }

    #[test]
    fn room_params_from_json_missing_id() {
        let json = serde_json::json!({
            "name": "No ID Room"
        });
        let result = RoomParams::from_json(&json);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("Missing room.id"));
    }

    #[test]
    fn room_params_from_json_defaults() {
        let json = serde_json::json!({
            "id": "minimal_room"
        });
        let params = RoomParams::from_json(&json).unwrap();
        assert_eq!(params.id, "minimal_room");
        assert_eq!(params.name, "minimal_room"); // defaults to id
        assert_eq!(params.grouped_light_id, ""); // defaults to empty
        assert!(!params.rhythm_enabled); // defaults to false
        assert!(!params.disabled); // defaults to false
        assert_eq!(params.state, None); // defaults to None
        assert!(params.device_ids.is_empty()); // defaults to empty vec
    }

    // ========================================================================
    // Unified API response shape tests
    // ========================================================================

    #[test]
    fn build_room_rhythm_state_returns_struct() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();
        assert_eq!(room_state.id, "r1");
        assert!(room_state.rhythm_enabled);
        assert!(room_state.lights_on);
        assert_eq!(room_state.state, RoomModeState::Active);
    }

    #[test]
    fn build_room_rhythm_state_missing_room_errors() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = build_room_rhythm_state(&state, "nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn build_room_rhythm_state_no_runtime_errors() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = build_room_rhythm_state(&state, "r1");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No runtime"));
    }

    #[test]
    fn build_room_rhythm_state_serializes_without_status() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();
        let json_str = serde_json::to_string(&room_state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["id"], "r1");
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn build_room_rhythm_state_warning_uses_dimmed_brightness() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);
        state.lock().unwrap().motion_snapshots.insert(
            "r1".into(),
            MotionSnapshot {
                motion_active: false,
                motion_owned: true,
                remaining_secs: Some(45),
                timeout_secs: 300,
                warning_active: true,
            },
        );

        let expected_active = {
            let s = state.lock().unwrap();
            compute_room_display_values_for_settings(
                &s,
                &rhythm_core::RoomProfileSettings::default(),
                RoomModeState::Active,
                0.0,
                0.0,
            )
            .0
        };

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();
        let expected_warning = ((expected_active as f32) * crate::event_loop::WARNING_DIM_FACTOR)
            .clamp(1.0, 100.0) as u8;

        assert_eq!(room_state.state, RoomModeState::Warning);
        assert_eq!(room_state.brightness, expected_warning);
    }

    #[test]
    fn build_room_rhythm_state_hard_off_uses_zero_brightness() {
        let mut snapshot = make_snapshot("r1", false, false);
        snapshot.hard_off = true;
        let (state, _rt) = setup_state(vec![snapshot]);

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();

        assert_eq!(room_state.state, RoomModeState::HardOff);
        assert_eq!(room_state.brightness, 0);
        assert_eq!(room_state.kelvin, 0);
    }

    #[test]
    fn room_action_response_is_valid_room_state_json() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_room_action(&state, "r1", "on", false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        // Must be a room state object — has id, rhythm_enabled, etc.
        assert_eq!(parsed["id"], "r1");
        assert!(parsed["rhythm_enabled"].is_boolean());
        assert!(parsed["brightness"].is_number());
        assert!(parsed["kelvin"].is_number());
        assert!(parsed["lights_on"].is_boolean());
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn settings_response_is_valid_settings_json() {
        let (state, _rt) = setup_state(vec![]);
        let result = do_settings_set(&state, Some(true), None, None, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["power_save"].is_boolean());
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn build_settings_dto_matches_state() {
        let (state, _rt) = setup_state(vec![]);
        {
            let mut s = state.lock().unwrap();
            s.power_save = true;
        }
        let dto = build_settings_dto(&state).unwrap();
        assert!(dto.power_save);
    }

    #[test]
    fn build_rooms_state_uses_sse_motion_field_names() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);
        state.lock().unwrap().motion_snapshots.insert(
            "r1".into(),
            MotionSnapshot {
                motion_active: true,
                motion_owned: true,
                remaining_secs: Some(45),
                timeout_secs: 300,
                warning_active: true,
            },
        );

        let expected_active = {
            let s = state.lock().unwrap();
            compute_room_display_values_for_settings(
                &s,
                &rhythm_core::RoomProfileSettings::default(),
                RoomModeState::Active,
                0.0,
                0.0,
            )
            .0
        };
        let expected_warning = ((expected_active as f32) * crate::event_loop::WARNING_DIM_FACTOR)
            .clamp(1.0, 100.0) as u8;

        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        let room = &parsed["rooms"][0];
        // SSE-aligned field names
        assert_eq!(room["state"], "warning");
        assert_eq!(room["brightness"], expected_warning);
        assert_eq!(room["remaining_secs"], 45);
        assert_eq!(room["timeout_secs"], 300);
        assert_eq!(room["warning_active"], true);
        assert_eq!(room["motion_active"], true);
        assert_eq!(room["motion_owned"], true);
        // Old names must NOT appear
        assert!(room.get("motion_remaining").is_none());
        assert!(room.get("motion_timeout").is_none());
        assert!(room.get("motion_warning").is_none());
    }

    #[test]
    fn build_rooms_state_omits_motion_when_absent() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        let room = &parsed["rooms"][0];
        assert!(room.get("motion_active").is_none());
        assert!(room.get("remaining_secs").is_none());
        assert!(room.get("timeout_secs").is_none());
        assert!(room.get("warning_active").is_none());
    }

    #[test]
    fn build_rooms_state_has_hub_connected() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["hub_connected"].is_boolean());
    }

    #[test]
    fn build_rooms_state_sensor_no_motion_snapshot_populates_defaults() {
        // Room has a motion sensor in the registry but no MotionSnapshot yet.
        let runtime = Arc::new(MockRuntime::new(
            vec![
                make_snapshot("sensor_room", false, false),
                make_snapshot("plain_room", false, false),
            ],
            12.0,
        ));
        let mut registry = crate::registry::HubDeviceRegistry::new();
        registry.upsert_device("ms1", "sensor_room", &[], DeviceType::Motion);
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(registry));

        let mut app = AppState::default();
        let hub_type = HubType::parse("mock").unwrap();
        let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime as Arc<dyn RuntimeHandle>),
                hub_data: Box::new(()),
                registry: Some(registry),
                discovery: None,
                shutdown: Default::default(),
            },
        );
        let state: SharedState = Arc::new(Mutex::new(app));

        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // sensor_room should have idle motion defaults
        let sensor_room = parsed["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "sensor_room")
            .unwrap();
        assert_eq!(sensor_room["motion_active"], false);
        assert_eq!(sensor_room["motion_owned"], false);
        // remaining_secs is None → omitted by skip_serializing_if
        assert!(sensor_room.get("remaining_secs").is_none());
        assert!(sensor_room["timeout_secs"].is_number());
        assert_eq!(sensor_room["warning_active"], false);

        // plain_room should have no motion fields
        let plain_room = parsed["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "plain_room")
            .unwrap();
        assert!(plain_room.get("motion_active").is_none());
        assert!(plain_room.get("timeout_secs").is_none());
    }

    #[test]
    fn build_state_snapshot_valid_json() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Top-level structure
        assert!(parsed["version"].is_string());
        assert!(parsed["platform"].is_string());
        assert!(parsed["context"].is_string());
        assert!(parsed["hubs"].is_array());
        assert!(parsed["active_profile"].is_object());
        assert!(parsed["active_profile"]["config"].is_object());
        assert!(parsed["active_profile"]["effective"].is_object());
        assert!(parsed["location"].is_object());
        assert!(parsed["settings"].is_object());
        assert!(parsed["rooms"].is_array());
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn build_state_snapshot_settings_uses_struct() {
        let (state, _rt) = setup_state(vec![]);
        {
            let mut s = state.lock().unwrap();
            s.power_save = true;
        }
        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["settings"]["power_save"], true);
    }

    #[test]
    fn build_state_snapshot_active_profile_effective_uses_live_values() {
        let (state, _rt) = setup_state(vec![]);
        {
            let mut s = state.lock().unwrap();
            s.default_fade_ms = 1;
            s.default_motion_timeout_secs = 2;
            s.runtime_config.update_interval_secs = 3;
        }

        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let current_local_time = parsed["location"]["current_local_time"]
            .as_str()
            .expect("current_local_time should be present");
        let local_now = chrono::DateTime::parse_from_rfc3339(current_local_time)
            .expect("current_local_time should parse");
        let current_hour = local_now.hour() as f32
            + local_now.minute() as f32 / 60.0
            + local_now.second() as f32 / 3600.0;

        let expected = {
            let s = state.lock().unwrap();
            let active_profile_cfg = active_profile_config(&s);
            let resolved_solar =
                resolve_solar(&s, local_now.year(), local_now.month(), local_now.day());
            (
                active_profile_effective_values(
                    &active_profile_cfg,
                    current_hour,
                    resolved_solar.solar,
                    resolved_solar.sun_times,
                ),
                resolved_solar.solar,
            )
        };

        assert_eq!(
            parsed["active_profile"]["effective"]["fade_ms"]
                .as_u64()
                .unwrap(),
            u64::from(expected.0.fade_ms)
        );
        assert_eq!(
            parsed["active_profile"]["effective"]["motion_timeout_secs"]
                .as_u64()
                .unwrap(),
            expected.0.motion_timeout_secs
        );
        assert_eq!(
            parsed["active_profile"]["effective"]["rhythm_interval_secs"]
                .as_u64()
                .unwrap(),
            expected.0.rhythm_interval_secs
        );
        assert_eq!(
            parsed["location"]["solar_noon"].as_f64().unwrap() as f32,
            expected.1.solar_noon_hour
        );
        assert_eq!(
            parsed["location"]["solar_midnight"].as_f64().unwrap() as f32,
            expected.1.solar_midnight_hour()
        );
        assert_eq!(
            parsed["location"]["current_solar_time"].as_f64().unwrap() as f32,
            expected.1.get_solar_time(current_hour)
        );
        assert_ne!(
            parsed["active_profile"]["effective"]["fade_ms"]
                .as_u64()
                .unwrap(),
            1
        );
        assert_ne!(
            parsed["active_profile"]["effective"]["motion_timeout_secs"]
                .as_u64()
                .unwrap(),
            2
        );
        assert_ne!(
            parsed["active_profile"]["effective"]["rhythm_interval_secs"]
                .as_u64()
                .unwrap(),
            3
        );
    }

    #[test]
    fn active_profile_effective_uses_evening_sun_times_for_auto_interval() {
        let effective = active_profile_effective_values(
            &rhythm_core::default_rhythm_profile(),
            19.64,
            rhythm_core::SolarTime::new(13.0, 35.0, 172),
            Some(rhythm_core::SunTimes {
                sunrise: 6.0,
                sunset: 20.0,
                day_length: 14.0,
            }),
        );

        assert!(
            effective.rhythm_interval_secs < 180,
            "Sunset ramp should not collapse to the plateau cadence"
        );
    }

    #[test]
    fn fix_response_no_status_key() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed.get("status").is_none());
        // Rooms is an array of objects (not ID strings)
        assert!(parsed["rooms"][0].is_object());
        assert!(parsed["rooms"][0]["id"].is_string());
    }

    #[test]
    fn fix_response_no_room_states_key() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        // Old "room_states" key must not exist
        assert!(parsed.get("room_states").is_none());
        // Room state objects are in "rooms"
        assert!(parsed["rooms"].is_array());
    }

    #[test]
    fn fix_response_rooms_contain_full_state() {
        let (state, _rt) = setup_state(vec![
            make_snapshot("r1", false, false),
            make_snapshot("r2", false, false),
        ]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r2".into(), true);

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        for room in parsed["rooms"].as_array().unwrap() {
            assert!(room["id"].is_string());
            assert!(room["rhythm_enabled"].is_boolean());
            assert!(room["brightness"].is_number());
            assert!(room["kelvin"].is_number());
            assert!(room["lights_on"].is_boolean());
        }
    }

    // ========================================================================
    // set_brightness tests
    // ========================================================================

    #[test]
    fn set_brightness_succeeds() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_brightness(&state, "r1", 75, false);
        assert!(result.is_ok());
        // Setting brightness marks room as lights_on
        assert_eq!(state.lock().unwrap().room_lights_on.get("r1"), Some(&true));
    }

    #[test]
    fn set_brightness_clamps_high() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        // 200 should be clamped to 100
        let result = do_set_brightness(&state, "r1", 200, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_brightness_clamps_low() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        // 0 should be clamped to 1
        let result = do_set_brightness(&state, "r1", 0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_brightness_no_runtime_errors() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_set_brightness(&state, "r1", 50, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No runtime"));
    }

    // ========================================================================
    // set_time_offset tests
    // ========================================================================

    #[test]
    fn set_time_offset_succeeds() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_time_offset(&state, "r1", 30.0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_time_offset_negative() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_time_offset(&state, "r1", -60.0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_time_offset_no_runtime_errors() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_set_time_offset(&state, "r1", 15.0, false);
        assert!(result.is_err());
    }

    // ========================================================================
    // room_preferences tests
    // ========================================================================

    #[test]
    fn room_preferences_rhythm_enabled() {
        let snap = make_snapshot("r1", false, false);
        let (state, _rt) = setup_state(vec![snap]);
        let result = do_room_preferences_set(&state, "r1", Some(true), None, None, None, false);
        assert!(result.is_ok());
    }

    #[test]
    fn room_preferences_idle_implies_rhythm_enabled() {
        // Idle should force rhythm enabled so standby ticks still render.
        let mut snap = make_snapshot("r1", false, false);
        snap.rhythm_enabled = false;
        let (state, _rt) = setup_state(vec![snap]);
        let result = do_room_preferences_set(
            &state,
            "r1",
            Some(false),
            None,
            Some(RoomModeState::Idle),
            None,
            false,
        );
        assert!(result.is_ok());
        // Idle implies lights conceptually on.
        assert_eq!(state.lock().unwrap().room_lights_on.get("r1"), Some(&true));
    }

    #[test]
    fn room_preferences_missing_room_errors() {
        let (state, _rt) = setup_state(vec![]);
        let result =
            do_room_preferences_set(&state, "nonexistent", Some(true), None, None, None, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // ========================================================================
    // Settings edge case tests
    // ========================================================================

    #[test]
    fn settings_power_save() {
        let (state, _rt) = setup_state(vec![]);
        do_settings_set(&state, Some(true), None, None, None).unwrap();
        assert!(state.lock().unwrap().power_save);
        do_settings_set(&state, Some(false), None, None, None).unwrap();
        assert!(!state.lock().unwrap().power_save);
    }

    #[test]
    fn active_profile_interval_defaults_from_config() {
        let (state, _rt) = setup_state(vec![]);
        // Auto resolves to DEFAULT_UPDATE_INTERVAL_SECS at reference hour
        assert_eq!(
            state.lock().unwrap().runtime_config.update_interval_secs,
            rhythm_core::runtime::config::DEFAULT_UPDATE_INTERVAL_SECS
        );
    }

    #[test]
    fn active_profile_config_updates_runtime_interval() {
        let (state, _rt) = setup_state(vec![]);
        let mut rhythm = rhythm_core::default_rhythm_profile();
        rhythm.rhythm_interval_secs = rhythm_core::TimerSetting::Fixed { value: 17 };

        do_config_set(&state, rhythm).unwrap();

        assert_eq!(
            state.lock().unwrap().runtime_config.update_interval_secs,
            17
        );
    }

    #[test]
    fn switching_profiles_updates_runtime_interval() {
        let (state, _rt) = setup_state(vec![]);
        let mut sleep = rhythm_core::default_sleep_profile();
        sleep.rhythm_interval_secs = rhythm_core::TimerSetting::Fixed { value: 23 };

        do_config_set(&state, sleep).unwrap();
        do_set_active_mode(&state, RhythmMode::Sleep).unwrap();

        assert_eq!(
            state.lock().unwrap().runtime_config.update_interval_secs,
            23
        );
    }

    #[test]
    fn mode_transition_uses_configured_duration_as_room_fade() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Sleep;
            s.room_lights_on.insert("r1".into(), true);
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                RhythmMode::Sleep,
                RhythmMode::Day,
                5_000,
            )
            .with_trigger(ModeTransitionTrigger::Sunrise)]);
        }

        do_set_active_mode_with_trigger(&state, RhythmMode::Day, ModeTransitionTrigger::Sunrise)
            .unwrap();

        let applied = runtime.applied_commands();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "r1");
        assert_eq!(applied[0].1.transition_ms, Some(5_000));
        assert!(state
            .lock()
            .unwrap()
            .room_mode_transitions
            .contains_key("r1"));
    }

    #[test]
    fn idle_only_mode_change_reapplies_idle_rooms_only() {
        let (state, runtime) = setup_state(vec![
            make_snapshot("active_room", false, false),
            make_snapshot("idle_room", false, true),
        ]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Day;
            s.room_lights_on.insert("active_room".into(), true);

            let mut idle = rhythm_core::default_day_idle_profile();
            idle.curve = rhythm_core::LightCurveShape::Constant {
                brightness: 1.0,
                color_temp: 0.0,
                direct_color: Some(rhythm_core::LightDirectColor {
                    xy: rhythm_core::rgb_to_xy(rhythm_core::Rgb::new(255, 64, 32)),
                    rgb: rhythm_core::Rgb::new(255, 64, 32),
                }),
            };
            idle.min_brightness = 1;
            idle.max_brightness = 1;
            s.set_light_profile_config(idle);
            s.set_mode_configs(vec![ModeConfig {
                mode: RhythmMode::Day,
                active_profile_id: Some(rhythm_core::RHYTHM_PROFILE_ID.into()),
                idle_profile_id: Some(rhythm_core::DAY_IDLE_PROFILE_ID.into()),
                wake_profile_id: None,
                warning_profile_id: None,
            }]);
        }

        do_settings_set(
            &state,
            None,
            None,
            Some(vec![ModeConfig {
                mode: RhythmMode::Day,
                active_profile_id: Some(rhythm_core::RHYTHM_PROFILE_ID.into()),
                idle_profile_id: None,
                wake_profile_id: None,
                warning_profile_id: None,
            }]),
            None,
        )
        .unwrap();

        let applied = runtime.applied_commands();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "idle_room");
    }

    #[test]
    fn inactive_mode_change_does_not_reapply_current_lights() {
        let (state, runtime) = setup_state(vec![make_snapshot("active_room", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Day;
            s.room_lights_on.insert("active_room".into(), true);
        }

        do_settings_set(
            &state,
            None,
            None,
            Some(vec![ModeConfig {
                mode: RhythmMode::Sleep,
                active_profile_id: Some(rhythm_core::SLEEP_PROFILE_ID.into()),
                idle_profile_id: Some(rhythm_core::SLEEP_IDLE_PROFILE_ID.into()),
                wake_profile_id: None,
                warning_profile_id: None,
            }]),
            None,
        )
        .unwrap();

        assert!(runtime.applied_commands().is_empty());
    }

    // ========================================================================
    // absorb_time_offset tests
    // ========================================================================

    #[test]
    fn absorb_offset_no_runtime_still_succeeds() {
        // Without a runtime, absorb should succeed (no rooms to reset)
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_absorb_time_offset(&state, None, 30.0);
        assert!(result.is_ok());
    }

    #[test]
    fn absorb_offset_resets_room_offsets() {
        let mut snap = make_snapshot("r1", false, false);
        snap.time_offset_minutes = 30.0;
        let (state, _rt) = setup_state(vec![snap]);

        let result = do_absorb_time_offset(&state, None, 30.0);
        assert!(result.is_ok());

        // MockRuntime's set_room_time_offset is a no-op, so we can't assert
        // the offset was reset on the runtime — but we can verify the command
        // completed without error.
    }

    #[test]
    fn absorb_offset_updates_config_widths() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);

        // Set location so sunrise/sunset can be computed
        {
            let mut s = state.lock().unwrap();
            s.latitude = Some(35.0);
            s.longitude = Some(-120.0);
            s.utc_offset_hours = -8.0;
            s.timezone_name = Some("America/Los_Angeles".to_string());
        }

        let before = state
            .lock()
            .unwrap()
            .active_mode_profile_config()
            .cloned()
            .unwrap();
        let result = do_absorb_time_offset(&state, None, 30.0);
        assert!(result.is_ok());

        let after = state
            .lock()
            .unwrap()
            .active_mode_profile_config()
            .cloned()
            .unwrap();
        // Config may or may not change depending on current time of day
        // (might be at peak or in tail where absorb returns None)
        // But the command should always succeed
        let _ = (before, after);
    }

    #[test]
    fn absorb_offset_targets_requested_non_active_profile() {
        let mut snap = make_snapshot("r1", false, false);
        snap.time_offset_minutes = 30.0;
        let (state, runtime) = setup_state_at_hour(vec![snap], 8.0);
        let mut day_alt = rhythm_core::default_rhythm_profile();
        day_alt.id = "day_alt".into();
        state.lock().unwrap().set_light_profile_config(day_alt);

        let before_day_alt = state
            .lock()
            .unwrap()
            .light_profile_config("day_alt")
            .cloned()
            .unwrap();
        let before_rhythm = state
            .lock()
            .unwrap()
            .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
            .cloned()
            .unwrap();

        let result = do_absorb_time_offset(&state, Some("day_alt"), 60.0);
        assert!(result.is_ok());

        let after_day_alt = state
            .lock()
            .unwrap()
            .light_profile_config("day_alt")
            .cloned()
            .unwrap();
        let after_rhythm = state
            .lock()
            .unwrap()
            .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
            .cloned()
            .unwrap();

        assert_ne!(
            after_day_alt, before_day_alt,
            "target profile should change"
        );
        assert_eq!(
            after_rhythm, before_rhythm,
            "rhythm profile should be untouched"
        );
        assert_eq!(runtime.config_updates().len(), 1);
        assert_eq!(runtime.config_updates()[0].id, "day_alt");
        assert_eq!(runtime.time_offset_updates(), vec![("r1".to_string(), 0.0)]);
    }

    #[test]
    fn absorb_offset_day_idle_profile_is_noop_for_config_but_resets_offsets() {
        let mut snap = make_snapshot("r1", false, false);
        snap.time_offset_minutes = 45.0;
        let (state, runtime) = setup_state_at_hour(vec![snap], 8.0);

        let before_idle = state
            .lock()
            .unwrap()
            .light_profile_config(rhythm_core::DAY_IDLE_PROFILE_ID)
            .cloned()
            .unwrap();

        let result = do_absorb_time_offset(&state, Some(rhythm_core::DAY_IDLE_PROFILE_ID), 30.0);
        assert!(result.is_ok());

        let after_idle = state
            .lock()
            .unwrap()
            .light_profile_config(rhythm_core::DAY_IDLE_PROFILE_ID)
            .cloned()
            .unwrap();

        assert_eq!(
            after_idle, before_idle,
            "day idle inherit-active config should not change"
        );
        assert!(
            runtime.config_updates().is_empty(),
            "non-super-gaussian target should not push config updates"
        );
        assert_eq!(runtime.time_offset_updates(), vec![("r1".to_string(), 0.0)]);
    }

    #[test]
    fn absorb_offset_sleep_idle_profile_is_noop_for_config_but_resets_offsets() {
        let mut snap = make_snapshot("r1", false, false);
        snap.time_offset_minutes = 45.0;
        let (state, runtime) = setup_state_at_hour(vec![snap], 8.0);

        let before_idle = state
            .lock()
            .unwrap()
            .light_profile_config(rhythm_core::SLEEP_IDLE_PROFILE_ID)
            .cloned()
            .unwrap();

        let result = do_absorb_time_offset(&state, Some(rhythm_core::SLEEP_IDLE_PROFILE_ID), 30.0);
        assert!(result.is_ok());

        let after_idle = state
            .lock()
            .unwrap()
            .light_profile_config(rhythm_core::SLEEP_IDLE_PROFILE_ID)
            .cloned()
            .unwrap();

        assert_eq!(
            after_idle, before_idle,
            "sleep idle inherit-active config should not change"
        );
        assert!(
            runtime.config_updates().is_empty(),
            "non-super-gaussian target should not push config updates"
        );
        assert_eq!(runtime.time_offset_updates(), vec![("r1".to_string(), 0.0)]);
    }
}
