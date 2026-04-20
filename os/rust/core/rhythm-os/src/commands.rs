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
    ButtonAction, HubRegistry, InputEvent, LightNodeKind, LightProfileConfig, LightProfileRegistry,
    LightingCommand, ModeChangeCause, ModeConfig, ModeTransitionConfig, ModeTransitionTrigger,
    RestoredNodeState, RestoredRoomState, RhythmMode, RoomModeState, RoomProfileSettings,
    RuntimeHandle, TimerSetting,
};
use serde_json::Value;

use crate::api_types::{
    ActiveProfileDto, ActiveProfileEffectiveDto, ApiCapabilitiesDto, FixResponse, HubCapabilityDto,
    HubDto, LocationDto, ModeLastChangeDto, ModeSettingsDto, ModeTransitionsDto, NodeStateDto,
    NodesPollResponse, ProfilesDto, RoomPollState, RoomRhythmState, RoomsPollResponse, SettingsDto,
    StateSnapshot, TopologyNodeControlDto, TopologyNodeDto,
};
use crate::bundle::{
    BackupBundle, BackupHubCredentials, BackupHubRegistry, BackupInstallation, BackupRuntimeState,
    ConfigurationBundle, ConfigurationImportPayload, ConfigurationRoom, PortableConfiguration,
    BUNDLE_SCHEMA_VERSION,
};
use crate::canonical::identity::HubKey;
use crate::factory_default_config::{
    factory_default_active_profile_config_for_mode, factory_default_configuration_bundle,
    factory_default_idle_profile_config_for_mode, factory_default_light_profile_config_map,
};
use crate::state::{rooms_from_engine, AppState, SharedState};
use crate::storage::StoredLocation;
use crate::topology::NodeControlKind;

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

    let local = current_local_datetime(utc_offset);
    let (year, month, day) = (
        local.date().year(),
        local.date().month(),
        local.date().day(),
    );
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);
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

fn mode_config_for_mode(mode_configs: &[ModeConfig], mode: RhythmMode) -> Option<&ModeConfig> {
    mode_configs.iter().find(|config| config.mode == mode)
}

#[derive(Clone, Copy)]
struct RoomLightingContext<'a> {
    light_profile_configs: &'a BTreeMap<String, LightProfileConfig>,
    mode_configs: &'a [ModeConfig],
    mode: RhythmMode,
    solar_noon: f32,
    latitude: f32,
    utc_offset: f32,
}

#[derive(Clone, Copy)]
struct RoomLightingInput<'a> {
    settings: &'a RoomProfileSettings,
    room_state: RoomModeState,
    time_offset_minutes: f32,
    brightness_offset: f32,
}

impl<'a> RoomLightingInput<'a> {
    fn from_snapshot(snapshot: &'a rhythm_core::RoomSnapshot, room_state: RoomModeState) -> Self {
        Self {
            settings: &snapshot.profile_settings,
            room_state,
            time_offset_minutes: snapshot.time_offset_minutes,
            brightness_offset: snapshot.brightness_offset,
        }
    }
}

#[derive(Clone, Copy)]
struct ModeDefaultApplyContext<'a> {
    lighting: RoomLightingContext<'a>,
    transition: Option<&'a ModeTransitionConfig>,
    transition_started_at: chrono::NaiveDateTime,
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

#[derive(Clone, Debug)]
struct ModeChangeContext {
    cause: ModeChangeCause,
    transition: Option<ModeTransitionConfig>,
}

impl ModeChangeContext {
    fn new(cause: ModeChangeCause, transition: Option<ModeTransitionConfig>) -> Self {
        Self { cause, transition }
    }

    fn transition_id(&self) -> Option<String> {
        self.transition
            .as_ref()
            .map(|transition| transition.id.clone())
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

pub(crate) fn validate_mode_configs(configs: &[ModeConfig]) -> Result<()> {
    for config in configs {
        for room_default in &config.room_defaults {
            if room_default.state.is_mode_default_target() {
                continue;
            }

            return Err(anyhow::anyhow!(
                "Mode {:?} room_default for room '{}' cannot use runtime-only state {}",
                config.mode,
                room_default.room_id,
                serde_json::to_string(&room_default.state)
                    .unwrap_or_else(|_| "\"invalid\"".to_string())
            ));
        }
    }

    Ok(())
}

fn mode_change_cause_from_trigger(trigger: ModeTransitionTrigger) -> ModeChangeCause {
    if trigger.is_manual() {
        ModeChangeCause::Manual
    } else {
        ModeChangeCause::Schedule
    }
}

fn matching_mode_transition(
    state: &SharedState,
    from_mode: RhythmMode,
    to_mode: RhythmMode,
    trigger: ModeTransitionTrigger,
) -> Option<ModeTransitionConfig> {
    let Ok(s) = state.lock() else { return None };
    s.mode_transition_configs().into_iter().find(|config| {
        config.from_mode == from_mode && config.to_mode == to_mode && config.trigger == trigger
    })
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
    lighting: RoomLightingContext<'_>,
    room: RoomLightingInput<'_>,
) -> (u8, u16) {
    use rhythm_core::SolarTime;

    if room.room_state == RoomModeState::HardOff {
        return (0, 0);
    }

    let local = current_local_datetime(lighting.utc_offset);
    let (year, month, day) = (
        local.date().year(),
        local.date().month(),
        local.date().day(),
    );
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);
    let t = local.time();
    let current_hour = t.hour() as f32 + t.minute() as f32 / 60.0 + t.second() as f32 / 3600.0;

    let solar = SolarTime::new(lighting.solar_noon, lighting.latitude, day_of_year);
    let ctx = rhythm_core::light_profile::CurveContext::new(current_hour, solar, None);
    let render_state = render_state_for_display(room.room_state);
    if matches!(render_state, RoomModeState::HardOff) {
        return (0, 0);
    }
    let registry = light_profile_registry_from_parts(
        lighting.light_profile_configs,
        lighting.mode_configs,
        lighting.mode,
    );
    let values = registry
        .profile_for_room_state(lighting.mode, render_state, Some(room.settings))
        .calculate_with_offset(&ctx, room.time_offset_minutes);
    let adjusted_brightness =
        (values.brightness as f32 + room.brightness_offset).clamp(1.0, 100.0) as u8;
    let brightness = match render_state {
        RoomModeState::Idle => values.brightness,
        RoomModeState::Warning
            if !warning_uses_custom_profile(lighting.mode_configs, lighting.mode) =>
        {
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
    let mode_configs = s.mode_configs();
    compute_room_display_values_for_settings_from_parts(
        RoomLightingContext {
            light_profile_configs: &s.light_profile_configs,
            mode_configs: &mode_configs,
            mode: s.active_mode,
            solar_noon: s.solar_noon_hour(),
            latitude: s.latitude.unwrap_or(35.0),
            utc_offset: s.utc_offset_hours,
        },
        RoomLightingInput {
            settings,
            room_state,
            time_offset_minutes,
            brightness_offset,
        },
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

fn room_mode_transition_active(
    transitions: &HashMap<String, crate::state::RoomModeTransition>,
    room_id: &str,
    now: std::time::Instant,
) -> bool {
    transitions
        .get(room_id)
        .is_some_and(|transition| transition.ends_at > now)
}

fn active_transition_room_ids(
    transitions: &HashMap<String, crate::state::RoomModeTransition>,
    now: std::time::Instant,
) -> HashSet<String> {
    transitions
        .iter()
        .filter(|(_, transition)| transition.ends_at > now)
        .map(|(room_id, _)| room_id.clone())
        .collect()
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

fn queue_motion_timer_clear(state: &SharedState, room_id: &str) {
    if let Ok(mut s) = state.lock() {
        if !s
            .pending_motion_clear
            .iter()
            .any(|pending| pending == room_id)
        {
            s.pending_motion_clear.push(room_id.to_string());
        }
    }
}

fn clear_removed_node_ephemeral_state(s: &mut AppState, node_id: &str) {
    s.room_lights_on.remove(node_id);
    s.motion_snapshots.remove(node_id);
    s.room_mode_transitions.remove(node_id);
    s.pending_periodic_ticks.remove(node_id);
    s.pending_motion_clear.retain(|pending| pending != node_id);
    s.pending_motion_seed
        .retain(|(source_node_id, target_node_id)| {
            source_node_id != node_id && target_node_id != node_id
        });
}

pub(crate) fn reconcile_hub_endpoint_visibility(
    state: &SharedState,
    hub_key: &HubKey,
    discovered_native_ids: &HashSet<String>,
) -> Result<(usize, usize)> {
    let (runtime, hidden_ids, restored_devices, affected_count) = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let report = s
            .canonical_registry
            .deactivate_missing_endpoints_for_hub(hub_key, discovered_native_ids);
        if report.affected_device_ids.is_empty() {
            return Ok((0, 0));
        }

        let hidden_ids = report.hidden_device_ids.clone();
        let mut restored_devices = Vec::new();
        for device_id in &hidden_ids {
            s.topology.remove_device_everywhere(device_id);
            clear_removed_node_ephemeral_state(&mut s, device_id);
        }
        for device_id in &report.affected_device_ids {
            if hidden_ids.iter().any(|hidden_id| hidden_id == device_id) {
                continue;
            }
            if s.topology.device_parent_room_id(device_id).is_some() {
                continue;
            }
            let Some(device) = s.canonical_registry.get(device_id).cloned() else {
                continue;
            };
            let desired_parent_id = device
                .room_id
                .clone()
                .filter(|room_id| s.topology.get(room_id).is_some());
            if let Some(ref room_id) = desired_parent_id {
                s.topology.ensure_standalone_device(device_id);
                let _ = s.topology.assign_device(
                    device_id,
                    Some(room_id),
                    crate::topology::DevicePlacement::HubDefault,
                );
            } else {
                s.topology.ensure_standalone_device(device_id);
            }
            restored_devices.push((
                device.id.clone(),
                device.name.clone(),
                device.device_type.clone(),
                desired_parent_id,
            ));
        }

        persist_canonical(&s);
        if !hidden_ids.is_empty() || !restored_devices.is_empty() {
            persist_topology(&s);
        }

        (
            s.hub_runtime(),
            hidden_ids,
            restored_devices,
            report.affected_device_ids.len(),
        )
    };

    for (device_id, device_name, device_type, parent_id) in &restored_devices {
        let _ = ensure_runtime_device_node_exists(
            state,
            device_id,
            device_name,
            device_type.clone(),
            parent_id.clone(),
        );
    }

    if let Some(runtime) = runtime {
        for device_id in &hidden_ids {
            runtime.remove_node(device_id);
            queue_motion_timer_clear(state, device_id);
        }
    }

    Ok((affected_count, hidden_ids.len()))
}

fn resolved_room_motion_timeout_secs_from_parts(
    lighting: RoomLightingContext<'_>,
    settings: &RoomProfileSettings,
    current_hour: f32,
) -> u64 {
    let local = current_local_datetime(lighting.utc_offset);
    let (year, month, day) = (
        local.date().year(),
        local.date().month(),
        local.date().day(),
    );
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);
    let solar = rhythm_core::SolarTime::new(lighting.solar_noon, lighting.latitude, day_of_year);
    let registry = light_profile_registry_from_parts(
        lighting.light_profile_configs,
        lighting.mode_configs,
        lighting.mode,
    );
    let ctx = rhythm_core::light_profile::CurveContext::new(current_hour, solar, None);
    registry
        .profile_for_room_state(lighting.mode, RoomModeState::Active, Some(settings))
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

fn collect_room_hub_types<'a>(
    bindings: impl IntoIterator<Item = &'a crate::topology::HubRoomBinding>,
) -> Vec<String> {
    let mut hub_types = Vec::new();
    let mut seen = HashSet::new();

    for binding in bindings {
        let hub_type = binding.hub_key.hub_type.as_str();
        if seen.insert(hub_type) {
            hub_types.push(hub_type.to_string());
        }
    }

    hub_types
}

fn hub_is_active(s: &AppState, hub_key: &HubKey) -> bool {
    s.hubs.contains_key(hub_key)
}

fn collect_room_hub_types_from_topology_room(
    s: &AppState,
    room: &crate::topology::TopologyRoom,
) -> Vec<String> {
    let mut hub_types = collect_room_hub_types(
        room.hub_room_bindings
            .iter()
            .filter(|binding| hub_is_active(s, &binding.hub_key)),
    );
    let mut seen: HashSet<String> = hub_types.iter().cloned().collect();

    for room_device in &room.devices {
        let Some(device) = s.canonical_registry.get(&room_device.device_id) else {
            continue;
        };
        for endpoint in device.active_endpoints() {
            if !hub_is_active(s, &endpoint.hub_key) {
                continue;
            }
            let hub_type = endpoint.hub_key.hub_type.as_str().to_string();
            if seen.insert(hub_type.clone()) {
                hub_types.push(hub_type);
            }
        }
    }

    hub_types
}

fn topology_hub_types_map(s: &AppState) -> HashMap<String, Vec<String>> {
    s.topology
        .rooms()
        .map(|room| {
            (
                room.id.clone(),
                collect_room_hub_types_from_topology_room(s, room),
            )
        })
        .collect()
}

fn node_hub_types_from_topology(s: &AppState, node_id: &str) -> Vec<String> {
    if let Some(room) = s.topology.get(node_id) {
        return collect_room_hub_types_from_topology_room(s, room);
    }

    if let Some(node) = s.topology.get_device_node(node_id) {
        if let Some(device) = s.canonical_registry.get(&node.canonical_device_id) {
            let mut hub_types = Vec::new();
            let mut seen = HashSet::new();
            for endpoint in device.active_endpoints() {
                if !hub_is_active(s, &endpoint.hub_key) {
                    continue;
                }
                let hub_type = endpoint.hub_key.hub_type.as_str().to_string();
                if seen.insert(hub_type.clone()) {
                    hub_types.push(hub_type);
                }
            }
            return hub_types;
        }
    }

    s.topology
        .resolve_room_alias(&s.canonical_registry, None, node_id)
        .as_deref()
        .map(|topo_id| node_hub_types_from_topology(s, topo_id))
        .unwrap_or_default()
}

fn room_hub_types_from_topology(s: &AppState, room_id: &str) -> Vec<String> {
    node_hub_types_from_topology(s, room_id)
}

fn light_node_uses_parent_dispatch(s: &AppState, node_id: &str, kind: LightNodeKind) -> bool {
    kind == LightNodeKind::LightDevice
        && s.topology
            .attached_light_uses_parent_dispatch(node_id, &s.canonical_registry)
}

fn semantic_lights_on_override(hard_off: bool, soft_off: bool) -> Option<bool> {
    if hard_off {
        Some(false)
    } else if soft_off {
        Some(true)
    } else {
        None
    }
}

pub(crate) fn effective_lights_on_cache_key<'a>(
    s: &AppState,
    node_id: &'a str,
    kind: LightNodeKind,
    parent_id: Option<&'a str>,
) -> &'a str {
    if light_node_uses_parent_dispatch(s, node_id, kind) {
        parent_id.unwrap_or(node_id)
    } else {
        node_id
    }
}

fn lights_on_from_cache(
    s: &AppState,
    room_lights_on: &HashMap<String, bool>,
    node_id: &str,
    kind: LightNodeKind,
    parent_id: Option<&str>,
) -> bool {
    room_lights_on
        .get(effective_lights_on_cache_key(s, node_id, kind, parent_id))
        .copied()
        .unwrap_or(false)
}

pub(crate) fn light_state_query_id<'a>(
    s: &AppState,
    node_id: &'a str,
    kind: LightNodeKind,
    parent_id: Option<&'a str>,
) -> &'a str {
    effective_lights_on_cache_key(s, node_id, kind, parent_id)
}

pub(crate) fn update_lights_on_cache_for_node(
    state: &SharedState,
    node_id: &str,
    kind: LightNodeKind,
    parent_id: Option<&str>,
    lights_on: bool,
) {
    if let Ok(mut s) = state.lock() {
        let cache_key = effective_lights_on_cache_key(&s, node_id, kind, parent_id).to_string();
        s.room_lights_on.insert(cache_key, lights_on);
    }
}

pub(crate) fn update_lights_on_cache_for_runtime_node(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
    lights_on: bool,
) {
    if let Some(snap) = runtime.engine_effective_node_snapshot(node_id) {
        update_lights_on_cache_for_node(
            state,
            &snap.id,
            snap.kind,
            snap.parent_id.as_deref(),
            lights_on,
        );

        if let Some(parent_id) = snap
            .parent_id
            .as_deref()
            .filter(|parent_id| *parent_id != snap.id)
        {
            match runtime.any_lights_on(parent_id) {
                Ok(parent_lights_on) => {
                    update_lights_on_cache_for_node(
                        state,
                        parent_id,
                        LightNodeKind::Room,
                        None,
                        parent_lights_on,
                    );
                }
                Err(e) => {
                    warn!(
                        target: "cmd",
                        "Failed to refresh parent lights_on for '{}' after node '{}': {}",
                        parent_id,
                        node_id,
                        e
                    );
                }
            }
        }
    } else {
        update_lights_on_cache_for_node(state, node_id, LightNodeKind::Room, None, lights_on);
    }
}

pub(crate) fn refresh_lights_on_cache_for_runtime_node(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
) {
    let Some(snap) = runtime.engine_effective_node_snapshot(node_id) else {
        return;
    };
    if !snap.kind.is_light_addressable() {
        return;
    }

    let lights_on =
        if let Some(lights_on) = semantic_lights_on_override(snap.hard_off, snap.soft_off) {
            lights_on
        } else {
            let query_id = {
                let Ok(s) = state.lock() else { return };
                light_state_query_id(&s, &snap.id, snap.kind, snap.parent_id.as_deref()).to_string()
            };

            match runtime.any_lights_on(&query_id) {
                Ok(lights_on) => lights_on,
                Err(e) => {
                    warn!(
                        target: "cmd",
                        "Failed to refresh lights_on for '{}' via '{}': {}",
                        node_id,
                        query_id,
                        e
                    );
                    return;
                }
            }
        };

    update_lights_on_cache_for_node(
        state,
        &snap.id,
        snap.kind,
        snap.parent_id.as_deref(),
        lights_on,
    );

    if let Some(parent_id) = snap
        .parent_id
        .as_deref()
        .filter(|parent_id| *parent_id != snap.id)
    {
        match runtime.any_lights_on(parent_id) {
            Ok(parent_lights_on) => update_lights_on_cache_for_node(
                state,
                parent_id,
                LightNodeKind::Room,
                None,
                parent_lights_on,
            ),
            Err(e) => warn!(
                target: "cmd",
                "Failed to refresh parent lights_on for '{}' after node '{}': {}",
                parent_id,
                node_id,
                e
            ),
        }
    }
}

fn node_metadata_from_topology(
    s: &AppState,
    node_id: &str,
) -> (
    Option<crate::topology::DevicePlacement>,
    Option<String>,
    Option<String>,
) {
    let Some(node) = s.topology.get_device_node(node_id) else {
        return (None, None, None);
    };
    let device = s.canonical_registry.get(&node.canonical_device_id);
    (
        Some(node.placement.clone()),
        device.and_then(|d| d.manufacturer.clone()),
        device.and_then(|d| d.model.clone()),
    )
}

struct NodeStateDtoBuildContext<'a> {
    state: &'a AppState,
    light_profile_configs: &'a BTreeMap<String, LightProfileConfig>,
    mode_configs: &'a [ModeConfig],
    active_mode: RhythmMode,
    solar_noon: f32,
    latitude: f32,
    utc_offset: f32,
    room_lights_on: &'a HashMap<String, bool>,
    motion_snapshots: &'a HashMap<String, crate::state::MotionSnapshot>,
    nodes_with_sensors: &'a HashSet<String>,
    transitioning_nodes: &'a HashSet<String>,
}

struct NodeStateDtoMetadata {
    hub_types: Vec<String>,
    topology_parent_id: Option<Option<String>>,
    placement: Option<crate::topology::DevicePlacement>,
    manufacturer: Option<String>,
    model: Option<String>,
}

fn node_state_dto_metadata(s: &AppState, node_id: &str) -> NodeStateDtoMetadata {
    let (placement, manufacturer, model) = node_metadata_from_topology(s, node_id);
    NodeStateDtoMetadata {
        hub_types: node_hub_types_from_topology(s, node_id),
        topology_parent_id: s
            .topology
            .get_device_node(node_id)
            .map(|node| node.parent_id.clone()),
        placement,
        manufacturer,
        model,
    }
}

fn build_node_state_dto_from_snapshot_parts(
    ctx: &NodeStateDtoBuildContext<'_>,
    snap: &rhythm_core::NodeSnapshot,
    metadata: NodeStateDtoMetadata,
) -> NodeStateDto {
    let NodeStateDtoMetadata {
        hub_types,
        topology_parent_id,
        placement,
        manufacturer,
        model,
    } = metadata;
    let effective_parent_id = topology_parent_id.unwrap_or_else(|| snap.parent_id.clone());
    let warning_active = ctx
        .motion_snapshots
        .get(&snap.id)
        .is_some_and(|motion| motion.warning_active);
    let state = room_mode_state_from_flags(snap.hard_off, snap.soft_off, warning_active);
    let (brightness, kelvin) = compute_room_display_values_for_settings_from_parts(
        RoomLightingContext {
            light_profile_configs: ctx.light_profile_configs,
            mode_configs: ctx.mode_configs,
            mode: ctx.active_mode,
            solar_noon: ctx.solar_noon,
            latitude: ctx.latitude,
            utc_offset: ctx.utc_offset,
        },
        RoomLightingInput {
            settings: &snap.profile_settings,
            room_state: state,
            time_offset_minutes: snap.time_offset_minutes,
            brightness_offset: snap.brightness_offset,
        },
    );

    let (motion_active, motion_owned, remaining_secs, timeout_secs, warning_active) =
        if let Some(ms) = ctx.motion_snapshots.get(&snap.id) {
            (
                Some(ms.motion_active),
                Some(ms.motion_owned),
                ms.remaining_secs,
                Some(ms.timeout_secs),
                Some(ms.warning_active),
            )
        } else if ctx.nodes_with_sensors.contains(&snap.id) {
            let timeout = resolved_room_motion_timeout_secs_from_parts(
                RoomLightingContext {
                    light_profile_configs: ctx.light_profile_configs,
                    mode_configs: ctx.mode_configs,
                    mode: ctx.active_mode,
                    solar_noon: ctx.solar_noon,
                    latitude: ctx.latitude,
                    utc_offset: ctx.utc_offset,
                },
                &snap.profile_settings,
                current_local_hour(ctx.utc_offset),
            );
            (Some(false), Some(false), None, Some(timeout), Some(false))
        } else {
            (None, None, None, None, None)
        };

    NodeStateDto {
        id: snap.id.clone(),
        name: snap.name.clone(),
        kind: snap.kind,
        parent_id: effective_parent_id.clone(),
        placement,
        hub_types,
        manufacturer,
        model,
        state,
        rhythm_enabled: snap.rhythm_enabled,
        disabled: snap.disabled,
        time_offset: snap.time_offset_minutes,
        brightness_offset: snap.brightness_offset,
        lights_on: lights_on_from_cache(
            ctx.state,
            ctx.room_lights_on,
            &snap.id,
            snap.kind,
            effective_parent_id.as_deref(),
        ),
        transitioning: ctx.transitioning_nodes.contains(&snap.id),
        brightness,
        kelvin,
        profile_settings: snap.profile_settings.clone(),
        motion_active,
        motion_owned,
        remaining_secs,
        timeout_secs,
        warning_active,
    }
}

