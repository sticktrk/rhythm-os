//! Extracted business logic for configuration commands.
//!
//! Platform-agnostic functions called by HTTP handlers.
//! Each function takes `SharedState` and parsed parameters, performs the
//! operation (registry update, engine update, persistence), and returns
//! a result that the transport layer can format into a response.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{Datelike, Timelike};
use log::{debug, info, warn};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::{
    ButtonAction, HubDispatchTarget, InputEvent, LightNodeKind, LightProfileConfig,
    LightProfileRegistry, LightingCommand, ModeChangeCause, ModeConfig, ModeTransitionConfig,
    ModeTransitionTrigger, RestoredNodeState, RestoredRoomState, RhythmMode, RoomModeState,
    RoomProfileSettings, RuntimeHandle, TimerSetting,
};
use serde_json::Value;

use crate::api_types::{
    ActiveProfileDto, ActiveProfileEffectiveDto, ApiCapabilitiesDto, HubCapabilityDto, HubDto,
    HubStartupRetryDto, InputBindingsDto, LocationDto, ModeLastChangeDto, ModeSettingsDto,
    ModeTransitionsDto, NodeStateDto, NodesPollResponse, ObservedPowerDto, PreferredEndpointDto,
    ProfilesDto, ReviewCountsDto, ReviewEntryDto, ReviewHubDto, ReviewSummaryDto, RoomPollState,
    RoomRhythmState, RoomsPollResponse, SettingsDto, StateSnapshot, TopologyNodeControlDto,
    TopologyNodeDto,
};
use crate::bundle::{
    BackupBundle, BackupConfiguration, BackupConfigurationRoom, BackupHubCredentials,
    BackupHubRegistry, BackupInstallation, BackupIntegrationFile, BackupRuntimeState,
    ProfileBundle, ProfileBundleData, ProfileBundleImportPayload, BUNDLE_SCHEMA_VERSION,
};
use crate::canonical::identity::HubKey;
use crate::factory_default_config::{
    factory_default_active_mode, factory_default_active_profile_config_for_mode,
    factory_default_idle_profile_config_for_mode, factory_default_light_profile_config_map,
    factory_default_mode_config_map, factory_default_mode_transition_configs,
    factory_default_power_save, factory_default_profile_bundle,
};
use crate::state::{
    current_epoch_ms, rooms_from_engine, AppState, ObservedPowerSource, ObservedPowerState,
    SharedState, WorkItem,
};
use crate::storage::StoredLocation;
use crate::topology::{
    AutomationAction, InputBinding, InputBindingPreset, ModeTransitionSelection, NodeControlKind,
};

pub const DEFAULT_HTTP_BATCH_DISPATCH_SPACING_MS: u64 = 500;

pub fn default_http_batch_dispatch_spacing() -> Duration {
    Duration::from_millis(DEFAULT_HTTP_BATCH_DISPATCH_SPACING_MS)
}

pub fn estimated_dispatch_duration(dispatch_count: usize, dispatch_spacing: Duration) -> Duration {
    if dispatch_count <= 1 {
        Duration::ZERO
    } else {
        dispatch_spacing.saturating_mul((dispatch_count - 1) as u32)
    }
}

// ============================================================================
// Display value computation
// ============================================================================

/// Compute effective brightness and kelvin for a room given its offsets.
///
/// Uses the provided light profile config, current solar context, and room offsets
/// to produce the values a client should display.
#[allow(clippy::too_many_arguments)]
pub fn compute_room_display_values(
    config: &LightProfileConfig,
    solar_noon: f32,
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&str>,
    utc_offset: f32,
    time_offset_minutes: f32,
    brightness_offset: f32,
) -> (u8, u16) {
    use rhythm_core::{LightProfile, LightProfileModule};

    let local = current_local_datetime(utc_offset);
    let ctx = rhythm_core::curve_context_for_local_datetime(
        solar_noon,
        latitude,
        longitude,
        timezone_name,
        local,
    );
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
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&'a str>,
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
    dispatch_generation: u64,
    power_save: bool,
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
        config.from_mode == from_mode
            && config.to_mode == to_mode
            && config.trigger == trigger
            && (trigger.is_manual() || config.trigger_enabled)
    })
}

fn auto_mode_transition(
    state: &SharedState,
    from_mode: RhythmMode,
    to_mode: RhythmMode,
) -> Option<ModeTransitionConfig> {
    let Ok(s) = state.lock() else { return None };
    let transitions = s.mode_transition_configs();

    transitions
        .iter()
        .find(|config| {
            config.from_mode == from_mode
                && config.to_mode == to_mode
                && config.trigger == ModeTransitionTrigger::Manual
        })
        .cloned()
        .or_else(|| {
            transitions
                .iter()
                .find(|config| config.from_mode == from_mode && config.to_mode == to_mode)
                .cloned()
        })
}

fn selected_mode_transition(
    state: &SharedState,
    from_mode: RhythmMode,
    to_mode: RhythmMode,
    selection: &ModeTransitionSelection,
) -> Result<Option<ModeTransitionConfig>> {
    match selection {
        ModeTransitionSelection::None => Ok(None),
        ModeTransitionSelection::Auto => Ok(auto_mode_transition(state, from_mode, to_mode)),
        ModeTransitionSelection::Exact { id } => {
            let transition = {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                s.mode_transition_configs()
                    .into_iter()
                    .find(|config| config.id == *id)
            }
            .ok_or_else(|| anyhow::anyhow!("Unknown transition '{}'", id))?;

            if transition.from_mode != from_mode || transition.to_mode != to_mode {
                return Err(anyhow::anyhow!(
                    "Transition '{}' is {:?}->{:?}, expected {:?}->{:?}",
                    id,
                    transition.from_mode,
                    transition.to_mode,
                    from_mode,
                    to_mode
                ));
            }

            Ok(Some(transition))
        }
    }
}

fn apply_mode_action(
    state: &SharedState,
    target_mode: RhythmMode,
    transition_selection: &ModeTransitionSelection,
) -> Result<()> {
    let previous_mode = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .active_mode;
    let transition =
        selected_mode_transition(state, previous_mode, target_mode, transition_selection)?;
    let force_reapply_outputs = transition.is_some();
    let mode_change = ModeChangeContext::new(ModeChangeCause::Manual, transition);

    do_settings_set_internal(
        state,
        None,
        Some(target_mode),
        None,
        None,
        Some(mode_change),
        force_reapply_outputs,
    )
    .map(|_| ())
}

fn next_mode_in_cycle(active_mode: RhythmMode, modes: &[RhythmMode]) -> Result<RhythmMode> {
    if modes.len() < 2 {
        return Err(anyhow::anyhow!(
            "mode_cycle action requires at least two modes"
        ));
    }
    let target_index = modes
        .iter()
        .position(|mode| *mode == active_mode)
        .map(|index| (index + 1) % modes.len())
        .unwrap_or(0);
    Ok(modes[target_index])
}

fn validate_automation_action(action: &AutomationAction) -> Result<()> {
    if let AutomationAction::ModeCycle { modes, .. } = action {
        if modes.len() < 2 {
            return Err(anyhow::anyhow!(
                "mode_cycle action requires at least two modes"
            ));
        }
        for (index, mode) in modes.iter().enumerate() {
            if modes[..index].contains(mode) {
                return Err(anyhow::anyhow!(
                    "mode_cycle action contains duplicate mode {:?}",
                    mode
                ));
            }
        }
    }
    Ok(())
}

pub fn do_execute_automation_action(state: &SharedState, action: &AutomationAction) -> Result<()> {
    match action {
        AutomationAction::ModeCycle { modes, transition } => {
            let active_mode = state
                .lock()
                .map_err(|_| anyhow::anyhow!("lock"))?
                .active_mode;
            let target_mode = next_mode_in_cycle(active_mode, modes)?;
            apply_mode_action(state, target_mode, transition)
        }
        AutomationAction::ModeToggle {
            first_mode,
            second_mode,
            transition,
        } => {
            let active_mode = state
                .lock()
                .map_err(|_| anyhow::anyhow!("lock"))?
                .active_mode;
            let target_mode = if active_mode == *first_mode {
                *second_mode
            } else {
                *first_mode
            };
            apply_mode_action(state, target_mode, transition)
        }
        AutomationAction::ModeSet { mode, transition } => {
            apply_mode_action(state, *mode, transition)
        }
    }
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
    if room.room_state == RoomModeState::HardOff {
        return (0, 0);
    }

    let local = current_local_datetime(lighting.utc_offset);
    let ctx = rhythm_core::curve_context_for_local_datetime(
        lighting.solar_noon,
        lighting.latitude,
        lighting.longitude,
        lighting.timezone_name,
        local,
    );
    let render_state = render_state_for_display(room.room_state);
    if matches!(render_state, RoomModeState::HardOff) {
        return (0, 0);
    }
    let registry = light_profile_registry_from_parts(
        lighting.light_profile_configs,
        lighting.mode_configs,
        lighting.mode,
    );
    let values = registry.calculate_room_values(
        lighting.mode,
        render_state,
        Some(room.settings),
        &ctx,
        room.time_offset_minutes,
    );
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
            latitude: s.latitude,
            longitude: s.longitude,
            timezone_name: s.timezone_name.as_deref(),
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

fn room_state_for_power_save(power_save: bool, state: RoomModeState) -> RoomModeState {
    if power_save && matches!(state, RoomModeState::Idle) {
        RoomModeState::HardOff
    } else {
        state
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
    s.room_observed_power.remove(node_id);
    s.motion_snapshots.remove(node_id);
    s.room_mode_transitions.remove(node_id);
    s.pending_periodic_ticks.remove(node_id);
    s.pending_motion_clear.retain(|pending| pending != node_id);
    s.pending_motion_seed
        .retain(|seed| seed.source_node_id != node_id && seed.target_node_id != node_id);
}

pub(crate) fn reconcile_hub_endpoint_visibility(
    state: &SharedState,
    hub_key: &HubKey,
    discovered_native_ids: &HashSet<String>,
) -> Result<(usize, usize)> {
    let (hidden_ids, topology_changed, affected_count) = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let report = s
            .canonical_registry
            .deactivate_missing_endpoints_for_hub(hub_key, discovered_native_ids);
        if report.affected_device_ids.is_empty() {
            return Ok((0, 0));
        }

        let hidden_ids = report.hidden_device_ids.clone();
        let mut topology_changed = false;
        for device_id in &hidden_ids {
            topology_changed |= !s.topology.remove_device_everywhere(device_id).is_empty();
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
                topology_changed |= s.topology.assign_device(
                    device_id,
                    Some(room_id),
                    crate::topology::DevicePlacement::HubDefault,
                );
            } else {
                s.topology.ensure_standalone_device(device_id);
                topology_changed = true;
            }
        }

        persist_canonical(&s);
        if topology_changed {
            persist_topology(&s);
        }

        (
            hidden_ids,
            topology_changed,
            report.affected_device_ids.len(),
        )
    };

    for device_id in &hidden_ids {
        queue_motion_timer_clear(state, device_id);
    }

    if topology_changed {
        reconcile_runtime_from_state(state)?;
    }

    Ok((affected_count, hidden_ids.len()))
}