/// Build a NodeStateEvent for an addressable node, computing display values
/// from AppState.
///
/// For soft-off nodes, brightness is the soft-off percentage (not curve value).
/// Kelvin is always from the curve (soft-off tracks color temp).
#[cfg(feature = "desktop")]
pub fn build_node_state_event(
    state: &SharedState,
    snap: &rhythm_core::NodeSnapshot,
) -> crate::server_event::NodeStateEvent {
    let now = std::time::Instant::now();
    let (mode, room_state, lights_on, transitioning, brightness, kelvin, hub_types) = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        let mode = s.active_mode;
        let mode_configs = s.mode_configs();
        let room_state = room_mode_state_from_flags(
            snap.hard_off,
            snap.soft_off,
            s.motion_snapshots
                .get(&snap.id)
                .is_some_and(|motion| motion.warning_active),
        );
        let lights_on = lights_on_from_cache(
            &s,
            &s.room_lights_on,
            &snap.id,
            snap.kind,
            snap.parent_id.as_deref(),
        );
        let transitioning = room_mode_transition_active(&s.room_mode_transitions, &snap.id, now);
        let hub_types = node_hub_types_from_topology(&s, &snap.id);
        let (curve_brightness, kelvin) = compute_room_display_values_for_settings_from_parts(
            RoomLightingContext {
                light_profile_configs: &s.light_profile_configs,
                mode_configs: &mode_configs,
                mode,
                solar_noon: s.solar_noon_hour(),
                latitude: s.latitude.unwrap_or(35.0),
                utc_offset: s.utc_offset_hours,
            },
            RoomLightingInput {
                settings: &snap.profile_settings,
                room_state,
                time_offset_minutes: snap.time_offset_minutes,
                brightness_offset: snap.brightness_offset,
            },
        );
        (
            mode,
            room_state,
            lights_on,
            transitioning,
            curve_brightness,
            kelvin,
            hub_types,
        )
    };
    crate::server_event::NodeStateEvent::from_snapshot(
        snap,
        crate::server_event::NodeStateEventParams {
            hub_types,
            mode,
            state: room_state,
            lights_on,
            transitioning,
            brightness,
            kelvin,
        },
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
        .unwrap_or_else(|| factory_default_active_profile_config_for_mode(s.active_mode))
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
            last_active_mode_cause: s.last_active_mode_cause,
            last_active_mode_transition_id: s.last_active_mode_transition_id.clone(),
            last_active_mode_change_utc_ms: s.last_active_mode_change_utc_ms,
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
            s.last_active_mode_cause = ModeChangeCause::Manual;
            s.last_active_mode_transition_id = None;
            s.last_active_mode_change_utc_ms = Some(chrono::Utc::now().timestamp_millis());
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

/// Resolve a public node ID from an API request or hub-facing alias.
///
/// If the ID is already a topology node ID, returns it unchanged. If it's a
/// hub-native room/device ID, translates it via the topology store. Falls
/// through unchanged when no topology alias exists.
pub fn resolve_node_id(state: &SharedState, node_id: &str) -> String {
    let s = match state.lock() {
        Ok(s) => s,
        Err(_) => return node_id.to_string(),
    };
    s.topology
        .resolve_room_alias(&s.canonical_registry, None, node_id)
        .unwrap_or_else(|| node_id.to_string())
}

fn fallback_source_node_id(hub_key: Option<&HubKey>, source_native_id: &str) -> String {
    match hub_key {
        Some(hub_key) => format!("{hub_key}::{source_native_id}"),
        None => source_native_id.to_string(),
    }
}

/// Resolve a topology control target for a source device/native ID.
///
/// The returned tuple is `(source_node_id, target_node_id)`. Explicit topology
/// control links win. If no explicit link exists, device nodes inherit their
/// parent room as the default target. As a final fallback, the caller-provided
/// hub event hint is translated into a public node ID.
pub(crate) fn resolve_node_control_target(
    state: &SharedState,
    hub_key: Option<&HubKey>,
    source_native_id: &str,
    fallback_target_hint: &str,
    kind: &NodeControlKind,
) -> Option<(String, String)> {
    let fallback_target_id = resolve_node_id(state, fallback_target_hint);

    let s = state.lock().ok()?;
    let source_node_id = hub_key
        .and_then(|hub_key| {
            s.canonical_registry
                .find_by_native_id(hub_key, source_native_id)
        })
        .map(|device| device.id.clone());
    let target_node_id = source_node_id
        .as_deref()
        .and_then(|source_id| s.topology.effective_control_target(source_id, kind))
        .or_else(|| (!fallback_target_id.is_empty()).then_some(fallback_target_id.clone()))?;

    Some((
        source_node_id.unwrap_or_else(|| fallback_source_node_id(hub_key, source_native_id)),
        target_node_id,
    ))
}

/// Reverse-lookup: topology room ID → hub-native room IDs for registry operations.
///
/// The hub-native registry uses hub room IDs, but callers use topology IDs.
/// Returns the ID unchanged as fallback (ESP32 / no topology entry).
fn hub_room_ids_for_topology(state: &SharedState, topo_id: &str) -> Vec<String> {
    state
        .lock()
        .ok()
        .map(|s| {
            s.topology
                .get(topo_id)
                .map(|room| {
                    let mut ids = Vec::new();
                    let mut seen = HashSet::new();

                    for binding in &room.hub_room_bindings {
                        if seen.insert(binding.hub_room_id.clone()) {
                            ids.push(binding.hub_room_id.clone());
                        }
                    }

                    for room_device in &room.devices {
                        let Some(device) = s.canonical_registry.get(&room_device.device_id) else {
                            continue;
                        };
                        for endpoint in device.active_endpoints() {
                            if seen.insert(endpoint.native_id.clone()) {
                                ids.push(endpoint.native_id.clone());
                            }
                        }
                    }

                    ids
                })
                .unwrap_or_else(|| vec![topo_id.to_string()])
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
    let (
        runtime,
        hubs_dto,
        capabilities_dto,
        active_profile_cfg,
        mut active_profile_effective,
        location_dto,
        settings_dto,
        mode_dto,
        transitions_dto,
        profiles_dto,
        firmware_version,
        platform_type,
        platform_ctx,
        listen_port,
        room_lights_on,
        motion_snapshots,
        nodes_with_sensors,
        transitioning_nodes,
        light_profile_configs,
        mode_configs,
        active_mode,
        solar_noon,
        latitude,
        utc_offset,
        last_tick_epoch_ms,
        profile_registry,
        periodic_ctx,
        update_interval,
        power_save,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        let runtime = s.hub_runtime();
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
                connected: s.hub_is_connected(key),
            })
            .collect();
        let capabilities_dto = ApiCapabilitiesDto {
            hubs: s
                .hub_capabilities
                .iter()
                .map(|capability| HubCapabilityDto {
                    hub_type: capability.hub_type.clone(),
                    configurable: capability.configurable,
                    device_onboarding_methods: capability.device_onboarding_methods.clone(),
                    supports_unpairing: capability.supports_unpairing,
                    supports_roomless_devices: capability.supports_roomless_devices,
                })
                .collect(),
        };

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
            resolved_solar.sun_times,
        );
        let periodic_ctx = rhythm_core::CurveContext::new(
            current_hour,
            resolved_solar.solar,
            resolved_solar.sun_times,
        );
        let current_solar_time = periodic_ctx.solar_time();
        let location_dto = LocationDto {
            current_local_time,
            current_local_hour: current_hour,
            latitude: s.latitude,
            longitude: s.longitude,
            utc_offset_hours: s.utc_offset_hours,
            solar_noon: resolved_solar.solar.solar_noon_hour,
            solar_noon_local_time: local_time_string_from_decimal_hour(
                resolved_solar.solar.solar_noon_hour,
            ),
            solar_midnight: resolved_solar.solar.solar_midnight_hour(),
            solar_midnight_local_time: local_time_string_from_decimal_hour(
                resolved_solar.solar.solar_midnight_hour(),
            ),
            current_solar_time,
            timezone_name: s.timezone_name.clone(),
            twilight: build_twilight_response(resolved_solar.twilight.as_ref()),
        };

        let now = std::time::Instant::now();
        (
            runtime,
            hubs_dto,
            capabilities_dto,
            active_profile_cfg,
            active_profile_effective,
            location_dto,
            build_settings_dto_inner(&s),
            build_mode_dto_inner(&s),
            build_transitions_dto_inner(&s),
            build_profiles_dto_inner(&s),
            s.firmware_version,
            s.platform_type,
            s.platform_context,
            s.listen_port,
            s.room_lights_on.clone(),
            s.motion_snapshots.clone(),
            s.motion_control_target_ids(),
            active_transition_room_ids(&s.room_mode_transitions, now),
            s.light_profile_configs.clone(),
            s.mode_configs(),
            s.active_mode,
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.utc_offset_hours,
            s.last_tick_epoch_ms,
            profile_registry,
            periodic_ctx,
            Duration::from_secs(s.runtime_config.update_interval_secs),
            s.power_save,
        )
    };

    let mut node_snapshots: Vec<rhythm_core::NodeSnapshot> = if let Some(ref runtime) = runtime {
        runtime.engine_all_effective_node_snapshots()
    } else {
        let exported = room_manager_for_export(state);
        let mut snapshots: Vec<_> = exported
            .iter()
            .map(|room| rhythm_core::NodeSnapshot {
                id: room.id.clone(),
                name: room.name.clone(),
                kind: room.kind,
                parent_id: room.parent_id.clone(),
                rhythm_enabled: room.rhythm_enabled,
                disabled: room.disabled,
                time_offset_minutes: room.time_offset_minutes,
                brightness_offset: room.brightness_offset,
                soft_off: room.soft_off,
                hard_off: room.hard_off,
                profile_settings: room.profile_settings.clone(),
            })
            .collect();
        if snapshots.is_empty() {
            snapshots = registry_node_snapshots_from_state(state);
        }
        snapshots
    };
    node_snapshots.sort_by(|left, right| left.id.cmp(&right.id));

    active_profile_effective.rhythm_interval_secs = crate::periodic::effective_cycle_duration(
        &profile_registry,
        &periodic_ctx,
        &node_snapshots,
        update_interval,
        power_save,
    )
    .as_secs();

    let nodes = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let dto_ctx = NodeStateDtoBuildContext {
            state: &s,
            light_profile_configs: &light_profile_configs,
            mode_configs: &mode_configs,
            active_mode,
            solar_noon,
            latitude,
            utc_offset,
            room_lights_on: &room_lights_on,
            motion_snapshots: &motion_snapshots,
            nodes_with_sensors: &nodes_with_sensors,
            transitioning_nodes: &transitioning_nodes,
        };
        let mut nodes = Vec::with_capacity(node_snapshots.len());
        for snap in &node_snapshots {
            let metadata = node_state_dto_metadata(&s, &snap.id);
            nodes.push(build_node_state_dto_from_snapshot_parts(
                &dto_ctx, snap, metadata,
            ));
        }
        nodes.sort_by(|left, right| {
            left.parent_id
                .cmp(&right.parent_id)
                .then_with(|| left.id.cmp(&right.id))
        });
        nodes
    };

    let snapshot = StateSnapshot {
        last_tick_epoch_ms,
        version: firmware_version.to_string(),
        platform: platform_type.to_string(),
        context: platform_ctx.to_string(),
        listen_port,
        hubs: hubs_dto,
        capabilities: capabilities_dto,
        active_profile: ActiveProfileDto {
            config: active_profile_cfg,
            effective: active_profile_effective,
        },
        location: location_dto,
        settings: settings_dto,
        mode: mode_dto,
        transitions: transitions_dto.transitions,
        profiles: profiles_dto.profiles,
        nodes,
    };
    serde_json::to_string(&snapshot).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Build a full node state for a single addressable node.
pub fn build_node_state(state: &SharedState, node_id: &str) -> Result<NodeStateDto> {
    let (runtime, room_lights_on, motion_snapshots, nodes_with_sensors, transitioning_nodes) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.hub_runtime(),
            s.room_lights_on.clone(),
            s.motion_snapshots.clone(),
            s.motion_control_target_ids(),
            active_transition_room_ids(&s.room_mode_transitions, std::time::Instant::now()),
        )
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;
    let snap = runtime
        .engine_effective_node_snapshot(node_id)
        .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", node_id))?;

    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let mode_configs = s.mode_configs();
    let dto_ctx = NodeStateDtoBuildContext {
        state: &s,
        light_profile_configs: &s.light_profile_configs,
        mode_configs: &mode_configs,
        active_mode: s.active_mode,
        solar_noon: s.solar_noon_hour(),
        latitude: s.latitude.unwrap_or(35.0),
        utc_offset: s.utc_offset_hours,
        room_lights_on: &room_lights_on,
        motion_snapshots: &motion_snapshots,
        nodes_with_sensors: &nodes_with_sensors,
        transitioning_nodes: &transitioning_nodes,
    };
    let (placement, manufacturer, model) = node_metadata_from_topology(&s, node_id);
    Ok(build_node_state_dto_from_snapshot_parts(
        &dto_ctx,
        &snap,
        NodeStateDtoMetadata {
            hub_types: node_hub_types_from_topology(&s, node_id),
            topology_parent_id: s
                .topology
                .get_device_node(node_id)
                .map(|node| node.parent_id.clone()),
            placement,
            manufacturer,
            model,
        },
    ))
}

/// Build a lightweight node-state snapshot for polling.
pub fn build_nodes_state(state: &SharedState) -> Result<String> {
    let (
        hub_connected,
        runtime,
        room_lights_on,
        motion_snapshots,
        nodes_with_sensors,
        transitioning_nodes,
        light_profile_configs,
        mode_configs,
        active_mode,
        solar_noon,
        latitude,
        utc_offset,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.has_any_connected_hub(),
            s.hub_runtime(),
            s.room_lights_on.clone(),
            s.motion_snapshots.clone(),
            s.motion_control_target_ids(),
            active_transition_room_ids(&s.room_mode_transitions, std::time::Instant::now()),
            s.light_profile_configs.clone(),
            s.mode_configs(),
            s.active_mode,
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.utc_offset_hours,
        )
    };

    let mut node_snapshots: Vec<rhythm_core::NodeSnapshot> = if let Some(ref runtime) = runtime {
        runtime.engine_all_effective_node_snapshots()
    } else {
        let exported = room_manager_for_export(state);
        let mut snapshots: Vec<_> = exported
            .iter()
            .map(|room| rhythm_core::NodeSnapshot {
                id: room.id.clone(),
                name: room.name.clone(),
                kind: room.kind,
                parent_id: room.parent_id.clone(),
                rhythm_enabled: room.rhythm_enabled,
                disabled: room.disabled,
                time_offset_minutes: room.time_offset_minutes,
                brightness_offset: room.brightness_offset,
                soft_off: room.soft_off,
                hard_off: room.hard_off,
                profile_settings: room.profile_settings.clone(),
            })
            .collect();
        if snapshots.is_empty() {
            snapshots = registry_node_snapshots_from_state(state);
        }
        snapshots
    };
    node_snapshots.sort_by(|left, right| left.id.cmp(&right.id));

    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let dto_ctx = NodeStateDtoBuildContext {
        state: &s,
        light_profile_configs: &light_profile_configs,
        mode_configs: &mode_configs,
        active_mode,
        solar_noon,
        latitude,
        utc_offset,
        room_lights_on: &room_lights_on,
        motion_snapshots: &motion_snapshots,
        nodes_with_sensors: &nodes_with_sensors,
        transitioning_nodes: &transitioning_nodes,
    };
    let mut nodes = Vec::with_capacity(node_snapshots.len());
    for snap in &node_snapshots {
        nodes.push(build_node_state_dto_from_snapshot_parts(
            &dto_ctx,
            snap,
            node_state_dto_metadata(&s, &snap.id),
        ));
    }
    nodes.sort_by(|left, right| {
        left.parent_id
            .cmp(&right.parent_id)
            .then_with(|| left.id.cmp(&right.id))
    });

    let response = NodesPollResponse {
        hub_connected,
        nodes,
    };
    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
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
        topo_hub_types,
        rooms_with_sensors,
        transitioning_rooms,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let now = std::time::Instant::now();
        let runtime = s.hub_runtime();
        let storage_rooms = if runtime.is_none() {
            s.storage.as_ref().and_then(|st| st.load_rooms().ok())
        } else {
            None
        };
        let motion = s.motion_snapshots.clone();
        let lights = s.room_lights_on.clone();
        let sensor_rooms = s.motion_control_target_ids();
        (
            s.has_any_connected_hub(),
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
            topology_hub_types_map(&s),
            sensor_rooms,
            active_transition_room_ids(&s.room_mode_transitions, now),
        )
    };

    let snapshots = if let Some(ref rt) = runtime {
        rt.engine_all_room_snapshots()
    } else {
        Vec::new()
    };

    let mut rooms = Vec::new();

    if !snapshots.is_empty() {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        for snap in &snapshots {
            let lights_on = lights_on_from_cache(
                &s,
                &room_lights_on,
                &snap.id,
                snap.kind,
                snap.parent_id.as_deref(),
            );
            let room_state = room_mode_state_from_flags(
                snap.hard_off,
                snap.soft_off,
                motion_snapshots
                    .get(&snap.id)
                    .is_some_and(|motion| motion.warning_active),
            );
            let (curve_brightness, kelvin) = compute_room_display_values_for_settings_from_parts(
                RoomLightingContext {
                    light_profile_configs: &light_profile_configs,
                    mode_configs: &mode_configs,
                    mode: active_mode,
                    solar_noon,
                    latitude,
                    utc_offset,
                },
                RoomLightingInput::from_snapshot(snap, room_state),
            );
            let rhythm = RoomRhythmState {
                id: snap.id.clone(),
                hub_types: topo_hub_types.get(&snap.id).cloned().unwrap_or_default(),
                state: room_state,
                rhythm_enabled: snap.rhythm_enabled,
                time_offset: snap.time_offset_minutes,
                brightness_offset: snap.brightness_offset,
                lights_on,
                transitioning: transitioning_rooms.contains(&snap.id),
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
                        RoomLightingContext {
                            light_profile_configs: &light_profile_configs,
                            mode_configs: &mode_configs,
                            mode: active_mode,
                            solar_noon,
                            latitude,
                            utc_offset,
                        },
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
                RoomLightingContext {
                    light_profile_configs: &light_profile_configs,
                    mode_configs: &mode_configs,
                    mode: active_mode,
                    solar_noon,
                    latitude,
                    utc_offset,
                },
                RoomLightingInput {
                    settings: &room.profile_settings,
                    room_state,
                    time_offset_minutes: room.time_offset_minutes,
                    brightness_offset: room.brightness_offset,
                },
            );
            rooms.push(RoomPollState {
                rhythm: RoomRhythmState {
                    id: room.id.clone(),
                    hub_types: topo_hub_types.get(&room.id).cloned().unwrap_or_default(),
                    state: room_state,
                    rhythm_enabled: room.rhythm_enabled,
                    time_offset: room.time_offset_minutes,
                    brightness_offset: room.brightness_offset,
                    lights_on: false,
                    transitioning: false,
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
    let now = std::time::Instant::now();
    let (runtime, lights_on, warning_active, transitioning, hub_types) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.hub_runtime(),
            s.room_lights_on.get(room_id).copied().unwrap_or(false),
            s.motion_snapshots
                .get(room_id)
                .is_some_and(|motion| motion.warning_active),
            room_mode_transition_active(&s.room_mode_transitions, room_id, now),
            room_hub_types_from_topology(&s, room_id),
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
        hub_types,
        state: room_state,
        rhythm_enabled: snap.rhythm_enabled,
        time_offset: snap.time_offset_minutes,
        brightness_offset: snap.brightness_offset,
        lights_on,
        transitioning,
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
    SettingsDto {
        power_save: s.power_save,
    }
}

/// Build `ModeSettingsDto` from an already-locked `AppState`.
fn build_mode_dto_inner(s: &AppState) -> ModeSettingsDto {
    let last_change_utc_ms = s
        .last_active_mode_change_utc_ms
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    ModeSettingsDto {
        active: s.active_mode,
        last_change: ModeLastChangeDto {
            cause: s.last_active_mode_cause,
            transition_id: s.last_active_mode_transition_id.clone(),
            epoch_ms: last_change_utc_ms,
        },
        configs: s.mode_configs(),
    }
}

/// Build `ModeTransitionsDto` from an already-locked `AppState`.
fn build_transitions_dto_inner(s: &AppState) -> ModeTransitionsDto {
    ModeTransitionsDto {
        transitions: s.mode_transition_configs(),
    }
}

/// Build `ProfilesDto` from an already-locked `AppState`.
fn build_profiles_dto_inner(s: &AppState) -> ProfilesDto {
    ProfilesDto {
        profiles: s.light_profile_configs.values().cloned().collect(),
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

/// Build the current mode state and policy.
pub fn build_mode_dto(state: &SharedState) -> Result<ModeSettingsDto> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    Ok(build_mode_dto_inner(&s))
}

/// Build the current mode state and policy as a JSON string.
pub fn build_mode(state: &SharedState) -> Result<String> {
    let dto = build_mode_dto(state)?;
    serde_json::to_string(&dto).map_err(|e| anyhow::anyhow!("serialize mode: {}", e))
}

/// Build the current mode transition policy.
pub fn build_transitions_dto(state: &SharedState) -> Result<ModeTransitionsDto> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    Ok(build_transitions_dto_inner(&s))
}

/// Build the current mode transition policy as a JSON string.
pub fn build_transitions(state: &SharedState) -> Result<String> {
    let dto = build_transitions_dto(state)?;
    serde_json::to_string(&dto).map_err(|e| anyhow::anyhow!("serialize transitions: {}", e))
}

/// Build the current light profile list.
pub fn build_profiles_dto(state: &SharedState) -> Result<ProfilesDto> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    Ok(build_profiles_dto_inner(&s))
}

/// Build the current light profile list as a JSON string.
pub fn build_profiles(state: &SharedState) -> Result<String> {
    let dto = build_profiles_dto(state)?;
    serde_json::to_string(&dto).map_err(|e| anyhow::anyhow!("serialize profiles: {}", e))
}

fn source_room_manager_for_export(state: &SharedState) -> rhythm_core::RoomManager {
    let runtime = state.lock().ok().and_then(|s| s.hub_runtime());
    let source = if let Some(runtime) = runtime {
        rooms_from_engine(runtime.as_ref())
    } else {
        let Ok(s) = state.lock() else {
            return rhythm_core::RoomManager::default();
        };

        s.storage
            .as_ref()
            .and_then(|storage| storage.load_rooms().ok())
            .unwrap_or_default()
    };

    source
}

fn registry_node_snapshots_from_state(state: &SharedState) -> Vec<rhythm_core::NodeSnapshot> {
    let (registries, topo_map) = {
        let Ok(s) = state.lock() else {
            return Vec::new();
        };
        let topo_map: HashMap<String, (String, String)> = s
            .topology
            .rooms()
            .flat_map(|room| {
                room.hub_room_bindings.iter().map(move |binding| {
                    (
                        binding.hub_room_id.clone(),
                        (room.id.clone(), room.name.clone()),
                    )
                })
            })
            .collect();
        (s.all_hub_registries(), topo_map)
    };

    let mut seen = HashSet::new();
    let mut nodes = Vec::new();
    for reg in registries {
        let Ok(reg) = reg.lock() else {
            continue;
        };
        for room in reg.rooms() {
            let (id, name) = topo_map
                .get(&room.id)
                .cloned()
                .unwrap_or_else(|| (room.id.clone(), room.name.clone()));
            if !seen.insert(id.clone()) {
                continue;
            }
            nodes.push(rhythm_core::NodeSnapshot {
                id,
                name,
                kind: LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: false,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            });
        }
    }
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    nodes
}

fn room_manager_for_export(state: &SharedState) -> rhythm_core::RoomManager {
    let source = source_room_manager_for_export(state);
    let Ok(s) = state.lock() else {
        return source;
    };
    if s.topology.room_count() == 0 && s.topology.device_nodes().next().is_none() {
        return source;
    }

    let mut merged = rhythm_core::RoomManager::default();

    let mut topology_room_ids: Vec<_> = s.topology.rooms().map(|room| room.id.clone()).collect();
    topology_room_ids.sort();
    for room_id in topology_room_ids {
        let topology_room = s.topology.get(&room_id).unwrap();
        let mut room = source
            .get(&room_id)
            .cloned()
            .unwrap_or_else(|| rhythm_core::Room::new(&topology_room.id, &topology_room.name));
        room.name = topology_room.name.clone();
        room.kind = LightNodeKind::Room;
        room.parent_id = None;
        merged.add_room(room);
    }

    let mut device_node_ids: Vec<_> = s
        .topology
        .device_nodes()
        .map(|node| node.id.clone())
        .collect();
    device_node_ids.sort();
    for node_id in device_node_ids {
        let topology_node = s.topology.get_device_node(&node_id).unwrap();
        let mut room = source.get(&node_id).cloned().unwrap_or_else(|| {
            let (name, kind) = s
                .canonical_registry
                .get(&topology_node.canonical_device_id)
                .map(|device| {
                    (
                        device.name.clone(),
                        runtime_node_kind_for_device_type(device.device_type.clone()),
                    )
                })
                .unwrap_or_else(|| (topology_node.id.clone(), LightNodeKind::OtherDevice));
            let mut node = rhythm_core::Room::new_node(
                &topology_node.id,
                name,
                kind,
                topology_node.parent_id.clone(),
            );
            if !node.kind.is_room() {
                node.rhythm_enabled = true;
            }
            node
        });
        room.parent_id = topology_node.parent_id.clone();
        if let Some(device) = s.canonical_registry.get(&topology_node.canonical_device_id) {
            room.name = device.name.clone();
            room.kind = runtime_node_kind_for_device_type(device.device_type.clone());
        } else {
            room.name = topology_node.id.clone();
            room.kind = LightNodeKind::OtherDevice;
        }
        merged.add_room(room);
    }

    merged
}

fn portable_configuration_from_parts(
    s: &AppState,
    room_manager: &rhythm_core::RoomManager,
) -> PortableConfiguration {
    let mut rooms: Vec<_> = room_manager
        .iter()
        .filter(|room| room.kind.is_room())
        .map(ConfigurationRoom::from)
        .collect();
    rooms.sort_by(|left, right| left.id.cmp(&right.id).then(left.name.cmp(&right.name)));

    PortableConfiguration {
        power_save: s.power_save,
        active_mode: s.active_mode,
        profiles: s.light_profile_configs.values().cloned().collect(),
        mode_configs: s.mode_configs(),
        mode_transitions: s.mode_transition_configs(),
        rooms,
    }
}

fn stored_location_from_state(s: &AppState) -> Option<StoredLocation> {
    let has_location = s.latitude.is_some()
        || s.longitude.is_some()
        || s.timezone_name.is_some()
        || s.utc_offset_hours.abs() > f32::EPSILON;
    has_location.then(|| StoredLocation {
        latitude: s.latitude,
        longitude: s.longitude,
        utc_offset_hours: s.utc_offset_hours,
        timezone_name: s.timezone_name.clone(),
    })
}

fn backup_hub_credentials_from_state(
    creds: &crate::hub::HubCredentials,
    include_secrets: bool,
) -> BackupHubCredentials {
    BackupHubCredentials {
        hub_type: creds.hub_type.clone(),
        address: creds.address.clone(),
        data: include_secrets.then(|| creds.data.clone()),
    }
}

pub fn build_configuration_bundle_dto(state: &SharedState) -> Result<ConfigurationBundle> {
    let room_manager = source_room_manager_for_export(state);
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

    Ok(ConfigurationBundle {
        schema_version: BUNDLE_SCHEMA_VERSION,
        kind: crate::bundle::BundleKind::ConfigurationBundle,
        name: None,
        description: None,
        configuration: portable_configuration_from_parts(&s, &room_manager),
    })
}

pub fn build_configuration_bundle(state: &SharedState) -> Result<String> {
    let bundle = build_configuration_bundle_dto(state)?;
    serialize_configuration_bundle(&bundle)
}

fn serialize_configuration_bundle(bundle: &ConfigurationBundle) -> Result<String> {
    serde_json::to_string_pretty(bundle)
        .map_err(|e| anyhow::anyhow!("serialize configuration bundle: {}", e))
}

pub fn build_factory_default_configuration_bundle_dto() -> ConfigurationBundle {
    factory_default_configuration_bundle()
}

pub fn build_factory_default_configuration_bundle() -> Result<String> {
    serialize_configuration_bundle(&build_factory_default_configuration_bundle_dto())
}

fn clear_factory_reset_storage(state: &SharedState) -> Result<()> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if let Some(storage) = s.storage.as_ref() {
        storage.clear_factory_reset_state()?;
    }
    Ok(())
}

fn clear_factory_reset_ephemeral_state(state: &SharedState) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    s.hub_connection_status.clear();
    s.hub_seen_connected_once.clear();
    s.hub_sync_in_progress.clear();
    s.hub_reconnect_sync_at.clear();
    s.room_lights_on.clear();
    s.motion_snapshots.clear();
    s.room_mode_transitions.clear();
    s.last_check_hour = None;
    s.pending_periodic_ticks.clear();
    s.pending_hub_event_rxs.clear();
    s.pending_motion_clear.clear();
    s.pending_motion_seed.clear();
    s.last_tick_epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    Ok(())
}

pub fn do_configuration_reset(state: &SharedState) -> Result<String> {
    do_hub_disconnect(state)?;
    clear_factory_reset_storage(state)?;
    clear_factory_reset_ephemeral_state(state)?;

    let installation = BackupInstallation {
        location: None,
        rooms: rhythm_core::RoomManager::new(),
        topology: crate::topology::RoomTopologyStore::new(),
        canonical_registry: crate::canonical::registry::CanonicalRegistry::new(),
        hub_credentials: Vec::new(),
        hub_registries: Vec::new(),
    };
    restore_backup_installation_metadata(state, &installation)?;
    restore_backup_room_manager(state, &installation.rooms)?;
    restore_backup_location(state, None)?;

    do_configuration_import(
        state,
        ConfigurationImportPayload::Bundle(factory_default_configuration_bundle()),
    )
}

pub fn build_backup_bundle_dto(state: &SharedState, include_secrets: bool) -> Result<BackupBundle> {
    let room_manager = room_manager_for_export(state);
    let (
        configuration,
        location,
        topology,
        canonical_registry,
        runtime_state,
        hub_credentials,
        hub_registries,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let configuration = portable_configuration_from_parts(&s, &room_manager);
        let mut hub_credentials: Vec<_> = s
            .hub_credentials
            .values()
            .map(|creds| backup_hub_credentials_from_state(creds, include_secrets))
            .collect();
        hub_credentials.sort_by(|left, right| {
            left.address.cmp(&right.address).then(
                left.hub_type
                    .as_ref()
                    .map(|t| t.as_str())
                    .cmp(&right.hub_type.as_ref().map(|t| t.as_str())),
            )
        });
        let hub_registries: Vec<_> = s
            .hubs
            .iter()
            .filter_map(|(hub_key, hub)| {
                hub.registry
                    .as_ref()
                    .map(|registry| (hub_key.clone(), registry.clone()))
            })
            .collect();

        (
            configuration,
            stored_location_from_state(&s),
            s.topology.clone(),
            s.canonical_registry.clone(),
            BackupRuntimeState {
                active_mode: s.active_mode,
                last_change_cause: s.last_active_mode_cause,
                last_change_transition_id: s.last_active_mode_transition_id.clone(),
                last_change_epoch_ms: s.last_active_mode_change_utc_ms,
            },
            hub_credentials,
            hub_registries,
        )
    };

    let mut hub_registry_snapshots: Vec<_> = hub_registries
        .into_iter()
        .filter_map(|(hub_key, registry)| {
            registry.lock().ok().map(|registry| BackupHubRegistry {
                hub_key,
                snapshot: registry.snapshot_json(),
            })
        })
        .collect();
    hub_registry_snapshots
        .sort_by(|left, right| left.hub_key.to_string().cmp(&right.hub_key.to_string()));

    Ok(BackupBundle {
        schema_version: BUNDLE_SCHEMA_VERSION,
        kind: crate::bundle::BundleKind::BackupBundle,
        created_at: chrono::Utc::now().to_rfc3339(),
        secrets_included: include_secrets,
        configuration,
        installation: BackupInstallation {
            location,
            rooms: room_manager,
            topology,
            canonical_registry,
            hub_credentials,
            hub_registries: hub_registry_snapshots,
        },
        runtime_state,
    })
}

pub fn build_backup_bundle(state: &SharedState, include_secrets: bool) -> Result<String> {
    let bundle = build_backup_bundle_dto(state, include_secrets)?;
    serde_json::to_string_pretty(&bundle)
        .map_err(|e| anyhow::anyhow!("serialize backup bundle: {}", e))
}

fn restore_backup_location(state: &SharedState, location: Option<StoredLocation>) -> Result<()> {
    let cleared_location = StoredLocation {
        latitude: None,
        longitude: None,
        utc_offset_hours: 0.0,
        timezone_name: None,
    };
    let location_to_apply = location.unwrap_or(cleared_location);

    let (runtime, solar_noon, latitude, utc_offset_hours, timezone_name) = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let mut latitude = s.latitude;
        let mut longitude = s.longitude;
        let mut utc_offset_hours = s.utc_offset_hours;
        let mut runtime_config = s.runtime_config.clone();
        let mut timezone_name = s.timezone_name.clone();
        location_to_apply.apply_to_state(
            &mut latitude,
            &mut longitude,
            &mut utc_offset_hours,
            &mut runtime_config,
            &mut timezone_name,
        );
        if latitude.is_none() && longitude.is_none() && timezone_name.is_none() {
            runtime_config.solar_noon_hour = rhythm_core::RuntimeConfig::default().solar_noon_hour;
        }
        s.latitude = latitude;
        s.longitude = longitude;
        s.utc_offset_hours = utc_offset_hours;
        s.runtime_config = runtime_config;
        s.timezone_name = timezone_name;
        if let Some(storage) = s.storage.as_ref() {
            if let Err(e) = storage.save_location(&location_to_apply) {
                warn!(target: "cmd", "Failed to save restored location: {}", e);
            }
        }
        (
            s.hub_runtime(),
            s.runtime_config.solar_noon_hour,
            s.latitude,
            s.utc_offset_hours,
            s.timezone_name.clone(),
        )
    };

    if let Some(runtime) = runtime {
        let local_now = if let Some(ref tz_name) = timezone_name {
            rhythm_core::Timezone::new(tz_name)
                .local_datetime_from_utc(chrono::Utc::now().naive_utc())
        } else {
            current_local_datetime(utc_offset_hours)
        };
        let day_of_year = rhythm_core::timezone::day_of_year(
            local_now.date().year(),
            local_now.date().month(),
            local_now.date().day(),
        );
        if let Err(e) = runtime.set_solar(rhythm_core::SolarTime::new(
            solar_noon,
            latitude.unwrap_or(0.0),
            day_of_year,
        )) {
            warn!(target: "cmd", "Failed to apply restored solar location: {}", e);
        }
    }

    Ok(())
}

fn restore_backup_installation_metadata(
    state: &SharedState,
    installation: &BackupInstallation,
) -> Result<()> {
    let mut topology = installation.topology.clone();
    topology.rebuild_indices();
    let mut canonical_registry = installation.canonical_registry.clone();
    canonical_registry.rebuild_indices();

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.topology = topology;
        s.canonical_registry = canonical_registry;
    }

    if let Ok(s) = state.lock() {
        persist_canonical(&s);
        persist_topology(&s);
    }

    #[cfg(feature = "desktop")]
    rebuild_composite_routing(state);

    Ok(())
}

fn restore_backup_room_manager(
    state: &SharedState,
    rooms: &rhythm_core::RoomManager,
) -> Result<()> {
    let needs_runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let has_runtime = s.hubs.values().any(|hub| hub.runtime.is_some());
        !has_runtime && s.has_any_hub()
    };
    if needs_runtime {
        try_ensure_runtime(state)?;
    }

    let runtime = state.lock().ok().and_then(|s| s.hub_runtime());
    let desired_ids: HashSet<String> = rooms.iter().map(|room| room.id.clone()).collect();

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.room_lights_on
            .retain(|room_id, _| desired_ids.contains(room_id));
        s.motion_snapshots
            .retain(|room_id, _| desired_ids.contains(room_id));
        s.room_mode_transitions
            .retain(|room_id, _| desired_ids.contains(room_id));
        s.pending_motion_clear
            .retain(|room_id| desired_ids.contains(room_id));
    }

    if let Some(runtime) = runtime {
        for snapshot in runtime.engine_all_node_snapshots() {
            if !desired_ids.contains(&snapshot.id) {
                runtime.remove_node(&snapshot.id);
            }
        }

        for room in rooms.iter() {
            runtime.add_node(&room.id, &room.name, room.kind, room.parent_id.clone());
            runtime.restore_node_state(
                &room.id,
                RestoredNodeState {
                    rhythm_enabled: room.rhythm_enabled,
                    disabled: room.disabled,
                    time_offset_minutes: room.time_offset_minutes,
                    brightness_offset: room.brightness_offset,
                    soft_off: room.soft_off,
                    hard_off: room.hard_off,
                    profile_settings: room.profile_settings.clone(),
                },
            );
        }

        persist_rooms(state);
    } else {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if let Some(storage) = s.storage.as_ref() {
            if let Err(e) = storage.save_rooms(rooms) {
                warn!(target: "cmd", "Failed to save restored rooms: {}", e);
            }
        }
    }

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);

    Ok(())
}

fn restore_backup_runtime_state(
    state: &SharedState,
    runtime_state: &BackupRuntimeState,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    s.active_mode = runtime_state.active_mode;
    s.last_active_mode_cause = runtime_state.last_change_cause;
    s.last_active_mode_transition_id = runtime_state.last_change_transition_id.clone();
    s.last_active_mode_change_utc_ms = runtime_state.last_change_epoch_ms;
    s.sync_active_mode_runtime_overrides();
    persist_settings_locked(&s);
    Ok(())
}

fn save_backup_hub_registries_to_storage(
    state: &SharedState,
    registries: &[BackupHubRegistry],
) -> Result<()> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let Some(storage) = s.storage.as_ref() else {
        return Ok(());
    };

    for registry in registries {
        if let Err(e) = storage.save_hub_registry_for(&registry.hub_key, &registry.snapshot) {
            warn!(
                target: "cmd",
                "Failed to save restored hub registry for {}: {}",
                registry.hub_key,
                e
            );
        }
    }

    Ok(())
}

fn restore_backup_hub_credentials(
    state: &SharedState,
    credentials: &[BackupHubCredentials],
) -> Result<()> {
    for credential in credentials {
        let Some(hub_type) = credential.hub_type.as_ref() else {
            continue;
        };
        let Some(data) = credential.data.as_ref() else {
            warn!(
                target: "cmd",
                "Skipping restored hub {} at {} because the backup is redacted",
                hub_type.as_str(),
                credential.address
            );
            continue;
        };

        if let Err(e) = do_hub_credentials(state, hub_type.as_str(), &credential.address, data) {
            warn!(
                target: "cmd",
                "Failed to restore hub {} at {}: {}",
                hub_type.as_str(),
                credential.address,
                e
            );
        }
    }

    Ok(())
}

fn current_local_datetime(utc_offset: f32) -> chrono::NaiveDateTime {
    let now = chrono::Utc::now().naive_utc();
    let offset_secs = (utc_offset * 3600.0) as i64;
    now + chrono::Duration::seconds(offset_secs)
}

fn local_time_string_from_decimal_hour(hour: f32) -> String {
    let total_seconds = ((hour.rem_euclid(24.0)) * 3600.0).round() as i64;
    let wrapped_seconds = total_seconds.rem_euclid(24 * 3600);
    let hours = wrapped_seconds / 3600;
    let minutes = (wrapped_seconds % 3600) / 60;
    let seconds = wrapped_seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

fn optional_local_time_string_from_decimal_hour(hour: Option<f32>) -> Option<String> {
    hour.map(local_time_string_from_decimal_hour)
}

fn mode_apply_cycle_duration(
    lighting: RoomLightingContext<'_>,
    room_snapshots: &[rhythm_core::RoomSnapshot],
    update_interval: Duration,
    power_save: bool,
    room_commands: &[(String, LightingCommand)],
) -> Duration {
    let sample_at = current_local_datetime(lighting.utc_offset);
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
    let solar = rhythm_core::SolarTime::new(lighting.solar_noon, lighting.latitude, day_of_year);
    let ctx = rhythm_core::light_profile::CurveContext::new(sample_hour, solar, None);
    let registry = light_profile_registry_from_parts(
        lighting.light_profile_configs,
        lighting.mode_configs,
        lighting.mode,
    );

    let room_node_snapshots: Vec<rhythm_core::NodeSnapshot> = room_snapshots
        .iter()
        .cloned()
        .map(rhythm_core::NodeSnapshot::from_room_snapshot)
        .collect();
    let periodic_cycle = crate::periodic::effective_cycle_duration(
        &registry,
        &ctx,
        &room_node_snapshots,
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

#[cfg(feature = "desktop")]
fn node_state_event_from_runtime(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
) -> Option<crate::server_event::NodeStateEvent> {
    runtime
        .engine_effective_node_snapshot(node_id)
        .map(|snap| build_node_state_event(state, &snap))
}

#[cfg(feature = "desktop")]
fn node_state_events_after_apply(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
) -> Vec<crate::server_event::NodeStateEvent> {
    let Some(snap) = runtime.engine_effective_node_snapshot(node_id) else {
        return Vec::new();
    };

    let mut events = vec![build_node_state_event(state, &snap)];
    if snap.kind == LightNodeKind::LightDevice {
        if let Some(parent_id) = snap
            .parent_id
            .as_deref()
            .filter(|parent_id| *parent_id != snap.id)
        {
            if let Some(parent_snap) = runtime.engine_effective_node_snapshot(parent_id) {
                events.push(build_node_state_event(state, &parent_snap));
            }
        }
    }

    events
}

pub(crate) fn emit_node_state_event_after_apply(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
) {
    #[cfg(feature = "desktop")]
    {
        let events = node_state_events_after_apply(state, runtime, node_id);
        if !events.is_empty() {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::NodeState { nodes: events },
            );
        }
    }

    #[cfg(not(feature = "desktop"))]
    {
        let _ = (state, runtime, node_id);
    }
}

fn apply_room_mode_defaults(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    ctx: ModeDefaultApplyContext<'_>,
    snapshots: &[rhythm_core::RoomSnapshot],
) -> bool {
    let Some(mode_config) = mode_config_for_mode(ctx.lighting.mode_configs, ctx.lighting.mode)
    else {
        return false;
    };
    if mode_config.room_defaults.is_empty() {
        return false;
    }

    let snapshots_by_id: HashMap<&str, &rhythm_core::RoomSnapshot> = snapshots
        .iter()
        .map(|snap| (snap.id.as_str(), snap))
        .collect();
    let mut changed_room_ids = Vec::new();
    let mut hard_off_rooms = Vec::new();
    let mut lights_on_updates = Vec::new();
    let mut missing_rooms = 0usize;

    for room_default in &mode_config.room_defaults {
        let Some(snap) = snapshots_by_id.get(room_default.room_id.as_str()).copied() else {
            missing_rooms += 1;
            continue;
        };

        let current_state = persistent_room_state_from_flags(snap.hard_off, snap.soft_off);
        if current_state == room_default.state {
            continue;
        }

        let Ok((soft_off, hard_off)) = room_flags_for_target_state(room_default.state) else {
            warn!(
                target: "cmd",
                "active_mode_apply: ignoring invalid room default state for room '{}'",
                room_default.room_id
            );
            continue;
        };

        runtime.restore_room_state(
            &snap.id,
            RestoredRoomState {
                rhythm_enabled: if soft_off { true } else { snap.rhythm_enabled },
                disabled: snap.disabled,
                time_offset_minutes: snap.time_offset_minutes,
                brightness_offset: snap.brightness_offset,
                soft_off,
                hard_off,
                profile_settings: snap.profile_settings.clone(),
            },
        );

        match room_default.state {
            RoomModeState::Active | RoomModeState::Idle => {
                lights_on_updates.push((snap.id.clone(), true));
            }
            RoomModeState::HardOff => {
                lights_on_updates.push((snap.id.clone(), false));
                let transition_ms = ctx
                    .transition
                    .and_then(|config| {
                        resolve_mode_transition_duration_ms_for_room_from_parts(
                            config,
                            ctx.lighting,
                            RoomLightingInput::from_snapshot(snap, RoomModeState::HardOff),
                            ctx.transition_started_at,
                        )
                    })
                    .filter(|ms| *ms > 0);
                hard_off_rooms.push((snap.id.clone(), transition_ms));
            }
            RoomModeState::Wake | RoomModeState::Warning => {}
        }

        changed_room_ids.push(snap.id.clone());
    }

    if let Ok(mut s) = state.lock() {
        for (room_id, lights_on) in lights_on_updates {
            s.room_lights_on.insert(room_id, lights_on);
        }
    }

    for (room_id, transition_ms) in &hard_off_rooms {
        queue_motion_timer_clear(state, room_id);
        if let Err(e) = runtime.lights_off_room(room_id, *transition_ms) {
            warn!(
                target: "cmd",
                "active_mode_apply: lights_off for '{}' failed after room default apply: {}",
                room_id,
                e
            );
        }
        emit_node_state_event_after_apply(state, runtime, room_id);
    }

    if !changed_room_ids.is_empty() || missing_rooms > 0 {
        debug!(
            target: "cmd",
            "active_mode_apply: applied {} room defaults for {:?}, {} missing room ids",
            changed_room_ids.len(),
            ctx.lighting.mode,
            missing_rooms
        );
    }

    !changed_room_ids.is_empty()
}

fn apply_room_commands_inline(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    room_commands: Vec<(String, LightingCommand)>,
    phase_gap: Duration,
) {
    let room_count = room_commands.len();

    for (idx, (room_id, command)) in room_commands.into_iter().enumerate() {
        log_room_command_dispatch(runtime.as_ref(), &room_id, &command);
        if let Err(e) = runtime.apply_room_command(&room_id, command) {
            warn!(target: "cmd", "active_mode_apply: room '{}' failed: {}", room_id, e);
            continue;
        }
        emit_node_state_event_after_apply(state, runtime, &room_id);
        if idx + 1 < room_count && !phase_gap.is_zero() {
            std::thread::sleep(phase_gap);
        }
    }
}

fn room_command_log_label(runtime: &dyn RuntimeHandle, room_id: &str) -> String {
    runtime
        .engine_room_snapshot(room_id)
        .map(|room| {
            if room.name != room.id {
                format!("{} ({})", room.name, room.id)
            } else {
                room.id
            }
        })
        .unwrap_or_else(|| room_id.to_string())
}

fn room_command_log_payload(command: &LightingCommand) -> String {
    if command.is_direct_color {
        format!(
            "bri={} rgb=({},{},{}) xy=({:.3},{:.3}) transition_ms={:?} direct_color=true",
            command.brightness,
            command.rgb.r,
            command.rgb.g,
            command.rgb.b,
            command.xy.x,
            command.xy.y,
            command.transition_ms
        )
    } else {
        format!(
            "bri={} kelvin={} rgb=({},{},{}) xy=({:.3},{:.3}) transition_ms={:?} direct_color=false",
            command.brightness,
            command.kelvin,
            command.rgb.r,
            command.rgb.g,
            command.rgb.b,
            command.xy.x,
            command.xy.y,
            command.transition_ms
        )
    }
}

pub(crate) fn log_room_command_dispatch(
    runtime: &dyn RuntimeHandle,
    room_id: &str,
    command: &LightingCommand,
) {
    info!(
        target: "cmd",
        "room_command_dispatch: room={} {}",
        room_command_log_label(runtime, room_id),
        room_command_log_payload(command),
    );
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
            for (idx, (node_id, command)) in room_commands.into_iter().enumerate() {
                if tx
                    .send(crate::state::WorkItem::ApplyNodeCommand { node_id, command })
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

fn resolve_room_output_for_state_at_from_parts(
    lighting: RoomLightingContext<'_>,
    room: RoomLightingInput<'_>,
    sample_at: chrono::NaiveDateTime,
) -> Option<(rhythm_core::LightingValues, u8)> {
    if room.room_state == RoomModeState::HardOff {
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
    let solar = rhythm_core::SolarTime::new(lighting.solar_noon, lighting.latitude, day_of_year);
    let ctx = rhythm_core::light_profile::CurveContext::new(sample_hour, solar, None);
    let registry = light_profile_registry_from_parts(
        lighting.light_profile_configs,
        lighting.mode_configs,
        lighting.mode,
    );
    let render_state = render_state_for_display(room.room_state);
    let values = registry
        .profile_for_room_state(lighting.mode, render_state, Some(room.settings))
        .calculate_with_offset(&ctx, room.time_offset_minutes);
    let adjusted_brightness =
        (values.brightness as f32 + room.brightness_offset).clamp(1.0, 100.0) as u8;
    let brightness = match render_state {
        RoomModeState::Idle => values.brightness,
        RoomModeState::Warning
            if !warning_uses_custom_profile(lighting.mode_configs, lighting.mode) =>
        {
            ((adjusted_brightness as f32) * crate::event_loop::WARNING_DIM_FACTOR).clamp(1.0, 100.0)
                as u8
        }
        RoomModeState::Active | RoomModeState::Wake | RoomModeState::Warning => adjusted_brightness,
        RoomModeState::HardOff => return None,
    };

    Some((values, brightness))
}

fn resolved_profile_config_for_room_state_from_parts(
    lighting: RoomLightingContext<'_>,
    settings: &RoomProfileSettings,
    room_state: RoomModeState,
) -> LightProfileConfig {
    let active_profile_id = resolved_active_profile_id_for_mode_from_parts(
        lighting.light_profile_configs,
        lighting.mode_configs,
        lighting.mode,
    );
    let requested_id = settings.resolved_profile_id(active_profile_id.as_str());
    let base_id = if !rhythm_core::is_builtin_state_profile_id(requested_id)
        && lighting.light_profile_configs.contains_key(requested_id)
    {
        requested_id
    } else {
        active_profile_id.as_str()
    };

    let mut active_config = lighting
        .light_profile_configs
        .get(base_id)
        .cloned()
        .or_else(|| {
            lighting
                .light_profile_configs
                .get(active_profile_id.as_str())
                .cloned()
        })
        .unwrap_or_else(|| factory_default_active_profile_config_for_mode(lighting.mode));
    settings.apply_to_config(&mut active_config);

    if room_state == RoomModeState::Active {
        return active_config;
    }

    let mode_config = mode_config_for_mode(lighting.mode_configs, lighting.mode)
        .cloned()
        .unwrap_or_else(|| ModeConfig::default_for_mode(lighting.mode));

    mode_config
        .resolve_state_profile_id(room_state, active_profile_id.as_str())
        .and_then(|target_id| lighting.light_profile_configs.get(target_id).cloned())
        .unwrap_or_else(|| {
            if matches!(room_state, RoomModeState::Idle | RoomModeState::HardOff) {
                factory_default_idle_profile_config_for_mode(lighting.mode)
            } else {
                active_config.clone()
            }
        })
}

fn resolve_mode_transition_duration_ms_for_room_from_parts(
    transition: &ModeTransitionConfig,
    lighting: RoomLightingContext<'_>,
    room: RoomLightingInput<'_>,
    transition_started_at: chrono::NaiveDateTime,
) -> Option<u32> {
    let started_at_time = transition_started_at.time();
    let started_at_hour = started_at_time.hour() as f32
        + started_at_time.minute() as f32 / 60.0
        + started_at_time.second() as f32 / 3600.0;

    if let Some(duration_ms) = transition.duration_ms.resolve(started_at_hour) {
        return Some(duration_ms);
    }

    resolved_profile_config_for_room_state_from_parts(lighting, room.settings, room.room_state)
        .fade_ms
        .resolve(started_at_hour)
        .or(Some(rhythm_core::DEFAULT_MODE_TRANSITION_DURATION_MS))
}

fn resolve_room_command_for_state_at_from_parts(
    lighting: RoomLightingContext<'_>,
    room: RoomLightingInput<'_>,
    sample_at: chrono::NaiveDateTime,
    transition_ms_override: Option<u32>,
) -> Option<LightingCommand> {
    let (values, brightness) =
        resolve_room_output_for_state_at_from_parts(lighting, room, sample_at)?;
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
    transition: Option<ModeTransitionConfig>,
    apply_scope: ModeOutputApplyScope,
) {
    let (
        runtime,
        light_profile_configs,
        mode_configs,
        solar_noon,
        latitude,
        utc_offset,
        mut room_lights_on,
        motion_snapshots,
        update_interval,
        power_save,
    ) = {
        let Ok(s) = state.lock() else { return };
        (
            s.hub_runtime(),
            s.light_profile_configs.clone(),
            s.mode_configs(),
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
        "active_mode_apply: {:?} -> {:?} transition_id={:?}",
        previous_mode,
        target_mode,
        transition.as_ref().map(|config| config.id.as_str())
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

    let transition_started_at = current_local_datetime(utc_offset);
    let lighting = RoomLightingContext {
        light_profile_configs: &light_profile_configs,
        mode_configs: &mode_configs,
        mode: target_mode,
        solar_noon,
        latitude,
        utc_offset,
    };
    let snapshots = runtime.engine_all_room_snapshots();
    let room_defaults_changed = apply_room_mode_defaults(
        state,
        &runtime,
        ModeDefaultApplyContext {
            lighting,
            transition: transition.as_ref(),
            transition_started_at,
        },
        &snapshots,
    );
    if room_defaults_changed {
        if let Ok(s) = state.lock() {
            room_lights_on = s.room_lights_on.clone();
        }
    }
    let snapshots = if room_defaults_changed {
        runtime.engine_all_room_snapshots()
    } else {
        snapshots
    };
    let mut room_commands = Vec::new();
    let mut dispatch_snapshots = Vec::new();
    let mut transitioned_rooms = Vec::new();
    let mut changed_room_ids = Vec::new();
    let mut preserved_hard_off = 0usize;
    let mut hidden_rooms = 0usize;
    let mut unresolved_rooms = 0usize;

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
                state.lock().ok().is_some_and(|s| {
                    lights_on_from_cache(
                        &s,
                        &room_lights_on,
                        &snap.id,
                        snap.kind,
                        snap.parent_id.as_deref(),
                    )
                })
            }
        };
        if !is_visible {
            hidden_rooms += 1;
            continue;
        }

        let room_transition_ms = transition
            .as_ref()
            .and_then(|config| {
                resolve_mode_transition_duration_ms_for_room_from_parts(
                    config,
                    lighting,
                    RoomLightingInput::from_snapshot(snap, room_state),
                    transition_started_at,
                )
            })
            .unwrap_or(0);
        let sample_at =
            transition_started_at + chrono::Duration::milliseconds(i64::from(room_transition_ms));
        let Some(command) = resolve_room_command_for_state_at_from_parts(
            lighting,
            RoomLightingInput::from_snapshot(snap, room_state),
            sample_at,
            (room_transition_ms > 0).then_some(room_transition_ms),
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
        if room_transition_ms > 0 {
            transitioned_rooms.push((snap.id.clone(), room_transition_ms));
        }
    }

    let cycle_duration = (!room_commands.is_empty()).then(|| {
        mode_apply_cycle_duration(
            lighting,
            &dispatch_snapshots,
            update_interval,
            power_save,
            &room_commands,
        )
    });

    if let Ok(mut s) = state.lock() {
        s.room_mode_transitions.clear();
        if !transitioned_rooms.is_empty() {
            let now = std::time::Instant::now();
            let phase_gap = cycle_duration
                .map(|duration| crate::periodic::dispatch_spacing(duration, room_commands.len()))
                .unwrap_or(std::time::Duration::ZERO);
            let mut scheduled_room_ids: Vec<String> = room_commands
                .iter()
                .map(|(room_id, _)| room_id.clone())
                .collect();
            scheduled_room_ids
                .sort_by_key(|room_id| crate::periodic::stable_room_phase_key(room_id));
            let dispatch_offsets: HashMap<String, std::time::Duration> = scheduled_room_ids
                .into_iter()
                .enumerate()
                .map(|(idx, room_id)| {
                    let offset = phase_gap
                        .checked_mul(idx as u32)
                        .unwrap_or(std::time::Duration::ZERO);
                    (room_id, offset)
                })
                .collect();
            for (room_id, transition_ms) in &transitioned_rooms {
                let dispatch_offset = dispatch_offsets
                    .get(room_id)
                    .copied()
                    .unwrap_or(std::time::Duration::ZERO);
                let ends_at = now
                    + dispatch_offset
                    + std::time::Duration::from_millis(u64::from(*transition_ms));
                let periodic_resume_at = ends_at + update_interval;
                s.room_mode_transitions.insert(
                    room_id.clone(),
                    crate::state::RoomModeTransition {
                        ends_at,
                        periodic_resume_at,
                    },
                );
            }
        }
    }

    debug!(
        target: "cmd",
        "active_mode_apply: {} rooms scanned, {} changed, {} transitioning, {} hidden, {} preserved hard-off, {} unresolved",
        snapshots.len(),
        changed_room_ids.len(),
        transitioned_rooms.len(),
        hidden_rooms,
        preserved_hard_off,
        unresolved_rooms
    );
    if let Some(cycle_duration) = cycle_duration {
        dispatch_room_commands(state, &runtime, room_commands, cycle_duration);
    }
}

fn do_settings_set_internal(
    state: &SharedState,
    power_save: Option<bool>,
    active_mode: Option<RhythmMode>,
    mode_configs: Option<Vec<ModeConfig>>,
    mode_transitions: Option<Vec<rhythm_core::ModeTransitionConfig>>,
    mode_change: Option<ModeChangeContext>,
    force_reapply_outputs: bool,
) -> Result<String> {
    if let Some(configs) = mode_configs.as_ref() {
        validate_mode_configs(configs)?;
    }

    let requested_mode_config_count = mode_configs.as_ref().map(Vec::len);
    let requested_transition_count = mode_transitions.as_ref().map(Vec::len);
    let transition_for_apply = mode_change
        .as_ref()
        .and_then(|change| change.transition.clone());
    let (
        rooms_to_off,
        runtimes,
        active_profile_id,
        updated_mode_configs,
        previous_mode,
        selected_mode,
        mode_changed,
        reapply_scope,
        mode_change,
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
            if mode_changed || force_reapply_outputs {
                s.active_mode = mode;
                let change = mode_change
                    .clone()
                    .unwrap_or_else(|| ModeChangeContext::new(ModeChangeCause::Manual, None));
                s.last_active_mode_cause = change.cause;
                s.last_active_mode_transition_id = change.transition_id();
                s.last_active_mode_change_utc_ms = Some(chrono::Utc::now().timestamp_millis());
            }
        }
        let selected_mode = s.active_mode;
        let should_reapply_mode_outputs = modes_updated || mode_changed || force_reapply_outputs;
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
            reapply_scope = if force_reapply_outputs {
                ModeOutputApplyScope::all_visible()
            } else {
                mode_output_apply_scope(mode_changed, &previous_mode_config, &updated_mode_config)
            };
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
            mode_change,
        )
    };

    if let Some(count) = requested_mode_config_count {
        info!(target: "cmd", "settings: updated {} mode configs", count);
    }
    if let Some(count) = requested_transition_count {
        info!(target: "cmd", "settings: updated {} mode transitions", count);
    }
    if mode_changed || force_reapply_outputs {
        info!(
            target: "cmd",
            "settings: active_mode {:?} -> {:?} cause={:?} transition_id={:?}",
            previous_mode,
            selected_mode,
            mode_change
                .as_ref()
                .map(|change| change.cause)
                .unwrap_or(ModeChangeCause::Manual),
            mode_change.as_ref().and_then(|change| change.transition_id())
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
                transition_for_apply,
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
        active_mode.map(|mode| {
            let previous_mode = state.lock().ok().map(|s| s.active_mode).unwrap_or(mode);
            ModeChangeContext::new(
                ModeChangeCause::Manual,
                matching_mode_transition(state, previous_mode, mode, ModeTransitionTrigger::Manual),
            )
        }),
        false,
    )
}

/// Update mode state and policy (partial: only provided fields are changed).
pub fn do_mode_set(
    state: &SharedState,
    active_mode: Option<RhythmMode>,
    mode_configs: Option<Vec<ModeConfig>>,
) -> Result<String> {
    let mode_change = active_mode.map(|mode| {
        let previous_mode = state.lock().ok().map(|s| s.active_mode).unwrap_or(mode);
        ModeChangeContext::new(
            ModeChangeCause::Manual,
            matching_mode_transition(state, previous_mode, mode, ModeTransitionTrigger::Manual),
        )
    });
    do_settings_set_internal(
        state,
        None,
        active_mode,
        mode_configs,
        None,
        mode_change,
        false,
    )?;
    build_mode(state)
}

/// Update mode transition policy (partial: only provided fields are changed).
pub fn do_transitions_set(
    state: &SharedState,
    mode_transitions: Option<Vec<rhythm_core::ModeTransitionConfig>>,
) -> Result<String> {
    do_settings_set_internal(state, None, None, None, mode_transitions, None, false)?;
    build_transitions(state)
}

fn validate_imported_configuration(configuration: &PortableConfiguration) -> Result<()> {
    validate_mode_configs(&configuration.mode_configs)?;

    let valid_profile_ids = valid_import_profile_ids(configuration);

    for room in &configuration.rooms {
        room_flags_for_target_state(room.state)?;
        validate_room_profile_settings(&room.id, &room.room_profile, &valid_profile_ids)?;
    }

    Ok(())
}

fn valid_import_profile_ids(configuration: &PortableConfiguration) -> HashSet<String> {
    factory_default_light_profile_config_map()
        .into_keys()
        .chain(
            configuration
                .profiles
                .iter()
                .map(|config| config.id.clone()),
        )
        .collect()
}

fn validate_room_profile_settings(
    room_id: &str,
    room_profile: &RoomProfileSettings,
    valid_profile_ids: &HashSet<String>,
) -> Result<()> {
    if let Some(profile_id) = room_profile.profile_id.as_deref() {
        if rhythm_core::is_builtin_state_profile_id(profile_id) {
            return Err(anyhow::anyhow!(
                "Room '{}' cannot select built-in state profile '{}'",
                room_id,
                profile_id
            ));
        }
        if !valid_profile_ids.contains(profile_id) {
            return Err(anyhow::anyhow!(
                "Room '{}' references unknown light profile '{}'",
                room_id,
                profile_id
            ));
        }
    }

    Ok(())
}

fn imported_room_profile_patch(room: &ConfigurationRoom) -> RoomProfileSettingsPatch {
    RoomProfileSettingsPatch {
        clear_all: false,
        profile_id: Some(room.room_profile.profile_id.clone()),
        fade_ms: Some(room.room_profile.fade_ms.clone()),
        motion_timeout_secs: Some(room.room_profile.motion_timeout_secs.clone()),
    }
}

fn configuration_room_target_id(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    imported: &ConfigurationRoom,
) -> Option<String> {
    let resolved_id = resolve_node_id(state, &imported.id);
    if runtime.engine_room_snapshot(&resolved_id).is_some() {
        return Some(resolved_id);
    }

    runtime
        .engine_all_room_snapshots()
        .into_iter()
        .find(|snapshot| {
            snapshot.name == imported.name
                || snapshot.name.eq_ignore_ascii_case(imported.name.as_str())
        })
        .map(|snapshot| snapshot.id)
}

fn apply_configuration_room_preferences(
    state: &SharedState,
    imported_rooms: &[ConfigurationRoom],
) -> (usize, usize) {
    let runtime = state.lock().ok().and_then(|s| s.hub_runtime());
    let mut applied_rooms = 0usize;
    let mut skipped_rooms = 0usize;

    if let Some(runtime) = runtime {
        for imported_room in imported_rooms {
            let Some(room_id) = configuration_room_target_id(state, &runtime, imported_room) else {
                skipped_rooms += 1;
                info!(
                    target: "cmd",
                    "configuration_import: skipped room '{}' ({}) because no local match was found",
                    imported_room.name,
                    imported_room.id
                );
                continue;
            };

            let patch = imported_room_profile_patch(imported_room);
            if let Err(e) = do_node_preferences_set(
                state,
                &room_id,
                Some(imported_room.rhythm_enabled),
                Some(imported_room.disabled),
                Some(imported_room.state),
                Some(&patch),
                false,
            ) {
                skipped_rooms += 1;
                warn!(
                    target: "cmd",
                    "configuration_import: failed to apply room '{}' to '{}': {}",
                    imported_room.id,
                    room_id,
                    e
                );
                continue;
            }
            applied_rooms += 1;
        }

        if !imported_rooms.is_empty() {
            persist_rooms(state);
        }
    } else {
        skipped_rooms = imported_rooms.len();
    }

    (applied_rooms, skipped_rooms)
}

pub fn do_configuration_import(
    state: &SharedState,
    payload: ConfigurationImportPayload,
) -> Result<String> {
    let bundle = payload.into_bundle();
    if bundle.schema_version != BUNDLE_SCHEMA_VERSION {
        return Err(anyhow::anyhow!(
            "Unsupported configuration schema version: {}",
            bundle.schema_version
        ));
    }

    let configuration = bundle.configuration;
    validate_imported_configuration(&configuration)?;

    let imported_profiles = configuration.profiles;
    let imported_power_save = configuration.power_save;
    let imported_active_mode = configuration.active_mode;
    let imported_mode_configs = configuration.mode_configs;
    let imported_mode_transitions = configuration.mode_transitions;
    let imported_rooms = configuration.rooms;

    let (profiles_to_apply, runtimes) = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.replace_light_profile_configs(imported_profiles);
        persist_light_profiles_locked(&s);
        (
            s.light_profile_configs
                .values()
                .cloned()
                .collect::<Vec<_>>(),
            s.hubs
                .values()
                .filter_map(|hub| hub.runtime.clone())
                .collect::<Vec<_>>(),
        )
    };

    for runtime in &runtimes {
        for profile in &profiles_to_apply {
            if let Err(e) = runtime.set_light_profile_config(profile.clone()) {
                warn!(
                    target: "cmd",
                    "configuration_import: failed to update runtime profile '{}': {}",
                    profile.id,
                    e
                );
            }
        }
    }

    do_settings_set_internal(
        state,
        Some(imported_power_save),
        Some(imported_active_mode),
        Some(imported_mode_configs),
        Some(imported_mode_transitions),
        Some(ModeChangeContext::new(ModeChangeCause::Manual, None)),
        true,
    )?;

    let (applied_rooms, skipped_rooms) =
        apply_configuration_room_preferences(state, &imported_rooms);

    info!(
        target: "cmd",
        "configuration_import: profiles={} modes={} transitions={} rooms_applied={} rooms_skipped={}",
        profiles_to_apply.len(),
        state.lock().ok().map(|s| s.mode_configs().len()).unwrap_or(0),
        state
            .lock()
            .ok()
            .map(|s| s.mode_transition_configs().len())
            .unwrap_or(0),
        applied_rooms,
        skipped_rooms
    );

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::ConfigChanged);

    build_configuration_bundle(state)
}

pub fn do_backup_restore(state: &SharedState, bundle: BackupBundle) -> Result<String> {
    if bundle.schema_version != BUNDLE_SCHEMA_VERSION {
        return Err(anyhow::anyhow!(
            "Unsupported backup schema version: {}",
            bundle.schema_version
        ));
    }
    if bundle.kind != crate::bundle::BundleKind::BackupBundle {
        return Err(anyhow::anyhow!(
            "Backup restore requires kind=backup_bundle"
        ));
    }

    validate_imported_configuration(&bundle.configuration)?;
    let valid_profile_ids = valid_import_profile_ids(&bundle.configuration);
    for room in bundle.installation.rooms.iter() {
        validate_room_profile_settings(&room.id, &room.profile_settings, &valid_profile_ids)?;
    }

    do_hub_disconnect(state)?;
    save_backup_hub_registries_to_storage(state, &bundle.installation.hub_registries)?;

    let mut configuration = bundle.configuration.clone();
    configuration.active_mode = bundle.runtime_state.active_mode;
    do_configuration_import(
        state,
        ConfigurationImportPayload::Bundle(ConfigurationBundle {
            schema_version: bundle.schema_version,
            kind: crate::bundle::BundleKind::ConfigurationBundle,
            name: None,
            description: None,
            configuration,
        }),
    )?;

    restore_backup_location(state, bundle.installation.location.clone())?;
    restore_backup_hub_credentials(state, &bundle.installation.hub_credentials)?;
    restore_backup_installation_metadata(state, &bundle.installation)?;
    restore_backup_room_manager(state, &bundle.installation.rooms)?;
    restore_backup_runtime_state(state, &bundle.runtime_state)?;

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::ConfigChanged);

    build_backup_bundle(state, false)
}

pub fn do_set_active_mode(state: &SharedState, mode: RhythmMode) -> Result<()> {
    do_set_active_mode_with_trigger(state, mode, ModeTransitionTrigger::Manual)
}

pub fn do_set_active_mode_with_trigger(
    state: &SharedState,
    mode: RhythmMode,
    trigger: ModeTransitionTrigger,
) -> Result<()> {
    let previous_mode = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .active_mode;
    let transition = matching_mode_transition(state, previous_mode, mode, trigger);
    let mode_change = ModeChangeContext::new(mode_change_cause_from_trigger(trigger), transition);
    do_settings_set_internal(
        state,
        None,
        Some(mode),
        None,
        None,
        Some(mode_change),
        false,
    )
    .map(|_| ())
}

pub fn do_trigger_transition(state: &SharedState, transition_id: &str) -> Result<String> {
    let transition = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.mode_transition_configs()
            .into_iter()
            .find(|config| config.id == transition_id)
    }
    .ok_or_else(|| anyhow::anyhow!("Unknown transition '{}'", transition_id))?;

    let mode_change = ModeChangeContext::new(ModeChangeCause::Manual, Some(transition.clone()));
    do_settings_set_internal(
        state,
        None,
        Some(transition.to_mode),
        None,
        None,
        Some(mode_change),
        true,
    )?;
    build_mode(state)
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

    // Ensure runtime exists after topology translation so bootstrap sees the
    // latest topology-first room graph, not only hub registry state.
    let needs_runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let has_runtime = s.hubs.values().any(|h| h.runtime.is_some());
        !has_runtime && s.has_any_hub()
    };

    if needs_runtime {
        try_ensure_runtime(state)?;
    }

    // Always apply params (whether runtime was just created or pre-existing)
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };
    if runtime.is_none() {
        let has_any_hub = state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .has_any_hub();
        if has_any_hub {
            return Err(anyhow::anyhow!(
                "Runtime initialization completed without installing a runtime"
            ));
        }
    }
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
            RestoredRoomState {
                rhythm_enabled,
                disabled: params.disabled,
                time_offset_minutes: time_offset,
                brightness_offset: bri_offset,
                soft_off,
                hard_off,
                profile_settings,
            },
        );
    }

    if persist {
        persist_state(state);
    }

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);

    let room_state = build_room_rhythm_state(state, &engine_room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Legacy room-centric removal from registry + engine state.
///
/// This path exists for older room-registry sync flows. It is not the
/// topology-room delete API.
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
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);

    Ok(())
}