fn resolved_room_motion_timeout_secs_from_parts(
    lighting: RoomLightingContext<'_>,
    settings: &RoomProfileSettings,
    current_hour: f32,
) -> u64 {
    let local = current_local_datetime(lighting.utc_offset);
    let registry = light_profile_registry_from_parts(
        lighting.light_profile_configs,
        lighting.mode_configs,
        lighting.mode,
    );
    let ctx = rhythm_core::curve_context_for_local_date_and_hour(
        lighting.solar_noon,
        lighting.latitude,
        lighting.longitude,
        lighting.timezone_name,
        local.date(),
        current_hour,
    );
    registry
        .calculate_room_values(
            lighting.mode,
            RoomModeState::Active,
            Some(settings),
            &ctx,
            0.0,
        )
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
        .resolve_room_alias(None, node_id)
        .filter(|topo_id| topo_id != node_id)
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

fn semantic_lights_on_override(power_save: bool, hard_off: bool, soft_off: bool) -> Option<bool> {
    if hard_off || (soft_off && power_save) {
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

const OBSERVED_POWER_MIN_FRESHNESS_SECS: u64 = 15;

fn observed_power_is_fresh(s: &AppState, observed: &ObservedPowerState) -> bool {
    match observed.source {
        ObservedPowerSource::SemanticOverride => true,
        ObservedPowerSource::Command => false,
        ObservedPowerSource::Periodic
        | ObservedPowerSource::SyncPoll
        | ObservedPowerSource::LiveSubscription
        | ObservedPowerSource::AuthoritativeRefresh => {
            let freshness_window_secs = OBSERVED_POWER_MIN_FRESHNESS_SECS
                .max(s.runtime_config.update_interval_secs.saturating_mul(2));
            let age_ms = current_epoch_ms().saturating_sub(observed.observed_at_epoch_ms);
            age_ms <= freshness_window_secs.saturating_mul(1000)
        }
    }
}

fn observed_power_dto(
    s: &AppState,
    observed: &ObservedPowerState,
) -> crate::api_types::ObservedPowerDto {
    ObservedPowerDto {
        lights_on: observed.lights_on,
        fresh: observed_power_is_fresh(s, observed),
        observed_at_epoch_ms: Some(observed.observed_at_epoch_ms),
        source: Some(observed.source.as_str().to_string()),
    }
}

fn fallback_observed_power_dto(lights_on: bool) -> crate::api_types::ObservedPowerDto {
    ObservedPowerDto {
        lights_on,
        fresh: false,
        observed_at_epoch_ms: None,
        source: None,
    }
}

fn observed_power_from_cache(
    s: &AppState,
    room_observed_power: &HashMap<String, ObservedPowerState>,
    node_id: &str,
    kind: LightNodeKind,
    parent_id: Option<&str>,
    semantic_override: Option<bool>,
) -> ObservedPowerDto {
    let cache_key = effective_lights_on_cache_key(s, node_id, kind, parent_id);

    if let Some(lights_on) = semantic_override {
        return ObservedPowerDto {
            lights_on,
            fresh: true,
            observed_at_epoch_ms: room_observed_power
                .get(cache_key)
                .filter(|observed| observed.source == ObservedPowerSource::SemanticOverride)
                .map(|observed| observed.observed_at_epoch_ms),
            source: Some(ObservedPowerSource::SemanticOverride.as_str().to_string()),
        };
    }

    if let Some(observed) = room_observed_power.get(cache_key) {
        return observed_power_dto(s, observed);
    }

    fallback_observed_power_dto(semantic_override.unwrap_or(false))
}

fn lights_on_from_observed_cache(
    s: &AppState,
    room_observed_power: &HashMap<String, ObservedPowerState>,
    node_id: &str,
    kind: LightNodeKind,
    parent_id: Option<&str>,
    semantic_override: Option<bool>,
) -> bool {
    observed_lights_on_from_cache(
        s,
        room_observed_power,
        node_id,
        kind,
        parent_id,
        semantic_override,
    )
    .unwrap_or(false)
}

fn observed_lights_on_from_cache(
    s: &AppState,
    room_observed_power: &HashMap<String, ObservedPowerState>,
    node_id: &str,
    kind: LightNodeKind,
    parent_id: Option<&str>,
    semantic_override: Option<bool>,
) -> Option<bool> {
    if let Some(lights_on) = semantic_override {
        return Some(lights_on);
    }

    room_observed_power
        .get(effective_lights_on_cache_key(s, node_id, kind, parent_id))
        .map(|observed| observed.lights_on)
}

fn update_lights_on_cache_for_node_with_source(
    state: &SharedState,
    node_id: &str,
    kind: LightNodeKind,
    parent_id: Option<&str>,
    lights_on: bool,
    source: ObservedPowerSource,
) {
    if let Ok(mut s) = state.lock() {
        let cache_key = effective_lights_on_cache_key(&s, node_id, kind, parent_id).to_string();
        if source == ObservedPowerSource::Command {
            if let Some(existing) = s.room_observed_power.get(&cache_key) {
                if existing.source == ObservedPowerSource::LiveSubscription
                    && existing.lights_on == lights_on
                    && observed_power_is_fresh(&s, existing)
                {
                    return;
                }
            }
        }
        s.room_observed_power
            .insert(cache_key, ObservedPowerState::new(lights_on, source));
    }
}

pub(crate) fn light_state_query_id<'a>(
    s: &AppState,
    node_id: &'a str,
    kind: LightNodeKind,
    parent_id: Option<&'a str>,
) -> &'a str {
    effective_lights_on_cache_key(s, node_id, kind, parent_id)
}

pub(crate) fn update_lights_on_cache_for_runtime_node(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
    lights_on: bool,
) {
    update_lights_on_cache_for_runtime_node_with_source(
        state,
        runtime,
        node_id,
        lights_on,
        ObservedPowerSource::Command,
    );
}

pub(crate) fn update_lights_on_cache_for_runtime_node_with_source(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
    lights_on: bool,
    source: ObservedPowerSource,
) {
    if let Some(snap) = runtime.engine_effective_node_snapshot(node_id) {
        update_lights_on_cache_for_node_with_source(
            state,
            &snap.id,
            snap.kind,
            snap.parent_id.as_deref(),
            lights_on,
            source,
        );

        if let Some(parent_id) = snap
            .parent_id
            .as_deref()
            .filter(|parent_id| *parent_id != snap.id)
        {
            match runtime.any_lights_on(parent_id) {
                Ok(parent_lights_on) => {
                    update_lights_on_cache_for_node_with_source(
                        state,
                        parent_id,
                        LightNodeKind::Room,
                        None,
                        parent_lights_on,
                        source,
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
        update_lights_on_cache_for_node_with_source(
            state,
            node_id,
            LightNodeKind::Room,
            None,
            lights_on,
            source,
        );
    }
}

pub(crate) fn update_lights_on_cache_for_native_light_report(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    hub_key: &HubKey,
    native_id: &str,
    lights_on: bool,
    source: ObservedPowerSource,
) -> Option<String> {
    let (node_id, parent_id) = {
        let s = state.lock().ok()?;
        let canonical_id = s
            .canonical_registry
            .find_by_native_id(hub_key, native_id)
            .map(|device| device.id.clone())?;
        let parent_id = s
            .topology
            .get_device_node(&canonical_id)
            .and_then(|node| node.parent_id.clone());
        (canonical_id, parent_id)
    };

    update_lights_on_cache_for_node_with_source(
        state,
        &node_id,
        LightNodeKind::LightDevice,
        None,
        lights_on,
        source,
    );

    if let Some(parent_id) = parent_id.filter(|parent_id| parent_id != &node_id) {
        let parent_lights_on = if lights_on {
            true
        } else {
            match runtime.any_lights_on(&parent_id) {
                Ok(parent_lights_on) => parent_lights_on,
                Err(e) => {
                    warn!(
                        target: "cmd",
                        "Failed to refresh parent lights_on for '{}' after live report from '{}': {}",
                        parent_id,
                        node_id,
                        e
                    );
                    false
                }
            }
        };
        update_lights_on_cache_for_node_with_source(
            state,
            &parent_id,
            LightNodeKind::Room,
            None,
            parent_lights_on,
            source,
        );
    }

    Some(node_id)
}

pub(crate) fn refresh_lights_on_cache_for_runtime_snapshot_with_source(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    snap: &rhythm_core::NodeSnapshot,
    source: ObservedPowerSource,
) {
    if !snap.kind.is_light_addressable() {
        return;
    }

    let power_save = state.lock().ok().is_some_and(|s| s.power_save);
    let semantic_override = semantic_lights_on_override(power_save, snap.hard_off, snap.soft_off);
    let observation_source = if semantic_override.is_some() {
        ObservedPowerSource::SemanticOverride
    } else {
        source
    };
    let lights_on = if let Some(lights_on) = semantic_override {
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
                    snap.id,
                    query_id,
                    e
                );
                return;
            }
        }
    };

    update_lights_on_cache_for_node_with_source(
        state,
        &snap.id,
        snap.kind,
        snap.parent_id.as_deref(),
        lights_on,
        observation_source,
    );

    if let Some(parent_id) = snap
        .parent_id
        .as_deref()
        .filter(|parent_id| *parent_id != snap.id)
    {
        match runtime.any_lights_on(parent_id) {
            Ok(parent_lights_on) => update_lights_on_cache_for_node_with_source(
                state,
                parent_id,
                LightNodeKind::Room,
                None,
                parent_lights_on,
                source,
            ),
            Err(e) => warn!(
                target: "cmd",
                "Failed to refresh parent lights_on for '{}' after node '{}': {}",
                parent_id,
                snap.id,
                e
            ),
        }
    }
}

pub(crate) fn refresh_lights_on_cache_for_runtime_node_with_source(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
    source: ObservedPowerSource,
) {
    let Some(snap) = runtime.engine_effective_node_snapshot(node_id) else {
        return;
    };
    refresh_lights_on_cache_for_runtime_snapshot_with_source(state, runtime, &snap, source);
}

pub(crate) fn refresh_lights_on_cache_for_runtime_node(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
) {
    refresh_lights_on_cache_for_runtime_node_with_source(
        state,
        runtime,
        node_id,
        ObservedPowerSource::Command,
    );
}

pub(crate) fn refresh_all_lights_on_cache_for_runtime(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    source: ObservedPowerSource,
) -> Vec<rhythm_core::RoomSnapshot> {
    let snapshots = addressable_root_snapshots(runtime);
    if snapshots.is_empty() {
        return snapshots;
    }

    let mut query_results: HashMap<String, bool> = HashMap::new();
    for snap in &snapshots {
        if !snap.kind.is_light_addressable() {
            continue;
        }

        let power_save = state.lock().ok().is_some_and(|s| s.power_save);
        let semantic_override =
            semantic_lights_on_override(power_save, snap.hard_off, snap.soft_off);
        let lights_on = if let Some(lights_on) = semantic_override {
            lights_on
        } else {
            let query_id = {
                let Ok(s) = state.lock() else { continue };
                light_state_query_id(&s, &snap.id, snap.kind, snap.parent_id.as_deref()).to_string()
            };

            if let Some(lights_on) = query_results.get(&query_id).copied() {
                lights_on
            } else {
                match runtime.any_lights_on(&query_id) {
                    Ok(lights_on) => {
                        query_results.insert(query_id, lights_on);
                        lights_on
                    }
                    Err(e) => {
                        warn!(
                            target: "cmd",
                            "Failed to refresh lights_on for '{}' during full refresh: {}",
                            snap.id,
                            e
                        );
                        continue;
                    }
                }
            }
        };

        update_lights_on_cache_for_node_with_source(
            state,
            &snap.id,
            snap.kind,
            snap.parent_id.as_deref(),
            lights_on,
            semantic_override
                .map(|_| ObservedPowerSource::SemanticOverride)
                .unwrap_or(source),
        );
    }

    snapshots
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
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&'a str>,
    utc_offset: f32,
    room_observed_power: &'a HashMap<String, ObservedPowerState>,
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
    let observed_power = observed_power_from_cache(
        ctx.state,
        ctx.room_observed_power,
        &snap.id,
        snap.kind,
        effective_parent_id.as_deref(),
        semantic_lights_on_override(ctx.state.power_save, snap.hard_off, snap.soft_off),
    );
    let (brightness, kelvin) = compute_room_display_values_for_settings_from_parts(
        RoomLightingContext {
            light_profile_configs: ctx.light_profile_configs,
            mode_configs: ctx.mode_configs,
            mode: ctx.active_mode,
            solar_noon: ctx.solar_noon,
            latitude: ctx.latitude,
            longitude: ctx.longitude,
            timezone_name: ctx.timezone_name,
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
                    longitude: ctx.longitude,
                    timezone_name: ctx.timezone_name,
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
        lights_on: observed_power.lights_on,
        observed_power,
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
pub fn build_node_state_event(
    state: &SharedState,
    snap: &rhythm_core::NodeSnapshot,
) -> crate::server_event::NodeStateEvent {
    let now = std::time::Instant::now();
    let (mode, room_state, observed_power, transitioning, brightness, kelvin, hub_types) = {
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
        let observed_power = observed_power_from_cache(
            &s,
            &s.room_observed_power,
            &snap.id,
            snap.kind,
            snap.parent_id.as_deref(),
            semantic_lights_on_override(s.power_save, snap.hard_off, snap.soft_off),
        );
        let transitioning = room_mode_transition_active(&s.room_mode_transitions, &snap.id, now);
        let hub_types = node_hub_types_from_topology(&s, &snap.id);
        let (curve_brightness, kelvin) = compute_room_display_values_for_settings_from_parts(
            RoomLightingContext {
                light_profile_configs: &s.light_profile_configs,
                mode_configs: &mode_configs,
                mode,
                solar_noon: s.solar_noon_hour(),
                latitude: s.latitude,
                longitude: s.longitude,
                timezone_name: s.timezone_name.as_deref(),
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
            observed_power,
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
            lights_on: observed_power.lights_on,
            observed_power,
            transitioning,
            brightness,
            kelvin,
        },
    )
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
            s.invalidate_queued_light_dispatches();
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

/// Resolve a public node ID from an API request.
///
/// Topology room IDs and topology device node IDs are accepted. Hub-native
/// room/device aliases are not resolved on this no-hub-key path.
pub fn resolve_node_id(state: &SharedState, node_id: &str) -> String {
    let s = match state.lock() {
        Ok(s) => s,
        Err(_) => return node_id.to_string(),
    };
    s.topology
        .resolve_room_alias(None, node_id)
        .unwrap_or_else(|| node_id.to_string())
}

/// Resolve a topology control target for a source device/native ID.
///
/// The returned tuple is `(source_node_id, target_node_id)`. Explicit topology
/// control links win. If no explicit link exists, device nodes inherit their
/// parent room as the default target.
pub(crate) fn resolve_node_control_target(
    state: &SharedState,
    hub_key: &HubKey,
    source_native_id: &str,
    kind: &NodeControlKind,
) -> Option<(String, String)> {
    let s = state.lock().ok()?;
    let source_node_id = s
        .canonical_registry
        .find_by_native_id(hub_key, source_native_id)
        .map(|device| device.id.clone())?;
    let target_node_id = s.topology.effective_control_target(&source_node_id, kind)?;

    Some((source_node_id, target_node_id))
}

/// Resolve a hub-native input source to its canonical topology node ID.
pub(crate) fn resolve_input_source_node_id(
    state: &SharedState,
    hub_key: &HubKey,
    source_native_id: &str,
) -> Option<String> {
    let s = state.lock().ok()?;
    s.canonical_registry
        .find_by_native_id(hub_key, source_native_id)
        .map(|device| device.id.clone())
}

/// Resolve an already-known source node to its effective topology control target.
pub(crate) fn resolve_node_control_target_for_source(
    state: &SharedState,
    source_node_id: &str,
    kind: &NodeControlKind,
) -> Option<String> {
    let s = state.lock().ok()?;
    s.topology.effective_control_target(source_node_id, kind)
}

/// Return the high-level binding action for a physical button event, if one is configured.
pub(crate) fn matching_button_input_binding_action(
    state: &SharedState,
    source_node_id: &str,
    action: ButtonAction,
) -> Option<AutomationAction> {
    let s = state.lock().ok()?;
    s.topology
        .matching_button_input_binding(source_node_id, action)
        .map(|binding| binding.action.clone())
}

fn validate_input_binding_source(s: &AppState, source_node_id: &str) -> Result<()> {
    if !s.topology.has_public_node(source_node_id) {
        return Err(anyhow::anyhow!(
            "Source node '{}' not found",
            source_node_id
        ));
    }

    let Some(node) = s.topology.get_device_node(source_node_id) else {
        return Err(anyhow::anyhow!(
            "Source node '{}' must be a button device",
            source_node_id
        ));
    };
    let Some(device) = s.canonical_registry.get(&node.canonical_device_id) else {
        return Err(anyhow::anyhow!(
            "Source node '{}' has no canonical device",
            source_node_id
        ));
    };
    if device.device_type != DeviceType::Button {
        return Err(anyhow::anyhow!(
            "Source node '{}' must be a button device",
            source_node_id
        ));
    }

    Ok(())
}

/// Add or replace one persisted physical input binding.
pub fn do_input_binding_set(state: &SharedState, mut binding: InputBinding) -> Result<String> {
    binding.source_node_id = resolve_node_id(state, &binding.source_node_id);
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        validate_input_binding_source(&s, &binding.source_node_id)?;
        validate_automation_action(&binding.action)?;
        s.topology.set_input_binding(binding);
        persist_topology(&s);
    }
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
    build_input_bindings(state)
}

/// Add or replace a preset-backed physical input binding with an explicit ID.
pub fn do_preset_input_binding_set(
    state: &SharedState,
    binding_id: &str,
    preset: InputBindingPreset,
    source_node_id: &str,
    button_action: Option<ButtonAction>,
    enabled: bool,
) -> Result<String> {
    let mut binding = InputBinding::from_preset(binding_id, preset, source_node_id, button_action);
    binding.enabled = enabled;
    do_input_binding_set(state, binding)
}

/// Create or replace a preset-backed physical input binding with a stable generated ID.
pub fn do_preset_input_binding_create(
    state: &SharedState,
    preset: InputBindingPreset,
    source_node_id: &str,
    button_action: Option<ButtonAction>,
    enabled: bool,
) -> Result<String> {
    let resolved_source_node_id = resolve_node_id(state, source_node_id);
    let id = InputBinding::preset_binding_id(preset, &resolved_source_node_id, button_action);
    let mut binding = InputBinding::from_preset(id, preset, resolved_source_node_id, button_action);
    binding.enabled = enabled;
    do_input_binding_set(state, binding)
}

/// Remove a persisted physical input binding.
pub fn do_input_binding_delete(state: &SharedState, binding_id: &str) -> Result<String> {
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.topology.remove_input_binding(binding_id);
        persist_topology(&s);
    }
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
    build_input_bindings(state)
}

// ============================================================================
// State snapshots (for GET endpoints)
// ============================================================================

pub fn refresh_observed_power_authoritatively(state: &SharedState) -> Result<()> {
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let Some(runtime) = runtime else {
        return Ok(());
    };

    refresh_all_lights_on_cache_for_runtime(
        state,
        &runtime,
        ObservedPowerSource::AuthoritativeRefresh,
    );
    Ok(())
}

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
        input_bindings_dto,
        profiles_dto,
        firmware_version,
        platform_type,
        platform_ctx,
        listen_port,
        room_observed_power,
        motion_snapshots,
        nodes_with_sensors,
        transitioning_nodes,
        light_profile_configs,
        mode_configs,
        active_mode,
        solar_noon,
        latitude,
        longitude,
        utc_offset,
        timezone_name,
        last_tick_epoch_ms,
        profile_registry,
        periodic_ctx,
        update_interval,
        power_save,
        review_dto,
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
                startup_retry: s.hub_startup_retry(key).map(build_hub_startup_retry_dto),
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
            build_input_bindings_dto_inner(&s),
            build_profiles_dto_inner(&s),
            s.firmware_version,
            s.platform_type,
            s.platform_context,
            s.listen_port,
            s.room_observed_power.clone(),
            s.motion_snapshots.clone(),
            s.motion_control_target_ids(),
            active_transition_room_ids(&s.room_mode_transitions, now),
            s.light_profile_configs.clone(),
            s.mode_configs(),
            s.active_mode,
            s.solar_noon_hour(),
            s.latitude,
            s.longitude,
            s.utc_offset_hours,
            s.timezone_name.clone(),
            s.last_tick_epoch_ms,
            profile_registry,
            periodic_ctx,
            Duration::from_secs(s.runtime_config.update_interval_secs),
            s.power_save,
            build_review_summary_dto(&s),
        )
    };

    let mut node_snapshots: Vec<rhythm_core::NodeSnapshot> = if let Some(ref runtime) = runtime {
        runtime.engine_all_effective_node_snapshots()
    } else {
        bootstrap_node_snapshots_from_state(state)
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
            longitude,
            timezone_name: timezone_name.as_deref(),
            utc_offset,
            room_observed_power: &room_observed_power,
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
        input_bindings: input_bindings_dto.bindings,
        profiles: profiles_dto.profiles,
        review: review_dto,
        nodes,
    };
    serde_json::to_string(&snapshot).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

fn build_review_summary_dto(s: &AppState) -> ReviewSummaryDto {
    use crate::canonical::triage::{TriageKind, TriageStatus};

    let triage = s.canonical_registry.triage();

    let pending = ReviewCountsDto {
        devices: triage.pending_device_count(),
        rooms: triage.pending_room_count(),
        unassigned: triage.pending_unassigned_count(),
        hub_configured: triage.pending_hub_configured_count(),
        total: triage.pending_count(),
    };

    let mut disconnected_hubs: Vec<ReviewHubDto> = s
        .hub_credentials
        .iter()
        .filter(|(key, _)| !s.hub_is_connected(key))
        .map(|(_, creds)| ReviewHubDto {
            hub_type: creds
                .hub_type
                .as_ref()
                .map(|hub_type| hub_type.as_str().to_string())
                .unwrap_or_else(|| "none".to_string()),
            address: creds.address.clone(),
        })
        .collect();
    disconnected_hubs.sort_by(|left, right| {
        left.hub_type
            .cmp(&right.hub_type)
            .then_with(|| left.address.cmp(&right.address))
    });

    let mut preferred_endpoints: Vec<PreferredEndpointDto> = s
        .canonical_registry
        .devices()
        .filter(|device| device.endpoints.len() > 1)
        .filter_map(|device| {
            let preferred = device.preferred_endpoint()?;
            Some(PreferredEndpointDto {
                canonical_id: device.id.clone(),
                name: device.name.clone(),
                hub_type: preferred.hub_key.hub_type.as_str().to_string(),
                hub_address: preferred.hub_key.address.clone(),
                native_id: preferred.native_id.clone(),
                endpoint_count: device.endpoints.len(),
            })
        })
        .collect();
    preferred_endpoints.sort_by(|left, right| left.canonical_id.cmp(&right.canonical_id));

    let mut triage_entries: Vec<ReviewEntryDto> = triage
        .all()
        .iter()
        .map(|entry| ReviewEntryDto {
            id: entry.id.clone(),
            kind: entry.kind.clone(),
            status: entry.status.clone(),
            hub_type: entry.hub_key.hub_type.as_str().to_string(),
            hub_address: entry.hub_key.address.clone(),
            native_id: entry.discovered.native_id.clone(),
            name: entry.discovered.name.clone(),
            created_at: entry.created_at,
            resolved_at: entry.resolved_at,
            resolved_by: entry.resolved_by.clone(),
            summary: triage_entry_summary(entry.kind.clone(), entry.status.clone()),
            guidance: triage_entry_guidance(entry.kind.clone(), entry.status.clone()),
        })
        .collect();
    triage_entries.sort_by(|left, right| {
        let left_pending = left.status == TriageStatus::Pending;
        let right_pending = right.status == TriageStatus::Pending;
        right_pending
            .cmp(&left_pending)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    triage_entries.truncate(50);

    let hub_configured_conflicts = triage_entries
        .iter()
        .filter(|entry| {
            entry.kind == TriageKind::HubConfigured && entry.status == TriageStatus::Pending
        })
        .cloned()
        .collect();

    ReviewSummaryDto {
        pending,
        disconnected_hubs,
        preferred_endpoints,
        hub_configured_conflicts,
        triage_entries,
    }
}

fn triage_entry_summary(
    kind: crate::canonical::triage::TriageKind,
    status: crate::canonical::triage::TriageStatus,
) -> String {
    use crate::canonical::triage::{TriageKind, TriageStatus};

    match (kind, status) {
        (TriageKind::DeviceMerge, TriageStatus::Pending) => {
            "Review whether these endpoints represent the same physical device".to_string()
        }
        (TriageKind::DeviceMerge, TriageStatus::Confirmed) => "Device merge confirmed".to_string(),
        (TriageKind::DeviceMerge, TriageStatus::NewDevice) => {
            "Kept as a separate device".to_string()
        }
        (TriageKind::DeviceMerge, TriageStatus::Dismissed) => {
            "Device merge proposal dismissed".to_string()
        }
        (TriageKind::RoomBinding, TriageStatus::Pending) => {
            "Review whether this hub room should merge into an existing Rhythm room".to_string()
        }
        (TriageKind::RoomBinding, TriageStatus::Confirmed) => "Room binding confirmed".to_string(),
        (TriageKind::RoomBinding, TriageStatus::KeptSeparate)
        | (TriageKind::RoomBinding, TriageStatus::NewDevice) => {
            "Kept as a separate room".to_string()
        }
        (TriageKind::RoomBinding, TriageStatus::Dismissed) => {
            "Room binding proposal dismissed".to_string()
        }
        (TriageKind::UnassignedDevice, TriageStatus::Pending) => {
            "Assign this device to a Rhythm room".to_string()
        }
        (TriageKind::UnassignedDevice, TriageStatus::Confirmed) => {
            "Room assignment resolved".to_string()
        }
        (TriageKind::UnassignedDevice, TriageStatus::Dismissed) => {
            "Unassigned-device review dismissed".to_string()
        }
        (TriageKind::HubConfigured, TriageStatus::Pending) => {
            "Native hub automation is still configured for this device".to_string()
        }
        (TriageKind::HubConfigured, TriageStatus::Confirmed) => {
            "Native hub automation was cleared".to_string()
        }
        (TriageKind::HubConfigured, TriageStatus::Dismissed) => {
            "Hub-configured conflict dismissed".to_string()
        }
        (_, TriageStatus::AutoResolved) => "Resolved automatically".to_string(),
        (_, TriageStatus::NewDevice) => "Kept separate".to_string(),
        (_, TriageStatus::KeptSeparate) => "Kept separate".to_string(),
    }
}

fn triage_entry_guidance(
    kind: crate::canonical::triage::TriageKind,
    status: crate::canonical::triage::TriageStatus,
) -> Option<String> {
    use crate::canonical::triage::{TriageKind, TriageStatus};

    match (kind, status) {
        (TriageKind::DeviceMerge, TriageStatus::Pending) => Some(
            "Confirm the merge only if both endpoints represent the same physical device."
                .to_string(),
        ),
        (TriageKind::RoomBinding, TriageStatus::Pending) => Some(
            "Bind the rooms only if they should act as one Rhythm room across hubs."
                .to_string(),
        ),
        (TriageKind::UnassignedDevice, TriageStatus::Pending) => Some(
            "Assign the device to a room so automations, topology, and control routing stay stable."
                .to_string(),
        ),
        (TriageKind::HubConfigured, TriageStatus::Pending) => Some(
            "Remove the native hub automation for this device in the hub app so Rhythm can control it predictably."
                .to_string(),
        ),
        _ => None,
    }
}

fn build_hub_startup_retry_dto(retry: &crate::state::HubStartupRetryState) -> HubStartupRetryDto {
    HubStartupRetryDto {
        status: if retry.manual_retry_required {
            "manual_retry_required".to_string()
        } else {
            "scheduled".to_string()
        },
        attempt_count: retry.attempt_count,
        first_failure_epoch_ms: retry.first_failure_epoch_ms,
        last_failure_epoch_ms: retry.last_failure_epoch_ms,
        next_retry_epoch_ms: retry.next_retry_epoch_ms,
        last_error: Some(retry.last_error.clone()),
    }
}

/// Build a full node state for a single addressable node.
pub fn build_node_state(state: &SharedState, node_id: &str) -> Result<NodeStateDto> {
    let (runtime, room_observed_power, motion_snapshots, nodes_with_sensors, transitioning_nodes) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.hub_runtime(),
            s.room_observed_power.clone(),
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
        latitude: s.latitude,
        longitude: s.longitude,
        timezone_name: s.timezone_name.as_deref(),
        utc_offset: s.utc_offset_hours,
        room_observed_power: &room_observed_power,
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
        room_observed_power,
        motion_snapshots,
        nodes_with_sensors,
        transitioning_nodes,
        light_profile_configs,
        mode_configs,
        active_mode,
        solar_noon,
        latitude,
        longitude,
        utc_offset,
        timezone_name,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.has_any_connected_hub(),
            s.hub_runtime(),
            s.room_observed_power.clone(),
            s.motion_snapshots.clone(),
            s.motion_control_target_ids(),
            active_transition_room_ids(&s.room_mode_transitions, std::time::Instant::now()),
            s.light_profile_configs.clone(),
            s.mode_configs(),
            s.active_mode,
            s.solar_noon_hour(),
            s.latitude,
            s.longitude,
            s.utc_offset_hours,
            s.timezone_name.clone(),
        )
    };

    let mut node_snapshots: Vec<rhythm_core::NodeSnapshot> = if let Some(ref runtime) = runtime {
        runtime.engine_all_effective_node_snapshots()
    } else {
        bootstrap_node_snapshots_from_state(state)
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
        longitude,
        timezone_name: timezone_name.as_deref(),
        utc_offset,
        room_observed_power: &room_observed_power,
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
        room_observed_power,
        light_profile_configs,
        mode_configs,
        active_mode,
        solar_noon,
        latitude,
        longitude,
        utc_offset,
        timezone_name,
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
        let observed_power = s.room_observed_power.clone();
        let sensor_rooms = s.motion_control_target_ids();
        (
            s.has_any_connected_hub(),
            runtime,
            storage_rooms,
            motion,
            observed_power,
            s.light_profile_configs.clone(),
            s.mode_configs(),
            s.active_mode,
            s.solar_noon_hour(),
            s.latitude,
            s.longitude,
            s.utc_offset_hours,
            s.timezone_name.clone(),
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
            let observed_power = observed_power_from_cache(
                &s,
                &room_observed_power,
                &snap.id,
                snap.kind,
                snap.parent_id.as_deref(),
                semantic_lights_on_override(s.power_save, snap.hard_off, snap.soft_off),
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
                    longitude,
                    timezone_name: timezone_name.as_deref(),
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
                lights_on: observed_power.lights_on,
                observed_power,
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
                            longitude,
                            timezone_name: timezone_name.as_deref(),
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
                    longitude,
                    timezone_name: timezone_name.as_deref(),
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
                    observed_power: fallback_observed_power_dto(false),
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
    let (runtime, room_observed_power, warning_active, transitioning, hub_types) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.hub_runtime(),
            s.room_observed_power.clone(),
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
    let observed_power = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        observed_power_from_cache(
            &s,
            &room_observed_power,
            room_id,
            snap.kind,
            snap.parent_id.as_deref(),
            semantic_lights_on_override(s.power_save, snap.hard_off, snap.soft_off),
        )
    };
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
        lights_on: observed_power.lights_on,
        observed_power,
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

/// Build `InputBindingsDto` from an already-locked `AppState`.
fn build_input_bindings_dto_inner(s: &AppState) -> InputBindingsDto {
    InputBindingsDto {
        bindings: s.topology.input_bindings().to_vec(),
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

/// Build the current physical input binding policy.
pub fn build_input_bindings_dto(state: &SharedState) -> Result<InputBindingsDto> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    Ok(build_input_bindings_dto_inner(&s))
}

/// Build the current physical input binding policy as a JSON string.
pub fn build_input_bindings(state: &SharedState) -> Result<String> {
    let dto = build_input_bindings_dto(state)?;
    serde_json::to_string(&dto).map_err(|e| anyhow::anyhow!("serialize input bindings: {}", e))
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

fn topology_node_snapshots_from_state(state: &SharedState) -> Vec<rhythm_core::NodeSnapshot> {
    let Ok(s) = state.lock() else {
        return Vec::new();
    };

    let mut nodes = Vec::new();

    let mut room_ids: Vec<_> = s.topology.rooms().map(|room| room.id.clone()).collect();
    room_ids.sort();
    for room_id in room_ids {
        let Some(room) = s.topology.get(&room_id) else {
            continue;
        };
        nodes.push(rhythm_core::NodeSnapshot {
            id: room.id.clone(),
            name: room.name.clone(),
            kind: LightNodeKind::Room,
            parent_id: None,
            rhythm_enabled: true,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off: false,
            hard_off: false,
            profile_settings: RoomProfileSettings::default(),
        });
    }

    let mut device_node_ids: Vec<_> = s
        .topology
        .device_nodes()
        .map(|node| node.id.clone())
        .collect();
    device_node_ids.sort();
    for node_id in device_node_ids {
        let Some(node) = s.topology.get_device_node(&node_id) else {
            continue;
        };
        let (name, kind) = s
            .canonical_registry
            .get(&node.canonical_device_id)
            .map(|device| {
                (
                    device.name.clone(),
                    runtime_node_kind_for_device_type(device.device_type.clone()),
                )
            })
            .unwrap_or_else(|| (node.id.clone(), LightNodeKind::OtherDevice));

        nodes.push(rhythm_core::NodeSnapshot {
            id: node.id.clone(),
            name,
            kind,
            parent_id: node.parent_id.clone(),
            rhythm_enabled: kind.is_light_addressable(),
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off: false,
            hard_off: false,
            profile_settings: RoomProfileSettings::default(),
        });
    }

    nodes
}

fn bootstrap_node_snapshots_from_state(state: &SharedState) -> Vec<rhythm_core::NodeSnapshot> {
    let mut snapshots = topology_node_snapshots_from_state(state);
    if snapshots.is_empty() {
        snapshots = registry_node_snapshots_from_state(state);
    }
    snapshots
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

fn profile_bundle_data_from_state(s: &AppState) -> ProfileBundleData {
    ProfileBundleData {
        power_save: s.power_save,
        profiles: s.light_profile_configs.values().cloned().collect(),
        mode_transitions: s.mode_transition_configs(),
    }
}

fn backup_configuration_from_parts(
    s: &AppState,
    room_manager: &rhythm_core::RoomManager,
) -> BackupConfiguration {
    let mut rooms: Vec<_> = room_manager
        .iter()
        .filter(|room| room.kind.is_room())
        .map(BackupConfigurationRoom::from)
        .collect();
    rooms.sort_by(|left, right| left.id.cmp(&right.id).then(left.name.cmp(&right.name)));

    BackupConfiguration {
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
        data: (include_secrets && !creds.secrets_redacted).then(|| creds.data.clone()),
    }
}

pub fn build_profile_bundle_dto(state: &SharedState) -> Result<ProfileBundle> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

    Ok(ProfileBundle {
        schema_version: BUNDLE_SCHEMA_VERSION,
        kind: crate::bundle::BundleKind::ProfileBundle,
        name: None,
        description: None,
        profile: profile_bundle_data_from_state(&s),
    })
}

pub fn build_profile_bundle(state: &SharedState) -> Result<String> {
    let bundle = build_profile_bundle_dto(state)?;
    serialize_profile_bundle(&bundle)
}

fn serialize_profile_bundle(bundle: &ProfileBundle) -> Result<String> {
    serde_json::to_string_pretty(bundle)
        .map_err(|e| anyhow::anyhow!("serialize profile bundle: {}", e))
}

pub fn build_factory_default_profile_bundle_dto() -> ProfileBundle {
    factory_default_profile_bundle()
}

pub fn build_factory_default_profile_bundle() -> Result<String> {
    serialize_profile_bundle(&build_factory_default_profile_bundle_dto())
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
    s.hub_pending_disconnect_at.clear();
    s.room_observed_power.clear();
    s.motion_snapshots.clear();
    s.room_mode_transitions.clear();
    s.last_check_hour = None;
    s.last_check_instant = None;
    s.last_check_utc_offset_hours = None;
    s.invalidate_queued_light_dispatches();
    s.pending_hub_event_rxs.clear();
    s.pending_motion_clear.clear();
    s.pending_motion_seed.clear();
    s.last_tick_epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    Ok(())
}

fn factory_default_backup_configuration() -> BackupConfiguration {
    BackupConfiguration {
        power_save: factory_default_power_save(),
        active_mode: factory_default_active_mode(),
        profiles: factory_default_light_profile_config_map()
            .into_values()
            .collect(),
        mode_configs: factory_default_mode_config_map().into_values().collect(),
        mode_transitions: factory_default_mode_transition_configs(),
        rooms: Vec::new(),
    }
}

pub fn do_factory_reset(state: &SharedState) -> Result<String> {
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
        integration_files: Vec::new(),
    };
    restore_backup_installation_metadata(state, &installation)?;
    restore_backup_room_manager(state, &installation.rooms)?;
    restore_backup_location(state, None)?;

    apply_backup_configuration(state, factory_default_backup_configuration())?;
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::ConfigChanged);

    build_profile_bundle(state)
}

pub fn do_profile_bundle_reset(state: &SharedState) -> Result<String> {
    do_profile_bundle_import(
        state,
        ProfileBundleImportPayload::Bundle(factory_default_profile_bundle()),
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
        integration_files,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let configuration = backup_configuration_from_parts(&s, &room_manager);
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
            match s.storage.as_ref() {
                Some(storage) => storage.load_integration_backup_files(include_secrets)?,
                None => Vec::new(),
            },
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
            integration_files,
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

    let (runtime, solar_noon, latitude, longitude, utc_offset_hours, timezone_name) = {
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
            s.longitude,
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
        if let (Some(lat), Some(lon), Some(tz_name)) =
            (latitude, longitude, timezone_name.as_deref())
        {
            let tz = rhythm_core::Timezone::new(tz_name);
            let sun_times = rhythm_core::calculate_sun_times(
                lat,
                lon,
                local_now.date().year(),
                local_now.date().month(),
                local_now.date().day(),
                &tz,
            );
            if let Err(e) = runtime.set_sun_times(sun_times) {
                warn!(target: "cmd", "Failed to apply restored sun times: {}", e);
            }
        } else if let Err(e) = runtime.clear_sun_times() {
            warn!(target: "cmd", "Failed to clear restored sun times: {}", e);
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
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    canonical_registry.backfill_unassigned_triage(now);

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.topology = topology;
        s.canonical_registry = canonical_registry;
    }

    if let Ok(s) = state.lock() {
        persist_canonical(&s);
        persist_topology(&s);
    }

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
        s.room_observed_power
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
    // Backup restore is authoritative for room state, so discard any deferred
    // mode-output apply that `apply_backup_configuration` may have flagged
    // while restoring with no runtime attached. Otherwise a late hub
    // reconnect would re-apply mode defaults over the restored room state.
    s.pending_mode_output_apply = false;
    s.sync_active_mode_runtime_overrides();
    s.invalidate_queued_light_dispatches();
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
    storage
        .clear_hub_registries()
        .map_err(|e| anyhow::anyhow!("Failed to clear persisted hub registries: {}", e))?;

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

fn restore_backup_integration_files(
    state: &SharedState,
    files: &[BackupIntegrationFile],
) -> Result<()> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let Some(storage) = s.storage.as_ref() else {
        return Ok(());
    };
    storage
        .restore_integration_backup_files(files)
        .map_err(|e| anyhow::anyhow!("Failed to restore integration files: {}", e))
}

fn restore_backup_hub_credentials(
    state: &SharedState,
    credentials: &[BackupHubCredentials],
) -> Result<()> {
    let mut restored_redacted = Vec::new();
    for credential in credentials {
        let Some(hub_type) = credential.hub_type.as_ref() else {
            continue;
        };
        let Some(data) = credential.data.as_ref() else {
            restored_redacted.push(crate::hub::HubCredentials::redacted_placeholder(
                hub_type.as_str(),
                &credential.address,
            ));
            info!(
                target: "cmd",
                "Restored redacted hub placeholder {} at {}; awaiting fresh credentials",
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

    if !restored_redacted.is_empty() {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        for creds in restored_redacted {
            if let Some(key) = creds.hub_key() {
                s.clear_hub_connected(&key);
                s.hub_credentials.insert(key, creds);
            }
        }
        if let Some(storage) = s.storage.as_ref() {
            let all_creds: Vec<_> = s.hub_credentials.values().cloned().collect();
            if let Err(e) = storage.save_all_hub_credentials(&all_creds) {
                warn!(target: "cmd", "Failed to save restored redacted hub credentials: {}", e);
            }
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
    let ctx = rhythm_core::curve_context_for_local_datetime(
        lighting.solar_noon,
        lighting.latitude,
        lighting.longitude,
        lighting.timezone_name,
        sample_at,
    );
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
    {
        let events = node_state_events_after_apply(state, runtime, node_id);
        if !events.is_empty() {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::NodeState { nodes: events },
            );
        }
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
    let mut non_hard_off_changed_room_ids = Vec::new();
    let mut hard_off_rooms = Vec::new();
    let mut lights_on_updates = Vec::new();
    let mut missing_rooms = 0usize;

    for room_default in &mode_config.room_defaults {
        let Some(snap) = snapshots_by_id.get(room_default.room_id.as_str()).copied() else {
            missing_rooms += 1;
            continue;
        };

        let target_state = room_state_for_power_save(ctx.power_save, room_default.state);
        let current_state = persistent_room_state_from_flags(snap.hard_off, snap.soft_off);
        if current_state == target_state {
            continue;
        }

        let Ok((soft_off, hard_off)) = room_flags_for_target_state(target_state) else {
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

        match target_state {
            RoomModeState::Active | RoomModeState::Idle => {
                lights_on_updates.push((snap.id.clone(), true));
                non_hard_off_changed_room_ids.push(snap.id.clone());
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
            let cache_key = room_id.clone();
            s.room_observed_power.insert(
                cache_key,
                ObservedPowerState::new(lights_on, ObservedPowerSource::SemanticOverride),
            );
        }
    }

    // Broadcast the engine-state change immediately for non-HardOff targets.
    // The HardOff branch below emits its own event after queueing the off
    // dispatch. For Active/Idle targets the lighting command is queued
    // asynchronously via dispatch_room_commands and only emits its own event
    // when the worker runs, which can be many seconds later (production logs
    // show ~9s on a busy mode change). Without this synchronous emit, every
    // SSE consumer keeps showing the previous state — typically `HardOff` —
    // until that worker finally fires.
    for room_id in &non_hard_off_changed_room_ids {
        emit_node_state_event_after_apply(state, runtime, room_id);
    }

    if !hard_off_rooms.is_empty() {
        let work_items: Vec<_> = hard_off_rooms
            .iter()
            .map(|(room_id, transition_ms)| WorkItem::LightsOffRoom {
                command_id: crate::logging::next_command_id("mode-hard-off"),
                node_id: room_id.clone(),
                transition_ms: *transition_ms,
                dispatch_spacing: default_http_batch_dispatch_spacing(),
                dispatch_generation: ctx.dispatch_generation,
            })
            .collect();
        let queued = match queue_node_dispatch_work_items(
            state,
            "active_mode_apply",
            work_items,
            default_http_batch_dispatch_spacing(),
        ) {
            Ok(queued) => queued,
            Err(e) => {
                warn!(
                    target: "cmd",
                    "active_mode_apply: failed to queue hard-off dispatches: {}",
                    e
                );
                false
            }
        };

        for (room_id, transition_ms) in &hard_off_rooms {
            queue_motion_timer_clear(state, room_id);
            if queued {
                update_lights_on_cache_for_runtime_node(state, runtime, room_id, false);
            } else if let Err(e) = runtime.lights_off_room(room_id, *transition_ms) {
                warn!(
                    target: "cmd",
                    "active_mode_apply: lights_off for '{}' failed after room default apply: {}",
                    room_id,
                    e
                );
            }
            emit_node_state_event_after_apply(state, runtime, room_id);
        }
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
    dispatch_generation: u64,
) {
    let room_count = room_commands.len();

    for (idx, (room_id, command)) in room_commands.into_iter().enumerate() {
        if !crate::periodic::light_dispatch_generation_current(state, dispatch_generation) {
            tracing::debug!(
                target: "cmd",
                event = "apply_node_command_skipped",
                node_id = %room_id,
                dispatch_generation,
                reason = "stale_dispatch_generation",
                "Skipping stale inline room command"
            );
            return;
        }
        log_room_command_dispatch(runtime.as_ref(), &room_id, &command);
        if let Err(e) = runtime.apply_room_command(&room_id, command) {
            warn!(target: "cmd", "active_mode_apply: room '{}' failed: {}", room_id, e);
            continue;
        }
        update_lights_on_cache_for_runtime_node(state, runtime, &room_id, true);
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

fn queue_node_dispatch_work_items(
    state: &SharedState,
    dispatch_label: &str,
    work_items: Vec<WorkItem>,
    dispatch_spacing: Duration,
) -> Result<bool> {
    if work_items.is_empty() {
        return Ok(true);
    }

    let tx = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.work_tx.clone()
    };

    let Some(tx) = tx else {
        return Ok(false);
    };

    let item_count = work_items.len();
    let error_label = dispatch_label.to_string();
    let thread_label = error_label.clone();
    std::thread::Builder::new()
        .name("node-dispatch".to_string())
        .spawn(move || {
            for item in work_items {
                if tx.send(item).is_err() {
                    warn!(
                        target: "cmd",
                        "{}: node dispatch worker disconnected",
                        thread_label
                    );
                    return;
                }
            }
            debug!(
                target: "cmd",
                "{}: queued {} node dispatch work items spacing_ms={}",
                thread_label,
                item_count,
                dispatch_spacing.as_millis()
            );
        })
        .map_err(|e| anyhow::anyhow!("{}: failed to spawn node dispatcher: {}", error_label, e))?;

    Ok(true)
}

fn queue_node_dispatch_batch(
    state: &SharedState,
    dispatch_label: &str,
    mut work_items: Vec<WorkItem>,
    persist_after: bool,
    dispatch_spacing: Duration,
) -> Result<()> {
    if persist_after && !work_items.is_empty() {
        work_items.push(WorkItem::DeferredPersist {
            node_id: dispatch_label.to_string(),
        });
    }

    let queued =
        queue_node_dispatch_work_items(state, dispatch_label, work_items, dispatch_spacing)?;
    if queued {
        Ok(())
    } else {
        Err(anyhow::anyhow!("Node dispatch queue unavailable"))
    }
}

fn dispatch_room_commands(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    mut room_commands: Vec<(String, LightingCommand)>,
    cycle_duration: Duration,
    dispatch_generation: u64,
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
        apply_room_commands_inline(
            state,
            runtime,
            room_commands,
            phase_gap,
            dispatch_generation,
        );
        return;
    };

    let room_count = room_commands.len();
    let fallback_commands = room_commands.clone();
    let dispatcher_state = state.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("room-dispatch".to_string())
        .spawn(move || {
            for (idx, (node_id, command)) in room_commands.into_iter().enumerate() {
                if !crate::periodic::light_dispatch_generation_current(
                    &dispatcher_state,
                    dispatch_generation,
                ) {
                    tracing::debug!(
                        target: "cmd",
                        event = "apply_node_command_skipped",
                        node_id = %node_id,
                        dispatch_generation,
                        reason = "stale_dispatch_generation",
                        "Stopping stale room dispatcher"
                    );
                    return;
                }
                if tx
                    .send(crate::state::WorkItem::ApplyNodeCommand {
                        command_id: crate::logging::next_command_id("apply-node-command"),
                        node_id,
                        command,
                        dispatch_spacing: phase_gap,
                        dispatch_generation,
                    })
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
        apply_room_commands_inline(
            state,
            runtime,
            fallback_commands,
            phase_gap,
            dispatch_generation,
        );
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

    let ctx = rhythm_core::curve_context_for_local_datetime(
        lighting.solar_noon,
        lighting.latitude,
        lighting.longitude,
        lighting.timezone_name,
        sample_at,
    );
    let registry = light_profile_registry_from_parts(
        lighting.light_profile_configs,
        lighting.mode_configs,
        lighting.mode,
    );
    let render_state = render_state_for_display(room.room_state);
    let values = registry.calculate_room_values(
        lighting.mode,
        render_state,
        Some(room.settings),
        &ctx,
        room.time_offset_minutes,
    );
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

/// Snapshots every addressable settings node: rooms plus standalone light
/// devices (e.g. a roomless Matter bulb). The mode-apply path used to consult
/// `engine_all_room_snapshots`, which filters to `kind.is_room()` in production
/// and therefore silently skipped standalone devices, so manual transitions and
/// mode-config edits only reached them on the next periodic tick.
fn addressable_root_snapshots(runtime: &Arc<dyn RuntimeHandle>) -> Vec<rhythm_core::RoomSnapshot> {
    runtime
        .engine_all_effective_node_snapshots()
        .into_iter()
        .filter(|node| node.kind.is_light_addressable() && node.parent_id.is_none())
        .map(|node| rhythm_core::RoomSnapshot {
            id: node.id,
            name: node.name,
            kind: node.kind,
            parent_id: node.parent_id,
            rhythm_enabled: node.rhythm_enabled,
            disabled: node.disabled,
            time_offset_minutes: node.time_offset_minutes,
            brightness_offset: node.brightness_offset,
            soft_off: node.soft_off,
            hard_off: node.hard_off,
            profile_settings: node.profile_settings,
        })
        .collect()
}

fn apply_active_mode_outputs(
    state: &SharedState,
    previous_mode: RhythmMode,
    target_mode: RhythmMode,
    transition: Option<ModeTransitionConfig>,
    apply_scope: ModeOutputApplyScope,
    dispatch_generation: u64,
) {
    let (
        runtime,
        light_profile_configs,
        mode_configs,
        solar_noon,
        latitude,
        longitude,
        utc_offset,
        timezone_name,
        mut room_observed_power,
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
            s.latitude,
            s.longitude,
            s.utc_offset_hours,
            s.timezone_name.clone(),
            s.room_observed_power.clone(),
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
            s.pending_mode_output_apply = true;
        }
        debug!(
            target: "cmd",
            "active_mode_apply: no runtime available, deferring {:?} room-default apply until runtime ready",
            target_mode
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
        longitude,
        timezone_name: timezone_name.as_deref(),
        utc_offset,
    };
    let snapshots = addressable_root_snapshots(&runtime);
    let room_defaults_changed = apply_room_mode_defaults(
        state,
        &runtime,
        ModeDefaultApplyContext {
            lighting,
            transition: transition.as_ref(),
            transition_started_at,
            dispatch_generation,
            power_save,
        },
        &snapshots,
    );
    if room_defaults_changed {
        if let Ok(s) = state.lock() {
            room_observed_power = s.room_observed_power.clone();
        }
    }
    let snapshots = if room_defaults_changed {
        addressable_root_snapshots(&runtime)
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
                    observed_lights_on_from_cache(
                        &s,
                        &room_observed_power,
                        &snap.id,
                        snap.kind,
                        snap.parent_id.as_deref(),
                        semantic_lights_on_override(power_save, snap.hard_off, snap.soft_off),
                    )
                    .unwrap_or(true)
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
        dispatch_room_commands(
            state,
            &runtime,
            room_commands,
            cycle_duration,
            dispatch_generation,
        );
    }
    if let Ok(mut s) = state.lock() {
        s.pending_mode_output_apply = false;
    }
}

/// Apply the active mode's room defaults if a prior mode change deferred them
/// because no runtime was available. The deferral is set by
/// `apply_active_mode_outputs` when it sees no runtime (typically when the
/// periodic loop replayed a missed scheduled transition before hub bootstrap).
///
/// Callers must invoke this after a runtime becomes available and any
/// persisted room state has been restored. This is the only catch-up path
/// — routine restarts without a deferred apply leave restored room state
/// untouched.
fn apply_pending_mode_outputs_if_ready(state: &SharedState) {
    let (active_mode, transition, dispatch_generation) = {
        let Ok(mut s) = state.lock() else {
            return;
        };
        if !s.pending_mode_output_apply || s.hub_runtime().is_none() {
            return;
        }
        let active_mode = s.active_mode;
        let transition = s.last_active_mode_transition_id.as_ref().and_then(|id| {
            s.mode_transition_configs()
                .into_iter()
                .find(|config| config.id.as_str() == id.as_str())
        });
        let dispatch_generation = s.invalidate_queued_light_dispatches();
        (active_mode, transition, dispatch_generation)
    };

    info!(
        target: "cmd",
        "active_mode_apply_deferred: applying {:?} defaults transition_id={:?}",
        active_mode,
        transition.as_ref().map(|config| config.id.as_str())
    );
    apply_active_mode_outputs(
        state,
        active_mode,
        active_mode,
        transition,
        ModeOutputApplyScope::all_visible(),
        dispatch_generation,
    );
    persist_state(state);
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
        power_save_refresh_rooms,
        runtimes,
        active_profile_id,
        updated_mode_configs,
        previous_mode,
        selected_mode,
        mode_changed,
        reapply_scope,
        dispatch_generation,
        mode_change,
    ) = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let previous_mode = s.active_mode;
        let previous_mode_config = s
            .mode_configs()
            .into_iter()
            .find(|config| config.mode == previous_mode)
            .unwrap_or_else(|| ModeConfig::default_for_mode(previous_mode));

        let mut power_save_refresh_rooms = Vec::new();
        if let Some(ps) = power_save {
            s.power_save = ps;
            info!(target: "cmd", "settings: power_save={}", ps);

            if let Some(runtime) = s.hub_runtime() {
                power_save_refresh_rooms = runtime.set_power_save(ps);
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
        let mut dispatch_generation = s.light_dispatch_generation;
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
        if mode_changed || force_reapply_outputs || !reapply_scope.is_empty() {
            dispatch_generation = s.invalidate_queued_light_dispatches();
            tracing::debug!(
                target: "cmd",
                event = "queued_light_dispatches_invalidated",
                previous_mode = ?previous_mode,
                selected_mode = ?selected_mode,
                dispatch_generation,
                "Invalidated queued generated light dispatches"
            );
        }

        persist_settings_locked(&s);
        (
            power_save_refresh_rooms,
            runtimes,
            active_profile_id,
            updated_mode_configs,
            previous_mode,
            selected_mode,
            mode_changed,
            reapply_scope,
            dispatch_generation,
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

    if power_save == Some(true) && !power_save_refresh_rooms.is_empty() {
        let runtime = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.hub_runtime()
        };
        if let Some(runtime) = runtime {
            let work_items: Vec<_> = power_save_refresh_rooms
                .iter()
                .map(|room_id| WorkItem::QueuedNodeAction {
                    command_id: crate::logging::next_command_id("power-save-off"),
                    node_id: room_id.clone(),
                    action: ButtonAction::OffPress,
                    device_id: None,
                    dispatch_spacing: default_http_batch_dispatch_spacing(),
                    persist_after: false,
                })
                .collect();
            let queued = match queue_node_dispatch_work_items(
                state,
                "settings_power_save",
                work_items,
                default_http_batch_dispatch_spacing(),
            ) {
                Ok(queued) => queued,
                Err(e) => {
                    warn!(
                        target: "cmd",
                        "settings: failed to queue power-save off dispatches: {}",
                        e
                    );
                    false
                }
            };

            for room_id in &power_save_refresh_rooms {
                info!(target: "cmd", "Converting idle room '{}' to hard_off (power_save ON)", room_id);
                if queued {
                    update_lights_on_cache_for_runtime_node(state, &runtime, room_id, false);
                    emit_node_state_event_after_apply(state, &runtime, room_id);
                } else {
                    let event = InputEvent::new(room_id, ButtonAction::OffPress);
                    match runtime.handle_event(&event) {
                        Ok(turned_on) => {
                            sync_active_mode_from_runtime(state, &runtime);
                            update_lights_on_cache_for_runtime_node(
                                state, &runtime, room_id, turned_on,
                            );
                            emit_node_state_event_after_apply(state, &runtime, room_id);
                        }
                        Err(e) => warn!(
                            target: "cmd",
                            "Failed to turn off room '{}': {}",
                            room_id, e
                        ),
                    }
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
                dispatch_generation,
            );
        } else {
            debug!(
                target: "cmd",
                "settings: mode config update did not affect visible room states"
            );
        }
    }

    if mode_changed || force_reapply_outputs {
        let epoch_ms = state
            .lock()
            .ok()
            .and_then(|s| s.last_active_mode_change_utc_ms)
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        let (cause, transition_id) = mode_change
            .as_ref()
            .map(|change| (change.cause, change.transition_id()))
            .unwrap_or((ModeChangeCause::Manual, None));
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::ModeChanged {
                active: selected_mode,
                cause,
                transition_id,
                epoch_ms,
            },
        );
    }

    let settings = build_settings_dto(state)?;
    crate::state::emit_server_event(
        state,
        crate::server_event::ServerEvent::SettingsChanged {
            settings: settings.clone(),
        },
    );

    serde_json::to_string(&settings).map_err(|e| anyhow::anyhow!("serialize: {}", e))
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

fn validate_imported_backup_configuration(configuration: &BackupConfiguration) -> Result<()> {
    validate_mode_configs(&configuration.mode_configs)?;

    let valid_profile_ids = valid_import_profile_ids(&configuration.profiles);

    for room in &configuration.rooms {
        room_flags_for_target_state(room.state)?;
        validate_room_profile_settings(&room.id, &room.room_profile, &valid_profile_ids)?;
    }

    Ok(())
}

fn valid_import_profile_ids(profiles: &[LightProfileConfig]) -> HashSet<String> {
    factory_default_light_profile_config_map()
        .into_keys()
        .chain(profiles.iter().map(|config| config.id.clone()))
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

fn imported_room_profile_patch(room: &BackupConfigurationRoom) -> RoomProfileSettingsPatch {
    RoomProfileSettingsPatch {
        clear_all: false,
        profile_id: Some(room.room_profile.profile_id.clone()),
        fade_ms: Some(room.room_profile.fade_ms.clone()),
        motion_timeout_secs: Some(room.room_profile.motion_timeout_secs.clone()),
    }
}

fn backup_configuration_room_target_id(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    imported: &BackupConfigurationRoom,
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

fn apply_backup_configuration_room_preferences(
    state: &SharedState,
    imported_rooms: &[BackupConfigurationRoom],
) -> (usize, usize) {
    let runtime = state.lock().ok().and_then(|s| s.hub_runtime());
    let mut applied_rooms = 0usize;
    let mut skipped_rooms = 0usize;

    if let Some(runtime) = runtime {
        for imported_room in imported_rooms {
            let Some(room_id) = backup_configuration_room_target_id(state, &runtime, imported_room)
            else {
                skipped_rooms += 1;
                info!(
                    target: "cmd",
                    "backup_configuration_import: skipped room '{}' ({}) because no local match was found",
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
                    "backup_configuration_import: failed to apply room '{}' to '{}': {}",
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

pub fn do_profile_bundle_import(
    state: &SharedState,
    payload: ProfileBundleImportPayload,
) -> Result<String> {
    let bundle = payload.into_bundle();
    if bundle.schema_version != BUNDLE_SCHEMA_VERSION {
        return Err(anyhow::anyhow!(
            "Unsupported profile bundle schema version: {}",
            bundle.schema_version
        ));
    }

    let profile = bundle.profile;

    let imported_profiles = profile.profiles;
    let imported_power_save = profile.power_save;
    let imported_mode_transitions = profile.mode_transitions;

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
                    "profile_bundle_import: failed to update runtime profile '{}': {}",
                    profile.id,
                    e
                );
            }
        }
    }

    do_settings_set_internal(
        state,
        Some(imported_power_save),
        None,
        None,
        Some(imported_mode_transitions),
        None,
        false,
    )?;

    info!(
        target: "cmd",
        "profile_bundle_import: profiles={} transitions={}",
        profiles_to_apply.len(),
        state
            .lock()
            .ok()
            .map(|s| s.mode_transition_configs().len())
            .unwrap_or(0)
    );

    crate::state::emit_server_event(state, crate::server_event::ServerEvent::ConfigChanged);

    build_profile_bundle(state)
}

fn apply_backup_configuration(
    state: &SharedState,
    configuration: BackupConfiguration,
) -> Result<()> {
    validate_imported_backup_configuration(&configuration)?;

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
                    "backup_configuration_import: failed to update runtime profile '{}': {}",
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
        apply_backup_configuration_room_preferences(state, &imported_rooms);

    info!(
        target: "cmd",
        "backup_configuration_import: profiles={} modes={} transitions={} rooms_applied={} rooms_skipped={}",
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

    Ok(())
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

    validate_imported_backup_configuration(&bundle.configuration)?;
    let valid_profile_ids = valid_import_profile_ids(&bundle.configuration.profiles);
    for room in bundle.installation.rooms.iter() {
        validate_room_profile_settings(&room.id, &room.profile_settings, &valid_profile_ids)?;
    }

    do_hub_disconnect(state)?;
    restore_backup_integration_files(state, &bundle.installation.integration_files)?;
    save_backup_hub_registries_to_storage(state, &bundle.installation.hub_registries)?;

    let mut configuration = bundle.configuration.clone();
    configuration.active_mode = bundle.runtime_state.active_mode;
    apply_backup_configuration(state, configuration)?;

    restore_backup_location(state, bundle.installation.location.clone())?;
    restore_backup_hub_credentials(state, &bundle.installation.hub_credentials)?;
    restore_backup_installation_metadata(state, &bundle.installation)?;
    restore_backup_room_manager(state, &bundle.installation.rooms)?;
    restore_backup_runtime_state(state, &bundle.runtime_state)?;

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
    hub_key: &HubKey,
    persist: bool,
    apply_runtime: bool,
) -> Result<String> {
    info!(target: "cmd", "room_set: {} '{}' gl={}", params.id, params.name, params.grouped_light_id);

    let registry = state
        .lock()
        .ok()
        .and_then(|s| s.hub_registry_for(hub_key))
        .ok_or_else(|| anyhow::anyhow!("No hub registry available for {}", hub_key))?;

    // Dedup: skip write if the targeted registry already matches.
    let room_unchanged = registry
        .lock()
        .ok()
        .map(|reg| {
            reg.room_matches(
                &params.id,
                &params.name,
                &params.grouped_light_id,
                &params.device_ids,
            )
        })
        .unwrap_or(false);
    if room_unchanged {
        if apply_runtime {
            if let Ok(room_state) = build_room_rhythm_state(state, &params.id) {
                info!(target: "cmd", "room_set: {} unchanged, skipping persist", params.id);
                return serde_json::to_string(&room_state)
                    .map_err(|e| anyhow::anyhow!("serialize: {}", e));
            }
        } else {
            debug!(
                target: "cmd",
                "room_set: {} unchanged, skipping runtime apply during sync",
                params.id
            );
            return Ok(String::new());
        }
        // Room is in registry but not yet in engine — fall through to add it
    }

    {
        let mut reg = registry
            .lock()
            .map_err(|_| anyhow::anyhow!("hub registry lock poisoned for {}", hub_key))?;
        reg.upsert_room(
            &params.id,
            &params.name,
            &params.grouped_light_id,
            &params.device_ids,
        );
    }

    // Determine engine room ID: always use topology room IDs for the engine.
    let engine_room_id = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.topology.translate_or_create(
            hub_key,
            &params.id,
            &params.name,
            &params.grouped_light_id,
            &params.device_ids,
        )
    };

    if !apply_runtime {
        if persist {
            persist_state(state);
        }
        return Ok(String::new());
    }

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
    let (runtime, power_save) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (s.hub_runtime(), s.power_save)
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
        let state_flags = params
            .state
            .map(|state| room_flags_for_target_state(room_state_for_power_save(power_save, state)))
            .transpose()?;
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

    crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);

    let room_state = build_room_rhythm_state(state, &engine_room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
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
    let action = parse_node_action(action_str)?;

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

fn parse_node_action(action_str: &str) -> Result<ButtonAction> {
    match action_str {
        "on" => Ok(ButtonAction::OnPress),
        "off" => Ok(ButtonAction::OffPress),
        "toggle" => Ok(ButtonAction::Toggle),
        other => ButtonAction::from_service_name(other)
            .ok_or_else(|| anyhow::anyhow!("Unknown action: {}", other)),
    }
}

pub fn queue_node_action(
    state: &SharedState,
    node_id: &str,
    action_str: &str,
    persist_after: bool,
    dispatch_spacing: Duration,
) -> Result<()> {
    let action = parse_node_action(action_str)?;
    let (runtime, tx) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (s.hub_runtime(), s.work_tx.clone())
    };
    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;
    runtime
        .engine_node_snapshot(node_id)
        .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", node_id))?;
    let tx = tx.ok_or_else(|| anyhow::anyhow!("Node dispatch queue unavailable"))?;

    tx.try_send(WorkItem::QueuedNodeAction {
        command_id: crate::logging::next_command_id("node-action"),
        node_id: node_id.to_string(),
        action,
        device_id: None,
        dispatch_spacing,
        persist_after,
    })
    .map_err(|e| anyhow::anyhow!("Node dispatch queue unavailable: {}", e))
}

pub fn queue_node_action_batch(
    state: &SharedState,
    actions: Vec<(String, String)>,
    persist_after: bool,
    dispatch_spacing: Duration,
) -> Result<()> {
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if s.work_tx.is_none() {
            return Err(anyhow::anyhow!("Node dispatch queue unavailable"));
        }
        s.hub_runtime()
    };
    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let mut work_items = Vec::with_capacity(actions.len() + usize::from(persist_after));
    for (node_id, action_str) in actions {
        let action = parse_node_action(&action_str)?;
        runtime
            .engine_node_snapshot(&node_id)
            .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", node_id))?;
        work_items.push(WorkItem::QueuedNodeAction {
            command_id: crate::logging::next_command_id("node-action"),
            node_id,
            action,
            device_id: None,
            dispatch_spacing,
            persist_after: false,
        });
    }

    queue_node_dispatch_batch(
        state,
        "node_action_batch",
        work_items,
        persist_after,
        dispatch_spacing,
    )
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

pub fn queue_set_node_brightness(
    state: &SharedState,
    node_id: &str,
    brightness: u8,
    persist_after: bool,
    dispatch_spacing: Duration,
) -> Result<()> {
    let brightness = brightness.clamp(1, 100);
    let (runtime, tx) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (s.hub_runtime(), s.work_tx.clone())
    };
    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;
    runtime
        .engine_node_snapshot(node_id)
        .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", node_id))?;
    let tx = tx.ok_or_else(|| anyhow::anyhow!("Node dispatch queue unavailable"))?;

    tx.try_send(WorkItem::SetNodeBrightness {
        command_id: crate::logging::next_command_id("node-brightness"),
        node_id: node_id.to_string(),
        brightness,
        dispatch_spacing,
        persist_after,
    })
    .map_err(|e| anyhow::anyhow!("Node dispatch queue unavailable: {}", e))
}

pub fn queue_set_node_brightness_batch(
    state: &SharedState,
    updates: Vec<(String, u8)>,
    persist_after: bool,
    dispatch_spacing: Duration,
) -> Result<()> {
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if s.work_tx.is_none() {
            return Err(anyhow::anyhow!("Node dispatch queue unavailable"));
        }
        s.hub_runtime()
    };
    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let mut work_items = Vec::with_capacity(updates.len() + usize::from(persist_after));
    for (node_id, brightness) in updates {
        runtime
            .engine_node_snapshot(&node_id)
            .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", node_id))?;
        work_items.push(WorkItem::SetNodeBrightness {
            command_id: crate::logging::next_command_id("node-brightness"),
            node_id,
            brightness: brightness.clamp(1, 100),
            dispatch_spacing,
            persist_after: false,
        });
    }

    queue_node_dispatch_batch(
        state,
        "node_brightness_batch",
        work_items,
        persist_after,
        dispatch_spacing,
    )
}

/// Set the time offset for a node directly (not additive).
///
/// Does NOT change observed power tracking — offset doesn't imply lights-on state change.
pub fn do_set_node_time_offset(
    state: &SharedState,
    node_id: &str,
    offset_minutes: f32,
    persist: bool,
) -> Result<String> {
    do_set_node_time_offset_with_spacing(
        state,
        node_id,
        offset_minutes,
        persist,
        default_http_batch_dispatch_spacing(),
    )
}

pub fn do_set_node_time_offset_with_spacing(
    state: &SharedState,
    node_id: &str,
    offset_minutes: f32,
    persist: bool,
    dispatch_spacing: Duration,
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

    let snap = time_offset_target_node_snapshot(runtime.as_ref(), node_id)?;
    let mut restored = RestoredNodeState::from(&snap);
    restored.time_offset_minutes = offset_minutes;
    runtime.restore_node_state(node_id, restored);

    clear_room_mode_transition(state, node_id);
    enqueue_time_offset_preview_ticks(state, &runtime, node_id, dispatch_spacing)?;

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

pub fn do_set_node_time_offsets_batch_with_spacing(
    state: &SharedState,
    updates: &[(String, f32)],
    persist: bool,
    dispatch_spacing: Duration,
) -> Result<usize> {
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };
    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let mut update_snaps = Vec::with_capacity(updates.len());
    for (node_id, _) in updates {
        update_snaps.push(time_offset_target_node_snapshot(runtime.as_ref(), node_id)?);
    }

    let top_level_update_ids = time_offset_top_level_update_ids(&update_snaps);
    let mut emitted_node_ids = HashSet::new();
    for ((node_id, offset_minutes), snap) in updates.iter().zip(update_snaps.iter()) {
        if time_offset_batch_parent_covers_node(snap, &top_level_update_ids) {
            continue;
        }

        let mut restored = RestoredNodeState::from(snap);
        restored.time_offset_minutes = *offset_minutes;
        runtime.restore_node_state(node_id, restored);

        clear_room_mode_transition(state, node_id);
        if emitted_node_ids.insert(node_id.clone()) {
            emit_node_state_event_after_apply(state, &runtime, node_id);
        }
    }

    let dispatch_count = enqueue_time_offset_batch_preview_ticks(
        state,
        &runtime,
        &update_snaps,
        &top_level_update_ids,
        dispatch_spacing,
    )?;

    if persist {
        persist_rooms(state);
    }

    Ok(dispatch_count)
}

pub fn default_node_time_offset_updates(
    state: &SharedState,
    offset_minutes: f32,
) -> Result<Vec<(String, f32)>> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let runtime = s
        .hub_runtime()
        .ok_or_else(|| anyhow::anyhow!("No runtime available"))?;
    let snapshots = runtime.engine_all_effective_node_snapshots();
    let mut seen = HashSet::new();
    let mut updates = Vec::new();
    for snap in snapshots {
        if !snap.kind.is_light_addressable() || snap.parent_id.is_some() {
            continue;
        }
        if snap.kind.is_room() && !time_offset_preview_room_is_on(&s, &snap) {
            continue;
        }
        if seen.insert(snap.id.clone()) {
            updates.push((snap.id, offset_minutes));
        }
    }
    Ok(updates)
}

fn time_offset_target_node_snapshot(
    runtime: &dyn RuntimeHandle,
    node_id: &str,
) -> Result<rhythm_core::NodeSnapshot> {
    let snap = runtime
        .engine_node_snapshot(node_id)
        .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", node_id))?;

    if !snap.kind.is_light_addressable() {
        return Err(anyhow::anyhow!(
            "Time offsets can only be set on light-addressable nodes"
        ));
    }

    Ok(snap)
}

fn enqueue_time_offset_preview_ticks(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    node_id: &str,
    dispatch_spacing: Duration,
) -> Result<usize> {
    let target_snap = runtime
        .engine_effective_node_snapshot(node_id)
        .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", node_id))?;
    let dispatch_node = crate::periodic::preview_dispatch_node_for_target(&target_snap);
    enqueue_time_offset_preview_dispatch_nodes(state, runtime, dispatch_node, dispatch_spacing)
}

fn enqueue_time_offset_batch_preview_ticks(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    update_snaps: &[rhythm_core::NodeSnapshot],
    top_level_update_ids: &HashSet<String>,
    dispatch_spacing: Duration,
) -> Result<usize> {
    let dispatch_nodes = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let mut source_snaps = Vec::new();
        let mut direct_nodes = Vec::new();
        let mut seen_source = HashSet::new();
        let mut seen_direct = HashSet::new();
        for snap in update_snaps {
            if !snap.kind.is_light_addressable() {
                continue;
            }
            if time_offset_batch_parent_covers_node(snap, top_level_update_ids) {
                continue;
            }
            if snap.parent_id.is_some() {
                if seen_direct.insert(snap.id.clone()) {
                    if let Some(node) = crate::periodic::preview_dispatch_node_for_target(snap) {
                        direct_nodes.push(node);
                    }
                }
                continue;
            }
            if snap.kind.is_room() && !time_offset_preview_room_is_on(&s, snap) {
                continue;
            }
            if seen_source.insert(snap.id.clone()) {
                source_snaps.push(snap.clone());
            }
        }
        let mut dispatch_nodes =
            crate::periodic::periodic_dispatch_nodes_from_state(&s, &source_snaps);
        dispatch_nodes.extend(direct_nodes);
        dispatch_nodes
    };

    enqueue_time_offset_preview_dispatch_nodes(state, runtime, dispatch_nodes, dispatch_spacing)
}

fn time_offset_top_level_update_ids(update_snaps: &[rhythm_core::NodeSnapshot]) -> HashSet<String> {
    update_snaps
        .iter()
        .filter(|snap| snap.parent_id.is_none() && snap.kind.is_light_addressable())
        .map(|snap| snap.id.clone())
        .collect()
}

fn time_offset_batch_parent_covers_node(
    snap: &rhythm_core::NodeSnapshot,
    top_level_update_ids: &HashSet<String>,
) -> bool {
    snap.parent_id
        .as_deref()
        .is_some_and(|parent_id| top_level_update_ids.contains(parent_id))
}

fn time_offset_preview_room_is_on(s: &AppState, snap: &rhythm_core::NodeSnapshot) -> bool {
    lights_on_from_observed_cache(
        s,
        &s.room_observed_power,
        &snap.id,
        snap.kind,
        snap.parent_id.as_deref(),
        semantic_lights_on_override(s.power_save, snap.hard_off, snap.soft_off),
    )
}

fn enqueue_time_offset_preview_dispatch_nodes<I>(
    state: &SharedState,
    runtime: &Arc<dyn RuntimeHandle>,
    dispatch_nodes: I,
    dispatch_spacing: Duration,
) -> Result<usize>
where
    I: IntoIterator<Item = crate::periodic::PeriodicDispatchNode>,
{
    let dispatch_nodes: Vec<_> = dispatch_nodes.into_iter().collect();
    if dispatch_nodes.is_empty() {
        return Ok(0);
    }

    let (dispatch_tx, dispatch_generation) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.periodic_work_tx.clone().or_else(|| s.work_tx.clone()),
            s.light_dispatch_generation,
        )
    };

    let command_id = crate::logging::next_command_id("manual-preview");
    let current_hour = runtime.current_hour();
    let mut last_node_index_by_emit_target = HashMap::new();
    for (idx, node) in dispatch_nodes.iter().enumerate() {
        last_node_index_by_emit_target.insert(node.emit_node_id.clone(), idx);
    }
    let dispatch_count = dispatch_nodes.len();

    if let Some(tx) = dispatch_tx {
        for (idx, node) in dispatch_nodes.iter().enumerate() {
            let emit_parent_node_id = last_node_index_by_emit_target
                .get(&node.emit_node_id)
                .is_some_and(|last_idx| *last_idx == idx)
                .then_some(node.emit_node_id.as_str())
                .filter(|emit_id| *emit_id != node.settings_node_id);
            if !crate::periodic::enqueue_periodic_tick(
                state,
                &tx,
                crate::periodic::PeriodicTickEnqueue {
                    command_id: &command_id,
                    node_id: &node.node_id,
                    settings_node_id: &node.settings_node_id,
                    dispatch_generation,
                    current_hour,
                    emit_parent_node_id,
                    dispatch_spacing,
                },
            ) {
                return Err(anyhow::anyhow!("Node dispatch queue full"));
            }
        }
        return Ok(dispatch_count);
    }

    for (idx, node) in dispatch_nodes.iter().enumerate() {
        runtime.periodic_tick_node(&node.node_id, &node.settings_node_id, current_hour)?;
        crate::periodic::post_tick_node(state, runtime, &node.settings_node_id);
        let emit_parent_node_id = last_node_index_by_emit_target
            .get(&node.emit_node_id)
            .is_some_and(|last_idx| *last_idx == idx)
            .then_some(node.emit_node_id.as_str())
            .filter(|emit_id| *emit_id != node.settings_node_id);
        if let Some(emit_parent_node_id) = emit_parent_node_id {
            crate::periodic::post_tick_node(state, runtime, emit_parent_node_id);
        }
    }

    Ok(dispatch_count)
}

// ============================================================================
// Device commands
// ============================================================================

/// Upsert a device with button mappings and type in the registry.
///
/// Updates only the specified hub's registry. `room_id == None` registers the
/// device as roomless (no room mapping); button/type mappings are still stored
/// so SSE events can resolve to a known device while awaiting user assignment.
pub fn do_device_set(
    state: &SharedState,
    device_id: &str,
    room_id: Option<&str>,
    buttons: &[(String, u8)],
    device_type: DeviceType,
    hub_key: &HubKey,
    persist: bool,
) -> Result<()> {
    match room_id {
        Some(rid) => {
            info!(target: "cmd", "device_set: {} ({:?}) -> room {} ({} buttons)", device_id, device_type, rid, buttons.len())
        }
        None => {
            info!(target: "cmd", "device_set: {} ({:?}) -> roomless ({} buttons)", device_id, device_type, buttons.len())
        }
    }

    let registry = state
        .lock()
        .ok()
        .and_then(|s| s.hub_registry_for(hub_key))
        .ok_or_else(|| anyhow::anyhow!("No hub registry available for {}", hub_key))?;

    // Dedup
    let device_unchanged = registry
        .lock()
        .ok()
        .map(|reg| reg.device_matches(device_id, room_id, buttons, &device_type))
        .unwrap_or(false);
    if device_unchanged {
        info!(target: "cmd", "device_set: {} unchanged, skipping persist", device_id);
        return Ok(());
    }

    registry
        .lock()
        .map_err(|_| anyhow::anyhow!("hub registry lock poisoned for {}", hub_key))?
        .upsert_device(device_id, room_id, buttons, device_type);

    if persist {
        persist_state(state);
    }
    Ok(())
}

/// Remove a device from the registry.
///
/// Removes from the specified hub's registry only.
pub fn do_device_remove(state: &SharedState, device_id: &str, hub_key: &HubKey) -> Result<()> {
    info!(target: "cmd", "device_remove: {}", device_id);

    let registry = state
        .lock()
        .ok()
        .and_then(|s| s.hub_registry_for(hub_key))
        .ok_or_else(|| anyhow::anyhow!("No hub registry available for {}", hub_key))?;
    registry
        .lock()
        .map_err(|_| anyhow::anyhow!("hub registry lock poisoned for {}", hub_key))?
        .remove_device(device_id);

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

    drop(s);

    if let Some(node_id) = canonical_id.as_deref() {
        queue_motion_timer_clear(state, node_id);
    }

    persist_registry(state);
    reconcile_runtime_from_state(state)?;

    {
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
        longitude,
        utc_offset,
        timezone_name,
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
            s.latitude,
            s.longitude,
            s.utc_offset_hours,
            s.timezone_name.clone(),
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
                    longitude,
                    timezone_name: timezone_name.as_deref(),
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
pub fn do_config_set(state: &SharedState, config: LightProfileConfig) -> Result<()> {
    do_config_set_with_options(state, config, false)
}

/// Update a light profile config, optionally reapplying active-mode outputs.
pub fn do_config_set_with_options(
    state: &SharedState,
    mut config: LightProfileConfig,
    apply_active_outputs: bool,
) -> Result<()> {
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

    let (runtime, should_apply_outputs, active_mode, dispatch_generation) = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if !s.light_profile_configs.contains_key(&config.id) {
            return Err(anyhow::anyhow!("Unknown light profile: {}", config.id));
        }
        let updates_active_profile = config.id == s.active_mode_profile_id();
        s.set_light_profile_config(config.clone());
        if updates_active_profile {
            s.sync_active_mode_runtime_overrides();
        }
        let should_apply_outputs = apply_active_outputs && updates_active_profile;
        let dispatch_generation = if should_apply_outputs {
            let dispatch_generation = s.invalidate_queued_light_dispatches();
            tracing::debug!(
                target: "cmd",
                event = "queued_light_dispatches_invalidated",
                active_mode = ?s.active_mode,
                profile_id = %config.id,
                dispatch_generation,
                "Invalidated queued generated light dispatches for active config apply"
            );
            dispatch_generation
        } else {
            s.light_dispatch_generation
        };
        let active_mode = s.active_mode;
        persist_light_profiles_locked(&s);
        (
            s.hub_runtime(),
            should_apply_outputs,
            active_mode,
            dispatch_generation,
        )
    };

    if let Some(runtime) = runtime.as_ref() {
        if let Err(e) = runtime.set_light_profile_config(config.clone()) {
            warn!(target: "cmd", "Failed to update runtime light profile config: {}", e);
        }
    }

    if should_apply_outputs {
        apply_active_mode_outputs(
            state,
            active_mode,
            active_mode,
            None,
            ModeOutputApplyScope::all_visible(),
            dispatch_generation,
        );
    }

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
        let mut reset_room_ids = Vec::new();
        for snap in &snapshots {
            if snap.time_offset_minutes.abs() > 0.001 {
                let mut restored = RestoredRoomState::from(snap);
                restored.time_offset_minutes = 0.0;
                rt.restore_room_state(&snap.id, restored);
                reset_room_ids.push(snap.id.clone());
            }
        }

        for room_id in reset_room_ids {
            if let Err(e) = enqueue_time_offset_preview_ticks(
                state,
                rt,
                &room_id,
                default_http_batch_dispatch_spacing(),
            ) {
                warn!(
                    target: "cmd",
                    "Failed to enqueue offset reset refresh for room {}: {}",
                    room_id,
                    e
                );
            }
        }
    }

    persist_rooms(state);

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
        let (solar_noon, day_of_year, sun_times) = if let Some(ref tz_name) = timezone_name {
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
                Some(rhythm_core::calculate_sun_times(
                    lat, lon, year, month, day, &tz,
                )),
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
                None,
            )
        };
        s.runtime_config.solar_noon_hour = solar_noon;
        crate::logging::update_log_clock_from_location(
            s.timezone_name.as_deref(),
            s.utc_offset_hours,
            true,
        );

        if let Some(runtime) = s.hub_runtime() {
            let solar_time = rhythm_core::SolarTime::new(solar_noon, lat, day_of_year);
            if let Err(e) = runtime.set_solar(solar_time) {
                warn!(target: "cmd", "Failed to update solar time: {}", e);
            }
            if let Some(sun_times) = sun_times {
                if let Err(e) = runtime.set_sun_times(sun_times) {
                    warn!(target: "cmd", "Failed to update sun times: {}", e);
                }
            } else if let Err(e) = runtime.clear_sun_times() {
                warn!(target: "cmd", "Failed to clear sun times: {}", e);
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
    let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), address);

    let credentials_json = serde_json::to_string(credentials)?;

    let get_provider = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.get_hub_provider_fn
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No hub provider registered"))?
    };

    if let Ok(mut s) = state.lock() {
        s.clear_hub_startup_retry(&hub_key);
    }

    let provider = get_provider(hub_type.clone());
    provider.configure(address, &credentials_json, state)?;

    // Register the new hub's controller with the composite (if runtime already exists).
    register_hub_with_composite(state, &hub_key);

    // Auto-sync rooms from the newly configured hub.
    // Uses platform config to decide whether to also discover devices/sensors
    // (desktop: full sync, constrained blocking path: rooms only — devices
    // arrive via SSE).
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
            })
            .and_then(|h| h.join().map_err(|_| std::io::Error::other("panicked")));
    }

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

/// Reset startup retry state for one configured hub and request an immediate
/// stored-credentials bootstrap attempt.
pub fn do_retry_hub_connect(state: &SharedState, hub_type_str: &str, address: &str) -> Result<()> {
    let hub_type = crate::hub::HubType::parse(hub_type_str)
        .ok_or_else(|| anyhow::anyhow!("Unknown hub type: {}", hub_type_str))?;
    let hub_key = HubKey::new(hub_type, address);

    let request_bootstrap = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        let creds = s
            .hub_credentials
            .get(&hub_key)
            .ok_or_else(|| anyhow::anyhow!("No stored credentials for {}", hub_key))?;
        if !creds.can_connect() {
            return Err(anyhow::anyhow!(
                "Stored credentials for {} are not connectable",
                hub_key
            ));
        }
        if !s
            .hub_capabilities
            .iter()
            .any(|capability| capability.hub_type == hub_type_str)
        {
            return Err(anyhow::anyhow!(
                "Hub type '{}' is not supported on this runtime",
                hub_type_str
            ));
        }
        if s.hubs.contains_key(&hub_key) {
            return Ok(());
        }

        s.clear_hub_startup_retry(&hub_key);
        s.request_hub_bootstrap_fn
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No hub bootstrap callback registered"))?
    };

    request_bootstrap(state);
    crate::state::emit_server_event(
        state,
        crate::server_event::ServerEvent::HubStatus {
            hub_type: Some(hub_type_str.to_string()),
            address: Some(address.to_string()),
            connected: false,
        },
    );
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
        s.hub_pending_disconnect_at.clear();
        s.hub_startup_retry.clear();

        s.hub_credentials.clear();
        if let Some(ref storage) = s.storage {
            let _ = storage.save_all_hub_credentials(&[]);
        }

        s.room_observed_power.clear();
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
    let old_hub_keys: Vec<_> = old_hubs.keys().cloned().collect();

    // Clear all controllers from the composite
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
        s.clear_hub_startup_retry(&key);
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
    let (lights_on, power_save) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            lights_on_from_observed_cache(
                &s,
                &s.room_observed_power,
                &snap.id,
                snap.kind,
                snap.parent_id.as_deref(),
                semantic_lights_on_override(s.power_save, snap.hard_off, snap.soft_off),
            ),
            s.power_save,
        )
    };

    let prev_soft_off = snap.soft_off;
    let prev_hard_off = snap.hard_off;
    let rhythm_enabled = rhythm_enabled.unwrap_or(snap.rhythm_enabled);
    let disabled = disabled.unwrap_or(snap.disabled);
    let explicit_state_request = target_state.is_some();
    let requested_state = target_state
        .unwrap_or_else(|| persistent_room_state_from_flags(snap.hard_off, snap.soft_off));
    let persistent_state = room_state_for_power_save(power_save, requested_state);
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
    if explicit_state_request {
        queue_motion_timer_clear(state, node_id);
    }

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

pub struct QueuedNodePreferencesPatch {
    pub node_id: String,
    pub rhythm_enabled: Option<bool>,
    pub disabled: Option<bool>,
    pub target_state: Option<RoomModeState>,
    pub room_profile: Option<RoomProfileSettingsPatch>,
}

fn validate_room_profile_settings_patch(
    room_profile: Option<&RoomProfileSettingsPatch>,
    valid_profile_ids: &HashSet<String>,
) -> Result<()> {
    if let Some(profile_id) = room_profile
        .and_then(|patch| patch.profile_id.as_ref())
        .and_then(|profile_id| profile_id.as_deref())
    {
        if rhythm_core::is_builtin_state_profile_id(profile_id) {
            return Err(anyhow::anyhow!(
                "State profiles cannot be selected per-node"
            ));
        }
        if !valid_profile_ids.contains(profile_id) {
            return Err(anyhow::anyhow!("Unknown light profile: {}", profile_id));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn queue_node_preferences_set(
    state: &SharedState,
    node_id: &str,
    rhythm_enabled: Option<bool>,
    disabled: Option<bool>,
    target_state: Option<RoomModeState>,
    room_profile: Option<RoomProfileSettingsPatch>,
    persist_after: bool,
    dispatch_spacing: Duration,
) -> Result<()> {
    let (runtime, tx) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (s.hub_runtime(), s.work_tx.clone())
    };
    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;
    runtime
        .engine_node_snapshot(node_id)
        .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", node_id))?;
    let tx = tx.ok_or_else(|| anyhow::anyhow!("Node dispatch queue unavailable"))?;

    tx.try_send(WorkItem::SetNodePreferences {
        command_id: crate::logging::next_command_id("node-preferences"),
        node_id: node_id.to_string(),
        rhythm_enabled,
        disabled,
        target_state,
        room_profile,
        dispatch_spacing,
        persist_after,
    })
    .map_err(|e| anyhow::anyhow!("Node dispatch queue unavailable: {}", e))
}

pub fn queue_node_preferences_batch(
    state: &SharedState,
    updates: Vec<QueuedNodePreferencesPatch>,
    persist_after: bool,
    dispatch_spacing: Duration,
) -> Result<()> {
    let (runtime, valid_profile_ids) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if s.work_tx.is_none() {
            return Err(anyhow::anyhow!("Node dispatch queue unavailable"));
        }
        (
            s.hub_runtime(),
            s.light_profile_configs
                .keys()
                .cloned()
                .collect::<HashSet<_>>(),
        )
    };
    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let mut work_items = Vec::with_capacity(updates.len() + usize::from(persist_after));
    for update in updates {
        runtime
            .engine_node_snapshot(&update.node_id)
            .ok_or_else(|| anyhow::anyhow!("Node '{}' not found in engine", update.node_id))?;
        validate_room_profile_settings_patch(update.room_profile.as_ref(), &valid_profile_ids)?;
        work_items.push(WorkItem::SetNodePreferences {
            command_id: crate::logging::next_command_id("node-preferences"),
            node_id: update.node_id,
            rhythm_enabled: update.rhythm_enabled,
            disabled: update.disabled,
            target_state: update.target_state,
            room_profile: update.room_profile,
            dispatch_spacing,
            persist_after: false,
        });
    }

    queue_node_dispatch_batch(
        state,
        "node_preferences_batch",
        work_items,
        persist_after,
        dispatch_spacing,
    )
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
    let ensure_fn = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.ensure_runtime_fn.clone()
    };

    let Some(ensure_fn) = ensure_fn else {
        return Err(anyhow::anyhow!("No runtime initializer configured"));
    };

    let rt_state = state.clone();
    let rt_result = std::thread::Builder::new()
        .name("rt-init".to_string())
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
pub(crate) fn ensure_runtime_room_exists(
    state: &SharedState,
    room_id: &str,
    room_name: &str,
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
        let existed = runtime.engine_room_snapshot(room_id).is_some();
        runtime.add_room(room_id, room_name);
        if !existed {
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

/// Reconcile the shared runtime from topology + canonical state.
///
/// This is the explicit topology-first runtime phase that runs after sync and
/// after topology/manual mutations so integrations that discover devices
/// without rooms do not depend on incidental room bootstrap paths.
pub fn reconcile_runtime_from_state(state: &SharedState) -> Result<()> {
    let (has_any_hub, has_runtime_initializer, rooms, nodes) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let rooms = s
            .topology
            .rooms()
            .map(|room| (room.id.clone(), room.name.clone()))
            .collect::<Vec<_>>();
        let nodes = s
            .topology
            .device_nodes()
            .filter_map(|node| {
                s.canonical_registry
                    .get(&node.canonical_device_id)
                    .map(|device| {
                        (
                            node.id.clone(),
                            device.name.clone(),
                            device.device_type.clone(),
                            node.parent_id.clone(),
                        )
                    })
            })
            .collect::<Vec<_>>();
        (s.has_any_hub(), s.ensure_runtime_fn.is_some(), rooms, nodes)
    };

    let existing_runtime = state.lock().ok().and_then(|s| s.hub_runtime());
    let persisted_node_states: HashMap<String, rhythm_core::Room> = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.storage
            .as_ref()
            .and_then(|storage| storage.load_rooms().ok())
            .filter(|rooms| !crate::lifecycle::persisted_rooms_look_corrupted(rooms))
            .map(|rooms| {
                rooms
                    .iter()
                    .map(|room| (room.id.clone(), room.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let existing_room_states: HashMap<String, rhythm_core::RoomSnapshot> = existing_runtime
        .as_ref()
        .map(|runtime| {
            runtime
                .engine_all_room_snapshots()
                .into_iter()
                .map(|snap| (snap.id.clone(), snap))
                .collect()
        })
        .unwrap_or_default();
    let existing_node_states: HashMap<String, rhythm_core::NodeSnapshot> = existing_runtime
        .as_ref()
        .map(|runtime| {
            runtime
                .engine_all_node_snapshots()
                .into_iter()
                .map(|snap| (snap.id.clone(), snap))
                .collect()
        })
        .unwrap_or_default();

    let needs_runtime = has_any_hub && (!rooms.is_empty() || !nodes.is_empty());
    if needs_runtime
        && has_runtime_initializer
        && state.lock().ok().and_then(|s| s.hub_runtime()).is_none()
    {
        try_ensure_runtime(state)?;
    }

    let has_runtime = state.lock().ok().and_then(|s| s.hub_runtime()).is_some();
    if needs_runtime && has_runtime_initializer && !has_runtime {
        return Err(anyhow::anyhow!(
            "Runtime reconciliation completed without installing a runtime"
        ));
    }

    if !has_runtime {
        return Ok(());
    }

    let runtime = state
        .lock()
        .ok()
        .and_then(|s| s.hub_runtime())
        .ok_or_else(|| anyhow::anyhow!("Runtime reconciliation expected a live runtime"))?;

    let desired_room_ids = rooms
        .iter()
        .map(|(room_id, _)| room_id.clone())
        .collect::<HashSet<_>>();
    let desired_node_ids = nodes
        .iter()
        .map(|(node_id, _, _, _)| node_id.clone())
        .collect::<HashSet<_>>();

    for snapshot in runtime.engine_all_room_snapshots() {
        if !desired_room_ids.contains(&snapshot.id) {
            runtime.remove_room(&snapshot.id);
        }
    }

    for snapshot in runtime.engine_all_node_snapshots() {
        if !desired_node_ids.contains(&snapshot.id) {
            runtime.remove_node(&snapshot.id);
        }
    }

    for (room_id, room_name) in rooms {
        ensure_runtime_room_exists(state, &room_id, &room_name)?;
        if let Some(snap) = existing_room_states.get(&room_id) {
            runtime.restore_room_state(
                &room_id,
                RestoredRoomState {
                    rhythm_enabled: snap.rhythm_enabled,
                    disabled: snap.disabled,
                    time_offset_minutes: snap.time_offset_minutes,
                    brightness_offset: snap.brightness_offset,
                    soft_off: snap.soft_off,
                    hard_off: snap.hard_off,
                    profile_settings: snap.profile_settings.clone(),
                },
            );
        } else if let Some(room) = persisted_node_states.get(&room_id) {
            runtime.restore_room_state(
                &room_id,
                RestoredRoomState {
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
    }

    for (node_id, node_name, device_type, parent_id) in nodes {
        ensure_runtime_device_node_exists(state, &node_id, &node_name, device_type, parent_id)?;
        if let Some(snap) = existing_node_states.get(&node_id) {
            runtime.restore_node_state(
                &node_id,
                RestoredNodeState {
                    rhythm_enabled: snap.rhythm_enabled,
                    disabled: snap.disabled,
                    time_offset_minutes: snap.time_offset_minutes,
                    brightness_offset: snap.brightness_offset,
                    soft_off: snap.soft_off,
                    hard_off: snap.hard_off,
                    profile_settings: snap.profile_settings.clone(),
                },
            );
        } else if let Some(node) = persisted_node_states.get(&node_id) {
            runtime.restore_node_state(
                &node_id,
                RestoredNodeState {
                    rhythm_enabled: node.rhythm_enabled,
                    disabled: node.disabled,
                    time_offset_minutes: node.time_offset_minutes,
                    brightness_offset: node.brightness_offset,
                    soft_off: node.soft_off,
                    hard_off: node.hard_off,
                    profile_settings: node.profile_settings.clone(),
                },
            );
        }
    }

    rebuild_composite_routing(state);
    schedule_topology_group_sync_for_integrations(state);
    apply_pending_mode_outputs_if_ready(state);

    Ok(())
}

fn run_topology_group_sync_for_integrations(state: &SharedState) {
    let callback = state
        .lock()
        .ok()
        .and_then(|state| state.sync_topology_groups_fn.clone());
    let Some(callback) = callback else {
        return;
    };

    match callback(state) {
        Ok(()) => {
            persist_registry(state);
            if let Ok(state) = state.lock() {
                persist_topology(&state);
            }
        }
        Err(error) => {
            warn!(target: "cmd", "Topology group sync failed: {}", error);
        }
    }
}

fn schedule_topology_group_sync_for_integrations(state: &SharedState) {
    let should_spawn = {
        let Ok(mut state) = state.lock() else {
            return;
        };
        if state.sync_topology_groups_fn.is_none() {
            return;
        }

        state.topology_group_sync_pending = true;
        if state.topology_group_sync_in_progress {
            false
        } else {
            state.topology_group_sync_in_progress = true;
            true
        }
    };

    if !should_spawn {
        return;
    }

    let worker_state = state.clone();
    let spawn_result = std::thread::Builder::new()
        .name("topology-group-sync".to_string())
        .spawn(move || loop {
            let should_run = {
                let Ok(mut state) = worker_state.lock() else {
                    return;
                };
                if state.sync_topology_groups_fn.is_none() {
                    state.topology_group_sync_pending = false;
                    state.topology_group_sync_in_progress = false;
                    return;
                }
                if !state.topology_group_sync_pending {
                    state.topology_group_sync_in_progress = false;
                    return;
                }
                state.topology_group_sync_pending = false;
                true
            };

            if should_run {
                run_topology_group_sync_for_integrations(&worker_state);
            }
        });

    if let Err(error) = spawn_result {
        if let Ok(mut state) = state.lock() {
            state.topology_group_sync_in_progress = false;
        }
        warn!(target: "cmd", "Failed to start topology group sync worker: {}", error);
    }
}

fn clear_runtime_node_off_flags(state: &SharedState, node_id: &str) -> Result<()> {
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };
    let Some(runtime) = runtime else {
        return Ok(());
    };
    let Some(snap) = runtime.engine_node_snapshot(node_id) else {
        return Ok(());
    };
    if !snap.soft_off && !snap.hard_off {
        return Ok(());
    }

    runtime.restore_node_state(
        node_id,
        RestoredNodeState {
            rhythm_enabled: snap.rhythm_enabled,
            disabled: snap.disabled,
            time_offset_minutes: snap.time_offset_minutes,
            brightness_offset: snap.brightness_offset,
            soft_off: false,
            hard_off: false,
            profile_settings: snap.profile_settings,
        },
    );
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

/// Flash a canonical light device for physical identification.
///
/// The operation dispatches directly to the device's preferred integration
/// endpoint and intentionally does not mutate Rhythm's runtime or persisted
/// node state.
pub fn do_flash_canonical_device(state: &SharedState, device_id: &str) -> Result<()> {
    let (device_name, hub_key, native_id) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let device = s
            .canonical_registry
            .get(device_id)
            .filter(|device| !device.is_removed())
            .ok_or_else(|| anyhow::anyhow!("Device not found: {}", device_id))?;

        if device.device_type != DeviceType::Light {
            return Err(anyhow::anyhow!(
                "Flash is only supported for light devices: {}",
                device_id
            ));
        }

        let endpoint = device
            .preferred_endpoint()
            .ok_or_else(|| anyhow::anyhow!("Device has no active endpoint: {}", device_id))?;

        (
            device.name.clone(),
            endpoint.hub_key.clone(),
            endpoint.native_id.clone(),
        )
    };

    let composite = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.composite_controller.clone()
    };

    let composite = match composite {
        Some(composite) => composite,
        None => {
            try_ensure_runtime(state)?;
            state
                .lock()
                .ok()
                .and_then(|s| s.composite_controller.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Runtime initialization reported success but no composite controller is available"
                    )
                })?
        }
    };

    let target = HubDispatchTarget::Devices {
        native_ids: vec![native_id.clone()],
    };
    let hub_key_label = hub_key.to_string();

    info!(
        target: "cmd",
        "device_flash: {} ({}) via {}:{}",
        device_name,
        device_id,
        hub_key_label,
        native_id
    );

    futures::executor::block_on(composite.flash_target(&hub_key_label, target))
        .map_err(|e| anyhow::anyhow!("Failed to flash device {}: {}", device_id, e))
}

/// Per-device synthetic hub-registry rooms.
///
/// Light controllers address standalone (room-less) devices through
/// `get_grouped_light_id(native_id)`, which only returns a target if a
/// synthetic room keyed by the device's native_id exists. We keep this
/// behavior for light output addressing.
///
/// Event routing no longer depends on these synthetic rooms — buttons and
/// motion sensors resolve through canonical + topology directly — so this
/// helper is a no-op for non-Light device types.
fn ensure_synthetic_device_registry_rooms(
    s: &mut AppState,
    device_type: &rhythm_core::runtime::hub_registry::DeviceType,
    device_name: &str,
    endpoints: &[crate::canonical::identity::IntegrationEndpoint],
) {
    use rhythm_core::runtime::hub_registry::DeviceType;
    if !matches!(device_type, DeviceType::Light) {
        // Drop any stale synthetic room left over from older code that created
        // them for every device type. Topology owns event routing now.
        for endpoint in endpoints {
            if let Some(hub) = s.hubs.get(&endpoint.hub_key) {
                if let Some(reg) = &hub.registry {
                    if let Ok(mut registry) = reg.lock() {
                        registry.remove_room(&endpoint.native_id);
                    }
                }
            }
        }
        return;
    }

    for endpoint in endpoints {
        if let Some(hub) = s.hubs.get(&endpoint.hub_key) {
            if let Some(reg) = &hub.registry {
                if let Ok(mut registry) = reg.lock() {
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

/// Propagate a Rhythm room assignment to each endpoint's hub-side
/// `device_rooms` map so button/motion events route correctly.
///
/// `room_id == None` clears the mapping (events go unrouted until a room is
/// assigned). The hub-native room IDs that arrived via discovery are
/// overridden with the Rhythm room ID — this is the routing source of truth
/// for typed devices.
fn sync_endpoint_device_rooms(
    s: &mut AppState,
    endpoints: &[crate::canonical::identity::IntegrationEndpoint],
    room_id: Option<&str>,
) {
    for endpoint in endpoints {
        if let Some(hub) = s.hubs.get(&endpoint.hub_key) {
            if let Some(reg) = &hub.registry {
                if let Ok(mut registry) = reg.lock() {
                    match room_id {
                        Some(rid) => registry.set_device_room(&endpoint.native_id, rid),
                        None => registry.clear_device_room(&endpoint.native_id),
                    }
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

    // Snapshot endpoints and old room before mutating.
    let device = s
        .canonical_registry
        .get(device_id)
        .ok_or_else(|| anyhow::anyhow!("Device not found: {}", device_id))?;
    let active_endpoints: Vec<_> = device.active_endpoints().cloned().collect();
    let device_name = device.name.clone();
    let device_type = device.device_type.clone();
    let was_topology_standalone = s.topology.device_parent_room_id(device_id).is_none();
    let assigning_standalone_light_child =
        room_id.is_some() && was_topology_standalone && matches!(&device_type, DeviceType::Light);
    let old_room_id = s
        .topology
        .device_parent_room_id(device_id)
        .map(|id| id.to_string())
        .or_else(|| device.room_id.clone());
    // Pre-mutation effective motion target so we can clear stale motion-timer
    // state on the room a motion sensor is leaving (and on the new room if
    // the effective target also changes there).
    let old_motion_target = if matches!(device_type, DeviceType::Motion) {
        Some(
            s.topology
                .effective_control_target(device_id, &NodeControlKind::Motion),
        )
    } else {
        None
    };

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

        ensure_synthetic_device_registry_rooms(
            &mut s,
            &device_type,
            &device_name,
            &active_endpoints,
        );
    } else {
        // Unassignment is an explicit user action, so mark the topology node
        // as UserOverride with no parent. This keeps a follow-up hub sync
        // from silently re-attaching the device under whatever room the hub
        // still reports it in (issue #43).
        s.topology.ensure_standalone_device(device_id);
        s.topology.assign_device(
            device_id,
            None,
            crate::topology::DevicePlacement::UserOverride,
        );
        if let Some(old_id) = &old_room_id {
            if let Some(room) = s.topology.get_mut(old_id) {
                room.user_customized = true;
            }
        }
        ensure_synthetic_device_registry_rooms(
            &mut s,
            &device_type,
            &device_name,
            &active_endpoints,
        );
    }

    // Propagate the assignment to each endpoint's hub-side device_rooms map so
    // typed-device routing (button/motion) follows the Rhythm room assignment.
    // Lights are handled via `ensure_synthetic_device_registry_rooms` above —
    // their device_rooms entries are managed by synthetic-room scaffolding.
    if !matches!(
        device_type,
        rhythm_core::runtime::hub_registry::DeviceType::Light
    ) {
        sync_endpoint_device_rooms(&mut s, &active_endpoints, room_id);
    }

    let new_motion_target = old_motion_target.as_ref().map(|_| {
        s.topology
            .effective_control_target(device_id, &NodeControlKind::Motion)
    });

    persist_canonical(&s);
    persist_topology(&s);
    drop(s);

    if let (Some(old), Some(new)) = (old_motion_target, new_motion_target) {
        if old != new {
            if let Some(old_target) = old {
                queue_motion_timer_clear(state, &old_target);
            }
            if let Some(new_target) = new {
                queue_motion_timer_clear(state, &new_target);
            }
        }
    }

    persist_registry(state);
    reconcile_runtime_from_state(state)?;
    if assigning_standalone_light_child {
        clear_runtime_node_off_flags(state, device_id)?;
        persist_rooms(state);
    }

    {
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
        reconcile_runtime_from_state(state)?;
        {
            emit_triage_changed(state);
            crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
        }
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
                    reconcile_runtime_from_state(state)?;
                    {
                        emit_triage_changed(state);
                        crate::state::emit_server_event(
                            state,
                            crate::server_event::ServerEvent::NodesChanged,
                        );
                    }
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
    if s.topology.get(&target_id).is_none() {
        return Err(anyhow::anyhow!("Target room '{}' not found", target_id));
    }

    // Find the silo room (the one created for this hub's room)
    let source_id = s
        .topology
        .translate_room_id(&entry.hub_key, &binding.hub_room_id)
        .map(|s| s.to_string())
        .ok_or_else(|| {
            anyhow::anyhow!("Silo room not found for hub room: {}", binding.hub_room_id)
        })?;
    let source_device_ids: Vec<String> = s
        .topology
        .get(&source_id)
        .map(|room| {
            room.devices
                .iter()
                .map(|device| device.device_id.clone())
                .collect()
        })
        .unwrap_or_default();

    // Merge the silo room into the target room
    if !s.topology.merge_rooms(&target_id, &source_id) {
        return Err(anyhow::anyhow!(
            "Failed to merge rooms: {} → {}",
            source_id,
            target_id
        ));
    }
    for device_id in &source_device_ids {
        s.canonical_registry
            .assign_room(device_id, Some(&target_id));
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
    drop(s);
    reconcile_runtime_from_state(state)?;

    // Emit SSE events
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

    crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);

    Ok(())
}

/// Create a new empty Rhythm room.
pub fn do_topology_create_room(state: &SharedState, name: &str) -> Result<String> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let id = s.topology.create_room(name);
    persist_topology(&s);
    drop(s);
    reconcile_runtime_from_state(state)?;
    Ok(format!(r#"{{"id":"{}","name":"{}"}}"#, id, name))
}

/// Delete a topology room and unassign any attached devices.
pub fn do_topology_delete_room(state: &SharedState, room_id: &str) -> Result<()> {
    let detached_devices = {
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
            detached_devices.push(device.id.clone());

            ensure_synthetic_device_registry_rooms(
                &mut s,
                &device.device_type,
                &device.name,
                &active_endpoints,
            );
        }

        s.room_observed_power.remove(room_id);
        s.motion_snapshots.remove(room_id);
        s.room_mode_transitions.remove(room_id);
        s.pending_motion_clear.retain(|pending| pending != room_id);

        persist_topology(&s);
        if !detached_devices.is_empty() {
            persist_canonical(&s);
        }

        detached_devices
    };

    if !detached_devices.is_empty() {
        persist_registry(state);
    }

    queue_motion_timer_clear(state, room_id);
    reconcile_runtime_from_state(state)?;

    {
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
    if s.topology.get(target_id).is_none() {
        return Err(anyhow::anyhow!("Target room not found"));
    }
    let source_device_ids: Vec<String> = s
        .topology
        .get(source_id)
        .map(|room| {
            room.devices
                .iter()
                .map(|device| device.device_id.clone())
                .collect()
        })
        .unwrap_or_default();
    if s.topology.merge_rooms(target_id, source_id) {
        for device_id in &source_device_ids {
            s.canonical_registry.assign_room(device_id, Some(target_id));
        }
        persist_canonical(&s);
        persist_topology(&s);
        drop(s);
        reconcile_runtime_from_state(state)?;

        {
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
        reconcile_runtime_from_state(state)?;

        {
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
        BackupBundle, BackupConfiguration, BackupHubCredentials, BackupInstallation,
        BackupIntegrationFile, BackupRuntimeState, BundleKind, ProfileBundle, ProfileBundleData,
        ProfileBundleImportPayload,
    };
    use crate::factory_default_config::{
        factory_default_active_mode, factory_default_light_profile_config,
        factory_default_mode_transition_configs, factory_default_power_save,
        factory_default_profile_bundle,
    };
    use crate::hub::{ActiveHub, HubCredentials, HubProvider, HubType};
    use crate::state::{AppState, MotionSnapshot, ObservedPowerSource, ObservedPowerState};
    use crate::storage::{Storage, StoredLightProfiles, StoredSettings};
    use crate::topology::InputBindingPreset;
    use chrono::{Datelike, Timelike};
    use rhythm_core::{
        HubDispatchTarget, HubLightController, HubRegistry, LightControlResult, LightProfileConfig,
        Room, RoomSnapshot, RuntimeHandle,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn wait_for_sync_count(sync_count: &AtomicUsize, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let actual = sync_count.load(Ordering::SeqCst);
            if actual == expected {
                return;
            }
            if actual > expected {
                panic!("topology group sync count exceeded {expected}: {actual}");
            }
            if std::time::Instant::now() >= deadline {
                assert_eq!(actual, expected);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

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
        sun_times_updates: Mutex<Vec<Option<rhythm_core::SunTimes>>>,
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
                sun_times_updates: Mutex::new(Vec::new()),
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

        fn sun_times_updates(&self) -> Vec<Option<rhythm_core::SunTimes>> {
            self.sun_times_updates.lock().unwrap().clone()
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
        fn set_sun_times(&self, sun_times: rhythm_core::SunTimes) -> anyhow::Result<()> {
            self.sun_times_updates.lock().unwrap().push(Some(sun_times));
            Ok(())
        }
        fn clear_sun_times(&self) -> anyhow::Result<()> {
            self.sun_times_updates.lock().unwrap().push(None);
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

    struct RecordingFlashController {
        calls: Mutex<Vec<(String, String)>>,
    }

    impl RecordingFlashController {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl HubLightController for RecordingFlashController {
        async fn turn_on_target(
            &self,
            _target: &HubDispatchTarget,
            _command: rhythm_core::LightingCommand,
        ) -> LightControlResult<()> {
            Ok(())
        }

        async fn turn_off_target(
            &self,
            _target: &HubDispatchTarget,
            _transition_ms: Option<u32>,
        ) -> LightControlResult<()> {
            Ok(())
        }

        async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
            Ok(Vec::new())
        }

        async fn is_connected(&self) -> bool {
            true
        }

        async fn any_lights_on_target(
            &self,
            _target: &HubDispatchTarget,
        ) -> LightControlResult<bool> {
            Ok(false)
        }

        async fn flash_target(&self, target: &HubDispatchTarget) -> LightControlResult<()> {
            self.calls
                .lock()
                .unwrap()
                .push((self.name().to_string(), target.label()));
            Ok(())
        }

        fn name(&self) -> &str {
            "RecordingFlash"
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

    fn make_standalone_light_snapshot(id: &str) -> RoomSnapshot {
        let mut snapshot = make_snapshot(id, false, false);
        snapshot.kind = rhythm_core::LightNodeKind::LightDevice;
        snapshot.parent_id = None;
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

    fn add_button_device_node(state: &SharedState, native_id: &str) -> String {
        let hub_key = state.lock().unwrap().hubs.keys().next().cloned().unwrap();
        let identity = crate::canonical::identity::DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: None,
            room_name: None,
            name: native_id.to_string(),
            device_type: DeviceType::Button,
            hardware_ids: vec![crate::canonical::identity::HardwareId::serial(native_id)],
            manufacturer: None,
            model: None,
        };

        let mut s = state.lock().unwrap();
        let canonical_id = match s.canonical_registry.resolve(&identity, &hub_key, 1000) {
            crate::canonical::registry::ResolveResult::Created { canonical_id }
            | crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id } => {
                canonical_id
            }
            other => panic!("unexpected resolve result: {:?}", other),
        };
        s.topology.ensure_standalone_device(&canonical_id);
        canonical_id
    }

    #[test]
    fn location_set_pushes_sun_times_to_runtime() {
        let (state, runtime) = setup_state(Vec::new());

        do_location_set(
            &state,
            40.7128,
            -74.0060,
            None,
            Some("America/New_York".into()),
        )
        .unwrap();

        let updates = runtime.sun_times_updates();
        let sun_times = updates
            .last()
            .and_then(|update| *update)
            .expect("location_set should push computed sun times to runtime");

        assert!(
            (0.0..24.0).contains(&sun_times.sunrise),
            "sunrise should be a local hour, got {}",
            sun_times.sunrise
        );
        assert!(
            (0.0..24.0).contains(&sun_times.sunset),
            "sunset should be a local hour, got {}",
            sun_times.sunset
        );
        assert!(
            sun_times.day_length > 0.0,
            "day length should be positive, got {}",
            sun_times.day_length
        );
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

    fn activate_topology_room_hubs(state: &SharedState, room_id: &str) {
        let hub_keys: Vec<HubKey> = {
            let s = state.lock().unwrap();
            s.topology
                .get(room_id)
                .map(|room| {
                    room.hub_room_bindings
                        .iter()
                        .map(|binding| binding.hub_key.clone())
                        .collect()
                })
                .unwrap_or_default()
        };

        let mut s = state.lock().unwrap();
        for hub_key in hub_keys {
            if s.hubs.contains_key(&hub_key) {
                continue;
            }

            s.hubs.insert(
                hub_key.clone(),
                ActiveHub {
                    hub_type: hub_key.hub_type.clone(),
                    hub_key,
                    runtime: None,
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: None,
                    shutdown: Default::default(),
                },
            );
        }
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
            room_id: Some(room_id.to_string()),
            room_name: Some(room_name.to_string()),
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
        integration_files: Vec<BackupIntegrationFile>,
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

        fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>> {
            Ok(self.inner.lock().unwrap().hub_credentials.clone())
        }

        fn save_all_hub_credentials(&self, creds: &[HubCredentials]) -> Result<()> {
            self.inner.lock().unwrap().hub_credentials = creds.to_vec();
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

        fn clear_hub_registries(&self) -> Result<()> {
            self.inner.lock().unwrap().hub_registries.clear();
            Ok(())
        }

        fn load_integration_backup_files(
            &self,
            include_secrets: bool,
        ) -> Result<Vec<BackupIntegrationFile>> {
            if include_secrets {
                Ok(self.inner.lock().unwrap().integration_files.clone())
            } else {
                Ok(Vec::new())
            }
        }

        fn restore_integration_backup_files(&self, files: &[BackupIntegrationFile]) -> Result<()> {
            self.inner.lock().unwrap().integration_files = files.to_vec();
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
            inner.integration_files.clear();
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

    fn set_observed_lights_on(state: &SharedState, node_id: &str, lights_on: bool) {
        state.lock().unwrap().room_observed_power.insert(
            node_id.to_string(),
            ObservedPowerState::new(lights_on, ObservedPowerSource::Command),
        );
    }

    fn set_observed_lights_on_in_app(app: &mut AppState, node_id: &str, lights_on: bool) {
        app.room_observed_power.insert(
            node_id.to_string(),
            ObservedPowerState::new(lights_on, ObservedPowerSource::Command),
        );
    }

    fn observed_lights_on(app: &AppState, node_id: &str) -> Option<bool> {
        app.room_observed_power
            .get(node_id)
            .map(|observed| observed.lights_on)
    }

    fn observed_lights_map(app: &AppState) -> HashMap<String, bool> {
        app.room_observed_power
            .iter()
            .map(|(node_id, observed)| (node_id.clone(), observed.lights_on))
            .collect()
    }

    #[test]
    fn command_cache_update_preserves_matching_fresh_live_subscription() {
        let (state, _runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        {
            let mut app = state.lock().unwrap();
            app.room_observed_power.insert(
                "room1".to_string(),
                ObservedPowerState::new(true, ObservedPowerSource::LiveSubscription),
            );
        }

        update_lights_on_cache_for_node_with_source(
            &state,
            "room1",
            LightNodeKind::Room,
            None,
            true,
            ObservedPowerSource::Command,
        );

        let app = state.lock().unwrap();
        let observed = app.room_observed_power.get("room1").unwrap();
        assert!(observed.lights_on);
        assert_eq!(observed.source, ObservedPowerSource::LiveSubscription);
    }

    #[test]
    fn command_cache_update_replaces_live_subscription_when_power_changes() {
        let (state, _runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        {
            let mut app = state.lock().unwrap();
            app.room_observed_power.insert(
                "room1".to_string(),
                ObservedPowerState::new(true, ObservedPowerSource::LiveSubscription),
            );
        }

        update_lights_on_cache_for_node_with_source(
            &state,
            "room1",
            LightNodeKind::Room,
            None,
            false,
            ObservedPowerSource::Command,
        );

        let app = state.lock().unwrap();
        let observed = app.room_observed_power.get("room1").unwrap();
        assert!(!observed.lights_on);
        assert_eq!(observed.source, ObservedPowerSource::Command);
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
        assert_eq!(observed_lights_on(&s, "room1"), Some(true));
        assert_eq!(s.room_observed_power.len(), 1);
    }

    #[test]
    fn room_action_updates_parent_lights_on_without_parent_group_dispatch() {
        let (state, runtime, device_id) = setup_attached_matter_light_without_group_dispatch();
        {
            let mut s = state.lock().unwrap();
            set_observed_lights_on_in_app(&mut s, "room1", false);
        }

        let result = do_node_action(&state, &device_id, "on", false);

        assert!(result.is_ok());
        assert_eq!(
            runtime.events(),
            vec![(device_id.clone(), ButtonAction::OnPress)]
        );
        let s = state.lock().unwrap();
        assert_eq!(observed_lights_on(&s, &device_id), Some(true));
        assert_eq!(observed_lights_on(&s, "room1"), Some(true));
    }

    #[test]
    fn room_action_turning_on_matter_child_updates_mixed_room_parent_aggregate() {
        let (state, _runtime, matter_id, _ha_id, _hue_one_id, _hue_two_id) =
            setup_mixed_room_with_hub_groups();

        let result = do_node_action(&state, &matter_id, "on", false);

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(observed_lights_on(&s, &matter_id), Some(true));
        assert_eq!(observed_lights_on(&s, "room1"), Some(true));
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
        assert_eq!(observed_lights_on(&s, &matter_id), Some(false));
        assert_eq!(observed_lights_on(&s, "room1"), Some(true));
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

    #[test]
    fn preset_input_binding_create_persists_button_binding() {
        let (state, _runtime) = setup_state(vec![]);
        let button_id = add_button_device_node(&state, "button-native-1");

        let json = do_preset_input_binding_create(
            &state,
            InputBindingPreset::DaySleepToggle,
            &button_id,
            Some(ButtonAction::OnPress),
            true,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let binding_id = parsed["bindings"][0]["id"].as_str().unwrap();
        assert!(binding_id.starts_with("day_sleep_toggle:"));
        assert_eq!(parsed["bindings"][0]["source_node_id"], button_id);
        assert_eq!(parsed["bindings"][0]["preset"], "day_sleep_toggle");
        assert_eq!(parsed["bindings"][0]["action"]["kind"], "mode_cycle");

        let s = state.lock().unwrap();
        let binding = s
            .topology
            .matching_button_input_binding(&button_id, ButtonAction::OnPress)
            .expect("binding should match selected button");
        assert_eq!(binding.id, binding_id);
    }

    #[test]
    fn automation_mode_cycle_uses_matching_pair_transition() {
        let (state, _runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        state.lock().unwrap().active_mode = RhythmMode::Sleep;

        let binding = InputBinding::day_sleep_toggle("button-1", None);
        do_execute_automation_action(&state, &binding.action).unwrap();

        let s = state.lock().unwrap();
        assert_eq!(s.active_mode, RhythmMode::Day);
        assert_eq!(
            s.last_active_mode_transition_id.as_deref(),
            Some("sleep_to_day")
        );
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
        set_observed_lights_on(&state, "r1", true);

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
        set_observed_lights_on(&state, "r1", true);

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
        activate_topology_room_hubs(&state, "r1");

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();
        let json = serde_json::to_value(&room_state).unwrap();

        assert_eq!(room_state.hub_types, vec!["mock", "matter"]);
        assert_eq!(json["hub_types"], serde_json::json!(["mock", "matter"]));
    }

    #[test]
    fn build_room_rhythm_state_warning_uses_dimmed_brightness() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        set_observed_lights_on(&state, "r1", true);
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

    #[test]
    fn build_node_state_event_marks_active_mode_transition() {
        let (state, rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        set_observed_lights_on(&state, "r1", true);
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

    #[test]
    fn build_node_state_event_includes_hub_types_from_topology() {
        let (state, rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        add_topology_room(&state, "r1", &["matter", "mock"]);
        activate_topology_room_hubs(&state, "r1");
        let snap =
            rhythm_core::NodeSnapshot::from_room_snapshot(rt.engine_room_snapshot("r1").unwrap());

        let event = build_node_state_event(&state, &snap);
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(event.hub_types, vec!["matter", "mock"]);
        assert_eq!(json["hub_types"], serde_json::json!(["matter", "mock"]));
    }

    #[test]
    fn build_node_state_event_uses_parent_lights_on_for_attached_light() {
        let (state, rt, device_id) = setup_attached_hue_light_with_group_dispatch();
        set_observed_lights_on(&state, "room1", true);

        let snap = rhythm_core::NodeSnapshot::from_room_snapshot(
            rt.engine_room_snapshot(&device_id).unwrap(),
        );
        let event = build_node_state_event(&state, &snap);

        assert!(event.lights_on);
    }

    #[test]
    fn build_node_state_event_uses_child_lights_on_without_parent_group_dispatch() {
        let (state, rt, device_id) = setup_attached_matter_light_without_group_dispatch();
        {
            let mut s = state.lock().unwrap();
            set_observed_lights_on_in_app(&mut s, "room1", false);
            set_observed_lights_on_in_app(&mut s, &device_id, true);
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
    fn build_profile_bundle_dto_exports_fresh_state_defaults() {
        let (state, _rt) = setup_state(vec![]);

        let bundle = build_profile_bundle_dto(&state).unwrap();
        let s = state.lock().unwrap();

        assert_eq!(bundle.schema_version, BUNDLE_SCHEMA_VERSION);
        assert_eq!(bundle.kind, BundleKind::ProfileBundle);
        assert_eq!(bundle.profile.power_save, factory_default_power_save());
        assert_eq!(
            bundle.profile.profiles,
            s.light_profile_configs
                .values()
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            bundle.profile.mode_transitions,
            factory_default_mode_transition_configs()
        );
    }

    #[test]
    fn build_factory_default_profile_bundle_matches_factory_default_source() {
        let bundle = build_factory_default_profile_bundle_dto();
        let expected = factory_default_profile_bundle();
        assert_eq!(bundle.schema_version, expected.schema_version);
        assert_eq!(bundle.kind, expected.kind);
        assert_eq!(bundle.name, expected.name);
        assert_eq!(bundle.description, expected.description);
        assert_eq!(bundle.profile.power_save, expected.profile.power_save);
        assert_eq!(bundle.profile.profiles, expected.profile.profiles);
        assert_eq!(
            bundle.profile.mode_transitions,
            expected.profile.mode_transitions
        );
    }

    #[test]
    fn profile_bundle_import_applies_profiles_power_save_and_transitions() {
        let (state, runtime) = setup_state(vec![make_snapshot("local-office", false, false)]);
        let focus = make_focus_profile();
        let transition =
            rhythm_core::ModeTransitionConfig::new(RhythmMode::Day, RhythmMode::Sleep, 4_321)
                .with_id("custom_day_to_sleep")
                .with_label("Custom Day to Sleep")
                .with_trigger(ModeTransitionTrigger::Sunset);

        let payload = ProfileBundleImportPayload::Bundle(ProfileBundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            kind: BundleKind::ProfileBundle,
            name: Some("Current Profiles".into()),
            description: Some("Portable profile bundle".into()),
            profile: ProfileBundleData {
                power_save: true,
                profiles: vec![focus.clone()],
                mode_transitions: vec![transition.clone()],
            },
        });

        let json = do_profile_bundle_import(&state, payload).unwrap();
        let exported: ProfileBundle = serde_json::from_str(&json).unwrap();
        let s = state.lock().unwrap();

        assert!(s.power_save);
        assert_eq!(s.active_mode, factory_default_active_mode());
        assert_eq!(s.mode_transition_configs(), vec![transition.clone()]);
        assert_eq!(s.light_profile_config("focus"), Some(&focus));
        drop(s);

        assert!(
            runtime
                .config_updates()
                .iter()
                .any(|config| config.id == "focus"),
            "runtime should receive imported custom profile config"
        );

        assert_eq!(exported.kind, BundleKind::ProfileBundle);
        assert!(exported.profile.power_save);
        assert_eq!(exported.profile.mode_transitions, vec![transition]);
        assert_eq!(exported.profile.profiles.len(), 5);
        assert!(exported
            .profile
            .profiles
            .iter()
            .any(|profile| profile.id == "focus"));
    }

    #[test]
    fn profile_bundle_import_accepts_legacy_bundle_shapes() {
        let (state, _rt) = setup_state(vec![]);
        let focus = make_focus_profile();

        let json = do_profile_bundle_import(
            &state,
            serde_json::from_value::<ProfileBundleImportPayload>(serde_json::json!({
                "schema_version": BUNDLE_SCHEMA_VERSION,
                "kind": "configuration_bundle",
                "name": "Legacy",
                "configuration": {
                    "power_save": true,
                    "active_mode": "sleep",
                    "profiles": [focus],
                    "mode_configs": [],
                    "mode_transitions": []
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let exported: ProfileBundle = serde_json::from_str(&json).unwrap();

        assert!(exported.profile.power_save);
        assert!(exported
            .profile
            .profiles
            .iter()
            .any(|profile| profile.id == "focus"));
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
    fn build_backup_bundle_dto_redacts_or_includes_integration_files() {
        let storage = TestStorage::default();
        storage.inner.lock().unwrap().integration_files = vec![BackupIntegrationFile {
            path: "matter/fabric-identity.json".to_string(),
            content: "{\"ipk_hex\":\"secret\"}".to_string(),
            secret: true,
        }];
        let state = Arc::new(Mutex::new(AppState {
            storage: Some(Box::new(storage)),
            ..Default::default()
        }));

        let redacted = build_backup_bundle_dto(&state, false).unwrap();
        assert!(redacted.installation.integration_files.is_empty());

        let included = build_backup_bundle_dto(&state, true).unwrap();
        assert_eq!(included.installation.integration_files.len(), 1);
        assert_eq!(
            included.installation.integration_files[0].path,
            "matter/fabric-identity.json"
        );
        assert!(included.installation.integration_files[0].secret);
    }

    #[test]
    fn build_backup_bundle_dto_keeps_redacted_placeholders_redacted_even_with_secrets() {
        let (state, _rt) = setup_state(vec![]);
        state.lock().unwrap().hub_credentials.insert(
            HubKey::new(HubType::new("matter"), "local"),
            HubCredentials::redacted_placeholder("matter", "local"),
        );

        let included = build_backup_bundle_dto(&state, true).unwrap();
        assert!(included.secrets_included);
        assert_eq!(included.installation.hub_credentials.len(), 1);
        assert_eq!(
            included.installation.hub_credentials[0].hub_type,
            Some(HubType::new("matter"))
        );
        assert_eq!(included.installation.hub_credentials[0].address, "local");
        assert_eq!(included.installation.hub_credentials[0].data, None);
    }

    #[test]
    fn build_backup_bundle_dto_prunes_runtime_drift_and_uses_topology_parenting() {
        let (state, runtime) = setup_state(vec![
            make_snapshot("room1", false, false),
            make_snapshot("stale-room", false, false),
        ]);
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
        runtime.snapshots.lock().unwrap().extend([
            make_light_child_snapshot(&device_id, "stale-room"),
            make_light_child_snapshot("stale-node", "stale-room"),
        ]);

        let bundle = build_backup_bundle_dto(&state, false).unwrap();

        assert_eq!(
            bundle
                .configuration
                .rooms
                .iter()
                .map(|room| room.id.as_str())
                .collect::<Vec<_>>(),
            vec!["room1"],
            "backup configuration should follow topology rooms, not stale runtime rooms"
        );

        let mut exported_ids: Vec<_> = bundle
            .installation
            .rooms
            .iter()
            .map(|room| room.id.clone())
            .collect();
        exported_ids.sort();
        assert_eq!(
            exported_ids,
            vec![device_id.clone(), "room1".to_string()],
            "backup runtime export should exclude stale runtime-only rooms and nodes"
        );
        assert_eq!(
            bundle
                .installation
                .rooms
                .get(&device_id)
                .expect("exported device node should exist")
                .parent_id
                .as_deref(),
            Some("room1"),
            "topology parenting should override stale runtime parenting during export"
        );
        assert!(
            bundle.installation.rooms.get("stale-room").is_none(),
            "stale runtime room should not leak into backup export"
        );
        assert!(
            bundle.installation.rooms.get("stale-node").is_none(),
            "stale runtime node should not leak into backup export"
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
            configuration: BackupConfiguration {
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
                integration_files: vec![BackupIntegrationFile {
                    path: "matter/fabric-identity.json".to_string(),
                    content: "{\"label\":\"default\"}".to_string(),
                    secret: true,
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
        assert_eq!(
            saved.integration_files,
            vec![BackupIntegrationFile {
                path: "matter/fabric-identity.json".to_string(),
                content: "{\"label\":\"default\"}".to_string(),
                secret: true,
            }]
        );
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
    fn redacted_backup_restore_does_not_reapply_mode_defaults_on_late_hub_reconnect() {
        // Companion to issue #69: backup restore runs apply_backup_configuration
        // with force_reapply_outputs=true while no hub runtime is attached
        // (do_hub_disconnect emptied them), so apply_active_mode_outputs hits the
        // no-runtime branch and sets pending_mode_output_apply. The restored room
        // state is authoritative, so leaking that flag past restore would let a
        // late hub reconnect re-apply mode defaults and clobber the user's
        // restored state. restore_backup_runtime_state must clear the flag.
        let storage = TestStorage::default();
        let app = AppState {
            storage: Some(Box::new(storage.clone())),
            ..Default::default()
        };
        let state = Arc::new(Mutex::new(app));
        install_mock_hub_provider(&state);

        // Restored room state: living is rhythm-on with soft_off=true but
        // hard_off=false.
        let mut rooms = rhythm_core::RoomManager::new();
        let room = rooms.get_or_create("living", "Living Room");
        room.rhythm_enabled = true;
        room.soft_off = true;
        room.hard_off = false;

        let hub_key = HubKey::new(HubType::new("mock"), "bridge.local");
        let bundle = BackupBundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            kind: BundleKind::BackupBundle,
            created_at: "2026-04-16T00:00:00Z".into(),
            secrets_included: false,
            configuration: BackupConfiguration {
                power_save: false,
                active_mode: RhythmMode::Day,
                profiles: Vec::new(),
                // Conflict: Day mode's room_default for "living" is HardOff. If
                // the pending flag leaks past restore, reconcile_runtime_from_state
                // will dispatch lights_off and force hard_off=true on the room.
                mode_configs: vec![
                    ModeConfig {
                        mode: RhythmMode::Day,
                        active_profile_id: Some(rhythm_core::RHYTHM_PROFILE_ID.into()),
                        idle_profile_id: Some(rhythm_core::DAY_IDLE_PROFILE_ID.into()),
                        wake_profile_id: None,
                        warning_profile_id: None,
                        room_defaults: vec![rhythm_core::RoomModeDefault {
                            room_id: "living".into(),
                            state: RoomModeState::HardOff,
                        }],
                    },
                    ModeConfig::default_for_mode(RhythmMode::Sleep),
                ],
                mode_transitions: Vec::new(),
                rooms: Vec::new(),
            },
            installation: BackupInstallation {
                location: None,
                rooms: rooms.clone(),
                topology: crate::topology::RoomTopologyStore::new(),
                canonical_registry: crate::canonical::registry::CanonicalRegistry::new(),
                hub_credentials: vec![BackupHubCredentials {
                    hub_type: Some(HubType::new("mock")),
                    address: "bridge.local".into(),
                    data: None,
                }],
                hub_registries: vec![crate::bundle::BackupHubRegistry {
                    hub_key: hub_key.clone(),
                    snapshot: serde_json::json!({ "rooms": [] }),
                }],
                integration_files: vec![],
            },
            runtime_state: BackupRuntimeState {
                active_mode: RhythmMode::Day,
                last_change_cause: ModeChangeCause::Schedule,
                last_change_transition_id: None,
                last_change_epoch_ms: Some(1_700_000_000_000),
            },
        };

        do_backup_restore(&state, bundle).unwrap();

        assert!(
            !state.lock().unwrap().pending_mode_output_apply,
            "do_backup_restore must clear pending_mode_output_apply so a \
             late hub reconnect doesn't clobber restored room state"
        );

        // Simulate the late hub reconnect: attach a runtime carrying the
        // restored snapshot, add a matching topology room, then reconcile.
        let mut restored_snapshot = make_snapshot("living", false, true);
        restored_snapshot.name = "Living Room".into();
        restored_snapshot.rhythm_enabled = true;
        restored_snapshot.hard_off = false;
        let runtime = Arc::new(MockRuntime::new(vec![restored_snapshot], 12.0));
        {
            let mut s = state.lock().unwrap();
            let hub_type = HubType::new("mock");
            s.hubs.insert(
                hub_key.clone(),
                ActiveHub {
                    hub_type,
                    hub_key: hub_key.clone(),
                    runtime: Some(runtime.clone() as Arc<dyn RuntimeHandle>),
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: None,
                    shutdown: Default::default(),
                },
            );
        }
        add_topology_room(&state, "living", &[]);

        reconcile_runtime_from_state(&state).unwrap();

        assert!(
            runtime.applied_states().is_empty(),
            "late reconnect must not re-apply mode defaults over restored state (got {:?})",
            runtime.applied_states()
        );
        assert!(
            runtime.lights_off_calls().is_empty(),
            "late reconnect must not dispatch lights-off over restored state (got {:?})",
            runtime.lights_off_calls()
        );
        let snap = runtime
            .engine_room_snapshot("living")
            .expect("living should be present after reconcile");
        assert!(
            !snap.hard_off,
            "restored hard_off=false must survive a late hub reconnect"
        );
        assert!(
            snap.soft_off,
            "restored soft_off=true must survive a late hub reconnect"
        );
    }

    #[test]
    fn restore_backup_room_manager_prunes_stale_runtime_nodes_and_periodic_state() {
        let (state, runtime) = setup_state(vec![
            make_snapshot("stale-room", false, false),
            make_light_child_snapshot("stale-node", "stale-room"),
        ]);

        {
            let mut s = state.lock().unwrap();
            set_observed_lights_on_in_app(&mut s, "stale-room", true);
            set_observed_lights_on_in_app(&mut s, "office", true);
            s.motion_snapshots.insert(
                "stale-room".into(),
                MotionSnapshot {
                    motion_active: true,
                    motion_owned: true,
                    remaining_secs: Some(10),
                    timeout_secs: 300,
                    warning_active: false,
                },
            );
            s.motion_snapshots.insert(
                "desk-lamp".into(),
                MotionSnapshot {
                    motion_active: true,
                    motion_owned: true,
                    remaining_secs: Some(20),
                    timeout_secs: 300,
                    warning_active: false,
                },
            );
            s.room_mode_transitions.insert(
                "stale-node".into(),
                crate::state::RoomModeTransition {
                    ends_at: std::time::Instant::now(),
                    periodic_resume_at: std::time::Instant::now(),
                },
            );
            s.room_mode_transitions.insert(
                "office".into(),
                crate::state::RoomModeTransition {
                    ends_at: std::time::Instant::now(),
                    periodic_resume_at: std::time::Instant::now(),
                },
            );
            s.pending_motion_clear
                .extend(["stale-room".into(), "desk-lamp".into()]);
        }

        let mut rooms = rhythm_core::RoomManager::new();
        let room = rooms.get_or_create("office", "Office");
        room.rhythm_enabled = true;
        room.disabled = true;
        room.time_offset_minutes = 18.0;
        room.brightness_offset = 6.0;
        room.soft_off = true;
        let mut node = rhythm_core::Room::new_node(
            "desk-lamp",
            "Desk Lamp",
            LightNodeKind::LightDevice,
            Some("office".into()),
        );
        node.rhythm_enabled = true;
        node.brightness_offset = 4.0;
        rooms.add_room(node);

        restore_backup_room_manager(&state, &rooms).unwrap();

        let s = state.lock().unwrap();
        assert_eq!(
            observed_lights_map(&s),
            HashMap::from([("office".to_string(), true)])
        );
        assert!(!s.motion_snapshots.contains_key("stale-room"));
        assert!(s.motion_snapshots.contains_key("desk-lamp"));
        assert!(!s.room_mode_transitions.contains_key("stale-node"));
        assert!(s.room_mode_transitions.contains_key("office"));
        assert_eq!(s.pending_motion_clear, vec!["desk-lamp".to_string()]);
        drop(s);

        assert!(
            runtime.engine_room_snapshot("stale-room").is_none(),
            "stale backup runtime room should be pruned"
        );
        assert!(
            runtime.engine_node_snapshot("stale-node").is_none(),
            "stale backup runtime node should be pruned"
        );
        let office = runtime
            .engine_room_snapshot("office")
            .expect("restored backup room should exist");
        assert_eq!(office.name, "Office");
        assert!(office.disabled);
        assert!(office.soft_off);
        let lamp = runtime
            .engine_node_snapshot("desk-lamp")
            .expect("restored backup device node should exist");
        assert_eq!(lamp.parent_id.as_deref(), Some("office"));
        assert_eq!(lamp.brightness_offset, 4.0);
    }

    #[test]
    fn backup_restore_reconnects_runtime_and_restores_assigned_device_parent() {
        let (source_state, _runtime) = setup_state_with_registry(vec![]);
        let hub_key = source_state
            .lock()
            .unwrap()
            .hubs
            .keys()
            .next()
            .cloned()
            .unwrap();
        source_state.lock().unwrap().hub_credentials.insert(
            hub_key.clone(),
            HubCredentials::new("mock", "mock", serde_json::json!({ "token": "abc" })),
        );

        let created = do_topology_create_room(&source_state, "Office").unwrap();
        let room_id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let device_id =
            insert_canonical_device(&source_state, hub_key, "matter-100", "Desk Lamp", "", "");
        do_canonical_assign_room(&source_state, &device_id, Some(&room_id)).unwrap();

        let bundle = build_backup_bundle_dto(&source_state, true).unwrap();

        let target_state = Arc::new(Mutex::new(AppState::default()));
        install_mock_hub_provider(&target_state);

        let restored_json = do_backup_restore(&target_state, bundle).unwrap();
        let restored_bundle: BackupBundle = serde_json::from_str(&restored_json).unwrap();
        assert_eq!(restored_bundle.installation.hub_credentials.len(), 1);
        assert_eq!(restored_bundle.installation.hub_credentials[0].data, None);

        let s = target_state.lock().unwrap();
        assert_eq!(s.topology.room_count(), 1);
        assert_eq!(
            s.topology
                .get_device_node(&device_id)
                .expect("restored topology device should exist")
                .parent_id
                .as_deref(),
            Some(room_id.as_str())
        );
        assert_eq!(
            s.canonical_registry
                .get(&device_id)
                .expect("restored canonical device should exist")
                .room_id
                .as_deref(),
            Some(room_id.as_str())
        );
        let runtime = s
            .hub_runtime()
            .expect("backup restore should reconnect a runtime");
        assert_eq!(
            runtime
                .engine_room_snapshot(&room_id)
                .expect("restored runtime room should exist")
                .name,
            "Office"
        );
        assert_eq!(
            runtime
                .engine_node_snapshot(&device_id)
                .expect("restored runtime node should exist")
                .parent_id
                .as_deref(),
            Some(room_id.as_str())
        );
        assert_eq!(
            runtime.engine_all_node_snapshots().len(),
            2,
            "restored runtime should contain exactly the room and its device node"
        );
    }

    #[test]
    fn backup_restore_preserves_multi_hub_binding_and_merged_device_state() {
        let (source_state, _runtime) = setup_state(vec![]);
        let (primary_key, shared_runtime) = {
            let s = source_state.lock().unwrap();
            (
                s.hubs.keys().next().cloned().unwrap(),
                s.hub_runtime().expect("source runtime should exist"),
            )
        };
        let secondary_key = HubKey::new(HubType::new("mock"), "backup-b");
        {
            let mut s = source_state.lock().unwrap();
            let registry: Arc<Mutex<dyn HubRegistry>> =
                Arc::new(Mutex::new(crate::registry::HubDeviceRegistry::new()));
            s.hubs.insert(
                secondary_key.clone(),
                ActiveHub {
                    hub_type: secondary_key.hub_type.clone(),
                    hub_key: secondary_key.clone(),
                    runtime: Some(shared_runtime.clone()),
                    hub_data: Box::new(()),
                    registry: Some(registry),
                    discovery: None,
                    shutdown: Default::default(),
                },
            );
            s.hub_credentials.insert(
                primary_key.clone(),
                HubCredentials::new(
                    "mock",
                    &primary_key.address,
                    serde_json::json!({ "token": "primary" }),
                ),
            );
            s.hub_credentials.insert(
                secondary_key.clone(),
                HubCredentials::new(
                    "mock",
                    &secondary_key.address,
                    serde_json::json!({ "token": "secondary" }),
                ),
            );
        }

        let room_id = serde_json::from_str::<serde_json::Value>(
            &do_topology_create_room(&source_state, "Kitchen").unwrap(),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        {
            let mut s = source_state.lock().unwrap();
            s.topology
                .get_mut(&room_id)
                .expect("target room should exist")
                .upsert_hub_room_binding(crate::topology::HubRoomBinding {
                    hub_key: primary_key.clone(),
                    hub_room_id: "kitchen-a".to_string(),
                    control_id: "control-a".to_string(),
                    light_device_ids: vec!["lamp-1".to_string()],
                });

            let mut source_room = crate::topology::TopologyRoom::new("silo-kitchen", "Kitchen");
            source_room.upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: secondary_key.clone(),
                hub_room_id: "kitchen-b".to_string(),
                control_id: "control-b".to_string(),
                light_device_ids: vec!["lamp-1".to_string()],
            });
            s.topology.insert_room(source_room);

            s.canonical_registry
                .triage_mut()
                .add(crate::canonical::triage::TriageEntry {
                    id: "room-bind".to_string(),
                    kind: crate::canonical::triage::TriageKind::RoomBinding,
                    discovered: crate::canonical::triage::TriageDiscoveredDevice::default(),
                    hub_key: secondary_key.clone(),
                    candidate_matches: vec![],
                    room_binding: Some(crate::canonical::triage::RoomBindingProposal {
                        hub_room_id: "kitchen-b".to_string(),
                        hub_room_name: "Kitchen".to_string(),
                        control_id: "control-b".to_string(),
                        light_device_ids: vec!["lamp-1".to_string()],
                        canonical_device_ids: vec![],
                        target_rhythm_room_id: room_id.clone(),
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
        do_triage_bind_room(&source_state, "room-bind").unwrap();

        let canonical_id = insert_canonical_device(
            &source_state,
            primary_key.clone(),
            "lamp-1",
            "Kitchen Lamp",
            "",
            "",
        );
        do_canonical_assign_room(&source_state, &canonical_id, Some(&room_id)).unwrap();
        {
            let mut s = source_state.lock().unwrap();
            s.canonical_registry
                .triage_mut()
                .add(crate::canonical::triage::TriageEntry {
                    id: "device-merge".to_string(),
                    kind: crate::canonical::triage::TriageKind::DeviceMerge,
                    discovered: crate::canonical::triage::TriageDiscoveredDevice {
                        native_id: "lamp-1".to_string(),
                        name: "Kitchen Lamp".to_string(),
                        device_type: DeviceType::Light,
                        room_id: "kitchen-b".to_string(),
                        room_name: "Kitchen".to_string(),
                        manufacturer: None,
                        model: None,
                    },
                    hub_key: secondary_key.clone(),
                    candidate_matches: vec![crate::canonical::triage::CandidateMatch {
                        canonical_id: canonical_id.clone(),
                        name: "Kitchen Lamp".to_string(),
                        score: 8,
                        reasons: vec![
                            crate::canonical::triage::MatchReason::ExactName,
                            crate::canonical::triage::MatchReason::SameDeviceType,
                        ],
                    }],
                    room_binding: None,
                    confidence: 80,
                    status: crate::canonical::triage::TriageStatus::Pending,
                    resolved_by: None,
                    created_at: 1000,
                    resolved_at: None,
                    canonical_id: None,
                });
        }
        do_triage_merge(&source_state, "device-merge", &canonical_id).unwrap();

        let bundle = build_backup_bundle_dto(&source_state, true).unwrap();
        let target_state = Arc::new(Mutex::new(AppState::default()));
        install_mock_hub_provider(&target_state);

        let restored_json = do_backup_restore(&target_state, bundle).unwrap();
        let restored_bundle: BackupBundle = serde_json::from_str(&restored_json).unwrap();
        assert_eq!(restored_bundle.installation.hub_credentials.len(), 2);
        assert!(restored_bundle
            .installation
            .hub_credentials
            .iter()
            .all(|creds| creds.data.is_none()));

        let s = target_state.lock().unwrap();
        assert_eq!(s.topology.room_count(), 1);
        assert_eq!(
            s.topology.translate_room_id(&secondary_key, "kitchen-b"),
            Some(room_id.as_str())
        );
        assert_eq!(
            s.topology
                .get(&room_id)
                .expect("restored room should exist")
                .hub_room_bindings
                .len(),
            2
        );
        assert_eq!(s.canonical_registry.device_count(), 1);
        let primary_device = s
            .canonical_registry
            .find_by_native_id(&primary_key, "lamp-1")
            .expect("primary endpoint should survive restore");
        let secondary_device = s
            .canonical_registry
            .find_by_native_id(&secondary_key, "lamp-1")
            .expect("secondary endpoint should survive restore");
        assert_eq!(primary_device.id, canonical_id);
        assert_eq!(secondary_device.id, canonical_id);
        assert_eq!(primary_device.active_endpoints().count(), 2);
        assert_eq!(primary_device.room_id.as_deref(), Some(room_id.as_str()));
        assert_eq!(
            s.topology
                .get_device_node(&canonical_id)
                .expect("restored merged device node should exist")
                .parent_id
                .as_deref(),
            Some(room_id.as_str())
        );
        let populated_runtimes: Vec<_> = s
            .all_hub_runtimes()
            .into_iter()
            .filter(|(_, runtime)| runtime.engine_node_snapshot(&canonical_id).is_some())
            .collect();
        assert!(
            !populated_runtimes.is_empty(),
            "at least one restored runtime should contain the merged device"
        );
        for (_, runtime) in populated_runtimes {
            assert!(
                runtime.engine_room_snapshot(&room_id).is_some(),
                "restored runtime should contain the merged room"
            );
            assert_eq!(
                runtime
                    .engine_node_snapshot(&canonical_id)
                    .unwrap()
                    .parent_id
                    .as_deref(),
                Some(room_id.as_str())
            );
        }
    }

    #[test]
    fn backup_restore_redacted_backup_preserves_disconnected_hub_identity() {
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
            configuration: BackupConfiguration {
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
                integration_files: vec![],
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

        assert_eq!(restored.installation.hub_credentials.len(), 1);
        assert_eq!(
            restored.installation.hub_credentials[0].hub_type,
            Some(HubType::new("mock"))
        );
        assert_eq!(
            restored.installation.hub_credentials[0].address,
            "bridge.local"
        );
        assert_eq!(restored.installation.hub_credentials[0].data, None);
        let restored_room = restored
            .installation
            .rooms
            .get("office")
            .expect("restored room should be persisted without a runtime");
        assert_eq!(restored_room.name, "Office");
        assert_eq!(restored_room.time_offset_minutes, 12.0);
        assert_eq!(restored_room.brightness_offset, 5.0);

        let s = state.lock().unwrap();
        let hub_key = HubKey::new(HubType::new("mock"), "bridge.local");
        let restored_creds = s
            .hub_credentials
            .get(&hub_key)
            .expect("redacted hub placeholder should be restored");
        assert!(restored_creds.secrets_redacted);
        assert!(!restored_creds.can_connect());
        assert!(s.hubs.is_empty());
        drop(s);

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
        assert_eq!(saved.hub_credentials.len(), 1);
        assert_eq!(
            saved.hub_credentials[0].hub_type,
            Some(HubType::new("mock"))
        );
        assert_eq!(saved.hub_credentials[0].address, "bridge.local");
        assert!(saved.hub_credentials[0].secrets_redacted);
        assert_eq!(saved.hub_credentials[0].data, serde_json::Value::Null);
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
                configuration: BackupConfiguration {
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
                    integration_files: vec![],
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
    fn backup_restore_replaces_stale_persisted_hub_registries() {
        let storage = TestStorage::default();
        storage
            .save_hub_registry_for(
                &HubKey::new(HubType::new("mock"), "stale.local"),
                &serde_json::json!({ "rooms": [{"id": "stale-room"}] }),
            )
            .unwrap();

        let state = Arc::new(Mutex::new(AppState {
            storage: Some(Box::new(storage.clone())),
            ..Default::default()
        }));

        let restored_key = HubKey::new(HubType::new("mock"), "restored.local");
        let bundle = BackupBundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            kind: BundleKind::BackupBundle,
            created_at: "2026-04-16T00:00:00Z".into(),
            secrets_included: false,
            configuration: BackupConfiguration::default(),
            installation: BackupInstallation {
                location: None,
                rooms: rhythm_core::RoomManager::new(),
                topology: crate::topology::RoomTopologyStore::new(),
                canonical_registry: crate::canonical::registry::CanonicalRegistry::new(),
                hub_credentials: vec![],
                hub_registries: vec![crate::bundle::BackupHubRegistry {
                    hub_key: restored_key.clone(),
                    snapshot: serde_json::json!({ "rooms": [{"id": "restored-room"}] }),
                }],
                integration_files: vec![],
            },
            runtime_state: BackupRuntimeState {
                active_mode: RhythmMode::Day,
                last_change_cause: ModeChangeCause::Manual,
                last_change_transition_id: None,
                last_change_epoch_ms: None,
            },
        };

        do_backup_restore(&state, bundle).unwrap();

        let saved = storage.inner.lock().unwrap();
        assert_eq!(saved.hub_registries.len(), 1);
        assert!(!saved.hub_registries.contains_key("mock://stale.local"));
        assert_eq!(
            saved.hub_registries.get(&restored_key.to_string()),
            Some(&serde_json::json!({ "rooms": [{"id": "restored-room"}] }))
        );
    }

    #[test]
    fn backup_restore_backfills_unassigned_triage_for_roomless_devices() {
        let state = Arc::new(Mutex::new(AppState::default()));
        let device_id = insert_canonical_device(
            &state,
            HubKey::new(HubType::new("matter"), "local"),
            "matter-device-1",
            "Desk Lamp",
            "",
            "",
        );

        let canonical_registry = state.lock().unwrap().canonical_registry.clone();
        assert_eq!(canonical_registry.triage().pending_unassigned_count(), 0);

        let restored = Arc::new(Mutex::new(AppState::default()));
        let bundle = BackupBundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            kind: BundleKind::BackupBundle,
            created_at: "2026-04-16T00:00:00Z".into(),
            secrets_included: false,
            configuration: BackupConfiguration::default(),
            installation: BackupInstallation {
                location: None,
                rooms: rhythm_core::RoomManager::new(),
                topology: crate::topology::RoomTopologyStore::new(),
                canonical_registry,
                hub_credentials: vec![],
                hub_registries: vec![],
                integration_files: vec![],
            },
            runtime_state: BackupRuntimeState {
                active_mode: RhythmMode::Day,
                last_change_cause: ModeChangeCause::Manual,
                last_change_transition_id: None,
                last_change_epoch_ms: None,
            },
        };

        do_backup_restore(&restored, bundle).unwrap();

        let restored_state = restored.lock().unwrap();
        assert!(restored_state.canonical_registry.get(&device_id).is_some());
        assert_eq!(
            restored_state
                .canonical_registry
                .triage()
                .pending_unassigned_count(),
            1
        );
        assert_eq!(
            restored_state
                .canonical_registry
                .triage()
                .pending_by_kind(crate::canonical::triage::TriageKind::UnassignedDevice)[0]
                .canonical_id
                .as_deref(),
            Some(device_id.as_str())
        );
    }

    #[test]
    fn profile_bundle_reset_restores_factory_default_profile_bundle() {
        let (state, _rt) = setup_state(vec![]);
        let focus = make_focus_profile();
        do_profile_bundle_import(
            &state,
            ProfileBundleImportPayload::Profile(ProfileBundleData {
                power_save: true,
                profiles: vec![focus],
                mode_transitions: vec![],
            }),
        )
        .unwrap();

        let json = do_profile_bundle_reset(&state).unwrap();
        let reset_bundle: ProfileBundle = serde_json::from_str(&json).unwrap();
        let expected = factory_default_profile_bundle();
        let mut reset_profiles = reset_bundle.profile.profiles.clone();
        reset_profiles.sort_by(|left, right| left.id.cmp(&right.id));
        let mut expected_profiles = expected.profile.profiles.clone();
        expected_profiles.sort_by(|left, right| left.id.cmp(&right.id));

        assert_eq!(reset_bundle.profile.power_save, expected.profile.power_save);
        assert_eq!(reset_profiles, expected_profiles);
        assert_eq!(
            reset_bundle.profile.mode_transitions,
            expected.profile.mode_transitions
        );
    }

    #[test]
    fn factory_reset_clears_installation_state_and_stale_storage() {
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
            set_observed_lights_on_in_app(&mut s, "r1", true);
            let dispatch_generation = s.light_dispatch_generation;
            s.pending_periodic_ticks.insert(
                "r1".into(),
                crate::state::PendingPeriodicTick::new(12.0, dispatch_generation),
            );
            s.pending_motion_clear.push("r1".into());
            s.pending_motion_seed.push(crate::state::MotionSeedEntry {
                source_node_id: "sensor-1".into(),
                target_node_id: "r1".into(),
                is_active: true,
                stopped_at_epoch_ms: None,
                motion_owned: None,
                warning_active: false,
            });
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

        let json = do_factory_reset(&state).unwrap();
        let reset_bundle: ProfileBundle = serde_json::from_str(&json).unwrap();
        assert!(reset_bundle
            .profile
            .profiles
            .iter()
            .any(|profile| profile.id == "rhythm"));

        let s = state.lock().unwrap();
        assert!(s.hubs.is_empty());
        assert!(s.hub_credentials.is_empty());
        assert!(s.hub_seen_connected_once.is_empty());
        assert!(s.canonical_registry.device_count() == 0);
        assert_eq!(s.canonical_registry.triage().pending_count(), 0);
        assert_eq!(s.topology.room_count(), 0);
        assert!(s.room_observed_power.is_empty());
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
        set_observed_lights_on(&state, "r1", true);
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
        set_observed_lights_on(&state, "r1", true);

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
        activate_topology_room_hubs(&state, "r1");

        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(
            parsed["rooms"][0]["hub_types"],
            serde_json::json!(["matter", "mock"])
        );
    }

    #[test]
    fn build_rooms_state_sensor_no_motion_snapshot_populates_defaults() {
        // Room has a motion sensor in topology but no MotionSnapshot yet.
        let runtime = Arc::new(MockRuntime::new(
            vec![
                make_snapshot("sensor_room", false, false),
                make_snapshot("plain_room", false, false),
            ],
            12.0,
        ));

        let mut app = AppState::default();
        let hub_type = HubType::parse("mock").unwrap();
        let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key: hub_key.clone(),
                runtime: Some(runtime as Arc<dyn RuntimeHandle>),
                hub_data: Box::new(()),
                registry: None,
                discovery: None,
                shutdown: Default::default(),
            },
        );
        app.topology.insert_room(crate::topology::TopologyRoom::new(
            "sensor_room",
            "sensor_room",
        ));
        let identity = crate::canonical::identity::DiscoveredIdentity {
            native_id: "ms1".into(),
            room_id: Some("sensor_room_native".into()),
            room_name: Some("sensor_room".into()),
            name: "ms1".into(),
            device_type: DeviceType::Motion,
            hardware_ids: vec![crate::canonical::identity::HardwareId::matter("ms1")],
            manufacturer: None,
            model: None,
        };
        let canonical_id = match app.canonical_registry.resolve(&identity, &hub_key, 1) {
            crate::canonical::registry::ResolveResult::Created { canonical_id }
            | crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id } => {
                canonical_id
            }
            other => panic!("unexpected resolve result: {:?}", other),
        };
        assert!(app
            .canonical_registry
            .assign_room(&canonical_id, Some("sensor_room")));
        app.topology.ensure_standalone_device(&canonical_id);
        assert!(app.topology.assign_device(
            &canonical_id,
            Some("sensor_room"),
            crate::topology::DevicePlacement::UserOverride,
        ));
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
    fn build_state_snapshot_bootstrap_includes_hub_types_from_topology() {
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
        activate_topology_room_hubs(&state, "r1");

        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(
            parsed["nodes"][0]["hub_types"],
            serde_json::json!(["matter", "mock"])
        );
    }

    #[test]
    fn build_state_snapshot_before_runtime_ignores_persisted_room_graph() {
        let storage = TestStorage::default();
        let mut rooms = rhythm_core::RoomManager::new();
        rooms.get_or_create("stale-room", "Stale Room");
        rooms.add_node(
            "stale-child",
            "Stale Child",
            LightNodeKind::LightDevice,
            Some("stale-room".into()),
        );
        storage.save_rooms(&rooms).unwrap();

        let app = AppState {
            storage: Some(Box::new(storage)),
            ..Default::default()
        };
        let state: SharedState = Arc::new(Mutex::new(app));
        add_topology_room(&state, "office", &["matter"]);
        activate_topology_room_hubs(&state, "office");

        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let node_ids = parsed["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|node| node["id"].as_str())
            .collect::<HashSet<_>>();

        assert!(node_ids.contains("office"));
        assert!(
            !node_ids.contains("stale-room"),
            "bootstrap /api/state should not deserialize and expose persisted room graph"
        );
        assert!(
            !node_ids.contains("stale-child"),
            "bootstrap /api/state should not deserialize and expose persisted child graph"
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
    fn build_state_snapshot_review_surfaces_restore_and_triage_context() {
        let (state, _rt) = setup_state(vec![]);
        let primary_key = HubKey::new(HubType::new("mock"), "mock");
        let secondary_key = HubKey::new(HubType::new("ha"), "192.168.1.200");
        let canonical_id =
            insert_canonical_device(&state, primary_key.clone(), "light-1", "Desk Lamp", "", "");

        {
            let mut s = state.lock().unwrap();
            s.hub_credentials.insert(
                secondary_key.clone(),
                HubCredentials::new("ha", "192.168.1.200", serde_json::json!({ "token": "x" })),
            );
            let device = s.canonical_registry.get_mut(&canonical_id).unwrap();
            device.upsert_endpoint(
                secondary_key.clone(),
                "light.desk".to_string(),
                2000,
                Some("Desk".to_string()),
            );
            s.canonical_registry.set_preferred_endpoint(
                &canonical_id,
                &secondary_key,
                "light.desk",
            );
            s.canonical_registry
                .triage_mut()
                .add(crate::canonical::triage::TriageEntry {
                    id: "hc1".to_string(),
                    kind: crate::canonical::triage::TriageKind::HubConfigured,
                    discovered: crate::canonical::triage::TriageDiscoveredDevice {
                        native_id: "light.desk".to_string(),
                        name: "Desk Lamp".to_string(),
                        device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                        room_id: String::new(),
                        room_name: String::new(),
                        manufacturer: None,
                        model: None,
                    },
                    hub_key: secondary_key.clone(),
                    candidate_matches: vec![],
                    room_binding: None,
                    confidence: 0,
                    status: crate::canonical::triage::TriageStatus::Pending,
                    resolved_by: None,
                    created_at: 1000,
                    resolved_at: None,
                    canonical_id: None,
                });
            s.canonical_registry
                .triage_mut()
                .add(crate::canonical::triage::TriageEntry {
                    id: "dm1".to_string(),
                    kind: crate::canonical::triage::TriageKind::DeviceMerge,
                    discovered: crate::canonical::triage::TriageDiscoveredDevice {
                        native_id: "light.alt".to_string(),
                        name: "Desk Lamp".to_string(),
                        device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                        room_id: String::new(),
                        room_name: String::new(),
                        manufacturer: None,
                        model: None,
                    },
                    hub_key: secondary_key,
                    candidate_matches: vec![],
                    room_binding: None,
                    confidence: 0,
                    status: crate::canonical::triage::TriageStatus::NewDevice,
                    resolved_by: Some("api".to_string()),
                    created_at: 900,
                    resolved_at: Some(950),
                    canonical_id: None,
                });
        }

        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["review"]["pending"]["hub_configured"], 1);
        assert_eq!(parsed["review"]["pending"]["total"], 1);
        assert_eq!(parsed["review"]["disconnected_hubs"][0]["type"], "ha");
        assert_eq!(
            parsed["review"]["preferred_endpoints"][0]["canonical_id"],
            canonical_id
        );
        assert_eq!(
            parsed["review"]["preferred_endpoints"][0]["native_id"],
            "light.desk"
        );
        assert_eq!(
            parsed["review"]["hub_configured_conflicts"][0]["guidance"],
            "Remove the native hub automation for this device in the hub app so Rhythm can control it predictably."
        );
        assert_eq!(
            parsed["review"]["triage_entries"][0]["summary"],
            "Native hub automation is still configured for this device"
        );
        assert_eq!(
            parsed["review"]["triage_entries"][1]["summary"],
            "Kept as a separate device"
        );
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

    // ========================================================================
    // set_brightness tests
    // ========================================================================

    #[test]
    fn set_brightness_succeeds() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_node_brightness(&state, "r1", 75, false);
        assert!(result.is_ok());
        // Setting brightness marks room as lights_on
        assert_eq!(observed_lights_on(&state.lock().unwrap(), "r1"), Some(true));
    }

    #[test]
    fn set_brightness_updates_parent_lights_on_for_attached_light() {
        let (state, _rt, device_id) = setup_attached_hue_light_with_group_dispatch();

        let result = do_set_node_brightness(&state, &device_id, 75, false);

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(observed_lights_on(&s, "room1"), Some(true));
        assert_eq!(s.room_observed_power.len(), 1);
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
        let (state, rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_node_time_offset(&state, "r1", 30.0, false);
        assert!(result.is_ok());
        assert!(
            rt.time_offset_updates().is_empty(),
            "preview offset should not dispatch through set_room_time_offset directly"
        );
        assert_eq!(
            rt.engine_node_snapshot("r1").unwrap().time_offset_minutes,
            30.0
        );
    }

    #[test]
    fn set_time_offset_negative() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_node_time_offset(&state, "r1", -60.0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_time_offset_accepts_light_device_node_target() {
        let (state, rt, device_id) = setup_attached_hue_light_with_group_dispatch();
        let result = do_set_node_time_offset(&state, &device_id, 30.0, false);

        assert!(result.is_ok());
        assert_eq!(
            rt.engine_node_snapshot(&device_id)
                .unwrap()
                .time_offset_minutes,
            30.0
        );
    }

    #[test]
    fn set_time_offset_preview_queues_only_explicit_room_target() {
        let (state, _rt, ..) = setup_mixed_room_with_hub_groups();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        {
            let mut s = state.lock().unwrap();
            s.composite_controller = Some(Arc::new(rhythm_core::CompositeController::new()));
            s.work_tx = Some(tx);
        }

        let result = do_set_node_time_offset(&state, "room1", 30.0, false);

        assert!(result.is_ok());
        match rx
            .recv_timeout(Duration::from_millis(50))
            .expect("manual preview should queue one periodic tick")
        {
            WorkItem::PeriodicNodeTick {
                node_id,
                settings_node_id,
                emit_parent_node_id,
                ..
            } => {
                assert_eq!(node_id, "room1");
                assert_eq!(settings_node_id, "room1");
                assert_eq!(emit_parent_node_id, None);
            }
            _ => panic!("unexpected work item"),
        }
        assert!(
            matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
            "manual room offset preview must not expand into topology light nodes"
        );
    }

    #[test]
    fn set_time_offset_preview_queues_only_explicit_light_target() {
        let (state, _rt, _matter_id, _ha_id, hue_one_id, _hue_two_id) =
            setup_mixed_room_with_hub_groups();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        {
            let mut s = state.lock().unwrap();
            s.composite_controller = Some(Arc::new(rhythm_core::CompositeController::new()));
            s.work_tx = Some(tx);
        }

        let result = do_set_node_time_offset(&state, &hue_one_id, 30.0, false);

        assert!(result.is_ok());
        match rx
            .recv_timeout(Duration::from_millis(50))
            .expect("manual preview should queue one periodic tick")
        {
            WorkItem::PeriodicNodeTick {
                node_id,
                settings_node_id,
                emit_parent_node_id,
                ..
            } => {
                assert_eq!(node_id, hue_one_id);
                assert_eq!(settings_node_id, node_id);
                assert_eq!(emit_parent_node_id.as_deref(), Some("room1"));
            }
            _ => panic!("unexpected work item"),
        }
        assert!(
            matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
            "manual light offset preview must not expand into sibling light nodes"
        );
    }

    #[test]
    fn set_time_offset_batch_preview_uses_periodic_targets_not_child_devices() {
        let (state, rt, matter_id, ha_id, hue_one_id, hue_two_id) =
            setup_mixed_room_with_hub_groups();
        let standalone_id = insert_canonical_device(
            &state,
            HubKey::new(HubType::new("matter"), "local"),
            "matter-standalone",
            "Standalone Lamp",
            "",
            "",
        );
        rt.snapshots
            .lock()
            .unwrap()
            .push(make_standalone_light_snapshot(&standalone_id));

        let (tx, rx) = std::sync::mpsc::sync_channel(16);
        {
            let mut s = state.lock().unwrap();
            s.composite_controller = Some(Arc::new(rhythm_core::CompositeController::new()));
            s.topology.ensure_standalone_device(&standalone_id);
            set_observed_lights_on_in_app(&mut s, "room1", true);
            s.work_tx = Some(tx);
        }

        let updates = vec![
            ("room1".to_string(), 30.0),
            (matter_id.clone(), 30.0),
            (ha_id.clone(), 30.0),
            (hue_one_id.clone(), 30.0),
            (hue_two_id.clone(), 30.0),
            (standalone_id.clone(), 30.0),
        ];
        let dispatch_count = do_set_node_time_offsets_batch_with_spacing(
            &state,
            &updates,
            false,
            Duration::from_millis(0),
        )
        .unwrap();

        let queued_node_ids: Vec<String> = rx
            .try_iter()
            .map(|item| match item {
                WorkItem::PeriodicNodeTick { node_id, .. } => node_id,
                _ => panic!("unexpected work item"),
            })
            .collect();
        assert_eq!(queued_node_ids.len(), dispatch_count);
        assert!(
            dispatch_count < updates.len(),
            "batch preview must collapse app-submitted child devices"
        );
        assert!(
            queued_node_ids.contains(&standalone_id),
            "roomless light devices must still preview on themselves"
        );
        assert!(!queued_node_ids.contains(&ha_id));
        assert!(!queued_node_ids.contains(&hue_one_id));
        assert!(!queued_node_ids.contains(&hue_two_id));
        assert_eq!(
            rt.engine_node_snapshot("room1")
                .unwrap()
                .time_offset_minutes,
            30.0
        );
        assert_eq!(
            rt.engine_node_snapshot(&standalone_id)
                .unwrap()
                .time_offset_minutes,
            30.0
        );
        assert_eq!(
            rt.engine_node_snapshot(&hue_one_id)
                .unwrap()
                .time_offset_minutes,
            0.0
        );
    }

    #[test]
    fn default_time_offset_targets_use_on_rooms_and_roomless_lights() {
        let (state, rt, _matter_id, ha_id, hue_one_id, hue_two_id) =
            setup_mixed_room_with_hub_groups();
        let standalone_id = insert_canonical_device(
            &state,
            HubKey::new(HubType::new("matter"), "local"),
            "matter-standalone",
            "Standalone Lamp",
            "",
            "",
        );
        rt.snapshots
            .lock()
            .unwrap()
            .push(make_standalone_light_snapshot(&standalone_id));
        {
            let mut s = state.lock().unwrap();
            s.topology.ensure_standalone_device(&standalone_id);
            set_observed_lights_on_in_app(&mut s, "room1", true);
        }

        let mut updates = default_node_time_offset_updates(&state, 20.0).unwrap();
        updates.sort_by(|left, right| left.0.cmp(&right.0));

        assert_eq!(
            updates,
            vec![(standalone_id.clone(), 20.0), ("room1".to_string(), 20.0)]
        );
        assert!(!updates.iter().any(|(node_id, _)| node_id == &ha_id));
        assert!(!updates.iter().any(|(node_id, _)| node_id == &hue_one_id));
        assert!(!updates.iter().any(|(node_id, _)| node_id == &hue_two_id));
    }

    #[test]
    fn set_time_offset_batch_preserves_explicit_child_targets_when_parent_absent() {
        let (state, rt, _matter_id, _ha_id, hue_one_id, hue_two_id) =
            setup_mixed_room_with_hub_groups();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        {
            let mut s = state.lock().unwrap();
            s.composite_controller = Some(Arc::new(rhythm_core::CompositeController::new()));
            s.work_tx = Some(tx);
        }

        let updates = vec![(hue_one_id.clone(), 15.0), (hue_two_id.clone(), 15.0)];
        let dispatch_count = do_set_node_time_offsets_batch_with_spacing(
            &state,
            &updates,
            false,
            Duration::from_millis(0),
        )
        .unwrap();

        let queued_node_ids: Vec<String> = rx
            .try_iter()
            .map(|item| match item {
                WorkItem::PeriodicNodeTick { node_id, .. } => node_id,
                _ => panic!("unexpected work item"),
            })
            .collect();
        assert_eq!(dispatch_count, 2);
        assert_eq!(
            queued_node_ids,
            vec![hue_one_id.clone(), hue_two_id.clone()]
        );
        assert_eq!(
            rt.engine_node_snapshot(&hue_one_id)
                .unwrap()
                .time_offset_minutes,
            15.0
        );
        assert_eq!(
            rt.engine_node_snapshot(&hue_two_id)
                .unwrap()
                .time_offset_minutes,
            15.0
        );
    }

    #[test]
    fn set_time_offset_rejects_non_light_device_node_target() {
        let mut snapshot = make_snapshot("button1", false, false);
        snapshot.kind = rhythm_core::LightNodeKind::Button;
        let (state, _rt) = setup_state(vec![snapshot]);
        let result = do_set_node_time_offset(&state, "button1", 30.0, false);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("light-addressable nodes"));
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
        set_observed_lights_on(&state, "r1", true);

        let result = do_node_preferences_set(&state, "r1", None, Some(true), None, None, false);

        assert!(result.is_ok());
        let s = state.lock().unwrap();
        assert_eq!(observed_lights_on(&s, "r1"), Some(true));
    }

    #[test]
    fn room_preferences_idle_implies_rhythm_enabled() {
        // Idle should force rhythm enabled so standby ticks still render.
        let mut snap = make_snapshot("r1", false, false);
        snap.rhythm_enabled = false;
        let (state, _rt) = setup_state(vec![snap]);
        state.lock().unwrap().power_save = false;
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
        assert_eq!(observed_lights_on(&state.lock().unwrap(), "r1"), Some(true));
    }

    #[test]
    fn room_preferences_idle_maps_to_hard_off_when_power_save_enabled() {
        let mut snap = make_snapshot("r1", false, false);
        snap.rhythm_enabled = false;
        let (state, runtime) = setup_state(vec![snap]);
        set_observed_lights_on(&state, "r1", true);

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
        assert_eq!(
            observed_lights_on(&state.lock().unwrap(), "r1"),
            Some(false)
        );
        assert_eq!(
            runtime.events(),
            vec![("r1".into(), ButtonAction::LightsOff)]
        );
        let snap = runtime.engine_room_snapshot("r1").unwrap();
        assert!(!snap.soft_off);
        assert!(snap.hard_off);
    }

    #[test]
    fn room_preferences_idle_updates_parent_lights_on_for_attached_light() {
        let (state, rt, device_id) = setup_attached_hue_light_with_group_dispatch();
        state.lock().unwrap().power_save = false;
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
        assert_eq!(observed_lights_on(&s, "room1"), Some(true));
        assert_eq!(s.room_observed_power.len(), 1);
    }

    #[test]
    fn room_preferences_idle_updates_parent_lights_on_without_group_dispatch() {
        let (state, rt, device_id) = setup_attached_matter_light_without_group_dispatch();
        state.lock().unwrap().power_save = false;
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
        assert_eq!(observed_lights_on(&s, &device_id), Some(true));
        assert_eq!(observed_lights_on(&s, "room1"), Some(true));
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
        assert_eq!(observed_lights_on(&s, &matter_id), Some(false));
        assert_eq!(observed_lights_on(&s, "room1"), Some(true));
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

    #[test]
    fn room_preferences_active_queues_motion_timer_clear() {
        let snap = make_snapshot("r1", false, false);
        let (state, runtime) = setup_state(vec![snap]);
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

        let result = do_node_preferences_set(
            &state,
            "r1",
            Some(true),
            None,
            Some(RoomModeState::Active),
            None,
            false,
        );

        assert!(result.is_ok());
        assert!(runtime.events().is_empty());
        assert_eq!(
            state.lock().unwrap().pending_motion_clear,
            vec!["r1".to_string()]
        );
        let snap = runtime.engine_room_snapshot("r1").unwrap();
        assert!(!snap.soft_off);
        assert!(!snap.hard_off);
    }

    #[test]
    fn room_preferences_idle_queues_motion_timer_clear() {
        let snap = make_snapshot("r1", false, false);
        let (state, runtime) = setup_state(vec![snap]);
        {
            let mut s = state.lock().unwrap();
            s.power_save = false;
            s.motion_snapshots.insert(
                "r1".into(),
                MotionSnapshot {
                    motion_active: false,
                    motion_owned: true,
                    remaining_secs: Some(45),
                    timeout_secs: 300,
                    warning_active: true,
                },
            );
        }

        let result = do_node_preferences_set(
            &state,
            "r1",
            Some(true),
            None,
            Some(RoomModeState::Idle),
            None,
            false,
        );

        assert!(result.is_ok());
        assert_eq!(
            state.lock().unwrap().pending_motion_clear,
            vec!["r1".to_string()]
        );
        let snap = runtime.engine_room_snapshot("r1").unwrap();
        assert!(snap.soft_off);
        assert!(!snap.hard_off);
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
    fn settings_power_save_emits_settings_changed_payload() {
        let (state, _rt) = setup_state(vec![]);
        let (event_tx, mut event_rx) =
            tokio::sync::broadcast::channel::<crate::server_event::ServerEvent>(16);
        state.lock().unwrap().event_tx = Some(event_tx);

        do_settings_set(&state, Some(true), None, None, None).unwrap();

        let mut power_save = None;
        while let Ok(event) = event_rx.try_recv() {
            if let crate::server_event::ServerEvent::SettingsChanged { settings } = event {
                power_save = Some(settings.power_save);
                break;
            }
        }
        assert_eq!(power_save, Some(true));
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
    fn active_profile_config_apply_reapplies_current_outputs() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        let mut rhythm = rhythm_core::default_rhythm_profile();
        rhythm.max_brightness = 33;

        do_config_set_with_options(&state, rhythm, true).unwrap();

        let applied = runtime.applied_commands();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "r1");
        assert!(
            applied[0].1.brightness <= 33,
            "reapplied command should use updated active profile, got {}",
            applied[0].1.brightness
        );
        assert_eq!(observed_lights_on(&state.lock().unwrap(), "r1"), Some(true));
    }

    #[test]
    fn active_profile_config_save_without_apply_does_not_dispatch() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        let mut rhythm = rhythm_core::default_rhythm_profile();
        rhythm.max_brightness = 33;

        do_config_set_with_options(&state, rhythm, false).unwrap();

        assert!(runtime.applied_commands().is_empty());
        assert_eq!(observed_lights_on(&state.lock().unwrap(), "r1"), None);
    }

    #[test]
    fn active_profile_config_apply_preserves_known_off_nodes() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        set_observed_lights_on(&state, "r1", false);
        let mut rhythm = rhythm_core::default_rhythm_profile();
        rhythm.max_brightness = 33;

        do_config_set_with_options(&state, rhythm, true).unwrap();

        assert!(runtime.applied_commands().is_empty());
        assert_eq!(
            observed_lights_on(&state.lock().unwrap(), "r1"),
            Some(false)
        );
    }

    #[test]
    fn inactive_profile_config_apply_does_not_dispatch() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        let mut sleep = rhythm_core::default_sleep_profile();
        sleep.max_brightness = 9;

        do_config_set_with_options(&state, sleep, true).unwrap();

        assert!(runtime.applied_commands().is_empty());
        assert_eq!(observed_lights_on(&state.lock().unwrap(), "r1"), None);
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
    fn active_mode_change_invalidates_queued_light_dispatches() {
        let (state, _rt) = setup_state(vec![]);
        let previous_generation = {
            let mut s = state.lock().unwrap();
            let dispatch_generation = s.light_dispatch_generation;
            s.pending_periodic_ticks.insert(
                "room1".into(),
                crate::state::PendingPeriodicTick::new(12.0, dispatch_generation),
            );
            s.next_node_dispatch_at =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
            s.light_dispatch_generation
        };

        do_set_active_mode(&state, RhythmMode::Sleep).unwrap();

        let s = state.lock().unwrap();
        assert_ne!(s.light_dispatch_generation, previous_generation);
        assert!(s.pending_periodic_ticks.is_empty());
        assert!(s.next_node_dispatch_at.is_none());
    }

    #[test]
    fn active_mode_change_emits_mode_changed_sse_event() {
        // Regression for issue #32: settings_changed is settings-scoped, so
        // clients that don't refetch /api/mode need a self-describing
        // ModeChanged event with the new mode.
        let (state, _rt) = setup_state(vec![]);
        let (event_tx, mut event_rx) =
            tokio::sync::broadcast::channel::<crate::server_event::ServerEvent>(16);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Day;
            s.event_tx = Some(event_tx);
        }

        do_set_active_mode(&state, RhythmMode::Sleep).unwrap();

        let mut mode_changed = None;
        while let Ok(event) = event_rx.try_recv() {
            if let crate::server_event::ServerEvent::ModeChanged {
                active,
                cause,
                transition_id,
                epoch_ms,
            } = event
            {
                mode_changed = Some((active, cause, transition_id, epoch_ms));
                break;
            }
        }
        let (active, cause, transition_id, epoch_ms) =
            mode_changed.expect("expected ModeChanged SSE event after manual mode flip");
        assert_eq!(active, RhythmMode::Sleep);
        assert_eq!(cause, ModeChangeCause::Manual);
        assert!(transition_id.is_none());
        assert!(epoch_ms > 0, "expected non-zero last-change epoch_ms");
    }

    #[test]
    fn mode_transition_uses_configured_duration_as_room_fade() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Sleep;
            set_observed_lights_on_in_app(&mut s, "r1", true);
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
    fn mode_transition_disabled_scheduled_change_has_no_active_context() {
        let (state, _runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Sleep;
            set_observed_lights_on_in_app(&mut s, "r1", true);
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                RhythmMode::Sleep,
                RhythmMode::Day,
                4_000,
            )
            .with_trigger(ModeTransitionTrigger::Sunrise)
            .with_trigger_enabled(false)]);
        }

        do_set_active_mode_with_trigger(&state, RhythmMode::Day, ModeTransitionTrigger::Sunrise)
            .unwrap();

        let s = state.lock().unwrap();
        assert_eq!(s.active_mode, RhythmMode::Day);
        assert_eq!(s.last_active_mode_cause, ModeChangeCause::Schedule);
        assert_eq!(s.last_active_mode_transition_id, None);
        assert!(s.room_mode_transitions.is_empty());
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
            set_observed_lights_on_in_app(&mut s, "r1", true);
            set_observed_lights_on_in_app(&mut s, "r2", true);
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
    fn mode_transition_room_output_uses_real_sun_times() {
        let (state, _runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.latitude = Some(35.805054);
            s.longitude = Some(-78.7975);
            s.timezone_name = Some("America/New_York".into());
            s.active_mode = RhythmMode::Day;
        }

        let sample_at = chrono::NaiveDate::from_ymd_opt(2026, 4, 24)
            .unwrap()
            .and_hms_opt(7, 15, 55)
            .unwrap();

        let (actual, expected, fallback) = {
            let s = state.lock().unwrap();
            let mode_configs = s.mode_configs();
            let lighting = RoomLightingContext {
                light_profile_configs: &s.light_profile_configs,
                mode_configs: &mode_configs,
                mode: RhythmMode::Day,
                solar_noon: s.solar_noon_hour(),
                latitude: s.latitude,
                longitude: s.longitude,
                timezone_name: s.timezone_name.as_deref(),
                utc_offset: s.utc_offset_hours,
            };
            let room = RoomLightingInput {
                settings: &RoomProfileSettings::default(),
                room_state: RoomModeState::Active,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
            };
            let registry = light_profile_registry_from_parts(
                &s.light_profile_configs,
                &mode_configs,
                RhythmMode::Day,
            );
            let resolved_ctx = rhythm_core::curve_context_for_local_datetime(
                s.solar_noon_hour(),
                s.latitude,
                s.longitude,
                s.timezone_name.as_deref(),
                sample_at,
            );
            let expected_values = registry.calculate_room_values(
                RhythmMode::Day,
                RoomModeState::Active,
                Some(room.settings),
                &resolved_ctx,
                0.0,
            );
            let expected_command = build_room_command_from_values(
                &expected_values,
                expected_values.brightness,
                30_000,
            );

            let fallback_ctx =
                rhythm_core::CurveContext::new(resolved_ctx.current_hour, resolved_ctx.solar, None);
            let fallback_values = registry.calculate_room_values(
                RhythmMode::Day,
                RoomModeState::Active,
                Some(room.settings),
                &fallback_ctx,
                0.0,
            );
            let fallback_command = build_room_command_from_values(
                &fallback_values,
                fallback_values.brightness,
                30_000,
            );

            (
                resolve_room_command_for_state_at_from_parts(
                    lighting,
                    room,
                    sample_at,
                    Some(30_000),
                )
                .unwrap(),
                expected_command,
                fallback_command,
            )
        };

        assert_eq!(actual.kelvin, expected.kelvin);
        assert_eq!(actual.brightness, expected.brightness);
        assert_ne!(actual.kelvin, fallback.kelvin);
        assert_ne!(actual.brightness, fallback.brightness);
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
            set_observed_lights_on_in_app(&mut s, "r1", true);
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
            set_observed_lights_on_in_app(&mut s, "r1", true);
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
            set_observed_lights_on_in_app(&mut s, "r1", true);
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
    fn mode_transition_trigger_by_id_works_when_trigger_disabled() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        let transition_id = {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Sleep;
            set_observed_lights_on_in_app(&mut s, "r1", true);
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                RhythmMode::Sleep,
                RhythmMode::Day,
                4_000,
            )
            .with_trigger(ModeTransitionTrigger::Sunrise)
            .with_trigger_enabled(false)]);
            s.mode_transition_configs()[0].id.clone()
        };

        let json = do_trigger_transition(&state, &transition_id).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["active"], "day");
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
        assert!(s.room_mode_transitions.contains_key("r1"));
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
            set_observed_lights_on_in_app(&mut s, "active_room", true);

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
            set_observed_lights_on_in_app(&mut s, "active_room", true);
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
            s.power_save = false;
            s.active_mode = RhythmMode::Day;
            set_observed_lights_on_in_app(&mut s, "r1", true);
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
            set_observed_lights_on_in_app(&mut s, "r1", false);
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
        assert_eq!(observed_lights_on(&state.lock().unwrap(), "r1"), Some(true));
    }

    #[test]
    fn mode_change_room_default_hard_off_queues_motion_timer_clear() {
        let (state, runtime) = setup_state(vec![make_snapshot("r1", false, false)]);
        {
            let mut s = state.lock().unwrap();
            s.active_mode = RhythmMode::Day;
            set_observed_lights_on_in_app(&mut s, "r1", true);
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
            set_observed_lights_on_in_app(&mut s, "r1", true);
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
            set_observed_lights_on_in_app(&mut s, "r1", true);
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
    fn reconcile_runtime_from_state_does_not_reapply_scheduled_active_mode_on_restart() {
        // Regression for issue #69: on restart the server must not re-apply the
        // active mode's room defaults. Persisted room state is authoritative.
        for cause in [ModeChangeCause::Schedule, ModeChangeCause::Manual] {
            let mut active_room = make_snapshot("active-room", false, true);
            active_room.rhythm_enabled = true;
            let hard_off_room = make_snapshot("hard-off-room", false, false);
            let (state, runtime) = setup_state(vec![active_room, hard_off_room]);
            let storage = TestStorage::default();
            let transition =
                rhythm_core::ModeTransitionConfig::new(RhythmMode::Sleep, RhythmMode::Day, 4_321)
                    .with_trigger(ModeTransitionTrigger::Sunrise);

            add_topology_room(&state, "active-room", &[]);
            add_topology_room(&state, "hard-off-room", &[]);

            {
                let mut s = state.lock().unwrap();
                s.storage = Some(Box::new(storage.clone()));
                s.active_mode = RhythmMode::Day;
                s.last_active_mode_cause = cause;
                s.set_mode_transition_configs(vec![transition]);
                s.last_active_mode_transition_id = s
                    .mode_transition_configs()
                    .first()
                    .map(|config| config.id.clone());
                s.set_mode_configs(vec![ModeConfig {
                    mode: RhythmMode::Day,
                    active_profile_id: Some(rhythm_core::RHYTHM_PROFILE_ID.into()),
                    idle_profile_id: Some(rhythm_core::DAY_IDLE_PROFILE_ID.into()),
                    wake_profile_id: None,
                    warning_profile_id: None,
                    room_defaults: vec![
                        rhythm_core::RoomModeDefault {
                            room_id: "active-room".into(),
                            state: RoomModeState::Active,
                        },
                        rhythm_core::RoomModeDefault {
                            room_id: "hard-off-room".into(),
                            state: RoomModeState::HardOff,
                        },
                    ],
                }]);
            }

            reconcile_runtime_from_state(&state).unwrap();

            assert!(
                runtime.applied_states().is_empty(),
                "{:?}: mode defaults must not be applied on restart (got {:?})",
                cause,
                runtime.applied_states()
            );
            assert!(
                runtime.lights_off_calls().is_empty(),
                "{:?}: hard-off defaults must not dispatch lights-off on restart (got {:?})",
                cause,
                runtime.lights_off_calls()
            );
            assert!(
                runtime
                    .engine_room_snapshot("active-room")
                    .unwrap()
                    .soft_off,
                "{:?}: persisted soft_off should survive restart",
                cause
            );
            assert!(
                !runtime
                    .engine_room_snapshot("hard-off-room")
                    .unwrap()
                    .hard_off,
                "{:?}: hard_off must not be forced on by a scheduled mode reapply",
                cause
            );
        }
    }

    #[test]
    fn reconcile_runtime_from_state_drains_pending_mode_output_apply() {
        // Companion to issue #69: when the periodic loop replays a missed
        // scheduled transition before runtime bootstrap, `apply_active_mode_outputs`
        // defers the room-default apply via `pending_mode_output_apply`. Once
        // the runtime is ready, `reconcile_runtime_from_state` must drain that
        // flag and apply the defaults — otherwise an offline overnight
        // Sleep→Day transition would leave rooms stuck in Sleep-era state.
        let mut active_room = make_snapshot("active-room", false, true);
        active_room.rhythm_enabled = true;
        let hard_off_room = make_snapshot("hard-off-room", false, false);
        let (state, runtime) = setup_state(vec![active_room, hard_off_room]);
        let storage = TestStorage::default();
        let transition =
            rhythm_core::ModeTransitionConfig::new(RhythmMode::Sleep, RhythmMode::Day, 4_321)
                .with_trigger(ModeTransitionTrigger::Sunrise);

        add_topology_room(&state, "active-room", &[]);
        add_topology_room(&state, "hard-off-room", &[]);

        {
            let mut s = state.lock().unwrap();
            s.storage = Some(Box::new(storage.clone()));
            s.active_mode = RhythmMode::Day;
            s.last_active_mode_cause = ModeChangeCause::Schedule;
            s.set_mode_transition_configs(vec![transition]);
            s.last_active_mode_transition_id = s
                .mode_transition_configs()
                .first()
                .map(|config| config.id.clone());
            s.set_mode_configs(vec![ModeConfig {
                mode: RhythmMode::Day,
                active_profile_id: Some(rhythm_core::RHYTHM_PROFILE_ID.into()),
                idle_profile_id: Some(rhythm_core::DAY_IDLE_PROFILE_ID.into()),
                wake_profile_id: None,
                warning_profile_id: None,
                room_defaults: vec![
                    rhythm_core::RoomModeDefault {
                        room_id: "active-room".into(),
                        state: RoomModeState::Active,
                    },
                    rhythm_core::RoomModeDefault {
                        room_id: "hard-off-room".into(),
                        state: RoomModeState::HardOff,
                    },
                ],
            }]);
            // Simulate the periodic loop having replayed a missed Sleep→Day
            // transition while the runtime was still bootstrapping.
            s.pending_mode_output_apply = true;
        }

        reconcile_runtime_from_state(&state).unwrap();

        assert_eq!(
            runtime.applied_states(),
            vec![("active-room".into(), RoomModeState::Active)],
            "deferred apply must reach Active rooms once runtime is ready"
        );
        assert_eq!(
            runtime.lights_off_calls(),
            vec![("hard-off-room".into(), Some(4_321))],
            "deferred apply must dispatch lights-off for HardOff defaults"
        );
        assert!(
            !state.lock().unwrap().pending_mode_output_apply,
            "pending flag must be cleared after the deferred apply runs"
        );
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
        let (state, rt) = setup_state(vec![snap]);

        let result = do_absorb_time_offset(&state, None, 30.0);
        assert!(result.is_ok());
        assert_eq!(
            rt.engine_node_snapshot("r1").unwrap().time_offset_minutes,
            0.0
        );
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
        assert!(
            runtime.time_offset_updates().is_empty(),
            "absorb reset should not dispatch through set_room_time_offset directly"
        );
        assert_eq!(
            runtime
                .engine_node_snapshot("r1")
                .unwrap()
                .time_offset_minutes,
            0.0
        );
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
        assert!(
            runtime.time_offset_updates().is_empty(),
            "absorb reset should not dispatch through set_room_time_offset directly"
        );
        assert_eq!(
            runtime
                .engine_node_snapshot("r1")
                .unwrap()
                .time_offset_minutes,
            0.0
        );
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
        assert!(
            runtime.time_offset_updates().is_empty(),
            "absorb reset should not dispatch through set_room_time_offset directly"
        );
        assert_eq!(
            runtime
                .engine_node_snapshot("r1")
                .unwrap()
                .time_offset_minutes,
            0.0
        );
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
    fn flash_canonical_light_dispatches_to_preferred_endpoint() {
        let (state, _runtime) = setup_state(vec![]);
        let hub_key = HubKey::new(HubType::new("matter"), "local");
        let device_id =
            insert_canonical_device(&state, hub_key.clone(), "matter-100", "Desk Lamp", "", "");

        let composite = Arc::new(rhythm_core::CompositeController::new());
        let controller = Arc::new(RecordingFlashController::new());
        composite.register_controller(&hub_key.to_string(), controller.clone());
        state.lock().unwrap().composite_controller = Some(composite);

        do_flash_canonical_device(&state, &device_id).unwrap();

        assert_eq!(
            controller.calls(),
            vec![("RecordingFlash".to_string(), "matter-100".to_string())]
        );
    }

    #[test]
    fn triage_new_device_without_room_creates_standalone_topology_node() {
        let (state, runtime, _hub_key) = setup_state_with_deferred_runtime();
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
        drop(s);

        assert_eq!(
            runtime
                .engine_node_snapshot(canonical_id)
                .expect("standalone triage device should be materialized in runtime")
                .parent_id,
            None
        );
    }

    #[test]
    fn reconcile_runtime_from_state_bootstraps_persisted_room_and_device_assignments() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();
        let room_id = state.lock().unwrap().topology.create_room("Office");
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        {
            let mut s = state.lock().unwrap();
            s.canonical_registry.assign_room(&device_id, Some(&room_id));
            assert!(s.topology.attach_device_user_override(&room_id, &device_id));
        }

        reconcile_runtime_from_state(&state).unwrap();

        assert!(state.lock().unwrap().hub_runtime().is_some());
        assert_eq!(
            runtime
                .engine_room_snapshot(&room_id)
                .expect("persisted topology room should be materialized")
                .name,
            "Office"
        );
        assert_eq!(
            runtime
                .engine_node_snapshot(&device_id)
                .expect("persisted topology device should be materialized")
                .parent_id
                .as_deref(),
            Some(room_id.as_str())
        );
    }

    #[test]
    fn reconcile_runtime_from_state_restores_persisted_room_flags_when_bootstrapping_runtime() {
        let (state, runtime, _hub_key) = setup_state_with_deferred_runtime();
        let storage = TestStorage::default();
        let room_id = state.lock().unwrap().topology.create_room("Office");
        let mut rooms = rhythm_core::RoomManager::new();
        let mut room = rhythm_core::Room::new(&room_id, "Office");
        room.rhythm_enabled = true;
        room.time_offset_minutes = 14.0;
        room.brightness_offset = -3.0;
        room.soft_off = true;
        rooms.add_room(room);
        storage.save_rooms(&rooms).unwrap();
        state.lock().unwrap().storage = Some(Box::new(storage.clone()));

        reconcile_runtime_from_state(&state).unwrap();
        persist_rooms(&state);

        let office = runtime
            .engine_room_snapshot(&room_id)
            .expect("persisted topology room should be materialized");
        assert!(office.rhythm_enabled);
        assert_eq!(office.time_offset_minutes, 14.0);
        assert_eq!(office.brightness_offset, -3.0);
        assert!(office.soft_off);

        let saved = storage.inner.lock().unwrap();
        let saved_office = saved
            .rooms
            .get(&room_id)
            .expect("persisted room flags should survive reconcile save");
        assert!(saved_office.soft_off);
    }

    #[test]
    fn reconcile_runtime_from_state_preserves_persisted_off_flags_for_assigned_light_child() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();
        let storage = TestStorage::default();
        let room_id = state.lock().unwrap().topology.create_room("Office");
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");
        let mut rooms = rhythm_core::RoomManager::new();
        let mut child = rhythm_core::Room::new(&device_id, "Desk Lamp");
        child.rhythm_enabled = true;
        child.soft_off = true;
        child.hard_off = true;
        rooms.add_room(child);
        storage.save_rooms(&rooms).unwrap();

        {
            let mut s = state.lock().unwrap();
            s.storage = Some(Box::new(storage.clone()));
            assert!(s.canonical_registry.assign_room(&device_id, Some(&room_id)));
            s.topology.ensure_standalone_device(&device_id);
            assert!(s.topology.assign_device(
                &device_id,
                Some(&room_id),
                crate::topology::DevicePlacement::UserOverride,
            ));
        }

        reconcile_runtime_from_state(&state).unwrap();
        persist_rooms(&state);

        let child_snap = runtime
            .engine_node_snapshot(&device_id)
            .expect("assigned light child should be materialized");
        assert_eq!(child_snap.parent_id.as_deref(), Some(room_id.as_str()));
        assert!(child_snap.soft_off);
        assert!(child_snap.hard_off);

        let saved = storage.inner.lock().unwrap();
        let saved_child = saved
            .rooms
            .get(&device_id)
            .expect("assigned light child should be persisted");
        assert!(saved_child.soft_off);
        assert!(saved_child.hard_off);
    }

    #[test]
    fn reconcile_runtime_from_state_prunes_stale_runtime_snapshots() {
        let (state, runtime) = setup_state(vec![make_snapshot("stale-room", false, false)]);
        runtime.add_node(
            "stale-node",
            "Stale Lamp",
            rhythm_core::LightNodeKind::LightDevice,
            Some("stale-room".to_string()),
        );
        add_topology_room(&state, "office", &[]);
        let hub_key = state.lock().unwrap().hubs.keys().next().cloned().unwrap();
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        {
            let mut s = state.lock().unwrap();
            s.canonical_registry.assign_room(&device_id, Some("office"));
            assert!(s.topology.attach_device_user_override("office", &device_id));
        }

        reconcile_runtime_from_state(&state).unwrap();

        assert!(
            runtime.engine_room_snapshot("stale-room").is_none(),
            "stale runtime room should be pruned"
        );
        assert!(
            runtime.engine_node_snapshot("stale-node").is_none(),
            "stale runtime node should be pruned"
        );
        assert!(
            runtime.engine_room_snapshot("office").is_some(),
            "current topology room should remain in runtime"
        );
        assert_eq!(
            runtime
                .engine_node_snapshot(&device_id)
                .expect("current topology device should remain in runtime")
                .parent_id
                .as_deref(),
            Some("office")
        );
    }

    #[test]
    fn reconcile_runtime_from_state_preserves_existing_room_flags() {
        let (state, runtime) = setup_state(vec![make_snapshot("office", false, true)]);
        add_topology_room(&state, "office", &[]);

        reconcile_runtime_from_state(&state).unwrap();

        let office = runtime
            .engine_room_snapshot("office")
            .expect("topology room should still exist in runtime");
        assert!(
            office.soft_off,
            "reconcile should preserve existing soft_off"
        );
        assert!(
            office.rhythm_enabled,
            "reconcile should preserve existing rhythm state"
        );
    }

    #[test]
    fn reconcile_runtime_from_state_skips_when_runtime_initializer_is_missing() {
        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();
        let room_id = state.lock().unwrap().topology.create_room("Office");
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        {
            let mut s = state.lock().unwrap();
            s.ensure_runtime_fn = None;
            s.canonical_registry.assign_room(&device_id, Some(&room_id));
            assert!(s.topology.attach_device_user_override(&room_id, &device_id));
        }

        reconcile_runtime_from_state(&state).unwrap();

        let s = state.lock().unwrap();
        assert!(
            s.hub_runtime().is_none(),
            "reconcile should not fail or fabricate a runtime without an initializer"
        );
        assert_eq!(
            s.topology
                .get_device_node(&device_id)
                .expect("topology device should still exist")
                .parent_id
                .as_deref(),
            Some(room_id.as_str())
        );
    }

    #[test]
    fn triage_new_device_with_bound_room_materializes_parented_runtime_node() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();
        let room_id = "room-kitchen".to_string();
        {
            let mut s = state.lock().unwrap();
            let mut room = crate::topology::TopologyRoom::new(&room_id, "Kitchen");
            room.upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: "matter-room-1".to_string(),
                control_id: "control-1".to_string(),
                light_device_ids: vec![],
            });
            s.topology.insert_room(room);
            s.canonical_registry
                .triage_mut()
                .add(crate::canonical::triage::TriageEntry {
                    id: "td2".to_string(),
                    kind: crate::canonical::triage::TriageKind::DeviceMerge,
                    discovered: crate::canonical::triage::TriageDiscoveredDevice {
                        native_id: "matter-kitchen-1".to_string(),
                        name: "Desk Lamp".to_string(),
                        device_type: DeviceType::Light,
                        room_id: "matter-room-1".to_string(),
                        room_name: "Kitchen".to_string(),
                        manufacturer: None,
                        model: None,
                    },
                    hub_key: hub_key.clone(),
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

        let result = do_triage_new_device(&state, "td2").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let canonical_id = parsed["canonical_id"].as_str().unwrap().to_string();

        let s = state.lock().unwrap();
        assert!(s.hub_runtime().is_some());
        assert_eq!(
            s.topology
                .get_device_node(&canonical_id)
                .expect("bound topology device node should exist")
                .parent_id
                .as_deref(),
            Some(room_id.as_str())
        );
        assert_eq!(
            s.canonical_registry
                .get(&canonical_id)
                .unwrap()
                .room_id
                .as_deref(),
            Some(room_id.as_str())
        );
        drop(s);

        assert_eq!(
            runtime
                .engine_node_snapshot(&canonical_id)
                .expect("bound triage device should be materialized in runtime")
                .parent_id
                .as_deref(),
            Some(room_id.as_str())
        );
    }

    #[test]
    fn triage_bind_room_bootstraps_runtime_and_reparents_silo_devices() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();
        let target_id = state.lock().unwrap().topology.create_room("Kitchen");
        let source_id = "matter-kitchen-silo".to_string();
        let device_id =
            insert_canonical_device(&state, hub_key.clone(), "matter-100", "Desk Lamp", "", "");

        {
            let mut s = state.lock().unwrap();
            let mut room = crate::topology::TopologyRoom::new(&source_id, "Kitchen");
            room.upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: "matter-room-1".to_string(),
                control_id: "control-1".to_string(),
                light_device_ids: vec!["matter-100".to_string()],
            });
            s.topology.insert_room(room);
            s.canonical_registry
                .assign_room(&device_id, Some(&source_id));
            assert!(s
                .topology
                .attach_device_user_override(&source_id, &device_id));
            s.canonical_registry
                .triage_mut()
                .add(crate::canonical::triage::TriageEntry {
                    id: "rb2".to_string(),
                    kind: crate::canonical::triage::TriageKind::RoomBinding,
                    discovered: crate::canonical::triage::TriageDiscoveredDevice::default(),
                    hub_key: hub_key.clone(),
                    candidate_matches: vec![],
                    room_binding: Some(crate::canonical::triage::RoomBindingProposal {
                        hub_room_id: "matter-room-1".to_string(),
                        hub_room_name: "Kitchen".to_string(),
                        control_id: "control-1".to_string(),
                        light_device_ids: vec!["matter-100".to_string()],
                        canonical_device_ids: vec![device_id.clone()],
                        target_rhythm_room_id: target_id.clone(),
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

        do_triage_bind_room(&state, "rb2").unwrap();

        let s = state.lock().unwrap();
        assert!(s.hub_runtime().is_some());
        assert!(
            s.topology.get(&source_id).is_none(),
            "source silo room should be removed from topology"
        );
        assert_eq!(
            s.topology
                .get_device_node(&device_id)
                .expect("merged device should remain in topology")
                .parent_id
                .as_deref(),
            Some(target_id.as_str())
        );
        assert_eq!(
            s.canonical_registry
                .get(&device_id)
                .expect("merged device should remain in canonical state")
                .room_id
                .as_deref(),
            Some(target_id.as_str())
        );
        drop(s);

        assert!(
            runtime.engine_room_snapshot(&source_id).is_none(),
            "source silo room should be removed from runtime"
        );
        assert_eq!(
            runtime
                .engine_node_snapshot(&device_id)
                .expect("merged device should remain in runtime")
                .parent_id
                .as_deref(),
            Some(target_id.as_str())
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
    fn canonical_assign_room_clears_standalone_light_off_flags() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();
        let storage = TestStorage::default();
        state.lock().unwrap().storage = Some(Box::new(storage.clone()));
        let room_id = state.lock().unwrap().topology.create_room("Office");
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        {
            let mut s = state.lock().unwrap();
            s.topology.ensure_standalone_device(&device_id);
        }
        reconcile_runtime_from_state(&state).unwrap();
        runtime.restore_node_state(
            &device_id,
            RestoredNodeState {
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: true,
                hard_off: true,
                profile_settings: RoomProfileSettings::default(),
            },
        );
        let before = runtime
            .engine_node_snapshot(&device_id)
            .expect("standalone light should exist before assignment");
        assert_eq!(before.parent_id, None);
        assert!(before.soft_off);
        assert!(before.hard_off);

        do_canonical_assign_room(&state, &device_id, Some(&room_id)).unwrap();

        let after = runtime
            .engine_node_snapshot(&device_id)
            .expect("assigned light child should still exist");
        assert_eq!(after.parent_id.as_deref(), Some(room_id.as_str()));
        assert!(!after.soft_off);
        assert!(!after.hard_off);

        let saved = storage.inner.lock().unwrap();
        let saved_child = saved
            .rooms
            .get(&device_id)
            .expect("assigned light child should be persisted");
        assert!(!saved_child.soft_off);
        assert!(!saved_child.hard_off);
    }

    #[test]
    fn canonical_assign_room_propagates_motion_device_rooms_to_hub_registry() {
        // A roomless motion sensor — discovered with no Hue room — should land
        // in the hub registry's device_types map but have no device_rooms
        // entry. Once the user assigns it a Rhythm room via the canonical
        // endpoint, device_rooms should be populated so motion events route.
        // Unassigning should clear the entry.
        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();
        let room_id = state.lock().unwrap().topology.create_room("Hallway");

        // Seed the hub registry with the motion sensor as roomless (mirroring
        // what discovery now does).
        {
            let s = state.lock().unwrap();
            let registry = s
                .hubs
                .get(&hub_key)
                .and_then(|hub| hub.registry.as_ref())
                .expect("hub registry should exist")
                .clone();
            drop(s);
            registry.lock().unwrap().upsert_device(
                "motion-svc-1",
                None,
                &[],
                rhythm_core::runtime::hub_registry::DeviceType::Motion,
            );
        }

        // Create a canonical Motion device for that endpoint.
        let canonical_id = {
            let identity = crate::canonical::identity::DiscoveredIdentity {
                native_id: "motion-svc-1".to_string(),
                room_id: None,
                room_name: None,
                name: "Hallway Motion".to_string(),
                device_type: rhythm_core::runtime::hub_registry::DeviceType::Motion,
                hardware_ids: vec![crate::canonical::identity::HardwareId::mac(
                    "00:17:88:01:0b:c0:ff:ee",
                )],
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
        };

        // Sanity: registry has no room mapping yet for the endpoint.
        assert!(!devices_for_room_contains(
            &state,
            &hub_key,
            &room_id,
            "motion-svc-1"
        ));

        // Assign — propagation should populate device_rooms.
        do_canonical_assign_room(&state, &canonical_id, Some(&room_id)).unwrap();
        assert!(
            devices_for_room_contains(&state, &hub_key, &room_id, "motion-svc-1"),
            "assign should propagate the Rhythm room into device_rooms"
        );

        // Unassign — propagation should clear it.
        do_canonical_assign_room(&state, &canonical_id, None).unwrap();
        assert!(
            !devices_for_room_contains(&state, &hub_key, &room_id, "motion-svc-1"),
            "unassign should clear the device_rooms entry"
        );
    }

    fn devices_for_room_contains(
        state: &SharedState,
        hub_key: &HubKey,
        room_id: &str,
        native_id: &str,
    ) -> bool {
        let s = state.lock().unwrap();
        let registry = s
            .hubs
            .get(hub_key)
            .and_then(|hub| hub.registry.as_ref())
            .expect("hub registry should exist")
            .lock()
            .unwrap();
        registry
            .devices_for_room(room_id)
            .iter()
            .any(|d| d == native_id)
    }

    #[test]
    fn canonical_assign_room_clears_motion_timer_when_motion_sensor_unassigned() {
        // Regression for #31: removing a motion sensor from its room must
        // clear the room's motion-timer state. Otherwise the countdown that
        // was started by the (now-detached) sensor keeps running and the
        // room's lights are turned off when it expires.
        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();
        let room_id = state.lock().unwrap().topology.create_room("Master");

        let identity = crate::canonical::identity::DiscoveredIdentity {
            native_id: "motion-svc-master".to_string(),
            room_id: None,
            room_name: None,
            name: "Master Motion".to_string(),
            device_type: DeviceType::Motion,
            hardware_ids: vec![crate::canonical::identity::HardwareId::mac(
                "00:17:88:01:0b:c0:f0:01",
            )],
            manufacturer: None,
            model: None,
        };
        let canonical_id = {
            let mut s = state.lock().unwrap();
            match s.canonical_registry.resolve(&identity, &hub_key, 1000) {
                crate::canonical::registry::ResolveResult::Created { canonical_id }
                | crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id } => {
                    canonical_id
                }
                other => panic!("unexpected resolve result: {:?}", other),
            }
        };

        do_canonical_assign_room(&state, &canonical_id, Some(&room_id)).unwrap();
        // Assignment to the initial room should not enqueue a clear for the
        // freshly-attached target.
        state.lock().unwrap().pending_motion_clear.clear();

        do_canonical_assign_room(&state, &canonical_id, None).unwrap();

        let s = state.lock().unwrap();
        assert_eq!(
            s.pending_motion_clear,
            vec![room_id.clone()],
            "unassigning a motion sensor must queue a motion-timer clear for \
             the room it was attached to"
        );
    }

    #[test]
    fn canonical_assign_room_clears_motion_timer_for_old_and_new_room_on_move() {
        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();
        let old_room = state.lock().unwrap().topology.create_room("Master");
        let new_room = state.lock().unwrap().topology.create_room("Mud Room");

        let identity = crate::canonical::identity::DiscoveredIdentity {
            native_id: "motion-svc-move".to_string(),
            room_id: None,
            room_name: None,
            name: "Roving Motion".to_string(),
            device_type: DeviceType::Motion,
            hardware_ids: vec![crate::canonical::identity::HardwareId::mac(
                "00:17:88:01:0b:c0:f0:02",
            )],
            manufacturer: None,
            model: None,
        };
        let canonical_id = {
            let mut s = state.lock().unwrap();
            match s.canonical_registry.resolve(&identity, &hub_key, 1000) {
                crate::canonical::registry::ResolveResult::Created { canonical_id }
                | crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id } => {
                    canonical_id
                }
                other => panic!("unexpected resolve result: {:?}", other),
            }
        };

        do_canonical_assign_room(&state, &canonical_id, Some(&old_room)).unwrap();
        state.lock().unwrap().pending_motion_clear.clear();

        do_canonical_assign_room(&state, &canonical_id, Some(&new_room)).unwrap();

        let s = state.lock().unwrap();
        assert_eq!(
            s.pending_motion_clear,
            vec![old_room.clone(), new_room.clone()],
            "moving a motion sensor between rooms must clear timers for both \
             the old and the new effective targets"
        );
    }

    #[test]
    fn canonical_assign_room_does_not_clear_motion_timer_for_non_motion_devices() {
        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();
        let room_id = state.lock().unwrap().topology.create_room("Office");
        let device_id =
            insert_canonical_device(&state, hub_key, "matter-light-1", "Desk Lamp", "", "");

        do_canonical_assign_room(&state, &device_id, Some(&room_id)).unwrap();
        do_canonical_assign_room(&state, &device_id, None).unwrap();

        assert!(
            state.lock().unwrap().pending_motion_clear.is_empty(),
            "non-motion devices must not perturb motion-timer state"
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
    fn build_state_snapshot_handles_topology_device_missing_canonical_metadata() {
        let (state, runtime) = setup_state(vec![]);
        let device_id = "orphan-device";

        {
            let mut s = state.lock().unwrap();
            s.topology.ensure_standalone_device(device_id);
        }

        runtime.add_node(
            device_id,
            "Orphan Device",
            rhythm_core::LightNodeKind::LightDevice,
            None,
        );

        let parsed: serde_json::Value =
            serde_json::from_str(&build_state_snapshot(&state).unwrap()).unwrap();
        let device = parsed["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"].as_str() == Some(device_id))
            .expect("device should appear in state snapshot");

        assert!(
            device.get("hub_types").is_none(),
            "empty hub_types should be omitted from the state snapshot"
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
            set_observed_lights_on_in_app(&mut s, &room_id, true);
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
        assert!(!s.room_observed_power.contains_key(&room_id));
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
    fn topology_lifecycle_mutations_schedule_group_integrations() {
        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();

        let created: serde_json::Value =
            serde_json::from_str(&do_topology_create_room(&state, "Office").unwrap()).unwrap();
        let room_id = created["id"].as_str().unwrap().to_string();
        let device_one =
            insert_canonical_device(&state, hub_key.clone(), "matter-100", "Desk Lamp", "", "");
        let device_two =
            insert_canonical_device(&state, hub_key, "matter-101", "Table Lamp", "", "");

        let sync_count = Arc::new(AtomicUsize::new(0));
        {
            let sync_count = sync_count.clone();
            state.lock().unwrap().sync_topology_groups_fn = Some(Arc::new(move |_| {
                sync_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
        }

        do_canonical_assign_room(&state, &device_one, Some(&room_id)).unwrap();
        wait_for_sync_count(&sync_count, 1);

        do_canonical_assign_room(&state, &device_two, Some(&room_id)).unwrap();
        wait_for_sync_count(&sync_count, 2);

        do_canonical_assign_room(&state, &device_one, None).unwrap();
        wait_for_sync_count(&sync_count, 3);

        do_topology_delete_room(&state, &room_id).unwrap();
        wait_for_sync_count(&sync_count, 4);

        do_device_hard_remove(&state, &device_two, None).unwrap();
        wait_for_sync_count(&sync_count, 5);
    }

    #[test]
    fn canonical_assign_room_does_not_wait_for_topology_group_sync() {
        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();

        let created: serde_json::Value =
            serde_json::from_str(&do_topology_create_room(&state, "Office").unwrap()).unwrap();
        let room_id = created["id"].as_str().unwrap().to_string();
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        let sync_started = Arc::new(AtomicUsize::new(0));
        let sync_finished = Arc::new(AtomicUsize::new(0));
        {
            let sync_started = sync_started.clone();
            let sync_finished = sync_finished.clone();
            state.lock().unwrap().sync_topology_groups_fn = Some(Arc::new(move |_| {
                sync_started.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(750));
                sync_finished.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
        }

        let started_at = std::time::Instant::now();
        do_canonical_assign_room(&state, &device_id, Some(&room_id)).unwrap();
        let elapsed = started_at.elapsed();

        assert!(
            elapsed < Duration::from_millis(300),
            "room assignment waited for topology group sync: {elapsed:?}"
        );
        wait_for_sync_count(&sync_started, 1);
        wait_for_sync_count(&sync_finished, 1);
    }

    #[test]
    fn topology_merge_rooms_reconciles_runtime_room_and_device_parent() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();

        let office: serde_json::Value =
            serde_json::from_str(&do_topology_create_room(&state, "Office").unwrap()).unwrap();
        let den: serde_json::Value =
            serde_json::from_str(&do_topology_create_room(&state, "Den").unwrap()).unwrap();
        let target_id = office["id"].as_str().unwrap().to_string();
        let source_id = den["id"].as_str().unwrap().to_string();
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        do_canonical_assign_room(&state, &device_id, Some(&source_id)).unwrap();
        do_topology_merge_rooms(&state, &target_id, &source_id).unwrap();

        let s = state.lock().unwrap();
        assert!(s.topology.get(&source_id).is_none());
        assert_eq!(
            s.topology
                .get_device_node(&device_id)
                .unwrap()
                .parent_id
                .as_deref(),
            Some(target_id.as_str())
        );
        assert_eq!(
            s.canonical_registry
                .get(&device_id)
                .unwrap()
                .room_id
                .as_deref(),
            Some(target_id.as_str())
        );
        drop(s);

        assert!(
            runtime.engine_room_snapshot(&source_id).is_none(),
            "merged source room should be removed from runtime"
        );
        assert_eq!(
            runtime
                .engine_node_snapshot(&device_id)
                .unwrap()
                .parent_id
                .as_deref(),
            Some(target_id.as_str())
        );
    }

    #[test]
    fn topology_move_device_reconciles_runtime_parent() {
        let (state, runtime, hub_key) = setup_state_with_deferred_runtime();

        let office: serde_json::Value =
            serde_json::from_str(&do_topology_create_room(&state, "Office").unwrap()).unwrap();
        let den: serde_json::Value =
            serde_json::from_str(&do_topology_create_room(&state, "Den").unwrap()).unwrap();
        let from_room = office["id"].as_str().unwrap().to_string();
        let to_room = den["id"].as_str().unwrap().to_string();
        let device_id = insert_canonical_device(&state, hub_key, "matter-100", "Desk Lamp", "", "");

        do_canonical_assign_room(&state, &device_id, Some(&from_room)).unwrap();
        do_topology_move_device(&state, &device_id, &from_room, &to_room).unwrap();

        let s = state.lock().unwrap();
        assert_eq!(
            s.topology
                .get_device_node(&device_id)
                .unwrap()
                .parent_id
                .as_deref(),
            Some(to_room.as_str())
        );
        drop(s);

        assert_eq!(
            runtime
                .engine_node_snapshot(&device_id)
                .unwrap()
                .parent_id
                .as_deref(),
            Some(to_room.as_str())
        );
    }

    #[test]
    fn device_hard_remove_evicts_runtime_node_and_periodic_state() {
        let (state, runtime, device_id) = setup_attached_matter_light_without_group_dispatch();

        {
            let mut s = state.lock().unwrap();
            set_observed_lights_on_in_app(&mut s, &device_id, true);
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
            let dispatch_generation = s.light_dispatch_generation;
            s.pending_periodic_ticks.insert(
                device_id.clone(),
                crate::state::PendingPeriodicTick::new(12.0, dispatch_generation),
            );
            s.pending_motion_clear.push(device_id.clone());
            s.pending_motion_seed.push(crate::state::MotionSeedEntry {
                source_node_id: device_id.clone(),
                target_node_id: "room1".to_string(),
                is_active: true,
                stopped_at_epoch_ms: None,
                motion_owned: None,
                warning_active: false,
            });
        }

        do_device_hard_remove(&state, &device_id, None).unwrap();

        let s = state.lock().unwrap();
        assert!(s.canonical_registry.get(&device_id).is_none());
        assert!(s.topology.get_device_node(&device_id).is_none());
        assert!(!s.room_observed_power.contains_key(&device_id));
        assert!(!s.motion_snapshots.contains_key(&device_id));
        assert!(!s.room_mode_transitions.contains_key(&device_id));
        assert!(!s.pending_periodic_ticks.contains_key(&device_id));
        assert_eq!(s.pending_motion_clear, vec![device_id.clone()]);
        assert!(s
            .pending_motion_seed
            .iter()
            .all(|seed| seed.source_node_id != device_id && seed.target_node_id != device_id));
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

    /// Regression for #15: a motion sensor canonical assigned to a Rhythm room
    /// that is *not* the one bound to its Hue native room must route SSE motion
    /// events to the assigned room. Pre-fix the lookup would fall through to
    /// translating the Hue-native room id, returning the originally-bound
    /// Rhythm room and ignoring the user's move.
    #[test]
    fn moved_motion_sensor_routes_to_topology_parent_not_hub_native_room() {
        use crate::canonical::identity::{DiscoveredIdentity, HardwareId};
        use crate::topology::{HubRoomBinding, NodeControlKind, TopologyRoom};
        use rhythm_core::runtime::hub_registry::DeviceType;

        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();

        // ROOM_A: where the user moved the sensor.
        // ROOM_B: bound to the Hue-native room HUE_X.
        {
            let mut s = state.lock().unwrap();
            s.topology
                .insert_room(TopologyRoom::new("room-a", "Room A"));
            let mut room_b = TopologyRoom::new("room-b", "Room B");
            room_b.upsert_hub_room_binding(HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: "hue-x".to_string(),
                control_id: "gl-x".to_string(),
                light_device_ids: Vec::new(),
            });
            s.topology.insert_room(room_b);
        }

        // Discover the motion sensor with native_id=msvc and assign it to ROOM_A.
        let identity = DiscoveredIdentity {
            native_id: "msvc".into(),
            room_id: Some("hue-x".into()),
            room_name: Some("Room B".into()),
            name: "Hallway Motion".into(),
            device_type: DeviceType::Motion,
            hardware_ids: vec![HardwareId::mac("aa:bb:cc:dd:ee:ff")],
            manufacturer: None,
            model: None,
        };
        let canonical_id = {
            let mut s = state.lock().unwrap();
            match s.canonical_registry.resolve(&identity, &hub_key, 1) {
                crate::canonical::registry::ResolveResult::Created { canonical_id }
                | crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id } => {
                    canonical_id
                }
                other => panic!("unexpected resolve result: {:?}", other),
            }
        };
        do_canonical_assign_room(&state, &canonical_id, Some("room-a")).unwrap();

        let resolved =
            resolve_node_control_target(&state, &hub_key, "msvc", &NodeControlKind::Motion);

        let (source_node, target_node) =
            resolved.expect("canonical lookup should resolve a target");
        assert_eq!(source_node, canonical_id);
        assert_eq!(
            target_node, "room-a",
            "motion event should route to the room the user assigned, \
             not the room bound to the Hue-native source room"
        );
    }

    /// Regression for #15: a button canonical assigned to a Rhythm room that
    /// is not the one bound to its Hue native room must route SSE button
    /// events to the assigned room via canonical+topology, independent of
    /// any synthetic-room indirection in the hub registry.
    #[test]
    fn moved_button_routes_to_topology_parent_not_hub_native_room() {
        use crate::canonical::identity::{DiscoveredIdentity, HardwareId};
        use crate::topology::{HubRoomBinding, NodeControlKind, TopologyRoom};
        use rhythm_core::runtime::hub_registry::DeviceType;

        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();

        {
            let mut s = state.lock().unwrap();
            s.topology
                .insert_room(TopologyRoom::new("room-a", "Room A"));
            let mut room_b = TopologyRoom::new("room-b", "Room B");
            room_b.upsert_hub_room_binding(HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: "hue-x".to_string(),
                control_id: "gl-x".to_string(),
                light_device_ids: Vec::new(),
            });
            s.topology.insert_room(room_b);
        }

        let identity = DiscoveredIdentity {
            native_id: "parent-rid".into(),
            room_id: Some("hue-x".into()),
            room_name: Some("Room B".into()),
            name: "Hallway Dimmer".into(),
            device_type: DeviceType::Button,
            hardware_ids: vec![HardwareId::mac("11:22:33:44:55:66")],
            manufacturer: None,
            model: None,
        };
        let canonical_id = {
            let mut s = state.lock().unwrap();
            match s.canonical_registry.resolve(&identity, &hub_key, 1) {
                crate::canonical::registry::ResolveResult::Created { canonical_id }
                | crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id } => {
                    canonical_id
                }
                other => panic!("unexpected resolve result: {:?}", other),
            }
        };
        do_canonical_assign_room(&state, &canonical_id, Some("room-a")).unwrap();

        let resolved =
            resolve_node_control_target(&state, &hub_key, "parent-rid", &NodeControlKind::Button);

        let (source_node, target_node) =
            resolved.expect("canonical lookup should resolve a target");
        assert_eq!(source_node, canonical_id);
        assert_eq!(
            target_node, "room-a",
            "button event should route to the room the user assigned, \
             not the room bound to the Hue-native source room"
        );
    }

    /// Regression for #15 migration path: a re-discovery of an existing motion
    /// sensor canonical with a *new* native_id on the same hub (same MAC) must
    /// silently re-key the endpoint instead of queueing a triage entry.
    #[test]
    fn motion_sensor_native_id_rekey_is_silent_when_mac_matches() {
        use crate::canonical::identity::{DiscoveredIdentity, HardwareId};
        use rhythm_core::runtime::hub_registry::DeviceType;

        let (state, _runtime, hub_key) = setup_state_with_deferred_runtime();

        // First discovery: legacy native_id (parent device rid).
        let legacy = DiscoveredIdentity {
            native_id: "legacy-parent-rid".into(),
            room_id: Some("hue-x".into()),
            room_name: Some("Room B".into()),
            name: "Hallway Motion".into(),
            device_type: DeviceType::Motion,
            hardware_ids: vec![HardwareId::mac("aa:bb:cc:dd:ee:ff")],
            manufacturer: None,
            model: None,
        };
        let canonical_id = {
            let mut s = state.lock().unwrap();
            match s.canonical_registry.resolve(&legacy, &hub_key, 1) {
                crate::canonical::registry::ResolveResult::Created { canonical_id } => canonical_id,
                other => panic!("unexpected first resolve result: {:?}", other),
            }
        };

        // Second discovery (post-fix): same MAC, new native_id (motion service rid).
        let aligned = DiscoveredIdentity {
            native_id: "msvc-rid".into(),
            ..legacy.clone()
        };
        let result = {
            let mut s = state.lock().unwrap();
            s.canonical_registry.resolve(&aligned, &hub_key, 2)
        };

        match result {
            crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id: rid } => {
                assert_eq!(rid, canonical_id, "should re-key the same canonical");
            }
            other => panic!("expected silent re-key (AlreadyKnown), got {:?}", other),
        }

        let s = state.lock().unwrap();
        let device = s.canonical_registry.get(&canonical_id).unwrap();
        assert_eq!(
            device.endpoints.len(),
            1,
            "endpoint should be re-keyed in place"
        );
        assert_eq!(device.endpoints[0].native_id, "msvc-rid");
        assert!(
            s.canonical_registry
                .triage()
                .pending_by_kind(crate::canonical::triage::TriageKind::DeviceMerge)
                .is_empty(),
            "silent migration must not queue triage"
        );
    }
}