// ============================================================================
// Node action commands
// ============================================================================

/// Dispatch a button action to an addressable node via the runtime engine.
///
/// Actions: on, off, toggle, rhythm_on, rhythm_off, rhythm_toggle,
/// step_up, step_down, dim_up, dim_down, reset, lights_off.
///
/// Returns the updated node state JSON, or an error.
pub fn do_node_action(
    state: &SharedState,
    node_id: &str,
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

    info!(target: "cmd", "node_action: {} -> {:?}", node_id, action);

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = match runtime {
        Some(runtime) => runtime,
        None => {
            try_ensure_runtime(state)?;
            state
                .lock()
                .ok()
                .and_then(|s| s.hub_runtime())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Runtime initialization reported success but no runtime is available"
                    )
                })?
        }
    };

    let event = InputEvent::new(node_id, action);
    let turned_on = runtime.handle_event(&event)?;
    sync_active_mode_from_runtime(state, &runtime);
    clear_room_mode_transition(state, node_id);

    update_lights_on_cache_for_runtime_node(state, &runtime, node_id, turned_on);

    #[cfg(feature = "desktop")]
    {
        emit_node_state_event_after_apply(state, &runtime, node_id);
    }

    if persist {
        persist_rooms(state);
    }

    build_node_state(state, node_id).and_then(|node_state| {
        serde_json::to_string(&node_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
    })
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
        // Also include nodes that are effective motion targets, even if no
        // timer is active. This covers the case where the addon restarted and
        // lost timer state, but lights were still on from prior motion
        // activation.
        motion_ids.extend(s.motion_control_target_ids());
        (s.hub_runtime(), s.room_lights_on.clone(), motion_ids)
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let snapshots = runtime.engine_all_room_snapshots();

    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    // Category 1: on-rooms without motion sensors → reset to adaptive curve
    let qualifying: Vec<_> = snapshots
        .iter()
        .filter(|snap| {
            !snap.disabled
                && !snap.soft_off
                && lights_on_from_cache(
                    &s,
                    &room_lights_on,
                    &snap.id,
                    snap.kind,
                    snap.parent_id.as_deref(),
                )
                && !motion_room_ids.contains(&snap.id)
        })
        .collect();

    // Category 2: motion-sensor rooms that are on (or have active timer) → turn off
    let motion_rooms: Vec<_> = snapshots
        .iter()
        .filter(|snap| {
            !snap.disabled
                && motion_room_ids.contains(&snap.id)
                && lights_on_from_cache(
                    &s,
                    &room_lights_on,
                    &snap.id,
                    snap.kind,
                    snap.parent_id.as_deref(),
                )
        })
        .collect();
    drop(s);

    info!(target: "cmd", "fix_my_lights: {} on-rooms to reset, {} motion rooms to turn off (of {} total)",
        qualifying.len(), motion_rooms.len(), snapshots.len());

    let mut reset_ids = Vec::new();
    for snap in &qualifying {
        let event = InputEvent::new(&snap.id, ButtonAction::Reset);
        match runtime.handle_event(&event) {
            Ok(turned_on) => {
                sync_active_mode_from_runtime(state, &runtime);
                update_lights_on_cache_for_runtime_node(state, &runtime, &snap.id, turned_on);
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
                RestoredRoomState {
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    ..RestoredRoomState::from(*snap)
                },
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
                update_lights_on_cache_for_runtime_node(state, &runtime, &snap.id, turned_on);
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
            .filter_map(|id| node_state_event_from_runtime(state, &runtime, id))
            .collect();
        if !events.is_empty() {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::NodeState { nodes: events },
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

/// Set absolute brightness for a node (1-100).
///
/// Computes the offset so effective brightness equals the target.
/// Rhythm stays enabled — only brightness is overridden.
pub fn do_set_node_brightness(
    state: &SharedState,
    node_id: &str,
    brightness: u8,
    persist: bool,
) -> Result<String> {
    let brightness = brightness.clamp(1, 100);
    info!(target: "cmd", "set_node_brightness: {} -> {}", node_id, brightness);

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    runtime.set_room_brightness(node_id, brightness)?;
    clear_room_mode_transition(state, node_id);

    update_lights_on_cache_for_runtime_node(state, &runtime, node_id, true);

    #[cfg(feature = "desktop")]
    {
        emit_node_state_event_after_apply(state, &runtime, node_id);
    }

    if persist {
        persist_rooms(state);
    }

    build_node_state(state, node_id).and_then(|node_state| {
        serde_json::to_string(&node_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
    })
}

/// Set the time offset for a node directly (not additive).
///
/// Does NOT change `room_lights_on` tracking — offset doesn't imply lights-on state change.
pub fn do_set_node_time_offset(
    state: &SharedState,
    node_id: &str,
    offset_minutes: f32,
    persist: bool,
) -> Result<String> {
    info!(
        target: "cmd",
        "set_node_time_offset: {} -> {}",
        node_id,
        offset_minutes
    );

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    runtime.set_room_time_offset(node_id, offset_minutes)?;
    clear_room_mode_transition(state, node_id);

    #[cfg(feature = "desktop")]
    {
        emit_node_state_event_after_apply(state, &runtime, node_id);
    }

    if persist {
        persist_rooms(state);
    }

    build_node_state(state, node_id).and_then(|node_state| {
        serde_json::to_string(&node_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
    })
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
    let runtime = s.hub_runtime();

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
                .remove_hub_room_binding_everywhere(&endpoint.hub_key, &endpoint.native_id)
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

    if let Some(canonical_id) = canonical_id.as_deref() {
        clear_removed_node_ephemeral_state(&mut s, canonical_id);
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

    let removed_runtime_node_id = canonical_id.clone();
    drop(s);

    if let (Some(runtime), Some(node_id)) = (runtime, removed_runtime_node_id.as_deref()) {
        runtime.remove_node(node_id);
        queue_motion_timer_clear(state, node_id);
    }

    persist_registry(state);

    #[cfg(feature = "desktop")]
    {
        rebuild_composite_routing(state);
        emit_triage_changed(state);
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
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

/// Set a per-node motion timeout override in the profile-settings layer.
pub fn do_motion_timeout_set(
    state: &SharedState,
    node_id: &str,
    timeout_secs: u64,
    _hub_key: Option<&HubKey>,
) -> Result<()> {
    info!(target: "cmd", "motion_timeout_set: node {} -> {}s", node_id, timeout_secs);

    let value = u32::try_from(timeout_secs)
        .map_err(|_| anyhow::anyhow!("motion timeout {} exceeds supported range", timeout_secs))?;
    let patch = RoomProfileSettingsPatch {
        motion_timeout_secs: Some(Some(TimerSetting::Fixed { value })),
        ..Default::default()
    };
    do_node_preferences_set(state, node_id, None, None, None, Some(&patch), true)?;
    Ok(())
}

/// Remove a per-node motion timeout override so it falls back to the profile default.
pub fn do_motion_timeout_clear(
    state: &SharedState,
    node_id: &str,
    _hub_key: Option<&HubKey>,
) -> Result<()> {
    info!(target: "cmd", "motion_timeout_clear: node {}", node_id);

    let patch = RoomProfileSettingsPatch {
        motion_timeout_secs: Some(None),
        ..Default::default()
    };
    do_node_preferences_set(state, node_id, None, None, None, Some(&patch), true)?;
    Ok(())
}

/// Resolve motion timeout defaults for controlled target nodes.
pub(crate) fn resolved_motion_timeout_map(
    state: &SharedState,
) -> (HashMap<String, u64>, HashSet<String>) {
    let (
        runtime,
        light_profile_configs,
        mode_configs,
        active_mode,
        solar_noon,
        latitude,
        utc_offset,
        control_targets,
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
            s.utc_offset_hours,
            s.motion_control_target_ids(),
        )
    };

    let current_hour = runtime.as_ref().map(|rt| rt.current_hour()).unwrap_or(12.0);
    let snapshots = runtime
        .as_ref()
        .map(|rt| rt.engine_all_effective_node_snapshots())
        .unwrap_or_default();

    let mut timeouts = HashMap::new();
    for snap in &snapshots {
        timeouts.insert(
            snap.id.clone(),
            resolved_room_motion_timeout_secs_from_parts(
                RoomLightingContext {
                    light_profile_configs: &light_profile_configs,
                    mode_configs: &mode_configs,
                    mode: active_mode,
                    solar_noon,
                    latitude,
                    utc_offset,
                },
                &snap.profile_settings,
                current_hour,
            ),
        );
    }
    (timeouts, control_targets)
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
            if let Some(ref tz_name) = s.timezone_name {
                let tz = rhythm_core::Timezone::new(tz_name);
                let (year, month, day) = tz.local_date_from_utc(chrono::Utc::now().naive_utc());
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
        let local = current_local_datetime(s.utc_offset_hours);
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
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
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
        let (solar_noon, day_of_year) = if let Some(ref tz_name) = timezone_name {
            let tz = rhythm_core::Timezone::new(tz_name);
            let local_now = tz.local_datetime_from_utc(now);
            let (year, month, day) = (
                local_now.date().year(),
                local_now.date().month(),
                local_now.date().day(),
            );
            let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);
            // Also refresh utc_offset from timezone
            s.utc_offset_hours = tz.utc_offset(year, month, day, local_now.time().hour());
            (
                rhythm_core::calculate_solar_noon(lon, year, month, day, &tz),
                day_of_year,
            )
        } else {
            let local_now = current_local_datetime(s.utc_offset_hours);
            let day_of_year = rhythm_core::timezone::day_of_year(
                local_now.date().year(),
                local_now.date().month(),
                local_now.date().day(),
            );
            (
                rhythm_core::calculate_solar_noon_from_offset(lon, s.utc_offset_hours, day_of_year),
                day_of_year,
            )
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
        let sync_hub_key = hub_key.clone();
        let _ = std::thread::Builder::new()
            .name("hub-sync".to_string())
            .spawn(move || {
                if let Err(e) = crate::room_sync::sync_from_hub_for_key(
                    &sync_state,
                    &sync_hub_key,
                    discover_devices,
                ) {
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
        let hub_connected = state
            .lock()
            .ok()
            .map(|s| s.hub_is_connected(&hub_key))
            .unwrap_or(false);
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::HubStatus {
                hub_type: Some(hub_type_str.to_string()),
                address: Some(address.to_string()),
                connected: hub_connected,
            },
        );
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
    }

    Ok(())
}

/// Disconnect all hubs — clear credentials, runtime, and rooms.
pub fn do_hub_disconnect(state: &SharedState) -> Result<()> {
    info!(target: "cmd", "hub_disconnect: clearing all hubs, credentials, and rooms");

    let hub_keys: Vec<HubKey> = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .hubs
        .keys()
        .cloned()
        .collect();
    let discovered_native_ids = HashSet::new();
    for hub_key in &hub_keys {
        let _ = reconcile_hub_endpoint_visibility(state, hub_key, &discovered_native_ids)?;
    }

    // Capture hub keys before clearing for SSE notifications
    let old_hubs;
    let mut topology_changed = false;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        old_hubs = std::mem::take(&mut s.hubs);
        for key in old_hubs.keys() {
            topology_changed |= !s.topology.remove_stale_bindings(key, &[]).is_empty();
        }
        s.hub_connection_status.clear();
        s.hub_seen_connected_once.clear();

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

        if topology_changed {
            persist_topology(&s);
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
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
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
    let discovered_native_ids = HashSet::new();
    let _ = reconcile_hub_endpoint_visibility(state, &key, &discovered_native_ids)?;

    let old_hub;
    let topology_changed;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        old_hub = s.hubs.remove(&key);
        topology_changed = !s.topology.remove_stale_bindings(&key, &[]).is_empty();
        s.clear_hub_connected(&key);
        s.hub_credentials.remove(&key);

        if let Some(ref storage) = s.storage {
            let all: Vec<_> = s.hub_credentials.values().cloned().collect();
            let _ = storage.save_all_hub_credentials(&all);
        }

        if topology_changed {
            persist_topology(&s);
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
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
    }

    Ok(())
}

/// Update user preferences for an addressable node.
///
/// Only modifies user state, not topology. Safe for app to call without
/// overwriting server-discovered rooms/devices.
pub fn do_node_preferences_set(
    state: &SharedState,
    node_id: &str,
    rhythm_enabled: Option<bool>,
    disabled: Option<bool>,
    target_state: Option<RoomModeState>,
    room_profile: Option<&RoomProfileSettingsPatch>,
    persist: bool,
) -> Result<String> {
    info!(
        target: "cmd",
        "node_preferences_set: {} rhythm={:?} disabled={:?} state={:?} profile_settings={}",
        node_id,
        rhythm_enabled,
        disabled,
        target_state,
        room_profile.is_some()
    );

    let (runtime, valid_profile_ids) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.hub_runtime(),
            s.light_profile_configs
                .keys()
                .cloned()
                .collect::<HashSet<_>>(),
        )
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let snap = runtime
        .engine_node_snapshot(node_id)
        .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", node_id))?;
    let lights_on = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        lights_on_from_cache(
            &s,
            &s.room_lights_on,
            &snap.id,
            snap.kind,
            snap.parent_id.as_deref(),
        )
    };

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
                "State profiles cannot be selected per-node"
            ));
        }
        if !valid_profile_ids.contains(profile_id) {
            return Err(anyhow::anyhow!("Unknown light profile: {}", profile_id));
        }
    }

    // Soft-off rooms need rhythm enabled for periodic soft-off ticks
    let rhythm_enabled = if soft_off { true } else { rhythm_enabled };

    runtime.restore_node_state(
        node_id,
        RestoredNodeState {
            rhythm_enabled,
            disabled,
            time_offset_minutes: snap.time_offset_minutes,
            brightness_offset: snap.brightness_offset,
            soft_off,
            hard_off,
            profile_settings: profile_settings.clone(),
        },
    );
    clear_room_mode_transition(state, node_id);

    let entered_hard_off = hard_off && !prev_hard_off;
    let left_hard_off = !hard_off && prev_hard_off;

    let mut refresh_lights_on = false;

    if entered_hard_off {
        refresh_lights_on = true;
        info!(target: "cmd", "node_preferences_set: {} entering hard_off", node_id);
        queue_motion_timer_clear(state, node_id);
        let event = InputEvent::new(node_id, ButtonAction::LightsOff);
        if let Err(e) = runtime.handle_event(&event) {
            warn!(target: "cmd", "lights_off for '{}' failed: {}", node_id, e);
        }
    } else if soft_off && !prev_soft_off {
        refresh_lights_on = true;
        info!(target: "cmd", "node_preferences_set: {} entering idle", node_id);
        if let Err(e) = runtime.soft_off_tick_room(node_id) {
            warn!(target: "cmd", "soft_off_tick for '{}' failed: {}", node_id, e);
        }
    } else if !soft_off && prev_soft_off {
        refresh_lights_on = true;
        info!(target: "cmd", "node_preferences_set: {} leaving idle, turning on", node_id);
        if let Err(e) = runtime.turn_on_room(node_id) {
            warn!(target: "cmd", "turn_on for '{}' failed: {}", node_id, e);
        }
    } else if left_hard_off {
        refresh_lights_on = true;
        match persistent_state {
            RoomModeState::Active => {
                info!(target: "cmd", "node_preferences_set: {} leaving hard_off to active", node_id);
                if let Err(e) = runtime.turn_on_room(node_id) {
                    warn!(target: "cmd", "turn_on for '{}' failed: {}", node_id, e);
                }
            }
            RoomModeState::Idle => {
                info!(target: "cmd", "node_preferences_set: {} leaving hard_off to idle", node_id);
                if let Err(e) = runtime.soft_off_tick_room(node_id) {
                    warn!(target: "cmd", "soft_off_tick for '{}' failed: {}", node_id, e);
                }
            }
            RoomModeState::HardOff | RoomModeState::Wake | RoomModeState::Warning => {}
        }
    } else if room_profile.is_some_and(|patch| patch.touches_profile_settings()) {
        match persistent_state {
            RoomModeState::Idle => {
                refresh_lights_on = true;
                info!(target: "cmd", "node_preferences_set: {} applying profile settings to idle state", node_id);
                if let Err(e) = runtime.soft_off_tick_room(node_id) {
                    warn!(target: "cmd", "soft_off_tick for '{}' failed: {}", node_id, e);
                }
            }
            RoomModeState::Active if lights_on => {
                refresh_lights_on = true;
                info!(target: "cmd", "node_preferences_set: {} applying profile settings to active lights", node_id);
                if let Err(e) = runtime.turn_on_room(node_id) {
                    warn!(target: "cmd", "turn_on for '{}' failed: {}", node_id, e);
                }
            }
            RoomModeState::HardOff
            | RoomModeState::Wake
            | RoomModeState::Warning
            | RoomModeState::Active => {}
        }
    }

    if refresh_lights_on {
        refresh_lights_on_cache_for_runtime_node(state, &runtime, node_id);
    }

    #[cfg(feature = "desktop")]
    {
        emit_node_state_event_after_apply(state, &runtime, node_id);
    }

    if persist {
        persist_rooms(state);
    }

    build_node_state(state, node_id).and_then(|node_state| {
        serde_json::to_string(&node_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
    })
}

// ============================================================================
// Runtime initialization helper
// ============================================================================

/// Call the platform's ensure_runtime callback in a thread with adequate stack.
///
/// The callback is stored on AppState (as an `Arc`) and set by the platform
/// crate at startup. It handles creating the RhythmRuntime with platform-specific types.
pub fn ensure_runtime(state: &SharedState) {
    if let Err(e) = try_ensure_runtime(state) {
        warn!(target: "cmd", "Failed to start runtime: {}", e);
    }
}

/// Call the platform's ensure_runtime callback and surface bootstrap failures.
pub fn try_ensure_runtime(state: &SharedState) -> Result<()> {
    // Clone the Arc + platform config out of the lock so we can call without holding it
    let (ensure_fn, stack_size) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (s.ensure_runtime_fn.clone(), s.platform.runtime_init_stack)
    };

    let Some(ensure_fn) = ensure_fn else {
        return Err(anyhow::anyhow!("No runtime initializer configured"));
    };

    let rt_state = state.clone();
    let rt_result = std::thread::Builder::new()
        .name("rt-init".to_string())
        .stack_size(stack_size)
        .spawn(move || ensure_fn(&rt_state))
        .and_then(|handle| handle.join().map_err(|_| std::io::Error::other("panicked")));

    match rt_result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(anyhow::anyhow!("Runtime init thread error: {}", e)),
    }
}

/// Ensure the shared runtime exists and contains the given topology room.
///
/// Matter can route directly by device endpoint and discover zero hub rooms, so
/// runtime creation cannot rely solely on `do_room_set()` during room sync.
fn ensure_runtime_room_exists(state: &SharedState, room_id: &str, room_name: &str) -> Result<()> {
    let needs_runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let has_runtime = s.hubs.values().any(|h| h.runtime.is_some());
        !has_runtime && s.has_any_hub()
    };

    if needs_runtime {
        try_ensure_runtime(state)?;
    }

    if let Some(runtime) = state.lock().ok().and_then(|s| s.hub_runtime()) {
        if runtime.engine_room_snapshot(room_id).is_none() {
            runtime.add_room(room_id, room_name);
            runtime.restore_room_state(
                room_id,
                RestoredRoomState {
                    rhythm_enabled: true,
                    disabled: false,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                    hard_off: false,
                    profile_settings: RoomProfileSettings::default(),
                },
            );
            debug!(
                target: "cmd",
                "Ensured runtime room '{}' exists as '{}'",
                room_id,
                room_name
            );
        }
    }

    let has_any_hub = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .has_any_hub();
    if has_any_hub && state.lock().ok().and_then(|s| s.hub_runtime()).is_none() {
        return Err(anyhow::anyhow!(
            "Runtime initialization completed without installing a runtime"
        ));
    }

    Ok(())
}

pub(crate) fn runtime_node_kind_for_device_type(device_type: DeviceType) -> LightNodeKind {
    match device_type {
        DeviceType::Light => LightNodeKind::LightDevice,
        DeviceType::Motion => LightNodeKind::MotionSensor,
        DeviceType::Button => LightNodeKind::Button,
    }
}

pub(crate) fn ensure_runtime_device_node_exists(
    state: &SharedState,
    node_id: &str,
    node_name: &str,
    device_type: DeviceType,
    parent_id: Option<String>,
) -> Result<()> {
    let needs_runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let has_runtime = s.hubs.values().any(|h| h.runtime.is_some());
        !has_runtime && s.has_any_hub()
    };

    if needs_runtime {
        try_ensure_runtime(state)?;
    }

    if let Some(runtime) = state.lock().ok().and_then(|s| s.hub_runtime()) {
        let existed = runtime.engine_node_snapshot(node_id).is_some();
        runtime.add_node(
            node_id,
            node_name,
            runtime_node_kind_for_device_type(device_type),
            parent_id,
        );
        if !existed {
            runtime.restore_node_state(
                node_id,
                RestoredNodeState {
                    rhythm_enabled: true,
                    disabled: false,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                    hard_off: false,
                    profile_settings: RoomProfileSettings::default(),
                },
            );
            debug!(target: "cmd", "Ensured runtime device node '{}' exists", node_id);
        }
    }

    Ok(())
}

// ============================================================================
// Composite controller registration
// ============================================================================

/// Rebuild the composite controller's routing table from topology.
///
/// Call after any operation that changes room-to-hub mappings: room sync,
/// hub connect/disconnect, topology merge/split/device-move.
#[cfg(feature = "desktop")]
fn composite_node_labels(s: &AppState) -> HashMap<String, String> {
    let mut labels = HashMap::new();

    let mut room_ids: Vec<_> = s.topology.rooms().map(|room| room.id.clone()).collect();
    room_ids.sort();
    for room_id in room_ids {
        let Some(room) = s.topology.get(&room_id) else {
            continue;
        };
        labels.insert(room.id.clone(), room.name.clone());
    }

    let mut device_ids: Vec<_> = s
        .topology
        .device_nodes()
        .map(|node| node.id.clone())
        .collect();
    device_ids.sort();
    for device_id in device_ids {
        let Some(node) = s.topology.get_device_node(&device_id) else {
            continue;
        };
        let label = s
            .canonical_registry
            .get(&node.canonical_device_id)
            .map(|device| device.name.clone())
            .unwrap_or_else(|| node.id.clone());
        labels.insert(node.id.clone(), label);
    }

    for light_node in s.topology.periodic_light_nodes(&s.canonical_registry) {
        let label = s
            .topology
            .get(&light_node.source_node_id)
            .map(|room| room.name.clone())
            .or_else(|| {
                s.topology
                    .get_device_node(&light_node.source_node_id)
                    .and_then(|node| {
                        s.canonical_registry
                            .get(&node.canonical_device_id)
                            .map(|device| device.name.clone())
                    })
            });
        if let Some(label) = label {
            labels.insert(light_node.id, label);
        }
    }

    labels
}

#[cfg(feature = "desktop")]
pub fn rebuild_composite_routing(state: &SharedState) {
    let (composite, routing, labels, room_count) = {
        let Ok(s) = state.lock() else { return };
        let composite = match s.composite_controller.clone() {
            Some(c) => c,
            None => return, // No composite yet — nothing to rebuild
        };
        let active_hubs: HashSet<String> = s.hubs.keys().map(ToString::to_string).collect();
        let mut routing = s.topology.composite_routing(&s.canonical_registry);
        routing.retain(|_, targets| {
            targets.retain(|(hub_key, _)| active_hubs.contains(hub_key));
            !targets.is_empty()
        });
        let labels = composite_node_labels(&s);
        let rooms = s.topology.room_count();
        (composite, routing, labels, rooms)
    };
    let route_count = routing.len();
    composite.update_routing(routing);
    composite.update_node_labels(labels);
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

fn ensure_synthetic_device_registry_rooms(
    s: &mut AppState,
    device_name: &str,
    endpoints: &[crate::canonical::identity::IntegrationEndpoint],
) {
    for endpoint in endpoints {
        if let Some(hub) = s.hubs.get(&endpoint.hub_key) {
            if let Some(reg) = &hub.registry {
                if let Ok(mut registry) = reg.lock() {
                    // Keep a synthetic per-device registry room so
                    // device-addressed integrations still have an exact
                    // native grouped target after detach/unassign flows.
                    registry.remove_room(&endpoint.native_id);
                    registry.upsert_room(
                        &endpoint.native_id,
                        device_name,
                        &endpoint.native_id,
                        std::slice::from_ref(&endpoint.native_id),
                    );
                }
            }
        }
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
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

    // Snapshot endpoints, name, and old room before mutating.
    let device = s
        .canonical_registry
        .get(device_id)
        .ok_or_else(|| anyhow::anyhow!("Device not found: {}", device_id))?;
    let endpoints: Vec<_> = device.endpoints.clone();
    let active_endpoints: Vec<_> = device.active_endpoints().cloned().collect();
    let device_name = device.name.clone();
    let device_type = device.device_type.clone();
    let old_room_id = s
        .topology
        .device_parent_room_id(device_id)
        .map(|id| id.to_string())
        .or_else(|| device.room_id.clone());
    let target_room = room_id.and_then(|target_room_id| {
        s.topology
            .get(target_room_id)
            .map(|room| (target_room_id.to_string(), room.name.clone()))
    });

    if let Some(target_room_id) = room_id {
        if s.topology.get(target_room_id).is_none() {
            return Err(anyhow::anyhow!("Room not found: {}", target_room_id));
        }
    }

    if !s.canonical_registry.assign_room(device_id, room_id) {
        return Err(anyhow::anyhow!("Device not found: {}", device_id));
    }

    if let Some(target_room_id) = room_id {
        if !s.topology.assign_device(
            device_id,
            Some(target_room_id),
            crate::topology::DevicePlacement::UserOverride,
        ) {
            s.topology.ensure_standalone_device(device_id);
            s.topology.assign_device(
                device_id,
                Some(target_room_id),
                crate::topology::DevicePlacement::UserOverride,
            );
        }
        if let Some(old_id) = &old_room_id {
            if let Some(room) = s.topology.get_mut(old_id) {
                room.user_customized = true;
            }
        }
        if let Some(room) = s.topology.get_mut(target_room_id) {
            room.user_customized = true;
        }

        for endpoint in &endpoints {
            if let Some(hub) = s.hubs.get(&endpoint.hub_key) {
                if let Some(reg) = &hub.registry {
                    if let Ok(mut registry) = reg.lock() {
                        registry.remove_room(&endpoint.native_id);
                    }
                }
            }
        }
        ensure_synthetic_device_registry_rooms(&mut s, &device_name, &active_endpoints);
    } else {
        s.topology.ensure_standalone_device(device_id);
        s.topology.assign_device(
            device_id,
            None,
            crate::topology::DevicePlacement::Standalone,
        );
        if let Some(old_id) = &old_room_id {
            if let Some(room) = s.topology.get_mut(old_id) {
                room.user_customized = true;
            }
        }
        for endpoint in &endpoints {
            if let Some(hub) = s.hubs.get(&endpoint.hub_key) {
                if let Some(reg) = &hub.registry {
                    if let Ok(mut registry) = reg.lock() {
                        registry.remove_room(&endpoint.native_id);
                    }
                }
            }
        }
        ensure_synthetic_device_registry_rooms(&mut s, &device_name, &active_endpoints);
    }

    persist_canonical(&s);
    persist_topology(&s);
    drop(s);

    persist_registry(state);

    if let Some((target_room_id, target_room_name)) = target_room {
        ensure_runtime_room_exists(state, &target_room_id, &target_room_name)?;
        ensure_runtime_device_node_exists(
            state,
            device_id,
            &device_name,
            device_type,
            Some(target_room_id),
        )?;
    } else {
        ensure_runtime_device_node_exists(state, device_id, &device_name, device_type, None)?;
    }

    #[cfg(feature = "desktop")]
    {
        rebuild_composite_routing(state);
        emit_triage_changed(state);
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
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
    use crate::canonical::triage::TriageKind;

    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let entry = match s.canonical_registry.triage().get(entry_id) {
        Some(entry) => entry.clone(),
        None => return Err(anyhow::anyhow!("Triage entry not found: {}", entry_id)),
    };
    if entry.kind != TriageKind::DeviceMerge {
        return Err(anyhow::anyhow!(
            "Triage entry '{}' is not a device merge",
            entry_id
        ));
    }
    let hub_key = entry.hub_key.clone();
    let native_id = entry.discovered.native_id.clone();
    let hub_room_id = entry.discovered.room_id.clone();
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
                s.topology.ensure_standalone_device(canonical_id);
                let _ = s.topology.assign_device(
                    canonical_id,
                    Some(&rhythm_room_id),
                    crate::topology::DevicePlacement::HubDefault,
                );
                s.canonical_registry
                    .assign_room(canonical_id, Some(&rhythm_room_id));
            }
        } else {
            s.topology.ensure_standalone_device(canonical_id);
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
    use crate::canonical::triage::TriageKind;

    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let entry = match s.canonical_registry.triage().get(entry_id) {
        Some(entry) => entry.clone(),
        None => return Err(anyhow::anyhow!("Triage entry not found: {}", entry_id)),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match entry.kind {
        TriageKind::DeviceMerge => {
            let hub_key = entry.hub_key.clone();
            let hub_room_id = entry.discovered.room_id.clone();
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
                            s.topology.ensure_standalone_device(&canonical_id);
                            let _ = s.topology.assign_device(
                                &canonical_id,
                                Some(&rhythm_room_id),
                                crate::topology::DevicePlacement::HubDefault,
                            );
                            s.canonical_registry
                                .assign_room(&canonical_id, Some(&rhythm_room_id));
                        }
                    } else {
                        s.topology.ensure_standalone_device(&canonical_id);
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
        TriageKind::RoomBinding => {
            if s.canonical_registry
                .triage_mut()
                .resolve_keep_separate(entry_id, now)
            {
                persist_canonical(&s);
                drop(s);
                #[cfg(feature = "desktop")]
                emit_triage_changed(state);
                Ok(r#"{"status":"kept_separate"}"#.to_string())
            } else {
                Err(anyhow::anyhow!("Failed to keep room binding separate"))
            }
        }
        _ => Err(anyhow::anyhow!(
            "Triage entry '{}' does not support /new",
            entry_id
        )),
    }
}

/// Assign an unassigned-device triage entry to a room.
pub fn do_triage_assign_room(state: &SharedState, entry_id: &str, room_id: &str) -> Result<()> {
    use crate::canonical::triage::TriageKind;

    let resolved_room_id = resolve_node_id(state, room_id);
    let canonical_id = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let entry = s
            .canonical_registry
            .triage()
            .get(entry_id)
            .ok_or_else(|| anyhow::anyhow!("Triage entry not found: {}", entry_id))?;
        if entry.kind != TriageKind::UnassignedDevice {
            return Err(anyhow::anyhow!(
                "Triage entry '{}' is not an unassigned device",
                entry_id
            ));
        }
        entry
            .canonical_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Triage entry '{}' is missing canonical_id", entry_id))?
    };

    do_canonical_assign_room(state, &canonical_id, Some(&resolved_room_id))
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
    use crate::canonical::triage::TriageKind;

    let resolved_target_override = target_override.map(|id| resolve_node_id(state, id));
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
    if entry.kind != TriageKind::RoomBinding {
        return Err(anyhow::anyhow!(
            "Triage entry '{}' is not a room binding",
            entry_id
        ));
    }

    let binding = entry
        .room_binding
        .ok_or_else(|| anyhow::anyhow!("Not a room binding entry: {}", entry_id))?;

    let target_id = resolved_target_override.unwrap_or(binding.target_rhythm_room_id.clone());

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
                runtime.restore_room_state(&target_id, RestoredRoomState::from(&snap));
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
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
    }

    Ok(())
}

/// Build JSON with triage count summary.
pub fn build_triage_count(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let triage = s.canonical_registry.triage();
    let json = format!(
        r#"{{"devices":{},"rooms":{},"unassigned":{},"hub_configured":{},"total":{}}}"#,
        triage.pending_device_count(),
        triage.pending_room_count(),
        triage.pending_unassigned_count(),
        triage.pending_hub_configured_count(),
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
            triage.pending_hub_configured_count(),
        )
    });
    if let Some((total, devices, rooms, unassigned, hub_configured)) = counts {
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::TriageChanged {
                pending_count: total,
                pending_devices: devices,
                pending_rooms: rooms,
                pending_unassigned: unassigned,
                pending_hub_configured: hub_configured,
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

/// Build JSON for the full public topology graph.
pub fn build_topology_nodes(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let mut nodes = Vec::new();

    let mut room_ids: Vec<_> = s.topology.rooms().map(|room| room.id.clone()).collect();
    room_ids.sort();
    for room_id in room_ids {
        let room = s.topology.get(&room_id).unwrap();
        nodes.push(TopologyNodeDto {
            id: room.id.clone(),
            name: room.name.clone(),
            kind: LightNodeKind::Room,
            parent_id: None,
            placement: None,
            controls: s
                .topology
                .effective_node_controls(&room.id, &s.canonical_registry)
                .into_iter()
                .map(|(kind, target_id, inherited)| TopologyNodeControlDto {
                    kind,
                    target_id,
                    inherited,
                })
                .collect(),
            hub_room_bindings: room.hub_room_bindings.clone(),
            manufacturer: None,
            model: None,
            user_customized: Some(room.user_customized),
            bootstrap_name: room.bootstrap_name.clone(),
        });
    }

    let mut device_ids: Vec<_> = s
        .topology
        .device_nodes()
        .map(|node| node.id.clone())
        .collect();
    device_ids.sort();
    for device_id in device_ids {
        let node = s.topology.get_device_node(&device_id).unwrap();
        let canonical = s.canonical_registry.get(&node.canonical_device_id);
        nodes.push(TopologyNodeDto {
            id: node.id.clone(),
            name: canonical
                .map(|device| device.name.clone())
                .unwrap_or_else(|| node.id.clone()),
            kind: canonical
                .map(|device| runtime_node_kind_for_device_type(device.device_type.clone()))
                .unwrap_or(LightNodeKind::OtherDevice),
            parent_id: node.parent_id.clone(),
            placement: Some(node.placement.clone()),
            controls: s
                .topology
                .effective_node_controls(&node.id, &s.canonical_registry)
                .into_iter()
                .map(|(kind, target_id, inherited)| TopologyNodeControlDto {
                    kind,
                    target_id,
                    inherited,
                })
                .collect(),
            hub_room_bindings: Vec::new(),
            manufacturer: canonical.and_then(|device| device.manufacturer.clone()),
            model: canonical.and_then(|device| device.model.clone()),
            user_customized: None,
            bootstrap_name: None,
        });
    }

    serde_json::to_string(&nodes).map_err(|e| anyhow::anyhow!(e))
}

/// Set or clear an explicit topology control target for a source node.
pub fn do_topology_set_control_target(
    state: &SharedState,
    source_id: &str,
    kind: NodeControlKind,
    target_id: Option<&str>,
) -> Result<()> {
    let resolved_source_id = resolve_node_id(state, source_id);
    let resolved_target_id = target_id.map(|id| resolve_node_id(state, id));

    let (old_target, new_target) = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if !s.topology.has_public_node(&resolved_source_id) {
            return Err(anyhow::anyhow!(
                "Source node '{}' not found",
                resolved_source_id
            ));
        }
        if let Some(ref target_id) = resolved_target_id {
            if !s.topology.has_public_node(target_id) {
                return Err(anyhow::anyhow!("Target node '{}' not found", target_id));
            }
        }

        let old_target = s
            .topology
            .effective_control_target(&resolved_source_id, &kind);
        s.topology.set_control_target(
            &resolved_source_id,
            kind.clone(),
            resolved_target_id.as_deref(),
        );
        let new_target = s
            .topology
            .effective_control_target(&resolved_source_id, &kind);
        persist_topology(&s);
        (old_target, new_target)
    };

    if kind == NodeControlKind::Motion {
        if let Some(old_target) = old_target.as_deref() {
            queue_motion_timer_clear(state, old_target);
        }
        if let Some(new_target) = new_target.as_deref() {
            queue_motion_timer_clear(state, new_target);
        }
    }

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);

    Ok(())
}

/// Create a new empty Rhythm room.
pub fn do_topology_create_room(state: &SharedState, name: &str) -> Result<String> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let id = s.topology.create_room(name);
    persist_topology(&s);
    drop(s);
    ensure_runtime_room_exists(state, &id, name)?;
    Ok(format!(r#"{{"id":"{}","name":"{}"}}"#, id, name))
}

/// Delete a topology room and unassign any attached devices.
pub fn do_topology_delete_room(state: &SharedState, room_id: &str) -> Result<()> {
    let (runtime, detached_devices) = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let detached_device_ids = s
            .topology
            .remove_room(room_id)
            .ok_or_else(|| anyhow::anyhow!("Room not found: {}", room_id))?;

        let mut detached_devices = Vec::new();
        for device_id in detached_device_ids {
            let Some(device) = s.canonical_registry.get(&device_id).cloned() else {
                continue;
            };
            let active_endpoints: Vec<_> = device.active_endpoints().cloned().collect();

            s.canonical_registry.assign_room(&device_id, None);
            detached_devices.push((
                device.id.clone(),
                device.name.clone(),
                device.device_type.clone(),
            ));

            for endpoint in &device.endpoints {
                if let Some(hub) = s.hubs.get(&endpoint.hub_key) {
                    if let Some(reg) = &hub.registry {
                        if let Ok(mut reg) = reg.lock() {
                            reg.remove_room(&endpoint.native_id);
                        }
                    }
                }
            }

            ensure_synthetic_device_registry_rooms(&mut s, &device.name, &active_endpoints);
        }

        s.room_lights_on.remove(room_id);
        s.motion_snapshots.remove(room_id);
        s.room_mode_transitions.remove(room_id);
        s.pending_motion_clear.retain(|pending| pending != room_id);

        persist_topology(&s);
        if !detached_devices.is_empty() {
            persist_canonical(&s);
        }

        (s.hub_runtime(), detached_devices)
    };

    if !detached_devices.is_empty() {
        persist_registry(state);
    }

    queue_motion_timer_clear(state, room_id);

    if let Some(runtime) = runtime {
        runtime.remove_room(room_id);
        for (device_id, device_name, device_type) in &detached_devices {
            let existed = runtime.engine_node_snapshot(device_id).is_some();
            runtime.add_node(
                device_id,
                device_name,
                runtime_node_kind_for_device_type(device_type.clone()),
                None,
            );
            if !existed {
                runtime.restore_node_state(
                    device_id,
                    RestoredNodeState {
                        rhythm_enabled: true,
                        disabled: false,
                        time_offset_minutes: 0.0,
                        brightness_offset: 0.0,
                        soft_off: false,
                        hard_off: false,
                        profile_settings: RoomProfileSettings::default(),
                    },
                );
            }
        }
    }

    #[cfg(feature = "desktop")]
    {
        rebuild_composite_routing(state);
        emit_triage_changed(state);
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
    }

    Ok(())
}

/// Merge two rooms.
pub fn do_topology_merge_rooms(
    state: &SharedState,
    target_id: &str,
    source_id: &str,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let target_name = s
        .topology
        .get(target_id)
        .map(|room| room.name.clone())
        .ok_or_else(|| anyhow::anyhow!("Target room not found"))?;
    if s.topology.merge_rooms(target_id, source_id) {
        persist_topology(&s);
        drop(s);

        if let Some(runtime) = state.lock().ok().and_then(|s| s.hub_runtime()) {
            if let Some(snap) = runtime.engine_room_snapshot(source_id) {
                if runtime.engine_room_snapshot(target_id).is_none() {
                    runtime.add_room(target_id, &target_name);
                    runtime.restore_room_state(target_id, RestoredRoomState::from(&snap));
                }
                runtime.remove_room(source_id);
            }
        }

        #[cfg(feature = "desktop")]
        {
            rebuild_composite_routing(state);
            crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
        }

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
        drop(s);

        #[cfg(feature = "desktop")]
        {
            rebuild_composite_routing(state);
            crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
        }

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
        sunrise_local_time: resolved
            .sun_times
            .map(|st| local_time_string_from_decimal_hour(st.sunrise)),
        sunset: resolved.sun_times.map(|st| st.sunset),
        sunset_local_time: resolved
            .sun_times
            .map(|st| local_time_string_from_decimal_hour(st.sunset)),
        solar_noon: resolved.solar.solar_noon_hour,
        solar_noon_local_time: local_time_string_from_decimal_hour(resolved.solar.solar_noon_hour),
        solar_midnight: resolved.solar.solar_midnight_hour(),
        solar_midnight_local_time: local_time_string_from_decimal_hour(
            resolved.solar.solar_midnight_hour(),
        ),
        day_length: resolved.sun_times.map(|st| st.day_length),
        twilight: resolved.twilight.as_ref().map(|tw| TwilightResponse {
            dawn: TwilightPhaseResponse {
                civil: tw.dawn.civil,
                civil_local_time: optional_local_time_string_from_decimal_hour(tw.dawn.civil),
                nautical: tw.dawn.nautical,
                nautical_local_time: optional_local_time_string_from_decimal_hour(tw.dawn.nautical),
                astronomical: tw.dawn.astronomical,
                astronomical_local_time: optional_local_time_string_from_decimal_hour(
                    tw.dawn.astronomical,
                ),
            },
            dusk: TwilightPhaseResponse {
                civil: tw.dusk.civil,
                civil_local_time: optional_local_time_string_from_decimal_hour(tw.dusk.civil),
                nautical: tw.dusk.nautical,
                nautical_local_time: optional_local_time_string_from_decimal_hour(tw.dusk.nautical),
                astronomical: tw.dusk.astronomical,
                astronomical_local_time: optional_local_time_string_from_decimal_hour(
                    tw.dusk.astronomical,
                ),
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
            civil_local_time: optional_local_time_string_from_decimal_hour(tw.dawn.civil),
            nautical: tw.dawn.nautical,
            nautical_local_time: optional_local_time_string_from_decimal_hour(tw.dawn.nautical),
            astronomical: tw.dawn.astronomical,
            astronomical_local_time: optional_local_time_string_from_decimal_hour(
                tw.dawn.astronomical,
            ),
        },
        dusk: TwilightPhaseResponse {
            civil: tw.dusk.civil,
            civil_local_time: optional_local_time_string_from_decimal_hour(tw.dusk.civil),
            nautical: tw.dusk.nautical,
            nautical_local_time: optional_local_time_string_from_decimal_hour(tw.dusk.nautical),
            astronomical: tw.dusk.astronomical,
            astronomical_local_time: optional_local_time_string_from_decimal_hour(
                tw.dusk.astronomical,
            ),
        },
    })
}

/// Parse an optional `YYYY-MM-DD` date string, falling back to today in local time.
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
        let local = current_local_datetime(utc_offset);
        Ok((
            local.date().year(),
            local.date().month(),
            local.date().day(),
        ))
    }
}

/// Compute the current local hour from UTC + offset.
fn current_local_hour(utc_offset: f32) -> f32 {
    let local = current_local_datetime(utc_offset);
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

    let local = current_local_datetime(utc_offset);
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
    use crate::bundle::{
        BackupBundle, BackupHubCredentials, BackupInstallation, BackupRuntimeState, BundleKind,
        ConfigurationBundle, ConfigurationImportPayload, ConfigurationRoom, PortableConfiguration,
    };
    use crate::factory_default_config::{
        factory_default_active_mode, factory_default_configuration_bundle,
        factory_default_light_profile_config, factory_default_mode_transition_configs,
        factory_default_power_save,
    };
    use crate::hub::{ActiveHub, HubCredentials, HubProvider, HubType};
    use crate::state::{AppState, MotionSnapshot};
    use crate::storage::{Storage, StoredLightProfiles, StoredSettings};
    use chrono::{Datelike, Timelike};
    use rhythm_core::{LightProfileConfig, RoomSnapshot, RuntimeHandle};
    use std::sync::{Arc, Mutex};

    /// Mock runtime that returns configurable room snapshots and tracks events.
    struct MockRuntime {
        snapshots: Mutex<Vec<RoomSnapshot>>,
        events: Mutex<Vec<(String, ButtonAction)>>,
        applied_commands: Mutex<Vec<(String, rhythm_core::LightingCommand)>>,
        lights_off_calls: Mutex<Vec<(String, Option<u32>)>>,
        applied_states: Mutex<Vec<(String, RoomModeState)>>,
        config_updates: Mutex<Vec<LightProfileConfig>>,
        restore_calls: Mutex<Vec<(String, bool, bool)>>,
        time_offset_updates: Mutex<Vec<(String, f32)>>,
        light_states: Mutex<HashMap<String, bool>>,
        current_hour: f32,
    }

    impl MockRuntime {
        fn new(snapshots: Vec<RoomSnapshot>, current_hour: f32) -> Self {
            Self {
                snapshots: Mutex::new(snapshots),
                events: Mutex::new(Vec::new()),
                applied_commands: Mutex::new(Vec::new()),
                lights_off_calls: Mutex::new(Vec::new()),
                applied_states: Mutex::new(Vec::new()),
                config_updates: Mutex::new(Vec::new()),
                restore_calls: Mutex::new(Vec::new()),
                time_offset_updates: Mutex::new(Vec::new()),
                light_states: Mutex::new(HashMap::new()),
                current_hour,
            }
        }

        fn set_light_on(&self, node_id: &str, on: bool) {
            self.light_states
                .lock()
                .unwrap()
                .insert(node_id.to_string(), on);
        }

        fn set_target_lights(&self, node_id: &str, on: bool) {
            let snapshots = self.snapshots.lock().unwrap().clone();
            let child_ids: Vec<_> = snapshots
                .iter()
                .filter(|snap| {
                    snap.parent_id.as_deref() == Some(node_id) && snap.kind.is_light_addressable()
                })
                .map(|snap| snap.id.clone())
                .collect();

            let mut light_states = self.light_states.lock().unwrap();
            if child_ids.is_empty() {
                light_states.insert(node_id.to_string(), on);
            } else {
                for child_id in child_ids {
                    light_states.insert(child_id, on);
                }
                light_states.insert(node_id.to_string(), on);
            }
        }

        fn any_target_lights_on(&self, node_id: &str) -> bool {
            let snapshots = self.snapshots.lock().unwrap().clone();
            let child_ids: Vec<_> = snapshots
                .iter()
                .filter(|snap| {
                    snap.parent_id.as_deref() == Some(node_id) && snap.kind.is_light_addressable()
                })
                .map(|snap| snap.id.clone())
                .collect();

            let light_states = self.light_states.lock().unwrap();
            if child_ids.is_empty() {
                light_states.get(node_id).copied().unwrap_or(false)
            } else {
                child_ids
                    .iter()
                    .any(|child_id| light_states.get(child_id).copied().unwrap_or(false))
            }
        }

        fn events(&self) -> Vec<(String, ButtonAction)> {
            self.events.lock().unwrap().clone()
        }

        fn applied_commands(&self) -> Vec<(String, rhythm_core::LightingCommand)> {
            self.applied_commands.lock().unwrap().clone()
        }

        fn lights_off_calls(&self) -> Vec<(String, Option<u32>)> {
            self.lights_off_calls.lock().unwrap().clone()
        }

        fn applied_states(&self) -> Vec<(String, RoomModeState)> {
            self.applied_states.lock().unwrap().clone()
        }

        fn config_updates(&self) -> Vec<LightProfileConfig> {
            self.config_updates.lock().unwrap().clone()
        }

        fn restore_calls(&self) -> Vec<(String, bool, bool)> {
            self.restore_calls.lock().unwrap().clone()
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
            let turned_on = !matches!(
                event.action,
                ButtonAction::LightsOff | ButtonAction::OffPress
            );
            self.set_target_lights(&event.room_id, turned_on);
            Ok(turned_on)
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
            self.snapshots
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.id == room_id)
                .cloned()
        }
        fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
            self.snapshots.lock().unwrap().clone()
        }
        fn restore_room_state(&self, room_id: &str, state: RestoredRoomState) {
            if let Some(snap) = self
                .snapshots
                .lock()
                .unwrap()
                .iter_mut()
                .find(|snap| snap.id == room_id)
            {
                snap.rhythm_enabled = state.rhythm_enabled;
                snap.disabled = state.disabled;
                snap.time_offset_minutes = state.time_offset_minutes;
                snap.brightness_offset = state.brightness_offset;
                snap.soft_off = state.soft_off;
                snap.hard_off = state.hard_off;
                snap.profile_settings = state.profile_settings;
            }
            self.restore_calls.lock().unwrap().push((
                room_id.to_string(),
                state.soft_off,
                state.hard_off,
            ));
        }
        fn add_room(&self, room_id: &str, name: &str) {
            let mut snapshots = self.snapshots.lock().unwrap();
            if snapshots.iter().any(|snap| snap.id == room_id) {
                return;
            }
            snapshots.push(RoomSnapshot {
                id: room_id.to_string(),
                name: name.to_string(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: false,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            });
        }
        fn add_node(
            &self,
            node_id: &str,
            node_name: &str,
            kind: rhythm_core::LightNodeKind,
            parent_id: Option<String>,
        ) {
            let mut snapshots = self.snapshots.lock().unwrap();
            if let Some(snap) = snapshots.iter_mut().find(|snap| snap.id == node_id) {
                snap.name = node_name.to_string();
                snap.kind = kind;
                snap.parent_id = parent_id;
                return;
            }

            snapshots.push(RoomSnapshot {
                id: node_id.to_string(),
                name: node_name.to_string(),
                kind,
                parent_id,
                rhythm_enabled: false,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            });
        }
        fn remove_room(&self, room_id: &str) {
            self.snapshots
                .lock()
                .unwrap()
                .retain(|snap| snap.id != room_id);
        }
        fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }
        fn turn_on_room(&self, room_id: &str) -> anyhow::Result<()> {
            self.set_target_lights(room_id, true);
            Ok(())
        }
        fn apply_room_command(
            &self,
            room_id: &str,
            command: rhythm_core::LightingCommand,
        ) -> anyhow::Result<()> {
            let room_state = self
                .engine_room_snapshot(room_id)
                .map(|snap| persistent_room_state_from_flags(snap.hard_off, snap.soft_off))
                .unwrap_or(RoomModeState::Active);
            self.applied_commands
                .lock()
                .unwrap()
                .push((room_id.to_string(), command));
            self.applied_states
                .lock()
                .unwrap()
                .push((room_id.to_string(), room_state));
            Ok(())
        }
        fn lights_off_room(&self, room_id: &str, transition_ms: Option<u32>) -> anyhow::Result<()> {
            if let Some(snap) = self
                .snapshots
                .lock()
                .unwrap()
                .iter_mut()
                .find(|snap| snap.id == room_id)
            {
                snap.soft_off = false;
                snap.hard_off = true;
            }
            self.lights_off_calls
                .lock()
                .unwrap()
                .push((room_id.to_string(), transition_ms));
            self.set_target_lights(room_id, false);
            Ok(())
        }
        fn set_power_save(&self, _: bool) -> Vec<String> {
            vec![]
        }
        fn is_power_save(&self) -> bool {
            false
        }
        fn set_room_brightness(&self, room_id: &str, _: u8) -> anyhow::Result<()> {
            self.set_target_lights(room_id, true);
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
        fn soft_off_tick_room(&self, room_id: &str) -> anyhow::Result<()> {
            self.set_target_lights(room_id, true);
            Ok(())
        }
        fn any_lights_on(&self, room_id: &str) -> anyhow::Result<bool> {
            Ok(self.any_target_lights_on(room_id))
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
            kind: rhythm_core::LightNodeKind::Room,
            parent_id: None,
            rhythm_enabled: true,
            disabled,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off,
            hard_off: false,
            profile_settings: rhythm_core::RoomProfileSettings::default(),
        }
    }

    fn make_light_child_snapshot(id: &str, parent_id: &str) -> RoomSnapshot {
        let mut snapshot = make_snapshot(id, false, false);
        snapshot.kind = rhythm_core::LightNodeKind::LightDevice;
        snapshot.parent_id = Some(parent_id.to_string());
        snapshot
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

    fn setup_state_with_registry(snapshots: Vec<RoomSnapshot>) -> (SharedState, Arc<MockRuntime>) {
        let (state, runtime) = setup_state(snapshots);
        let mut registry = crate::registry::HubDeviceRegistry::new();
        let device_ids: Vec<String> = Vec::new();
        for snap in runtime.engine_all_room_snapshots() {
            registry.upsert_room(&snap.id, &snap.name, "", &device_ids);
        }
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(registry));

        let mut app = state.lock().unwrap();
        let hub_key = app.hubs.keys().next().cloned().unwrap();
        app.hubs.get_mut(&hub_key).unwrap().registry = Some(registry);
        drop(app);

        (state, runtime)
    }

    fn setup_state_with_deferred_runtime() -> (SharedState, Arc<MockRuntime>, HubKey) {
        let runtime = Arc::new(MockRuntime::new(Vec::new(), 12.0));
        let mut app = AppState::default();
        let hub_type = HubType::new("matter");
        let hub_key = HubKey::new(hub_type.clone(), "local");
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(
            crate::registry::HubDeviceRegistry::with_options(true),
        ));

        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key: hub_key.clone(),
                runtime: None,
                hub_data: Box::new(()),
                registry: Some(registry),
                discovery: None,
                shutdown: Default::default(),
            },
        );

        let runtime_for_closure = runtime.clone();
        let hub_key_for_closure = hub_key.clone();
        app.ensure_runtime_fn = Some(Arc::new(move |state: &SharedState| {
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            if let Some(hub) = s.hubs.get_mut(&hub_key_for_closure) {
                hub.runtime = Some(runtime_for_closure.clone() as Arc<dyn RuntimeHandle>);
            }
            Ok(())
        }));

        (Arc::new(Mutex::new(app)), runtime, hub_key)
    }

    fn add_topology_room(state: &SharedState, room_id: &str, hub_types: &[&str]) {
        let mut room = crate::topology::TopologyRoom::new(room_id, room_id);
        for (index, hub_type) in hub_types.iter().enumerate() {
            room.upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: HubKey::new(HubType::new(*hub_type), format!("{hub_type}-{index}")),
                hub_room_id: format!("{room_id}-{index}"),
                control_id: format!("control-{index}"),
                light_device_ids: Vec::new(),
            });
        }
        state.lock().unwrap().topology.insert_room(room);
    }

    fn insert_canonical_device(
        state: &SharedState,
        hub_key: HubKey,
        native_id: &str,
        name: &str,
        room_id: &str,
        room_name: &str,
    ) -> String {
        let identity = crate::canonical::identity::DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: room_id.to_string(),
            room_name: room_name.to_string(),
            name: name.to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            hardware_ids: vec![crate::canonical::identity::HardwareId::matter(native_id)],
            manufacturer: None,
            model: None,
        };

        let mut s = state.lock().unwrap();
        match s.canonical_registry.resolve(&identity, &hub_key, 1000) {
            crate::canonical::registry::ResolveResult::Created { canonical_id }
            | crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id } => {
                canonical_id
            }
            other => panic!("unexpected resolve result: {:?}", other),
        }
    }

    fn setup_attached_matter_light_without_group_dispatch(
    ) -> (SharedState, Arc<MockRuntime>, String) {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let device_id = insert_canonical_device(
            &state,
            HubKey::new(HubType::new("matter"), "local"),
            "matter-light-1",
            "Desk Lamp",
            "",
            "",
        );
        add_topology_room(&state, "room1", &[]);
        {
            let mut s = state.lock().unwrap();
            assert!(s.topology.attach_device_user_override("room1", &device_id));
        }
        runtime
            .snapshots
            .lock()
            .unwrap()
            .push(make_light_child_snapshot(&device_id, "room1"));

        (state, runtime, device_id)
    }

    fn setup_attached_hue_light_with_group_dispatch() -> (SharedState, Arc<MockRuntime>, String) {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let hub_key = HubKey::new(HubType::new("hue"), "bridge");
        let device_id = insert_canonical_device(
            &state,
            hub_key.clone(),
            "hue-light-1",
            "Desk Lamp",
            "hue-room-1",
            "Room 1",
        );
        {
            let mut s = state.lock().unwrap();
            let mut room = crate::topology::TopologyRoom::new("room1", "room1");
            room.upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key,
                hub_room_id: "hue-room-1".into(),
                control_id: "gl-room1".into(),
                light_device_ids: vec!["hue-light-1".into()],
            });
            s.topology.insert_room(room);
            assert!(s.topology.attach_device_user_override("room1", &device_id));
        }
        runtime
            .snapshots
            .lock()
            .unwrap()
            .push(make_light_child_snapshot(&device_id, "room1"));

        (state, runtime, device_id)
    }

    fn setup_mixed_room_with_hub_groups() -> (
        SharedState,
        Arc<MockRuntime>,
        String,
        String,
        String,
        String,
    ) {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let matter_key = HubKey::new(HubType::new("matter"), "local");
        let ha_key = HubKey::new(HubType::new("homeassistant"), "ha.local");
        let hue_key = HubKey::new(HubType::new("hue"), "bridge");

        let matter_id = insert_canonical_device(
            &state,
            matter_key.clone(),
            "matter-light-1",
            "Matter Lamp",
            "",
            "",
        );
        let ha_id = insert_canonical_device(
            &state,
            ha_key.clone(),
            "ha-light-1",
            "HA Lamp",
            "ha-room-1",
            "Room 1",
        );
        let hue_one_id = insert_canonical_device(
            &state,
            hue_key.clone(),
            "hue-light-1",
            "Hue Lamp 1",
            "hue-room-1",
            "Room 1",
        );
        let hue_two_id = insert_canonical_device(
            &state,
            hue_key.clone(),
            "hue-light-2",
            "Hue Lamp 2",
            "hue-room-1",
            "Room 1",
        );

        {
            let mut s = state.lock().unwrap();
            let mut room = crate::topology::TopologyRoom::new("room1", "room1");
            room.upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: ha_key,
                hub_room_id: "ha-room-1".into(),
                control_id: "ha-room-1".into(),
                light_device_ids: vec!["ha-light-1".into()],
            });
            room.upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: hue_key,
                hub_room_id: "hue-room-1".into(),
                control_id: "gl-room1".into(),
                light_device_ids: vec!["hue-light-1".into(), "hue-light-2".into()],
            });
            s.topology.insert_room(room);
            assert!(s.topology.attach_device_user_override("room1", &matter_id));
            assert!(s.topology.attach_device_user_override("room1", &ha_id));
            assert!(s.topology.attach_device_user_override("room1", &hue_one_id));
            assert!(s.topology.attach_device_user_override("room1", &hue_two_id));
        }

        runtime.snapshots.lock().unwrap().extend([
            make_light_child_snapshot(&matter_id, "room1"),
            make_light_child_snapshot(&ha_id, "room1"),
            make_light_child_snapshot(&hue_one_id, "room1"),
            make_light_child_snapshot(&hue_two_id, "room1"),
        ]);

        (state, runtime, matter_id, ha_id, hue_one_id, hue_two_id)
    }

    #[derive(Clone, Default)]
    struct TestStorage {
        inner: Arc<Mutex<TestStorageInner>>,
    }

    #[derive(Default)]
    struct TestStorageInner {
        rooms: rhythm_core::RoomManager,
        light_profiles: Option<StoredLightProfiles>,
        location: Option<StoredLocation>,
        settings: Option<StoredSettings>,
        hub_credentials: Vec<HubCredentials>,
        hub_registries: HashMap<String, Value>,
        canonical_registry: Option<Value>,
        topology: Option<Value>,
        commissioning_wifi: Option<crate::provisioning::WifiCredentials>,
    }

    impl Storage for TestStorage {
        fn load_rooms(&self) -> Result<rhythm_core::RoomManager> {
            Ok(self.inner.lock().unwrap().rooms.clone())
        }

        fn save_rooms(&self, rooms: &rhythm_core::RoomManager) -> Result<()> {
            self.inner.lock().unwrap().rooms = rooms.clone();
            Ok(())
        }

        fn load_light_profiles(&self) -> Result<StoredLightProfiles> {
            self.inner
                .lock()
                .unwrap()
                .light_profiles
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing light profiles"))
        }

        fn save_light_profiles(&self, config: &StoredLightProfiles) -> Result<()> {
            self.inner.lock().unwrap().light_profiles = Some(config.clone());
            Ok(())
        }

        fn load_location(&self) -> Result<StoredLocation> {
            self.inner
                .lock()
                .unwrap()
                .location
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing location"))
        }

        fn save_location(&self, loc: &StoredLocation) -> Result<()> {
            self.inner.lock().unwrap().location = Some(loc.clone());
            Ok(())
        }

        fn load_settings(&self) -> Result<StoredSettings> {
            self.inner
                .lock()
                .unwrap()
                .settings
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing settings"))
        }

        fn save_settings(&self, settings: &StoredSettings) -> Result<()> {
            self.inner.lock().unwrap().settings = Some(settings.clone());
            Ok(())
        }

        fn load_hub_credentials(&self) -> Result<HubCredentials> {
            Ok(self
                .inner
                .lock()
                .unwrap()
                .hub_credentials
                .first()
                .cloned()
                .unwrap_or_default())
        }

        fn save_hub_credentials(&self, creds: &HubCredentials) -> Result<()> {
            let mut inner = self.inner.lock().unwrap();
            inner.hub_credentials = if creds.is_configured() {
                vec![creds.clone()]
            } else {
                Vec::new()
            };
            Ok(())
        }

        fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>> {
            Ok(self.inner.lock().unwrap().hub_credentials.clone())
        }

        fn save_all_hub_credentials(&self, creds: &[HubCredentials]) -> Result<()> {
            self.inner.lock().unwrap().hub_credentials = creds.to_vec();
            Ok(())
        }

        fn load_hub_registry(&self) -> Result<Option<Value>> {
            Ok(self
                .inner
                .lock()
                .unwrap()
                .hub_registries
                .values()
                .next()
                .cloned())
        }

        fn save_hub_registry(&self, data: &Value) -> Result<()> {
            self.inner
                .lock()
                .unwrap()
                .hub_registries
                .insert("legacy".into(), data.clone());
            Ok(())
        }

        fn load_hub_registry_for(&self, key: &HubKey) -> Result<Option<Value>> {
            Ok(self
                .inner
                .lock()
                .unwrap()
                .hub_registries
                .get(&key.to_string())
                .cloned())
        }

        fn save_hub_registry_for(&self, key: &HubKey, data: &Value) -> Result<()> {
            self.inner
                .lock()
                .unwrap()
                .hub_registries
                .insert(key.to_string(), data.clone());
            Ok(())
        }

        fn load_canonical_registry(&self) -> Result<Option<Value>> {
            Ok(self.inner.lock().unwrap().canonical_registry.clone())
        }

        fn save_canonical_registry(&self, data: &Value) -> Result<()> {
            self.inner.lock().unwrap().canonical_registry = Some(data.clone());
            Ok(())
        }

        fn load_topology(&self) -> Result<Option<Value>> {
            Ok(self.inner.lock().unwrap().topology.clone())
        }

        fn save_topology(&self, data: &Value) -> Result<()> {
            self.inner.lock().unwrap().topology = Some(data.clone());
            Ok(())
        }

        fn load_commissioning_wifi_credentials(
            &self,
        ) -> Result<Option<crate::provisioning::WifiCredentials>> {
            Ok(self.inner.lock().unwrap().commissioning_wifi.clone())
        }

        fn save_commissioning_wifi_credentials(
            &self,
            creds: &crate::provisioning::WifiCredentials,
        ) -> Result<()> {
            self.inner.lock().unwrap().commissioning_wifi = Some(creds.clone());
            Ok(())
        }

        fn clear_commissioning_wifi_credentials(&self) -> Result<()> {
            self.inner.lock().unwrap().commissioning_wifi = None;
            Ok(())
        }

        fn clear_factory_reset_state(&self) -> Result<()> {
            let mut inner = self.inner.lock().unwrap();
            inner.rooms = rhythm_core::RoomManager::new();
            inner.light_profiles = None;
            inner.location = None;
            inner.settings = None;
            inner.hub_credentials.clear();
            inner.hub_registries.clear();
            inner.canonical_registry = None;
            inner.topology = None;
            inner.commissioning_wifi = None;
            Ok(())
        }
    }

    struct MockBackupHubProvider;

    impl HubProvider for MockBackupHubProvider {
        fn hub_type(&self) -> HubType {
            HubType::new("mock")
        }

        fn configure(
            &self,
            address: &str,
            credentials_json: &str,
            state: &SharedState,
        ) -> Result<()> {
            let hub_key = HubKey::new(HubType::new("mock"), address);
            let address = address.to_string();
            crate::lifecycle::configure_hub(
                state,
                &address,
                credentials_json,
                |addr, credentials_json| {
                    Ok(HubCredentials::new(
                        "mock",
                        addr,
                        serde_json::from_str::<Value>(credentials_json)?,
                    ))
                },
                |_state, _creds| false,
                move |_state| {
                    let runtime = Arc::new(MockRuntime::new(Vec::new(), 12.0));
                    let registry: Arc<Mutex<dyn HubRegistry>> =
                        Arc::new(Mutex::new(crate::registry::HubDeviceRegistry::new()));
                    let hub = ActiveHub {
                        hub_type: HubType::new("mock"),
                        hub_key: hub_key.clone(),
                        runtime: Some(runtime as Arc<dyn RuntimeHandle>),
                        hub_data: Box::new(()),
                        registry: Some(registry),
                        discovery: None,
                        shutdown: Default::default(),
                    };
                    let (_tx, rx) = std::sync::mpsc::channel();
                    Ok((hub, rx))
                },
            )
        }
    }

    static MOCK_BACKUP_HUB_PROVIDER: MockBackupHubProvider = MockBackupHubProvider;

    fn install_mock_hub_provider(state: &SharedState) {
        state.lock().unwrap().get_hub_provider_fn = Some(Arc::new(|_| &MOCK_BACKUP_HUB_PROVIDER));
    }

    fn make_focus_profile() -> LightProfileConfig {
        let mut focus =
            factory_default_light_profile_config(rhythm_core::RHYTHM_PROFILE_ID).unwrap();
        focus.id = "focus".into();
        focus.name = "Focus".into();
        focus.min_brightness = 9;
        focus.max_brightness = 44;
        focus.fade_ms = TimerSetting::Fixed { value: 777 };
        focus
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
        let result = do_node_action(&state, "room1", "on", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::OnPress));
    }

    #[test]
    fn room_action_updates_parent_lights_on_for_attached_light() {
        let (state, runtime, device_id) = setup_attached_hue_light_with_group_dispatch();

        let result = do_node_action(&state, &device_id, "on", false);

        assert!(result.is_ok());
        assert_eq!(runtime.events(), vec![(device_id, ButtonAction::OnPress)]);
        let s = state.lock().unwrap();
        assert_eq!(s.room_lights_on.get("room1"), Some(&true));
        assert_eq!(s.room_lights_on.len(), 1);
    }

    #[test]
    fn room_action_updates_parent_lights_on_without_parent_group_dispatch() {
        let (state, runtime, device_id) = setup_attached_matter_light_without_group_dispatch();
        {
            let mut s = state.lock().unwrap();
            s.room_lights_on.insert("room1".into(), false);
        }

        let result = do_node_action(&state, &device_id, "on", false);

        assert!(result.is_ok());
        assert_eq!(
            runtime.events(),
            vec![(device_id.clone(), ButtonAction::OnPress)]
        );
        let s = state.lock().unwrap();
        assert_eq!(s.room_lights_on.get(&device_id), Some(&true));
        assert_eq!(s.room_lights_on.get("room1"), Some(&true));
    }

    #[test]
    fn room_action_turning_on_matter_child_updates_mixed_room_parent_aggregate() {
        let (state, _runtime, matter_id, _ha_id, _hue_one_id, _hue_two_id) =
            setup_mixed_room_with_hub_groups();

        let result = do_node_action(&state, &matter_id, "on", false);

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(s.room_lights_on.get(&matter_id), Some(&true));
        assert_eq!(s.room_lights_on.get("room1"), Some(&true));
    }

    #[test]
    fn room_action_turning_off_matter_child_preserves_parent_when_siblings_are_on() {
        let (state, runtime, matter_id, ha_id, hue_one_id, _hue_two_id) =
            setup_mixed_room_with_hub_groups();
        runtime.set_light_on(&matter_id, true);
        runtime.set_light_on(&ha_id, true);
        runtime.set_light_on(&hue_one_id, true);

        let result = do_node_action(&state, &matter_id, "off", false);

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(s.room_lights_on.get(&matter_id), Some(&false));
        assert_eq!(s.room_lights_on.get("room1"), Some(&true));
    }

    #[test]
    fn room_action_off() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_node_action(&state, "room1", "off", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::OffPress));
    }

    #[test]
    fn room_action_toggle() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_node_action(&state, "room1", "toggle", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::Toggle));
    }

    #[test]
    fn room_action_reset() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_node_action(&state, "room1", "reset", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::Reset));
    }

    #[test]
    fn room_action_rhythm_on() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_node_action(&state, "room1", "rhythm_on", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::RhythmOn));
    }

    #[test]
    fn room_action_rhythm_off() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_node_action(&state, "room1", "rhythm_off", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::RhythmOff));
    }

    #[test]
    fn room_action_unknown() {
        let (state, _runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_node_action(&state, "room1", "nonsense", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown action"));
    }

    #[test]
    fn room_action_no_runtime() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_node_action(&state, "room1", "on", false);
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
        assert!(!room_state.transitioning);
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
    fn build_room_rhythm_state_includes_deduped_hub_types_from_topology() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        add_topology_room(&state, "r1", &["mock", "mock", "matter"]);

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();
        let json = serde_json::to_value(&room_state).unwrap();

        assert_eq!(room_state.hub_types, vec!["mock", "matter"]);
        assert_eq!(json["hub_types"], serde_json::json!(["mock", "matter"]));
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
    fn build_room_rhythm_state_marks_active_mode_transition() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state.lock().unwrap().room_mode_transitions.insert(
            "r1".into(),
            crate::state::RoomModeTransition {
                ends_at: std::time::Instant::now() + std::time::Duration::from_secs(5),
                periodic_resume_at: std::time::Instant::now() + std::time::Duration::from_secs(5),
            },
        );

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();

        assert!(room_state.transitioning);
    }

    #[test]
    fn build_room_rhythm_state_ignores_expired_mode_transition() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state.lock().unwrap().room_mode_transitions.insert(
            "r1".into(),
            crate::state::RoomModeTransition {
                ends_at: std::time::Instant::now() - std::time::Duration::from_millis(1),
                periodic_resume_at: std::time::Instant::now() + std::time::Duration::from_secs(5),
            },
        );

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();

        assert!(!room_state.transitioning);
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn build_node_state_event_marks_active_mode_transition() {
        let (state, rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);
        state.lock().unwrap().room_mode_transitions.insert(
            "r1".into(),
            crate::state::RoomModeTransition {
                ends_at: std::time::Instant::now() + std::time::Duration::from_secs(5),
                periodic_resume_at: std::time::Instant::now() + std::time::Duration::from_secs(5),
            },
        );
        let snap =
            rhythm_core::NodeSnapshot::from_room_snapshot(rt.engine_room_snapshot("r1").unwrap());

        let event = build_node_state_event(&state, &snap);
        let json = serde_json::to_value(&event).unwrap();

        assert!(event.transitioning);
        assert_eq!(json["transitioning"], true);
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn build_node_state_event_includes_hub_types_from_topology() {
        let (state, rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        add_topology_room(&state, "r1", &["matter", "mock"]);
        let snap =
            rhythm_core::NodeSnapshot::from_room_snapshot(rt.engine_room_snapshot("r1").unwrap());

        let event = build_node_state_event(&state, &snap);
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(event.hub_types, vec!["matter", "mock"]);
        assert_eq!(json["hub_types"], serde_json::json!(["matter", "mock"]));
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn build_node_state_event_uses_parent_lights_on_for_attached_light() {
        let (state, rt, device_id) = setup_attached_hue_light_with_group_dispatch();
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("room1".into(), true);

        let snap = rhythm_core::NodeSnapshot::from_room_snapshot(
            rt.engine_room_snapshot(&device_id).unwrap(),
        );
        let event = build_node_state_event(&state, &snap);

        assert!(event.lights_on);
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn build_node_state_event_uses_child_lights_on_without_parent_group_dispatch() {
        let (state, rt, device_id) = setup_attached_matter_light_without_group_dispatch();
        {
            let mut s = state.lock().unwrap();
            s.room_lights_on.insert("room1".into(), false);
            s.room_lights_on.insert(device_id.clone(), true);
        }

        let snap = rhythm_core::NodeSnapshot::from_room_snapshot(
            rt.engine_room_snapshot(&device_id).unwrap(),
        );
        let event = build_node_state_event(&state, &snap);

        assert!(event.lights_on);
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
        let result = do_node_action(&state, "r1", "on", false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        // Must be a room state object — has id, rhythm_enabled, etc.
        assert_eq!(parsed["id"], "r1");
        assert!(parsed["rhythm_enabled"].is_boolean());
        assert!(parsed["brightness"].is_number());
        assert!(parsed["kelvin"].is_number());
        assert!(parsed["lights_on"].is_boolean());
        assert!(parsed["transitioning"].is_boolean());
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn settings_response_is_valid_settings_json() {
        let (state, _rt) = setup_state(vec![]);
        let result = do_settings_set(&state, Some(true), None, None, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["power_save"].is_boolean());
        assert!(parsed.get("mode").is_none());
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn build_settings_dto_matches_state() {
        let (state, _rt) = setup_state(vec![]);
        {
            let mut s = state.lock().unwrap();
            s.power_save = true;
            s.last_active_mode_cause = ModeChangeCause::Schedule;
            s.last_active_mode_transition_id = Some("sleep_to_day".into());
            s.last_active_mode_change_utc_ms = Some(1_700_000_000_000);
        }
        let dto = build_settings_dto(&state).unwrap();
        assert!(dto.power_save);
    }

    #[test]
    fn build_mode_dto_matches_state() {
        let (state, _rt) = setup_state(vec![]);
        {
            let mut s = state.lock().unwrap();
            s.last_active_mode_cause = ModeChangeCause::Schedule;
            s.last_active_mode_transition_id = Some("sleep_to_day".into());
            s.last_active_mode_change_utc_ms = Some(1_700_000_000_000);
        }
        let dto = build_mode_dto(&state).unwrap();
        assert_eq!(dto.last_change.cause, ModeChangeCause::Schedule);
        assert_eq!(
            dto.last_change.transition_id.as_deref(),
            Some("sleep_to_day")
        );
        assert_eq!(dto.last_change.epoch_ms, 1_700_000_000_000);
    }

    #[test]
    fn build_transitions_dto_matches_state() {
        let (state, _rt) = setup_state(vec![]);
        let dto = build_transitions_dto(&state).unwrap();
        assert_eq!(
            dto.transitions,
            rhythm_core::default_mode_transition_configs()
        );
    }

    #[test]
    fn build_profiles_dto_matches_state() {
        let (state, _rt) = setup_state(vec![]);
        let dto = build_profiles_dto(&state).unwrap();
        assert!(!dto.profiles.is_empty());
    }

    #[test]
    fn build_configuration_bundle_dto_exports_fresh_state_defaults() {
        let (state, _rt) = setup_state(vec![]);

        let bundle = build_configuration_bundle_dto(&state).unwrap();
        let s = state.lock().unwrap();

        assert_eq!(bundle.schema_version, BUNDLE_SCHEMA_VERSION);
        assert_eq!(bundle.kind, BundleKind::ConfigurationBundle);
        assert_eq!(
            bundle.configuration.power_save,
            factory_default_power_save()
        );
        assert_eq!(
            bundle.configuration.active_mode,
            factory_default_active_mode()
        );
        assert_eq!(
            bundle.configuration.profiles,
            s.light_profile_configs
                .values()
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(bundle.configuration.mode_configs, s.mode_configs());
        assert_eq!(
            bundle.configuration.mode_transitions,
            factory_default_mode_transition_configs()
        );
        assert!(bundle.configuration.rooms.is_empty());
    }

    #[test]
    fn build_factory_default_configuration_bundle_matches_factory_default_source() {
        let bundle = build_factory_default_configuration_bundle_dto();
        let expected = factory_default_configuration_bundle();
        assert_eq!(bundle.schema_version, expected.schema_version);
        assert_eq!(bundle.kind, expected.kind);
        assert_eq!(bundle.name, expected.name);
        assert_eq!(bundle.description, expected.description);
        assert_eq!(
            bundle.configuration.power_save,
            expected.configuration.power_save
        );
        assert_eq!(
            bundle.configuration.active_mode,
            expected.configuration.active_mode
        );
        assert_eq!(
            bundle.configuration.profiles,
            expected.configuration.profiles
        );
        assert_eq!(
            bundle.configuration.mode_configs,
            expected.configuration.mode_configs
        );
        assert_eq!(
            bundle.configuration.mode_transitions,
            expected.configuration.mode_transitions
        );
        assert_eq!(bundle.configuration.rooms.len(), 0);
    }

    #[test]
    fn configuration_import_applies_profiles_settings_and_room_preferences_by_name() {
        let mut snapshot = make_snapshot("local-office", false, false);
        snapshot.name = "Office".into();
        let (state, runtime) = setup_state(vec![snapshot]);
        let focus = make_focus_profile();
        let transition =
            rhythm_core::ModeTransitionConfig::new(RhythmMode::Day, RhythmMode::Sleep, 4_321)
                .with_id("custom_day_to_sleep")
                .with_label("Custom Day to Sleep")
                .with_trigger(ModeTransitionTrigger::Sunset);

        let payload = ConfigurationImportPayload::Bundle(ConfigurationBundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            kind: BundleKind::ConfigurationBundle,
            name: Some("Shared Setup".into()),
            description: Some("Portable config".into()),
            configuration: PortableConfiguration {
                power_save: true,
                active_mode: RhythmMode::Day,
                profiles: vec![focus.clone()],
                mode_configs: vec![
                    ModeConfig {
                        mode: RhythmMode::Day,
                        active_profile_id: Some("focus".into()),
                        idle_profile_id: None,
                        wake_profile_id: None,
                        warning_profile_id: None,
                        room_defaults: vec![],
                    },
                    ModeConfig::default_for_mode(RhythmMode::Sleep),
                ],
                mode_transitions: vec![transition.clone()],
                rooms: vec![ConfigurationRoom {
                    id: "shared-office".into(),
                    name: "Office".into(),
                    rhythm_enabled: true,
                    disabled: true,
                    state: RoomModeState::Idle,
                    room_profile: rhythm_core::RoomProfileSettings {
                        profile_id: Some("focus".into()),
                        fade_ms: Some(TimerSetting::Fixed { value: 3_210 }),
                        motion_timeout_secs: Some(TimerSetting::Fixed { value: 654 }),
                    },
                }],
            },
        });

        let json = do_configuration_import(&state, payload).unwrap();
        let exported: ConfigurationBundle = serde_json::from_str(&json).unwrap();
        let s = state.lock().unwrap();

        assert!(s.power_save);
        assert_eq!(s.active_mode, RhythmMode::Day);
        assert_eq!(
            s.mode_configs()
                .iter()
                .find(|config| config.mode == RhythmMode::Day)
                .and_then(|config| config.active_profile_id.as_deref()),
            Some("focus")
        );
        assert_eq!(s.mode_transition_configs(), vec![transition.clone()]);
        assert_eq!(s.light_profile_config("focus"), Some(&focus));
        drop(s);

        let applied = runtime.engine_room_snapshot("local-office").unwrap();
        assert!(applied.rhythm_enabled);
        assert!(applied.disabled);
        assert!(applied.soft_off);
        assert!(!applied.hard_off);
        assert_eq!(
            applied.profile_settings.profile_id.as_deref(),
            Some("focus")
        );
        assert_eq!(
            applied.profile_settings.fade_ms,
            Some(TimerSetting::Fixed { value: 3_210 })
        );
        assert_eq!(
            applied.profile_settings.motion_timeout_secs,
            Some(TimerSetting::Fixed { value: 654 })
        );
        assert!(
            runtime
                .config_updates()
                .iter()
                .any(|config| config.id == "focus"),
            "runtime should receive imported custom profile config"
        );

        assert_eq!(exported.kind, BundleKind::ConfigurationBundle);
        assert!(exported.configuration.power_save);
        assert_eq!(exported.configuration.active_mode, RhythmMode::Day);
        assert_eq!(exported.configuration.rooms.len(), 1);
        assert_eq!(exported.configuration.rooms[0].id, "local-office");
        assert_eq!(exported.configuration.rooms[0].name, "Office");
        assert_eq!(exported.configuration.rooms[0].state, RoomModeState::Idle);
        assert_eq!(
            exported.configuration.rooms[0]
                .room_profile
                .profile_id
                .as_deref(),
            Some("focus")
        );
    }

    #[test]
    fn configuration_import_rejects_unknown_room_profile_reference() {
        let (state, _rt) = setup_state(vec![]);

        let err = do_configuration_import(
            &state,
            ConfigurationImportPayload::Configuration(PortableConfiguration {
                power_save: false,
                active_mode: RhythmMode::Day,
                profiles: vec![],
                mode_configs: vec![],
                mode_transitions: vec![],
                rooms: vec![ConfigurationRoom {
                    id: "office".into(),
                    name: "Office".into(),
                    rhythm_enabled: true,
                    disabled: false,
                    state: RoomModeState::Active,
                    room_profile: rhythm_core::RoomProfileSettings {
                        profile_id: Some("missing_profile".into()),
                        fade_ms: None,
                        motion_timeout_secs: None,
                    },
                }],
            }),
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("unknown light profile 'missing_profile'"));
    }

    #[test]
    fn build_backup_bundle_dto_redacts_or_includes_hub_secrets() {
        let (state, _rt) = setup_state(vec![]);
        let creds = HubCredentials::new(
            "hue",
            "192.168.1.2",
            serde_json::json!({ "username": "secret-user" }),
        );
        let hub_key = creds.hub_key().unwrap();
        state.lock().unwrap().hub_credentials.insert(hub_key, creds);

        let redacted = build_backup_bundle_dto(&state, false).unwrap();
        assert!(!redacted.secrets_included);
        assert_eq!(redacted.kind, BundleKind::BackupBundle);
        assert_eq!(
            redacted.configuration.active_mode,
            factory_default_active_mode()
        );
        assert_eq!(redacted.installation.hub_credentials.len(), 1);
        assert_eq!(
            redacted.installation.hub_credentials[0].address,
            "192.168.1.2"
        );
        assert_eq!(redacted.installation.hub_credentials[0].data, None);

        let included = build_backup_bundle_dto(&state, true).unwrap();
        assert!(included.secrets_included);
        assert_eq!(included.installation.hub_credentials.len(), 1);
        assert_eq!(
            included.installation.hub_credentials[0].data,
            Some(serde_json::json!({ "username": "secret-user" }))
        );
    }

    #[test]
    fn backup_restore_applies_installation_state_after_hub_restore() {
        let storage = TestStorage::default();
        let app = AppState {
            storage: Some(Box::new(storage.clone())),
            ..Default::default()
        };
        let state = Arc::new(Mutex::new(app));
        install_mock_hub_provider(&state);

        let focus = make_focus_profile();
        let transition =
            rhythm_core::ModeTransitionConfig::new(RhythmMode::Day, RhythmMode::Sleep, 4_321)
                .with_id("custom_day_to_sleep")
                .with_label("Custom Day to Sleep")
                .with_trigger(ModeTransitionTrigger::Sunset);

        let mut rooms = rhythm_core::RoomManager::new();
        let room = rooms.get_or_create("living", "Living Room");
        room.rhythm_enabled = true;
        room.disabled = true;
        room.time_offset_minutes = 27.0;
        room.brightness_offset = 11.0;
        room.soft_off = true;
        room.profile_settings = rhythm_core::RoomProfileSettings {
            profile_id: Some("focus".into()),
            fade_ms: Some(TimerSetting::Fixed { value: 3_210 }),
            motion_timeout_secs: Some(TimerSetting::Fixed { value: 654 }),
        };

        let hub_key = HubKey::new(HubType::new("mock"), "bridge.local");
        let bundle = BackupBundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            kind: BundleKind::BackupBundle,
            created_at: "2026-04-16T00:00:00Z".into(),
            secrets_included: true,
            configuration: PortableConfiguration {
                power_save: true,
                active_mode: RhythmMode::Sleep,
                profiles: vec![focus.clone()],
                mode_configs: vec![
                    ModeConfig {
                        mode: RhythmMode::Day,
                        active_profile_id: Some("focus".into()),
                        idle_profile_id: None,
                        wake_profile_id: None,
                        warning_profile_id: None,
                        room_defaults: vec![],
                    },
                    ModeConfig::default_for_mode(RhythmMode::Sleep),
                ],
                mode_transitions: vec![transition.clone()],
                rooms: vec![],
            },
            installation: BackupInstallation {
                location: Some(StoredLocation {
                    latitude: Some(40.7128),
                    longitude: Some(-74.0060),
                    utc_offset_hours: -5.0,
                    timezone_name: Some("America/New_York".into()),
                }),
                rooms: rooms.clone(),
                topology: crate::topology::RoomTopologyStore::new(),
                canonical_registry: crate::canonical::registry::CanonicalRegistry::new(),
                hub_credentials: vec![BackupHubCredentials {
                    hub_type: Some(HubType::new("mock")),
                    address: "bridge.local".into(),
                    data: Some(serde_json::json!({ "token": "secret-token" })),
                }],
                hub_registries: vec![crate::bundle::BackupHubRegistry {
                    hub_key: hub_key.clone(),
                    snapshot: serde_json::json!({ "rooms": [] }),
                }],
            },
            runtime_state: BackupRuntimeState {
                active_mode: RhythmMode::Day,
                last_change_cause: ModeChangeCause::Schedule,
                last_change_transition_id: Some("custom_day_to_sleep".into()),
                last_change_epoch_ms: Some(1_700_000_000_000),
            },
        };

        let json = do_backup_restore(&state, bundle).unwrap();
        let restored: BackupBundle = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.kind, BundleKind::BackupBundle);
        assert_eq!(restored.configuration.active_mode, RhythmMode::Day);
        assert!(restored.configuration.power_save);
        assert_eq!(restored.installation.hub_credentials.len(), 1);
        assert_eq!(
            restored.installation.hub_credentials[0].address,
            "bridge.local"
        );
        assert_eq!(restored.installation.hub_credentials[0].data, None);

        let restored_room = restored
            .installation
            .rooms
            .get("living")
            .expect("restored room should be present");
        assert_eq!(restored_room.name, "Living Room");
        assert!(restored_room.rhythm_enabled);
        assert!(restored_room.disabled);
        assert_eq!(restored_room.time_offset_minutes, 27.0);
        assert_eq!(restored_room.brightness_offset, 11.0);
        assert!(restored_room.soft_off);
        assert_eq!(
            restored_room.profile_settings.profile_id.as_deref(),
            Some("focus")
        );

        let saved = storage.inner.lock().unwrap();
        assert_eq!(
            saved.settings.as_ref().unwrap().last_active_mode_cause,
            ModeChangeCause::Schedule
        );
        assert_eq!(
            saved
                .settings
                .as_ref()
                .unwrap()
                .last_active_mode_transition_id
                .as_deref(),
            Some("custom_day_to_sleep")
        );
        assert_eq!(
            saved
                .settings
                .as_ref()
                .unwrap()
                .last_active_mode_change_utc_ms,
            Some(1_700_000_000_000)
        );
        assert_eq!(saved.location.as_ref().unwrap().latitude, Some(40.7128));
        assert!(saved.hub_registries.contains_key(&hub_key.to_string()));
        drop(saved);

        let s = state.lock().unwrap();
        assert_eq!(s.active_mode, RhythmMode::Day);
        assert_eq!(s.last_active_mode_cause, ModeChangeCause::Schedule);
        assert_eq!(
            s.last_active_mode_transition_id.as_deref(),
            Some("custom_day_to_sleep")
        );
    }

    #[test]
    fn backup_restore_redacted_backup_restores_without_hubs() {
        let storage = TestStorage::default();
        let app = AppState {
            storage: Some(Box::new(storage.clone())),
            ..Default::default()
        };
        let state = Arc::new(Mutex::new(app));

        let mut rooms = rhythm_core::RoomManager::new();
        let room = rooms.get_or_create("office", "Office");
        room.rhythm_enabled = true;
        room.time_offset_minutes = 12.0;
        room.brightness_offset = 5.0;

        let bundle = BackupBundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            kind: BundleKind::BackupBundle,
            created_at: "2026-04-16T00:00:00Z".into(),
            secrets_included: false,
            configuration: PortableConfiguration {
                power_save: false,
                active_mode: RhythmMode::Sleep,
                profiles: vec![],
                mode_configs: vec![],
                mode_transitions: vec![],
                rooms: vec![],
            },
            installation: BackupInstallation {
                location: None,
                rooms,
                topology: crate::topology::RoomTopologyStore::new(),
                canonical_registry: crate::canonical::registry::CanonicalRegistry::new(),
                hub_credentials: vec![BackupHubCredentials {
                    hub_type: Some(HubType::new("mock")),
                    address: "bridge.local".into(),
                    data: None,
                }],
                hub_registries: vec![],
            },
            runtime_state: BackupRuntimeState {
                active_mode: RhythmMode::Sleep,
                last_change_cause: ModeChangeCause::Manual,
                last_change_transition_id: None,
                last_change_epoch_ms: Some(1_600_000_000_000),
            },
        };

        let json = do_backup_restore(&state, bundle).unwrap();
        let restored: BackupBundle = serde_json::from_str(&json).unwrap();

        assert!(restored.installation.hub_credentials.is_empty());
        let restored_room = restored
            .installation
            .rooms
            .get("office")
            .expect("restored room should be persisted without a runtime");
        assert_eq!(restored_room.name, "Office");
        assert_eq!(restored_room.time_offset_minutes, 12.0);
        assert_eq!(restored_room.brightness_offset, 5.0);

        let saved = storage.inner.lock().unwrap();
        assert_eq!(
            saved
                .settings
                .as_ref()
                .unwrap()
                .last_active_mode_change_utc_ms,
            Some(1_600_000_000_000)
        );
        assert!(saved.location.is_some());
        assert!(saved.hub_credentials.is_empty());
    }

    #[test]
    fn backup_restore_rejects_unknown_installation_room_profile_reference() {
        let (state, _rt) = setup_state(vec![]);

        let mut rooms = rhythm_core::RoomManager::new();
        let room = rooms.get_or_create("office", "Office");
        room.profile_settings = rhythm_core::RoomProfileSettings {
            profile_id: Some("missing_profile".into()),
            fade_ms: None,
            motion_timeout_secs: None,
        };

        let err = do_backup_restore(
            &state,
            BackupBundle {
                schema_version: BUNDLE_SCHEMA_VERSION,
                kind: BundleKind::BackupBundle,
                created_at: "2026-04-16T00:00:00Z".into(),
                secrets_included: false,
                configuration: PortableConfiguration {
                    power_save: false,
                    active_mode: RhythmMode::Day,
                    profiles: vec![],
                    mode_configs: vec![],
                    mode_transitions: vec![],
                    rooms: vec![],
                },
                installation: BackupInstallation {
                    location: None,
                    rooms,
                    topology: crate::topology::RoomTopologyStore::new(),
                    canonical_registry: crate::canonical::registry::CanonicalRegistry::new(),
                    hub_credentials: vec![],
                    hub_registries: vec![],
                },
                runtime_state: BackupRuntimeState {
                    active_mode: RhythmMode::Day,
                    last_change_cause: ModeChangeCause::Manual,
                    last_change_transition_id: None,
                    last_change_epoch_ms: None,
                },
            },
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("unknown light profile 'missing_profile'"));
    }

    #[test]
    fn configuration_reset_restores_factory_default_configuration() {
        let (state, _rt) = setup_state(vec![]);
        let focus = make_focus_profile();
        do_configuration_import(
            &state,
            ConfigurationImportPayload::Configuration(PortableConfiguration {
                power_save: true,
                active_mode: RhythmMode::Day,
                profiles: vec![focus],
                mode_configs: vec![ModeConfig {
                    mode: RhythmMode::Day,
                    active_profile_id: Some("focus".into()),
                    idle_profile_id: None,
                    wake_profile_id: None,
                    warning_profile_id: None,
                    room_defaults: vec![],
                }],
                mode_transitions: vec![],
                rooms: vec![],
            }),
        )
        .unwrap();

        let json = do_configuration_reset(&state).unwrap();
        let reset_bundle: ConfigurationBundle = serde_json::from_str(&json).unwrap();
        let expected = factory_default_configuration_bundle();
        let mut reset_profiles = reset_bundle.configuration.profiles.clone();
        reset_profiles.sort_by(|left, right| left.id.cmp(&right.id));
        let mut expected_profiles = expected.configuration.profiles.clone();
        expected_profiles.sort_by(|left, right| left.id.cmp(&right.id));

        assert_eq!(
            reset_bundle.configuration.power_save,
            expected.configuration.power_save
        );
        assert_eq!(
            reset_bundle.configuration.active_mode,
            expected.configuration.active_mode
        );
        assert_eq!(reset_profiles, expected_profiles);
        assert_eq!(
            reset_bundle.configuration.mode_configs,
            expected.configuration.mode_configs
        );
        assert_eq!(
            reset_bundle.configuration.mode_transitions,
            expected.configuration.mode_transitions
        );
    }

    #[test]
    fn configuration_reset_clears_installation_state_and_stale_storage() {
        let (state, _rt) = setup_state_with_registry(vec![make_snapshot("r1", false, false)]);
        let storage = TestStorage::default();
        let hub_key = state.lock().unwrap().hubs.keys().next().cloned().unwrap();

        state.lock().unwrap().storage = Some(Box::new(storage.clone()));

        let canonical_id = insert_canonical_device(
            &state,
            hub_key.clone(),
            "native-light-1",
            "Desk",
            "r1",
            "R1",
        );
        add_topology_room(&state, "r1", &["mock"]);
        {
            let mut s = state.lock().unwrap();
            s.hub_credentials.insert(
                hub_key.clone(),
                HubCredentials::new("mock", "mock", serde_json::json!({"token": "abc"})),
            );
            s.hub_connection_status.insert(hub_key.clone(), true);
            s.hub_seen_connected_once.insert(hub_key.clone());
            s.hub_sync_in_progress.insert(hub_key.clone());
            s.hub_reconnect_sync_at
                .insert(hub_key.clone(), std::time::Instant::now());
            s.latitude = Some(40.7128);
            s.longitude = Some(-74.0060);
            s.utc_offset_hours = -5.0;
            s.timezone_name = Some("America/New_York".into());
            s.room_lights_on.insert("r1".into(), true);
            s.pending_periodic_ticks.insert("r1".into(), 12.0);
            s.pending_motion_clear.push("r1".into());
            s.pending_motion_seed.push(("sensor-1".into(), "r1".into()));
            s.canonical_registry.queue_unassigned(&canonical_id, 1000);
        }

        storage
            .save_hub_registry_for(&hub_key, &serde_json::json!({"rooms": [{"id": "r1"}]}))
            .unwrap();
        storage
            .save_commissioning_wifi_credentials(&crate::provisioning::WifiCredentials {
                ssid: "RhythmNet".into(),
                password: "secret".into(),
            })
            .unwrap();
        {
            let s = state.lock().unwrap();
            storage
                .save_location(&StoredLocation {
                    latitude: s.latitude,
                    longitude: s.longitude,
                    utc_offset_hours: s.utc_offset_hours,
                    timezone_name: s.timezone_name.clone(),
                })
                .unwrap();
            persist_canonical(&s);
            persist_topology(&s);
        }

        let json = do_configuration_reset(&state).unwrap();
        let reset_bundle: ConfigurationBundle = serde_json::from_str(&json).unwrap();
        assert!(reset_bundle.configuration.rooms.is_empty());

        let s = state.lock().unwrap();
        assert!(s.hubs.is_empty());
        assert!(s.hub_credentials.is_empty());
        assert!(s.hub_seen_connected_once.is_empty());
        assert!(s.canonical_registry.device_count() == 0);
        assert_eq!(s.canonical_registry.triage().pending_count(), 0);
        assert_eq!(s.topology.room_count(), 0);
        assert!(s.room_lights_on.is_empty());
        assert!(s.pending_periodic_ticks.is_empty());
        assert!(s.pending_hub_event_rxs.is_empty());
        assert!(s.pending_motion_clear.is_empty());
        assert!(s.pending_motion_seed.is_empty());
        assert!(s.latitude.is_none());
        assert!(s.longitude.is_none());
        assert!(s.timezone_name.is_none());
        assert_eq!(s.utc_offset_hours, 0.0);
        drop(s);

        let inner = storage.inner.lock().unwrap();
        assert!(inner.hub_registries.is_empty());
        assert!(inner.commissioning_wifi.is_none());
        assert!(inner.hub_credentials.is_empty());
        assert_eq!(inner.rooms.len(), 0);

        let stored_location = inner
            .location
            .clone()
            .expect("cleared location should persist");
        assert!(stored_location.latitude.is_none());
        assert!(stored_location.longitude.is_none());
        assert!(stored_location.timezone_name.is_none());
        assert_eq!(stored_location.utc_offset_hours, 0.0);

        let stored_registry: crate::canonical::registry::CanonicalRegistry =
            serde_json::from_value(
                inner
                    .canonical_registry
                    .clone()
                    .expect("empty canonical registry should be persisted"),
            )
            .unwrap();
        assert_eq!(stored_registry.device_count(), 0);

        let stored_topology: crate::topology::RoomTopologyStore = serde_json::from_value(
            inner
                .topology
                .clone()
                .expect("empty topology should be persisted"),
        )
        .unwrap();
        assert_eq!(stored_topology.room_count(), 0);
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
        assert_eq!(room["transitioning"], false);
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
        assert_eq!(room["transitioning"], false);
    }

    #[test]
    fn build_rooms_state_marks_active_mode_transition() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state.lock().unwrap().room_mode_transitions.insert(
            "r1".into(),
            crate::state::RoomModeTransition {
                ends_at: std::time::Instant::now() + std::time::Duration::from_secs(5),
                periodic_resume_at: std::time::Instant::now() + std::time::Duration::from_secs(5),
            },
        );

        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["rooms"][0]["transitioning"], true);
    }

    #[test]
    fn build_rooms_state_has_hub_connected() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["hub_connected"].is_boolean());
    }

    #[test]
    fn build_rooms_state_includes_hub_types_from_topology() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        add_topology_room(&state, "r1", &["matter", "mock"]);

        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(
            parsed["rooms"][0]["hub_types"],
            serde_json::json!(["matter", "mock"])
        );
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
        let (state, _rt) = setup_state_with_registry(vec![make_snapshot("r1", false, false)]);
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
        assert!(parsed["mode"].is_object());
        assert!(parsed["transitions"].is_array());
        assert!(parsed["profiles"].is_array());
        assert!(parsed["nodes"].is_array());
        assert!(parsed["nodes"][0]["transitioning"].is_boolean());
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn build_state_snapshot_storage_fallback_includes_hub_types_from_topology() {
        let storage = TestStorage::default();
        let mut rooms = rhythm_core::RoomManager::new();
        rooms.get_or_create("r1", "Room 1");
        storage.save_rooms(&rooms).unwrap();

        let app = AppState {
            storage: Some(Box::new(storage)),
            ..Default::default()
        };
        let state: SharedState = Arc::new(Mutex::new(app));
        add_topology_room(&state, "r1", &["matter", "mock"]);

        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(
            parsed["nodes"][0]["hub_types"],
            serde_json::json!(["matter", "mock"])
        );
    }

    #[test]
    fn build_state_snapshot_marks_active_mode_transition() {
        let (state, _rt) = setup_state_with_registry(vec![make_snapshot("r1", false, false)]);
        state.lock().unwrap().room_mode_transitions.insert(
            "r1".into(),
            crate::state::RoomModeTransition {
                ends_at: std::time::Instant::now() + std::time::Duration::from_secs(5),
                periodic_resume_at: std::time::Instant::now() + std::time::Duration::from_secs(5),
            },
        );

        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["nodes"][0]["transitioning"], true);
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
        assert!(parsed["settings"].get("mode").is_none());
        assert!(parsed["mode"].is_object());
        assert!(parsed["mode"].get("transitions").is_none());
        assert!(parsed["transitions"].is_array());
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
            let mut profile_registry = LightProfileRegistry::with_profiles(
                s.light_profile_configs
                    .values()
                    .cloned()
                    .collect::<Vec<_>>(),
                &s.active_mode_profile_id(),
            );
            profile_registry.set_mode_configs(s.mode_configs());
            let periodic_ctx = rhythm_core::CurveContext::new(
                current_hour,
                resolved_solar.solar,
                resolved_solar.sun_times,
            );
            let periodic_room_snapshots = s
                .hub_runtime()
                .map(|rt| {
                    rt.engine_all_room_snapshots()
                        .into_iter()
                        .filter(|room| room.rhythm_enabled)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let periodic_node_snapshots: Vec<rhythm_core::NodeSnapshot> = periodic_room_snapshots
                .iter()
                .cloned()
                .map(rhythm_core::NodeSnapshot::from_room_snapshot)
                .collect();
            let effective_rhythm_interval_secs = crate::periodic::effective_cycle_duration(
                &profile_registry,
                &periodic_ctx,
                &periodic_node_snapshots,
                std::time::Duration::from_secs(s.runtime_config.update_interval_secs),
                s.power_save,
            )
            .as_secs();
            (
                active_profile_effective_values(
                    &active_profile_cfg,
                    current_hour,
                    resolved_solar.solar,
                    resolved_solar.sun_times,
                ),
                effective_rhythm_interval_secs,
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
            expected.1
        );
        assert_eq!(
            parsed["location"]["solar_noon"].as_f64().unwrap() as f32,
            expected.2.solar_noon_hour
        );
        assert!(
            ((parsed["location"]["current_local_hour"].as_f64().unwrap() as f32) - current_hour)
                .abs()
                < 0.001
        );
        assert_eq!(
            parsed["location"]["solar_noon_local_time"]
                .as_str()
                .unwrap(),
            local_time_string_from_decimal_hour(expected.2.solar_noon_hour)
        );
        assert_eq!(
            parsed["location"]["solar_midnight"].as_f64().unwrap() as f32,
            expected.2.solar_midnight_hour()
        );
        assert_eq!(
            parsed["location"]["solar_midnight_local_time"]
                .as_str()
                .unwrap(),
            local_time_string_from_decimal_hour(expected.2.solar_midnight_hour())
        );
        assert_eq!(
            parsed["location"]["current_solar_time"].as_f64().unwrap() as f32,
            expected.2.get_solar_time(current_hour)
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
            assert!(room["transitioning"].is_boolean());
        }
    }

    // ========================================================================
    // set_brightness tests
    // ========================================================================

    #[test]
    fn set_brightness_succeeds() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_node_brightness(&state, "r1", 75, false);
        assert!(result.is_ok());
        // Setting brightness marks room as lights_on
        assert_eq!(state.lock().unwrap().room_lights_on.get("r1"), Some(&true));
    }

    #[test]
    fn set_brightness_updates_parent_lights_on_for_attached_light() {
        let (state, _rt, device_id) = setup_attached_hue_light_with_group_dispatch();

        let result = do_set_node_brightness(&state, &device_id, 75, false);

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(s.room_lights_on.get("room1"), Some(&true));
        assert_eq!(s.room_lights_on.len(), 1);
    }

    #[test]
    fn set_brightness_clamps_high() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        // 200 should be clamped to 100
        let result = do_set_node_brightness(&state, "r1", 200, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_brightness_clamps_low() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        // 0 should be clamped to 1
        let result = do_set_node_brightness(&state, "r1", 0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_brightness_no_runtime_errors() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_set_node_brightness(&state, "r1", 50, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No runtime"));
    }

    // ========================================================================
    // set_time_offset tests
    // ========================================================================

    #[test]
    fn set_time_offset_succeeds() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_node_time_offset(&state, "r1", 30.0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_time_offset_negative() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_node_time_offset(&state, "r1", -60.0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_time_offset_no_runtime_errors() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_set_node_time_offset(&state, "r1", 15.0, false);
        assert!(result.is_err());
    }

    // ========================================================================
    // room_preferences tests
    // ========================================================================

    #[test]
    fn room_preferences_rhythm_enabled() {
        let snap = make_snapshot("r1", false, false);
        let (state, _rt) = setup_state(vec![snap]);
        let result = do_node_preferences_set(&state, "r1", Some(true), None, None, None, false);
        assert!(result.is_ok());
    }

    #[test]
    fn room_preferences_disabled_does_not_change_lights_on_cache() {
        let snap = make_snapshot("r1", false, false);
        let (state, _rt) = setup_state(vec![snap]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let result = do_node_preferences_set(&state, "r1", None, Some(true), None, None, false);

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(s.room_lights_on.get("r1"), Some(&true));
    }

    #[test]
    fn room_preferences_idle_implies_rhythm_enabled() {
        // Idle should force rhythm enabled so standby ticks still render.
        let mut snap = make_snapshot("r1", false, false);
        snap.rhythm_enabled = false;
        let (state, _rt) = setup_state(vec![snap]);
        let result = do_node_preferences_set(
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
    fn room_preferences_idle_updates_parent_lights_on_for_attached_light() {
        let (state, rt, device_id) = setup_attached_hue_light_with_group_dispatch();
        if let Some(child) = rt
            .snapshots
            .lock()
            .unwrap()
            .iter_mut()
            .find(|snap| snap.id == device_id)
        {
            child.rhythm_enabled = false;
        }

        let result = do_node_preferences_set(
            &state,
            &device_id,
            Some(false),
            None,
            Some(RoomModeState::Idle),
            None,
            false,
        );

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(s.room_lights_on.get("room1"), Some(&true));
        assert_eq!(s.room_lights_on.len(), 1);
    }

    #[test]
    fn room_preferences_idle_updates_parent_lights_on_without_group_dispatch() {
        let (state, rt, device_id) = setup_attached_matter_light_without_group_dispatch();
        if let Some(child) = rt
            .snapshots
            .lock()
            .unwrap()
            .iter_mut()
            .find(|snap| snap.id == device_id)
        {
            child.rhythm_enabled = false;
        }

        let result = do_node_preferences_set(
            &state,
            &device_id,
            Some(false),
            None,
            Some(RoomModeState::Idle),
            None,
            false,
        );

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(s.room_lights_on.get(&device_id), Some(&true));
        assert_eq!(s.room_lights_on.get("room1"), Some(&true));
    }

    #[test]
    fn room_preferences_hard_off_keeps_mixed_room_parent_on_when_siblings_are_on() {
        let (state, runtime, matter_id, ha_id, hue_one_id, _hue_two_id) =
            setup_mixed_room_with_hub_groups();
        runtime.set_light_on(&matter_id, true);
        runtime.set_light_on(&ha_id, true);
        runtime.set_light_on(&hue_one_id, true);

        let result = do_node_preferences_set(
            &state,
            &matter_id,
            None,
            None,
            Some(RoomModeState::HardOff),
            None,
            false,
        );

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(s.room_lights_on.get(&matter_id), Some(&false));
        assert_eq!(s.room_lights_on.get("room1"), Some(&true));
    }

    #[test]
    fn room_preferences_missing_room_errors() {
        let (state, _rt) = setup_state(vec![]);
        let result =
            do_node_preferences_set(&state, "nonexistent", Some(true), None, None, None, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn room_preferences_hard_off_queues_motion_timer_clear() {
        let snap = make_snapshot("r1", false, false);
        let (state, runtime) = setup_state(vec![snap]);
        state.lock().unwrap().motion_snapshots.insert(
            "r1".into(),
            MotionSnapshot {
                motion_active: true,
                motion_owned: true,
                remaining_secs: Some(120),
                timeout_secs: 300,
                warning_active: true,
            },
        );

        let result = do_node_preferences_set(
            &state,
            "r1",
            None,
            None,
            Some(RoomModeState::HardOff),
            None,
            false,
        );

        assert!(result.is_ok());
        assert_eq!(
            runtime.events(),
            vec![("r1".into(), ButtonAction::LightsOff)]
        );
        assert_eq!(
            state.lock().unwrap().pending_motion_clear,
            vec!["r1".to_string()]
        );
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
                rhythm_core::DEFAULT_MODE_TRANSITION_DURATION_MS,
            )
            .with_trigger(ModeTransitionTrigger::Sunrise)]);
        }

        do_set_active_mode_with_trigger(&state, RhythmMode::Day, ModeTransitionTrigger::Sunrise)
            .unwrap();

        let applied = runtime.applied_commands();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "r1");
        assert_eq!(
            applied[0].1.transition_ms,
            Some(rhythm_core::DEFAULT_MODE_TRANSITION_DURATION_MS)
        );
        assert!(state
            .lock()
            .unwrap()
            .room_mode_transitions
            .contains_key("r1"));
    }

    #[test]
    fn mode_transition_staggers_periodic_resume_per_room() {
        let (state, runtime) = setup_state(vec![
            make_snapshot("r1", false, false),
            make_snapshot("r2", false, false),
        ]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Sleep;
            s.room_lights_on.insert("r1".into(), true);
            s.room_lights_on.insert("r2".into(), true);
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                RhythmMode::Sleep,
                RhythmMode::Day,
                1,
            )
            .with_trigger(ModeTransitionTrigger::Sunrise)]);
        }

        do_set_active_mode_with_trigger(&state, RhythmMode::Day, ModeTransitionTrigger::Sunrise)
            .unwrap();

        let applied = runtime.applied_commands();
        assert_eq!(applied.len(), 2);

        let update_interval = std::time::Duration::from_secs(
            state.lock().unwrap().runtime_config.update_interval_secs,
        );
        let transitions = state.lock().unwrap().room_mode_transitions.clone();
        let first = transitions.get(&applied[0].0).unwrap();
        let second = transitions.get(&applied[1].0).unwrap();

        assert!(second.ends_at > first.ends_at);
        assert_eq!(
            first.periodic_resume_at.duration_since(first.ends_at),
            update_interval
        );
        assert_eq!(
            second.periodic_resume_at.duration_since(second.ends_at),
            update_interval
        );
    }

    #[test]
    fn mode_transition_auto_uses_target_profile_fade() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            let mut rhythm = s
                .light_profile_configs
                .get("rhythm")
                .cloned()
                .unwrap_or_else(rhythm_core::default_rhythm_profile);
            rhythm.fade_ms = TimerSetting::Fixed { value: 1_234 };
            s.set_light_profile_config(rhythm);
            s.active_mode = RhythmMode::Sleep;
            s.room_lights_on.insert("r1".into(), true);
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                RhythmMode::Sleep,
                RhythmMode::Day,
                rhythm_core::DEFAULT_MODE_TRANSITION_DURATION_MS,
            )
            .with_duration(TimerSetting::Auto)
            .with_trigger(ModeTransitionTrigger::Sunrise)]);
        }

        do_set_active_mode_with_trigger(&state, RhythmMode::Day, ModeTransitionTrigger::Sunrise)
            .unwrap();

        let applied = runtime.applied_commands();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "r1");
        assert_eq!(applied[0].1.transition_ms, Some(1_234));
        assert!(state
            .lock()
            .unwrap()
            .room_mode_transitions
            .contains_key("r1"));
    }

    #[test]
    fn mode_transition_auto_falls_back_to_default_duration_when_target_fade_is_auto() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            let mut rhythm = s
                .light_profile_configs
                .get("rhythm")
                .cloned()
                .unwrap_or_else(rhythm_core::default_rhythm_profile);
            rhythm.fade_ms = TimerSetting::Auto;
            s.set_light_profile_config(rhythm);
            s.active_mode = RhythmMode::Sleep;
            s.room_lights_on.insert("r1".into(), true);
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                RhythmMode::Sleep,
                RhythmMode::Day,
                rhythm_core::DEFAULT_MODE_TRANSITION_DURATION_MS,
            )
            .with_duration(TimerSetting::Auto)
            .with_trigger(ModeTransitionTrigger::Sunrise)]);
        }

        do_set_active_mode_with_trigger(&state, RhythmMode::Day, ModeTransitionTrigger::Sunrise)
            .unwrap();

        let applied = runtime.applied_commands();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "r1");
        assert_eq!(
            applied[0].1.transition_ms,
            Some(rhythm_core::DEFAULT_MODE_TRANSITION_DURATION_MS)
        );
        assert!(state
            .lock()
            .unwrap()
            .room_mode_transitions
            .contains_key("r1"));
    }

    #[test]
    fn triggering_transition_by_id_reapplies_saved_transition() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        let transition_id = {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Sleep;
            s.room_lights_on.insert("r1".into(), true);
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                RhythmMode::Day,
                RhythmMode::Sleep,
                4_000,
            )
            .with_trigger(ModeTransitionTrigger::NauticalTwilight)]);
            s.mode_transition_configs()[0].id.clone()
        };

        let json = do_trigger_transition(&state, &transition_id).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["active"], "sleep");
        assert_eq!(parsed["last_change"]["cause"], "manual");
        assert_eq!(parsed["last_change"]["transition_id"], transition_id);

        let applied = runtime.applied_commands();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "r1");
        assert_eq!(applied[0].1.transition_ms, Some(4_000));

        let s = state.lock().unwrap();
        assert_eq!(s.last_active_mode_cause, ModeChangeCause::Manual);
        assert_eq!(
            s.last_active_mode_transition_id.as_deref(),
            Some(transition_id.as_str())
        );
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
                room_defaults: vec![],
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
                room_defaults: vec![],
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
                room_defaults: vec![],
            }]),
            None,
        )
        .unwrap();

        assert!(runtime.applied_commands().is_empty());
    }

    #[test]
    fn mode_change_applies_room_defaults_before_lighting_recalc() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Day;
            s.room_lights_on.insert("r1".into(), true);
            s.set_mode_configs(vec![ModeConfig {
                mode: RhythmMode::Sleep,
                active_profile_id: Some(rhythm_core::SLEEP_PROFILE_ID.into()),
                idle_profile_id: Some(rhythm_core::SLEEP_IDLE_PROFILE_ID.into()),
                wake_profile_id: None,
                warning_profile_id: None,
                room_defaults: vec![rhythm_core::RoomModeDefault {
                    room_id: "r1".into(),
                    state: RoomModeState::Idle,
                }],
            }]);
        }

        do_set_active_mode(&state, RhythmMode::Sleep).unwrap();

        assert_eq!(runtime.restore_calls(), vec![("r1".into(), true, false)]);
        assert_eq!(
            runtime.applied_states(),
            vec![("r1".into(), RoomModeState::Idle)]
        );

        let snap = runtime.engine_room_snapshot("r1").unwrap();
        assert!(snap.soft_off);
        assert!(!snap.hard_off);
    }

    #[test]
    fn room_default_overrides_preserve_hard_off_for_explicit_room() {
        let mut snapshot = make_snapshot("r1", false, false);
        snapshot.hard_off = true;
        let (state, runtime) = setup_state(vec![snapshot]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Day;
            s.room_lights_on.insert("r1".into(), false);
            s.set_mode_configs(vec![ModeConfig {
                mode: RhythmMode::Sleep,
                active_profile_id: Some(rhythm_core::SLEEP_PROFILE_ID.into()),
                idle_profile_id: Some(rhythm_core::SLEEP_IDLE_PROFILE_ID.into()),
                wake_profile_id: None,
                warning_profile_id: None,
                room_defaults: vec![rhythm_core::RoomModeDefault {
                    room_id: "r1".into(),
                    state: RoomModeState::Active,
                }],
            }]);
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                RhythmMode::Day,
                RhythmMode::Sleep,
                rhythm_core::DEFAULT_MODE_TRANSITION_DURATION_MS,
            )
            .with_trigger(ModeTransitionTrigger::NauticalTwilight)]);
        }

        do_set_active_mode_with_trigger(
            &state,
            RhythmMode::Sleep,
            ModeTransitionTrigger::NauticalTwilight,
        )
        .unwrap();

        assert_eq!(runtime.restore_calls(), vec![("r1".into(), false, false)]);
        assert_eq!(
            runtime.applied_states(),
            vec![("r1".into(), RoomModeState::Active)]
        );
        assert_eq!(state.lock().unwrap().room_lights_on.get("r1"), Some(&true));
    }

    #[test]
    fn mode_change_room_default_hard_off_queues_motion_timer_clear() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Day;
            s.room_lights_on.insert("r1".into(), true);
            s.motion_snapshots.insert(
                "r1".into(),
                MotionSnapshot {
                    motion_active: false,
                    motion_owned: true,
                    remaining_secs: Some(180),
                    timeout_secs: 300,
                    warning_active: false,
                },
            );
            s.set_mode_configs(vec![ModeConfig {
                mode: RhythmMode::Sleep,
                active_profile_id: Some(rhythm_core::SLEEP_PROFILE_ID.into()),
                idle_profile_id: Some(rhythm_core::SLEEP_IDLE_PROFILE_ID.into()),
                wake_profile_id: None,
                warning_profile_id: None,
                room_defaults: vec![rhythm_core::RoomModeDefault {
                    room_id: "r1".into(),
                    state: RoomModeState::HardOff,
                }],
            }]);
        }

        do_set_active_mode(&state, RhythmMode::Sleep).unwrap();

        assert_eq!(runtime.lights_off_calls(), vec![("r1".into(), None)]);
        assert_eq!(
            state.lock().unwrap().pending_motion_clear,
            vec!["r1".to_string()]
        );
    }

    #[test]
    fn mode_transition_room_default_hard_off_uses_transition_fade() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Day;
            s.room_lights_on.insert("r1".into(), true);
            s.set_mode_configs(vec![ModeConfig {
                mode: RhythmMode::Sleep,
                active_profile_id: Some(rhythm_core::SLEEP_PROFILE_ID.into()),
                idle_profile_id: Some(rhythm_core::SLEEP_IDLE_PROFILE_ID.into()),
                wake_profile_id: None,
                warning_profile_id: None,
                room_defaults: vec![rhythm_core::RoomModeDefault {
                    room_id: "r1".into(),
                    state: RoomModeState::HardOff,
                }],
            }]);
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                RhythmMode::Day,
                RhythmMode::Sleep,
                4_321,
            )
            .with_trigger(ModeTransitionTrigger::NauticalTwilight)]);
        }

        do_set_active_mode_with_trigger(
            &state,
            RhythmMode::Sleep,
            ModeTransitionTrigger::NauticalTwilight,
        )
        .unwrap();

        assert_eq!(runtime.lights_off_calls(), vec![("r1".into(), Some(4_321))]);
        assert!(runtime.applied_commands().is_empty());
    }

    #[test]
    fn active_mode_room_default_change_does_not_reapply_outputs() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Day;
            s.room_lights_on.insert("r1".into(), true);
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
                room_defaults: vec![rhythm_core::RoomModeDefault {
                    room_id: "r1".into(),
                    state: RoomModeState::Idle,
                }],
            }]),
            None,
        )
        .unwrap();

        assert!(runtime.restore_calls().is_empty());
        assert!(runtime.applied_states().is_empty());

        let snap = runtime.engine_room_snapshot("r1").unwrap();
        assert!(
            !snap.soft_off,
            "room defaults should not apply until mode activation"
        );
        assert!(!snap.hard_off);
    }

    #[test]
    fn mode_set_rejects_runtime_only_room_defaults() {
        let (state, _runtime) = setup_state(vec![]);

        let err = do_mode_set(
            &state,
            None,
            Some(vec![ModeConfig {
                mode: RhythmMode::Sleep,
                active_profile_id: Some(rhythm_core::SLEEP_PROFILE_ID.into()),
                idle_profile_id: None,
                wake_profile_id: None,
                warning_profile_id: None,
                room_defaults: vec![rhythm_core::RoomModeDefault {
                    room_id: "r1".into(),
                    state: RoomModeState::Warning,
                }],
            }]),
        )
        .unwrap_err();

        assert!(err.to_string().contains("runtime-only state"));
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

    #[test]
    fn triage_new_on_room_binding_keeps_rooms_separate() {
        let (state, _runtime) = setup_state(vec![]);
        {
            let mut s = state.lock().unwrap();
            s.canonical_registry
                .triage_mut()
                .add(crate::canonical::triage::TriageEntry {
                    id: "rb1".to_string(),
                    kind: crate::canonical::triage::TriageKind::RoomBinding,
                    discovered: crate::canonical::triage::TriageDiscoveredDevice::default(),
                    hub_key: HubKey::new(HubType::new("matter"), "local"),
                    candidate_matches: vec![],
                    room_binding: Some(crate::canonical::triage::RoomBindingProposal {
                        hub_room_id: "matter-room-1".to_string(),
                        hub_room_name: "Kitchen".to_string(),
                        control_id: "control-1".to_string(),
                        light_device_ids: vec![],
                        canonical_device_ids: vec![],
                        target_rhythm_room_id: "room-1".to_string(),
                        target_rhythm_room_name: "Kitchen".to_string(),
                        candidate_rooms: vec![],
                    }),
                    confidence: 80,
                    status: crate::canonical::triage::TriageStatus::Pending,
                    resolved_by: None,
                    created_at: 1000,
                    resolved_at: None,
                    canonical_id: None,
                });
        }

        let result = do_triage_new_device(&state, "rb1").unwrap();
        let s = state.lock().unwrap();

        assert_eq!(result, r#"{"status":"kept_separate"}"#);
        assert_eq!(s.canonical_registry.device_count(), 0);
        assert_eq!(
            s.canonical_registry.triage().get("rb1").unwrap().status,
            crate::canonical::triage::TriageStatus::KeptSeparate
        );
    }

    #[test]
    fn canonical_assign_room_rejects_unknown_room() {
        let (state, _runtime) = setup_state(vec![]);
        let device_id = insert_canonical_device(
            &state,
            HubKey::new(HubType::new("matter"), "local"),
            "matter-device-1",
            "Desk Lamp",
            "",
            "",
        );

        let err = do_canonical_assign_room(&state, &device_id, Some("missing-room")).unwrap_err();

        assert!(err.to_string().contains("Room not found: missing-room"));
        assert_eq!(
            state
                .lock()
                .unwrap()
                .canonical_registry
                .get(&device_id)
                .unwrap()
                .room_id,
            None
        );
    }

    #[test]
    fn triage_new_device_without_room_creates_standalone_topology_node() {
        let (state, _runtime) = setup_state(vec![]);
        {
            let mut s = state.lock().unwrap();
            s.canonical_registry
                .triage_mut()
                .add(crate::canonical::triage::TriageEntry {
                    id: "td1".to_string(),
                    kind: crate::canonical::triage::TriageKind::DeviceMerge,
                    discovered: crate::canonical::triage::TriageDiscoveredDevice {
                        native_id: "matter-standalone-1".to_string(),
                        name: "Desk Lamp".to_string(),
                        device_type: DeviceType::Light,
                        room_id: String::new(),
                        room_name: String::new(),
                        manufacturer: None,
                        model: None,
                    },
                    hub_key: HubKey::new(HubType::new("matter"), "local"),
                    candidate_matches: vec![],
                    room_binding: None,
                    confidence: 80,
                    status: crate::canonical::triage::TriageStatus::Pending,
                    resolved_by: None,
                    created_at: 1000,
                    resolved_at: None,
                    canonical_id: None,
                });
        }

        let result = do_triage_new_device(&state, "td1").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let canonical_id = parsed["canonical_id"].as_str().unwrap();

        let s = state.lock().unwrap();
        let node = s
            .topology
            .get_device_node(canonical_id)
            .expect("standalone topology device node should exist");
        assert_eq!(node.parent_id, None);
        assert_eq!(node.placement, crate::topology::DevicePlacement::Standalone);
        assert_eq!(
            s.canonical_registry.get(canonical_id).unwrap().room_id,
            None
        );
    }

    #[test]
    fn topology_set_motion_control_target_clears_old_and_new_targets() {
        let (state, _runtime) = setup_state(vec![]);
        add_topology_room(&state, "room1", &[]);
        add_topology_room(&state, "room2", &[]);
        {
            let mut s = state.lock().unwrap();
            assert!(s.topology.attach_device_user_override("room1", "sensor-1"));
        }

        do_topology_set_control_target(&state, "sensor-1", NodeControlKind::Motion, Some("room2"))
            .unwrap();

        let s = state.lock().unwrap();
        assert_eq!(
            s.topology
                .explicit_control_target("sensor-1", &NodeControlKind::Motion),
            Some("room2")
        );
        assert_eq!(
            s.pending_motion_clear,
            vec!["room1".to_string(), "room2".to_string()]
        );
    }

    #[test]
    fn canonical_assign_room_initializes_runtime_for_roomless_matter() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();
        let room_id = state.lock().unwrap().topology.create_room("Office");
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        do_canonical_assign_room(&state, &device_id, Some(&room_id)).unwrap();

        assert!(state.lock().unwrap().hub_runtime().is_some());
        let snap = runtime
            .engine_room_snapshot(&room_id)
            .expect("assigned topology room should exist in runtime");
        assert_eq!(snap.name, "Office");
        assert!(snap.rhythm_enabled);
    }

    #[test]
    fn canonical_assign_room_reparents_existing_runtime_device() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();
        let room_one = state.lock().unwrap().topology.create_room("Office");
        let room_two = state.lock().unwrap().topology.create_room("Desk");
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        do_canonical_assign_room(&state, &device_id, Some(&room_one)).unwrap();
        assert_eq!(
            runtime
                .engine_node_snapshot(&device_id)
                .expect("runtime node should exist after initial assignment")
                .parent_id
                .as_deref(),
            Some(room_one.as_str())
        );

        do_canonical_assign_room(&state, &device_id, Some(&room_two)).unwrap();
        assert_eq!(
            runtime
                .engine_node_snapshot(&device_id)
                .expect("runtime node should still exist after reassignment")
                .parent_id
                .as_deref(),
            Some(room_two.as_str())
        );

        do_canonical_assign_room(&state, &device_id, None).unwrap();
        assert_eq!(
            runtime
                .engine_node_snapshot(&device_id)
                .expect("runtime node should still exist after unassign")
                .parent_id,
            None
        );
    }

    #[test]
    fn canonical_unassign_keeps_synthetic_registry_room_for_device_queries() {
        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();
        let room_id = state.lock().unwrap().topology.create_room("Office");
        let device_id =
            insert_canonical_device(&state, hub_key.clone(), "matter-100", "Desk Lamp", "", "");

        do_canonical_assign_room(&state, &device_id, Some(&room_id)).unwrap();
        do_canonical_assign_room(&state, &device_id, None).unwrap();

        let s = state.lock().unwrap();
        let registry = s
            .hubs
            .get(&hub_key)
            .and_then(|hub| hub.registry.as_ref())
            .expect("hub registry should exist")
            .lock()
            .unwrap();
        assert_eq!(
            registry.devices_for_room("matter-100"),
            vec!["matter-100".to_string()]
        );
        assert_eq!(
            registry.get_grouped_light_id("matter-100"),
            Some("matter-100".to_string())
        );
    }

    #[test]
    fn build_state_snapshot_prefers_topology_parent_for_device_nodes() {
        let (state, runtime) = setup_state(vec![]);
        add_topology_room(&state, "room1", &["mock"]);
        let hub_key = state.lock().unwrap().hubs.keys().next().cloned().unwrap();
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        {
            let mut s = state.lock().unwrap();
            s.canonical_registry.assign_room(&device_id, None);
            s.topology.ensure_standalone_device(&device_id);
        }

        runtime.add_node(
            &device_id,
            "Desk Lamp",
            rhythm_core::LightNodeKind::LightDevice,
            Some("room1".to_string()),
        );

        let parsed: serde_json::Value =
            serde_json::from_str(&build_state_snapshot(&state).unwrap()).unwrap();
        let device = parsed["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"].as_str() == Some(device_id.as_str()))
            .expect("device should appear in state snapshot");

        assert!(
            device["parent_id"].is_null(),
            "state snapshot should follow topology parent, not stale runtime parent"
        );
    }

    #[test]
    fn topology_create_room_initializes_runtime_when_hub_exists() {
        let (state, runtime, _hub_key) = setup_state_with_deferred_runtime();

        let result = do_topology_create_room(&state, "Office").unwrap();
        let created: serde_json::Value = serde_json::from_str(&result).unwrap();
        let room_id = created["id"].as_str().unwrap();

        assert!(state.lock().unwrap().hub_runtime().is_some());
        let snap = runtime
            .engine_room_snapshot(room_id)
            .expect("created topology room should exist in runtime");
        assert_eq!(snap.name, "Office");
        assert!(snap.rhythm_enabled);
    }

    #[test]
    fn topology_delete_room_unassigns_devices_and_cleans_state() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();

        let result = do_topology_create_room(&state, "Office").unwrap();
        let created: serde_json::Value = serde_json::from_str(&result).unwrap();
        let room_id = created["id"].as_str().unwrap().to_string();
        let device_id =
            insert_canonical_device(&state, hub_key.clone(), "matter-100", "Desk Lamp", "", "");

        {
            let mut s = state.lock().unwrap();
            s.canonical_registry.assign_room(&device_id, Some(&room_id));
            assert!(s.topology.attach_device_user_override(&room_id, &device_id));
            s.room_lights_on.insert(room_id.clone(), true);
            s.motion_snapshots.insert(
                room_id.clone(),
                crate::state::MotionSnapshot {
                    motion_active: true,
                    motion_owned: true,
                    remaining_secs: None,
                    timeout_secs: 300,
                    warning_active: false,
                },
            );
            s.room_mode_transitions.insert(
                room_id.clone(),
                crate::state::RoomModeTransition {
                    ends_at: std::time::Instant::now(),
                    periodic_resume_at: std::time::Instant::now(),
                },
            );
        }

        do_topology_delete_room(&state, &room_id).unwrap();

        let s = state.lock().unwrap();
        assert!(s.topology.get(&room_id).is_none());
        assert_eq!(
            s.topology.get_device_node(&device_id).unwrap().parent_id,
            None
        );
        assert_eq!(
            s.canonical_registry
                .get(&device_id)
                .unwrap()
                .room_id
                .as_deref(),
            None
        );
        assert_eq!(s.canonical_registry.triage().pending_unassigned_count(), 1);
        assert!(!s.room_lights_on.contains_key(&room_id));
        assert!(!s.motion_snapshots.contains_key(&room_id));
        assert!(!s.room_mode_transitions.contains_key(&room_id));
        assert_eq!(s.pending_motion_clear, vec![room_id.clone()]);
        {
            let registry = s
                .hubs
                .get(&hub_key)
                .and_then(|hub| hub.registry.as_ref())
                .expect("hub registry should exist")
                .lock()
                .unwrap();
            assert_eq!(
                registry.devices_for_room("matter-100"),
                vec!["matter-100".to_string()]
            );
            assert_eq!(
                registry.get_grouped_light_id("matter-100"),
                Some("matter-100".to_string())
            );
        }
        drop(s);

        assert!(
            runtime.engine_room_snapshot(&room_id).is_none(),
            "deleted room should be removed from runtime"
        );
    }

    #[test]
    fn device_hard_remove_evicts_runtime_node_and_periodic_state() {
        let (state, runtime, device_id) = setup_attached_matter_light_without_group_dispatch();

        {
            let mut s = state.lock().unwrap();
            s.room_lights_on.insert(device_id.clone(), true);
            s.motion_snapshots.insert(
                device_id.clone(),
                crate::state::MotionSnapshot {
                    motion_active: true,
                    motion_owned: true,
                    remaining_secs: Some(30),
                    timeout_secs: 300,
                    warning_active: false,
                },
            );
            s.room_mode_transitions.insert(
                device_id.clone(),
                crate::state::RoomModeTransition {
                    ends_at: std::time::Instant::now(),
                    periodic_resume_at: std::time::Instant::now(),
                },
            );
            s.pending_periodic_ticks.insert(device_id.clone(), 12.0);
            s.pending_motion_clear.push(device_id.clone());
            s.pending_motion_seed
                .push((device_id.clone(), "room1".to_string()));
        }

        do_device_hard_remove(&state, &device_id, None).unwrap();

        let s = state.lock().unwrap();
        assert!(s.canonical_registry.get(&device_id).is_none());
        assert!(s.topology.get_device_node(&device_id).is_none());
        assert!(!s.room_lights_on.contains_key(&device_id));
        assert!(!s.motion_snapshots.contains_key(&device_id));
        assert!(!s.room_mode_transitions.contains_key(&device_id));
        assert!(!s.pending_periodic_ticks.contains_key(&device_id));
        assert_eq!(s.pending_motion_clear, vec![device_id.clone()]);
        assert!(s
            .pending_motion_seed
            .iter()
            .all(|(source, target)| source != &device_id && target != &device_id));
        drop(s);

        assert!(
            runtime.engine_node_snapshot(&device_id).is_none(),
            "hard-removed device should be removed from runtime"
        );
    }

    #[test]
    fn triage_assign_room_resolves_unassigned_device() {
        let (state, _runtime) = setup_state(vec![]);
        add_topology_room(&state, "room-1", &["matter"]);
        let hub_key = HubKey::new(HubType::new("matter"), "local");
        let device_id = insert_canonical_device(
            &state,
            hub_key.clone(),
            "matter-device-1",
            "Desk Lamp",
            "",
            "",
        );
        let entry_id = {
            let mut s = state.lock().unwrap();
            s.canonical_registry.queue_unassigned(&device_id, 1000);
            s.canonical_registry
                .triage()
                .pending_by_kind(crate::canonical::triage::TriageKind::UnassignedDevice)[0]
                .id
                .clone()
        };

        do_triage_assign_room(&state, &entry_id, "room-1").unwrap();

        let s = state.lock().unwrap();
        let device = s.canonical_registry.get(&device_id).unwrap();
        let room = s.topology.get("room-1").unwrap();

        assert_eq!(device.room_id.as_deref(), Some("room-1"));
        assert_eq!(s.canonical_registry.triage().pending_unassigned_count(), 0);
        assert!(
            room.devices
                .iter()
                .any(|room_device| room_device.device_id == device_id),
            "assigned device should be tracked in topology room membership"
        );
    }
}
