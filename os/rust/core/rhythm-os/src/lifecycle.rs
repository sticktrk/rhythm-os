//! Generic hub lifecycle functions.
//!
//! Shared connect, runtime creation, event translation, and configuration
//! logic used by all hub crates. Hub-specific behavior is injected via
//! closures rather than traits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::{debug, info, warn};

use crate::canonical::identity::HubKey;
use crate::hub::{ActiveHub, HubCredentials, HubEvent, HubType};
use crate::registry::{HubDeviceRegistry, RegistrySnapshot};
use crate::state::SharedState;

const HUB_EVENT_CHANNEL_CAPACITY: usize = 256;

/// Forward hub dispatch failures to the SSE event bus and hold the
/// user-visible `pending_dispatch` flag until the physical outcome.
///
/// Command dispatch is fire-and-forget: `turn_on`/`turn_off` only enqueue.
/// Successful deliveries surface as the usual `NodeState` updates; anything
/// that fails, times out, is cooldown-skipped, or dropped is broadcast as a
/// `DispatchFailure` event so clients see it without watching server logs.
///
/// Queued events mark the node pending and their paired outcomes clear it,
/// so the app's spinner ends when the lights actually resolved — not when
/// the command was merely handed to the hub mailbox. Only nodes that already
/// have user-initiated pending work get marked: periodic ticks dispatch
/// through the same mailboxes, and flagging them would paint card spinners
/// every rhythm cycle (for seconds at a time on a slow or failing target).
pub fn install_dispatch_outcome_listener(
    state: &SharedState,
    composite: &rhythm_core::CompositeController,
) {
    // Both listeners run inline: queued events fire on the engine's dispatch
    // stack, which never holds engine locks across `turn_on`/`turn_off` (the
    // plan is computed under the lock, dispatch happens after it drops), and
    // inline execution keeps mark/clear strictly ordered — deferring to a
    // task pool could run a clear before its mark and leak the flag.
    let queued_state = state.clone();
    composite.set_queued_listener(Arc::new(move |queued| {
        if let Some(old) = queued.superseded_node_id.as_deref() {
            crate::commands::clear_node_dispatch_pending(&queued_state, old);
        }
        if crate::commands::node_has_user_pending_dispatch(&queued_state, &queued.node_id) {
            crate::commands::mark_node_dispatch_pending(&queued_state, &queued.node_id);
        }
    }));

    let state = state.clone();
    composite.set_outcome_listener(Arc::new(move |outcome| {
        if let rhythm_core::HubDispatchStatus::Accepted {
            command_ids,
            controller_stream_id,
        } = &outcome.status
        {
            match crate::commands::register_integration_dispatch(
                &state,
                &outcome.hub_key,
                &outcome.node_id,
                controller_stream_id,
                command_ids,
            ) {
                Ok(registration) => {
                    for early in registration.early_outcomes {
                        if !matches!(early.status, crate::hub::HubCommandOutcomeStatus::Succeeded) {
                            crate::state::emit_server_event(
                                &state,
                                crate::server_event::ServerEvent::DispatchFailure {
                                    hub_type: outcome.hub_type.clone(),
                                    hub_key: outcome.hub_key.clone(),
                                    node_id: outcome.node_id.clone(),
                                    target_node_id:
                                        crate::commands::resolve_dispatch_target_node_id(
                                            &state,
                                            &outcome.hub_key,
                                            &early.device_id,
                                        ),
                                    target: early.device_id,
                                    kind: "matter_controller_command".to_string(),
                                    status: early.status.as_str().to_string(),
                                    detail: early.detail,
                                    queued_ms: outcome.queued_ms,
                                    dispatch_ms: outcome.dispatch_ms,
                                    epoch_ms: chrono::Utc::now().timestamp_millis(),
                                },
                            );
                            crate::commands::request_observed_power_refresh_after_dispatch_failure(
                                &state,
                                &outcome.node_id,
                            );
                        }
                    }
                    if registration.complete {
                        crate::commands::clear_node_dispatch_pending(&state, &outcome.node_id);
                    }
                }
                Err(error) => {
                    crate::state::emit_server_event(
                        &state,
                        crate::server_event::ServerEvent::DispatchFailure {
                            hub_type: outcome.hub_type.clone(),
                            hub_key: outcome.hub_key.clone(),
                            node_id: outcome.node_id.clone(),
                            target_node_id: crate::commands::resolve_dispatch_target_node_id(
                                &state,
                                &outcome.hub_key,
                                &outcome.target_label,
                            ),
                            target: outcome.target_label.clone(),
                            kind: outcome.kind.as_str().to_string(),
                            status: "acceptance_invalid".to_string(),
                            detail: Some(error),
                            queued_ms: outcome.queued_ms,
                            dispatch_ms: outcome.dispatch_ms,
                            epoch_ms: chrono::Utc::now().timestamp_millis(),
                        },
                    );
                    crate::commands::request_observed_power_refresh_after_dispatch_failure(
                        &state,
                        &outcome.node_id,
                    );
                    crate::commands::clear_node_dispatch_pending(&state, &outcome.node_id);
                }
            }
            return;
        }

        if !outcome.status.is_success() {
            // Broadcast the failure before clearing pending so clients hold
            // the failure by the time the spinner flag drops.
            crate::state::emit_server_event(
                &state,
                crate::server_event::ServerEvent::DispatchFailure {
                    hub_type: outcome.hub_type.clone(),
                    hub_key: outcome.hub_key.clone(),
                    node_id: outcome.node_id.clone(),
                    target_node_id: crate::commands::resolve_dispatch_target_node_id(
                        &state,
                        &outcome.hub_key,
                        &outcome.target_label,
                    ),
                    target: outcome.target_label.clone(),
                    kind: outcome.kind.as_str().to_string(),
                    status: outcome.status.as_str().to_string(),
                    detail: outcome.status.detail(),
                    queued_ms: outcome.queued_ms,
                    dispatch_ms: outcome.dispatch_ms,
                    epoch_ms: chrono::Utc::now().timestamp_millis(),
                },
            );
            crate::commands::request_observed_power_refresh_after_dispatch_failure(
                &state,
                &outcome.node_id,
            );
        }
        crate::commands::clear_node_dispatch_pending(&state, &outcome.node_id);
    }));
}

// ============================================================================
// connect_hub — replaces connect_hue_sse() and connect_ha()
// ============================================================================

/// Connect to a hub and set up the device registry.
///
/// Generic over hub type. The caller provides:
/// - `hub_type`: identifier for this hub
/// - `default_grouped_light_to_room_id`: HA sets true, Hue sets false
/// - `load_registry_snapshot`: optional snapshot to restore
/// - `hub_data_builder`: closure that creates hub-specific data from the registry
/// - `start_event_stream`: closure that starts the event stream
///
/// Returns `(ActiveHub, Receiver<HubEvent>)`. The runtime inside ActiveHub
/// is `None` — call `ensure_hub_runtime` when the first room arrives.
#[allow(clippy::too_many_arguments)]
pub fn connect_hub<D, F>(
    _state: &SharedState,
    hub_type: HubType,
    hub_key: HubKey,
    default_grouped_light_to_room_id: bool,
    load_registry_snapshot: Option<RegistrySnapshot>,
    hub_data_builder: D,
    start_event_stream: F,
) -> Result<(ActiveHub, Receiver<HubEvent>)>
where
    D: FnOnce(Arc<Mutex<HubDeviceRegistry>>) -> Box<dyn std::any::Any + Send + Sync>,
    F: FnOnce(Arc<Mutex<HubDeviceRegistry>>, Arc<AtomicBool>) -> Receiver<HubEvent>,
{
    // Build device registry (restore from snapshot if available)
    let mut registry = HubDeviceRegistry::with_options(default_grouped_light_to_room_id);
    if let Some(snapshot) = load_registry_snapshot {
        info!(target: "sys", "Restoring registry from snapshot...");
        registry.restore_from_snapshot(snapshot);
    }

    let registry = Arc::new(Mutex::new(registry));
    let shutdown = Arc::new(AtomicBool::new(false));

    // Start event stream via hub-specific closure, then tag every event with
    // the concrete hub key so downstream status and topology updates stay per-hub.
    let raw_event_rx = start_event_stream(registry.clone(), shutdown.clone());
    let event_rx = tag_hub_events(raw_event_rx, hub_key.clone());

    // Build hub-specific data
    let hub_data = hub_data_builder(registry.clone());

    let hub = ActiveHub {
        hub_type,
        hub_key,
        runtime: None,
        hub_data,
        registry: Some(registry),
        discovery: None, // Set by platform lifecycle after connect
        shutdown,
    };

    info!(
        target: "sys",
        "Hub initialized (runtime deferred until rooms are discovered)"
    );
    Ok((hub, event_rx))
}

/// Detect the fingerprint of a corrupted persisted-rooms file: every room at
/// its factory-default state (rhythm disabled, no offsets, no persisted state).
/// This is the signature of the historical `do_room_set` bug that dropped
/// params during runtime creation, and on detection the caller should fall
/// back to seeding rooms with `rhythm_enabled=true` rather than trusting the
/// broken payload.
///
/// Returns `false` for an empty list — a genuinely empty rooms file (first
/// boot) must not be treated as corruption.
pub(crate) fn persisted_rooms_look_corrupted(rooms: &rhythm_core::room::RoomManager) -> bool {
    let mut any = false;
    for room in rooms.iter() {
        any = true;
        if room.rhythm_enabled
            || room.disabled
            || room.time_offset_minutes != 0.0
            || room.brightness_offset != 0.0
            || room.soft_off
            || room.mood_active
            || room.standby_enabled
            || room.hard_off
            || !room.profile_settings.is_empty()
        {
            return false;
        }
    }
    any
}

fn tag_hub_events(raw_rx: Receiver<HubEvent>, hub_key: HubKey) -> Receiver<HubEvent> {
    tag_hub_events_with_capacity(raw_rx, hub_key, HUB_EVENT_CHANNEL_CAPACITY)
}

fn tag_hub_events_with_capacity(
    raw_rx: Receiver<HubEvent>,
    hub_key: HubKey,
    capacity: usize,
) -> Receiver<HubEvent> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<HubEvent>(capacity);

    let spawn_result = std::thread::Builder::new()
        .name(format!("hub-tag-{}", hub_key))
        .spawn(move || {
            while let Ok(event) = raw_rx.recv() {
                // Input events are non-coalescible. Backpressure the upstream
                // receiver when the bounded event queue is full so a press is
                // never acknowledged and then discarded by this adapter.
                if tx.send(event.with_hub_key(hub_key.clone())).is_err() {
                    return;
                }
            }
        });
    // Spawn failure (thread/fd exhaustion) drops `tx`; the event loop sees a
    // disconnected receiver. Degraded, but panicking here would abort a hub
    // configure request instead.
    if let Err(e) = spawn_result {
        warn!(target: "evt", "Failed to spawn hub event tagger thread: {}", e);
    }

    rx
}

// ============================================================================
// ensure_hub_runtime — replaces ensure_hue_runtime() and ensure_ha_runtime()
// ============================================================================

/// Create the RhythmRuntime for any hub.
///
/// Reads config from AppState, creates the runtime with the provided
/// controller. Restores persisted room state from storage.
///
/// # Arguments
///
/// * `state` - Shared application state
/// * `controller` - Platform-specific light controller
/// * `registry` - Shared device registry
/// * `warmup` - Optional TLS warmup closure (Hue uses this, HA passes None)
#[allow(clippy::type_complexity)]
pub fn ensure_hub_runtime<C: rhythm_core::LightController + Send + Sync + 'static>(
    state: &SharedState,
    hub_key: &HubKey,
    controller: C,
    registry: Arc<Mutex<HubDeviceRegistry>>,
    warmup: Option<Box<dyn FnOnce(&C)>>,
) -> Result<()> {
    use rhythm_core::solar::SolarTime;
    use rhythm_core::{
        DeviceRegistry, HubRegistry, RhythmRuntime, RuntimeConfig, RuntimeHandle,
        SimpleDeviceRegistry, SystemTimeProvider, ThreadScheduler, TimeProvider,
    };

    let (
        light_profiles,
        mode_configs,
        active_profile_id,
        runtime_config,
        utc_offset,
        latitude,
        longitude,
        eager_warmup,
        timezone_name,
    ) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;

        // Check if runtime already exists
        let any_runtime = s.hubs.values().any(|h| h.runtime.is_some());
        if any_runtime {
            return Ok(()); // Already running
        }

        (
            s.light_profile_configs
                .values()
                .cloned()
                .collect::<Vec<_>>(),
            s.mode_configs(),
            s.active_mode_profile_id(),
            s.runtime_config.clone(),
            s.utc_offset_hours,
            s.latitude,
            s.longitude,
            s.platform.eager_tls_warmup,
            s.timezone_name.clone(),
        )
    };

    info!(target: "sys", "Creating Rhythm runtime...");
    debug!(
        target: "sys",
        "Runtime bootstrap: profiles={} mode_configs={} active_profile='{}' tz={:?}",
        light_profiles.len(),
        mode_configs.len(),
        active_profile_id,
        timezone_name
    );

    // Run warmup if provided and eager warmup is enabled
    if eager_warmup {
        if let Some(warmup_fn) = warmup {
            warmup_fn(&controller);
        }
    } else {
        info!(target: "sys", "Skipping TLS warmup (deferred until first command)");
    }

    // Calculate solar noon — prefer timezone-aware when IANA name is available
    let lat = latitude.unwrap_or(35.22);
    let lon = longitude.unwrap_or(-80.84);
    let (utc_offset, solar_noon, day_of_year, sun_times) = if let Some(ref tz_name) = timezone_name
    {
        let tz = rhythm_core::Timezone::new(tz_name);
        let (year, month, day, hour) = tz.local_date_hour_from_utc(chrono::Utc::now().naive_utc());
        let fresh_offset = tz.utc_offset(year, month, day, hour);
        let noon = rhythm_core::calculate_solar_noon(lon, year, month, day, &tz);
        let sun_times = rhythm_core::calculate_sun_times(lat, lon, year, month, day, &tz);
        let doy = rhythm_core::timezone::day_of_year(year, month, day);
        (fresh_offset, noon, doy, Some(sun_times))
    } else {
        let day_of_year = SystemTimeProvider::new(utc_offset).day_of_year();
        let noon = rhythm_core::calculate_solar_noon_from_offset(lon, utc_offset, day_of_year);
        (utc_offset, noon, day_of_year, None)
    };
    let time_provider = SystemTimeProvider::new(utc_offset);

    // Write back the fresh offset to state (may differ from startup if DST changed)
    if let Some(ref tz_name) = timezone_name {
        if let Ok(mut s) = state.lock() {
            if (s.utc_offset_hours - utc_offset).abs() > 0.01 {
                info!(target: "sys", "Refreshed utc_offset at runtime creation: {:.1} → {:.1} ({})",
                    s.utc_offset_hours, utc_offset, tz_name);
                s.utc_offset_hours = utc_offset;
            }
        }
    }

    info!(target: "sys",
        "Solar noon calculated: {:.2} (lat={}, lon={}, utc_offset={}, doy={}, tz={:?})",
        solar_noon, lat, lon, utc_offset, day_of_year, timezone_name
    );

    let new_runtime_config = RuntimeConfig::default()
        .with_update_interval(runtime_config.update_interval_secs)
        .with_solar_noon(solar_noon)
        .with_location(lat as f64, lon as f64)
        .with_utc_offset(utc_offset);

    let runtime = RhythmRuntime::new(
        Arc::new(controller),
        time_provider,
        ThreadScheduler::new(),
        {
            let state = state
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
            let reg = registry
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock registry"))?;
            let mut rt_reg = SimpleDeviceRegistry::new();
            for room_id in reg.list_rooms() {
                let Some(topology_room_id) = state.topology.translate_room_id(hub_key, &room_id)
                else {
                    continue;
                };
                for device_id in HubRegistry::devices_for_room(&*reg, &room_id) {
                    rt_reg.register_device(&device_id, topology_room_id);
                }
            }
            rt_reg
        },
        new_runtime_config.clone(),
    );

    let solar_time = SolarTime::new(solar_noon, lat, day_of_year);
    runtime
        .set_solar(solar_time)
        .map_err(|e| anyhow::anyhow!("Failed to set solar: {}", e))?;
    if let Some(sun_times) = sun_times {
        runtime
            .set_sun_times(sun_times)
            .map_err(|e| anyhow::anyhow!("Failed to set sun times: {}", e))?;
    }
    for profile in light_profiles {
        runtime
            .set_light_profile_config(profile)
            .map_err(|e| anyhow::anyhow!("Failed to set profile config: {}", e))?;
    }
    let mode_config_count = mode_configs.len();
    runtime
        .set_mode_configs(mode_configs)
        .map_err(|e| anyhow::anyhow!("Failed to set mode configs: {}", e))?;
    info!(
        target: "sys",
        "Runtime mode config applied: {} modes, active_profile={}",
        mode_config_count,
        active_profile_id
    );
    if !runtime.set_light_profile(&active_profile_id) {
        return Err(anyhow::anyhow!(
            "Failed to activate light profile '{}'",
            active_profile_id
        ));
    }

    // Add rooms from registry to the engine
    {
        let reg = registry
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock registry"))?;
        for room in reg.rooms() {
            runtime.add_room(&room.id, &room.name);
        }
    }

    // Restore persisted room state
    {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        if let Some(ref storage) = s.storage {
            match storage.load_rooms() {
                Ok(persisted) => {
                    // Detect corrupted persistence: all rooms at default values
                    // (rhythm_enabled=false, no offsets) is the fingerprint of the
                    // do_room_set bug that dropped params during runtime creation.
                    let all_defaults = persisted_rooms_look_corrupted(&persisted);

                    if all_defaults {
                        warn!(target: "sys",
                            "All {} rooms at default state — likely corrupted. Defaulting to rhythm_enabled=true.",
                            persisted.len()
                        );
                        for snap in runtime.engine_all_room_snapshots() {
                            runtime.restore_room_state(
                                &snap.id,
                                rhythm_core::RestoredRoomState {
                                    rhythm_enabled: true,
                                    disabled: false,
                                    time_offset_minutes: 0.0,
                                    brightness_offset: 0.0,
                                    soft_off: false,
                                    mood_active: false,
                                    standby_enabled: false,
                                    hard_off: false,
                                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                                },
                            );
                        }
                    } else {
                        for room in persisted.iter() {
                            runtime.restore_room_state(
                                &room.id,
                                rhythm_core::RestoredRoomState {
                                    rhythm_enabled: room.rhythm_enabled,
                                    disabled: room.disabled,
                                    time_offset_minutes: room.time_offset_minutes,
                                    brightness_offset: room.brightness_offset,
                                    soft_off: room.soft_off,
                                    mood_active: room.mood_active,
                                    standby_enabled: room.standby_enabled,
                                    hard_off: room.hard_off,
                                    profile_settings: room.profile_settings.clone(),
                                },
                            );
                            info!(target: "sys",
                                "Restored room '{}': rhythm={}, disabled={}, time_offset={}, bri_offset={}, soft_off={}, hard_off={}, room_profile={}",
                                room.id, room.rhythm_enabled, room.disabled,
                                room.time_offset_minutes, room.brightness_offset, room.soft_off,
                                room.hard_off, !room.profile_settings.is_empty()
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(target: "sys", "No persisted rooms (first boot?): {}", e);
                    for snap in runtime.engine_all_room_snapshots() {
                        runtime.restore_room_state(
                            &snap.id,
                            rhythm_core::RestoredRoomState {
                                rhythm_enabled: true,
                                disabled: false,
                                time_offset_minutes: 0.0,
                                brightness_offset: 0.0,
                                soft_off: false,
                                mood_active: false,
                                standby_enabled: false,
                                hard_off: false,
                                profile_settings: rhythm_core::RoomProfileSettings::default(),
                            },
                        );
                    }
                }
            }
        }
    }

    let runtime = Arc::new(runtime);

    // Push runtime settings to engine. If restored rooms were idle and
    // power_save is active, convert them to hard-off and refresh output.
    let (power_save, power_save_refresh_rooms) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        let rooms = runtime.set_power_save(s.power_save);
        info!(target: "sys", "Active mode: {:?}", s.active_mode);
        info!(target: "sys", "Power save: {}", s.power_save);
        (s.power_save, rooms)
    };
    if power_save {
        for room_id in power_save_refresh_rooms {
            if let Err(e) = runtime.lights_off_room(&room_id, None) {
                warn!(
                    target: "sys",
                    "Failed to apply power-save hard-off output for '{}': {}",
                    room_id, e
                );
            }
        }
    }

    // Store runtime in ActiveHub and update runtime_config
    {
        let mut s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        s.runtime_config = new_runtime_config;
        for hub in s.hubs.values_mut() {
            if hub.runtime.is_none() {
                hub.runtime = Some(runtime.clone());
            }
        }
    }

    info!(target: "sys", "Rhythm runtime created and started");
    Ok(())
}

// ============================================================================
// start_event_translator — replaces both identical functions
// ============================================================================

/// Spawn a translator thread that converts raw hub events to `HubEvent`s.
///
/// Takes a `Receiver<E>` (from the platform's event reader) and spawns
/// a thread that translates each event via the provided closure.
pub fn start_event_translator<E: Send + 'static>(
    raw_rx: Receiver<E>,
    translate: impl Fn(&E) -> Vec<HubEvent> + Send + 'static,
    shutdown: Arc<AtomicBool>,
    thread_name: &str,
    on_activity: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Receiver<HubEvent> {
    let (hub_tx, hub_rx) = std::sync::mpsc::sync_channel::<HubEvent>(HUB_EVENT_CHANNEL_CAPACITY);

    let spawn_result = std::thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || {
            while let Ok(raw_event) = raw_rx.recv() {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(ref cb) = on_activity {
                    cb();
                }
                for hub_event in translate(&raw_event) {
                    if !forward_translated_hub_event(&hub_tx, hub_event, HUB_EVENT_CHANNEL_CAPACITY)
                    {
                        return;
                    }
                }
            }
        });
    if let Err(e) = spawn_result {
        warn!(target: "evt", "Failed to spawn event translator thread: {}", e);
    }

    hub_rx
}

/// Forward one translated event without losing a one-shot topology invalidation.
///
/// Most high-volume hub events retain the existing lossy behavior when the
/// consumer is saturated. Add/delete topology events are different: the hub
/// might never repeat them, so backpressure until the event is accepted or the
/// receiver disconnects.
fn forward_translated_hub_event(
    hub_tx: &std::sync::mpsc::SyncSender<HubEvent>,
    hub_event: HubEvent,
    capacity: usize,
) -> bool {
    match hub_tx.try_send(hub_event) {
        Ok(()) => true,
        Err(std::sync::mpsc::TrySendError::Full(hub_event)) => {
            if matches!(&hub_event, HubEvent::TopologyChanged { .. }) {
                hub_tx.send(hub_event).is_ok()
            } else {
                warn!(
                    target: "evt",
                    "Hub event channel full (capacity={}), dropping event",
                    capacity
                );
                true
            }
        }
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => false,
    }
}

// ============================================================================
// configure_hub — replaces configure_hue_hub() and configure_ha_hub()
// ============================================================================

// ============================================================================
// ensure_composite_runtime — desktop multi-hub runtime creation
// ============================================================================

/// Create the shared RhythmRuntime with a CompositeController.
///
/// Collects per-hub controllers from all connected hubs via
/// `ExternalLightHubIntegration::create_controller`, wraps them in a
/// `CompositeController`, and creates the single shared runtime.
///
/// The `CompositeController` is stored on `AppState.composite_controller`
/// so that subsequent hub connections can register their controllers
/// dynamically without recreating the runtime.
pub fn ensure_composite_runtime(
    state: &SharedState,
    integrations: &[&dyn crate::hub::ExternalLightHubIntegration],
) -> Result<()> {
    use rhythm_core::solar::SolarTime;
    use rhythm_core::{
        CompositeController, DeviceRegistry, HubRegistry, RhythmRuntime, RuntimeConfig,
        RuntimeHandle, SimpleDeviceRegistry, SystemTimeProvider, ThreadScheduler, TimeProvider,
    };

    let (
        light_profiles,
        mode_configs,
        active_profile_id,
        runtime_config,
        utc_offset,
        latitude,
        longitude,
        timezone_name,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        // Check if runtime already exists
        let any_runtime = s.hubs.values().any(|h| h.runtime.is_some());
        if any_runtime {
            return Ok(());
        }

        (
            s.light_profile_configs
                .values()
                .cloned()
                .collect::<Vec<_>>(),
            s.mode_configs(),
            s.active_mode_profile_id(),
            s.runtime_config.clone(),
            s.utc_offset_hours,
            s.latitude,
            s.longitude,
            s.timezone_name.clone(),
        )
    };

    info!(target: "sys", "Creating composite Rhythm runtime...");

    // Build CompositeController and register per-hub controllers.
    // Collect hub keys first (brief lock), then create controllers outside the lock.
    let composite = std::sync::Arc::new(CompositeController::new());
    install_dispatch_outcome_listener(state, &composite);
    let hub_keys: Vec<(HubKey, String)> = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hubs
            .keys()
            .map(|k| (k.clone(), k.hub_type.as_str().to_string()))
            .collect()
    };
    for (key, hub_type_str) in &hub_keys {
        if let Some(integration) = crate::hub::find_integration(integrations, hub_type_str) {
            match integration.create_controller(state, key) {
                Ok(controller) => {
                    let key_str = key.to_string();
                    info!(target: "sys", "Registered {} controller with composite", key_str);
                    composite.register_controller(&key_str, controller);
                }
                Err(e) => {
                    warn!(target: "sys", "Failed to create controller for {}: {}", key, e);
                }
            }
        }
    }

    if composite.controller_count() == 0 {
        return Err(anyhow::anyhow!(
            "No controllers registered — cannot create runtime"
        ));
    }

    // Build a merged device-to-topology-room registry for the runtime.
    //
    // Runtime event routing must be topology-first. Hub registries still own
    // discovery and native button/motion mappings, but the runtime only
    // should ever see topology room IDs.
    let merged_registry = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let mut rt_reg = SimpleDeviceRegistry::new();
        for (hub_key, hub) in &s.hubs {
            if let Some(ref reg) = hub.registry {
                if let Ok(reg) = reg.lock() {
                    for room in reg.rooms() {
                        let Some(topology_room_id) =
                            s.topology.translate_room_id(hub_key, &room.id)
                        else {
                            continue;
                        };
                        for device_id in HubRegistry::devices_for_room(&*reg, &room.id) {
                            rt_reg.register_device(&device_id, topology_room_id);
                        }
                    }
                }
            }
        }
        rt_reg
    };

    // Calculate solar noon
    let lat = latitude.unwrap_or(35.22);
    let lon = longitude.unwrap_or(-80.84);
    let (utc_offset, solar_noon, day_of_year, sun_times) = if let Some(ref tz_name) = timezone_name
    {
        let tz = rhythm_core::Timezone::new(tz_name);
        let (year, month, day, hour) = tz.local_date_hour_from_utc(chrono::Utc::now().naive_utc());
        let fresh_offset = tz.utc_offset(year, month, day, hour);
        let noon = rhythm_core::calculate_solar_noon(lon, year, month, day, &tz);
        let sun_times = rhythm_core::calculate_sun_times(lat, lon, year, month, day, &tz);
        let doy = rhythm_core::timezone::day_of_year(year, month, day);
        (fresh_offset, noon, doy, Some(sun_times))
    } else {
        let day_of_year = SystemTimeProvider::new(utc_offset).day_of_year();
        let noon = rhythm_core::calculate_solar_noon_from_offset(lon, utc_offset, day_of_year);
        (utc_offset, noon, day_of_year, None)
    };
    let time_provider = SystemTimeProvider::new(utc_offset);

    // Write back fresh offset
    if timezone_name.is_some() {
        if let Ok(mut s) = state.lock() {
            if (s.utc_offset_hours - utc_offset).abs() > 0.01 {
                info!(target: "sys", "Refreshed utc_offset: {:.1} → {:.1}", s.utc_offset_hours, utc_offset);
                s.utc_offset_hours = utc_offset;
            }
        }
    }

    info!(target: "sys",
        "Solar noon: {:.2} (lat={}, lon={}, utc_offset={}, doy={}, tz={:?})",
        solar_noon, lat, lon, utc_offset, day_of_year, timezone_name
    );

    let new_runtime_config = RuntimeConfig::default()
        .with_update_interval(runtime_config.update_interval_secs)
        .with_solar_noon(solar_noon)
        .with_location(lat as f64, lon as f64)
        .with_utc_offset(utc_offset);

    // Create the shared runtime with the composite controller.
    // Arc::clone gives the runtime a shared ref; AppState holds another for dynamic registration.
    let runtime = RhythmRuntime::new(
        composite.clone(),
        time_provider,
        ThreadScheduler::new(),
        merged_registry,
        new_runtime_config.clone(),
    );

    let solar_time = SolarTime::new(solar_noon, lat, day_of_year);
    runtime
        .set_solar(solar_time)
        .map_err(|e| anyhow::anyhow!("set solar: {}", e))?;
    if let Some(sun_times) = sun_times {
        runtime
            .set_sun_times(sun_times)
            .map_err(|e| anyhow::anyhow!("set sun times: {}", e))?;
    }
    for profile in light_profiles {
        runtime
            .set_light_profile_config(profile)
            .map_err(|e| anyhow::anyhow!("set profile config: {}", e))?;
    }
    let mode_config_count = mode_configs.len();
    runtime
        .set_mode_configs(mode_configs)
        .map_err(|e| anyhow::anyhow!("set mode configs: {}", e))?;
    info!(
        target: "sys",
        "Runtime mode config applied: {} modes, active_profile={}",
        mode_config_count,
        active_profile_id
    );
    if !runtime.set_light_profile(&active_profile_id) {
        return Err(anyhow::anyhow!(
            "Failed to activate light profile '{}'",
            active_profile_id
        ));
    }

    // Seed runtime rooms from persisted topology, not hub registries.
    // A topology room may dispatch through grouped hub bindings, direct device
    // targets, or a future mix of both. The engine should not care.
    {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        for room in s.topology.rooms() {
            runtime.add_room(&room.id, &room.name);
        }
        for node in s.topology.device_nodes() {
            if crate::commands::canonical_device_is_quarantined(&s, &node.canonical_device_id) {
                continue;
            }
            if let Some(device) = s.canonical_registry.get(&node.canonical_device_id) {
                runtime.add_node(
                    &node.id,
                    &device.name,
                    crate::commands::runtime_node_kind_for_device_type(device.device_type.clone()),
                    node.parent_id.clone(),
                );
            }
        }
    }

    // Restore persisted room state
    {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if let Some(ref storage) = s.storage {
            match storage.load_rooms() {
                Ok(persisted) => {
                    let all_defaults = persisted_rooms_look_corrupted(&persisted);

                    if all_defaults {
                        warn!(target: "sys", "All {} rooms at default state — defaulting to rhythm_enabled=true", persisted.len());
                        for snap in runtime.engine_all_node_snapshots() {
                            runtime.restore_node_state(
                                &snap.id,
                                rhythm_core::RestoredNodeState {
                                    rhythm_enabled: true,
                                    disabled: false,
                                    time_offset_minutes: 0.0,
                                    brightness_offset: 0.0,
                                    soft_off: false,
                                    mood_active: false,
                                    standby_enabled: false,
                                    hard_off: false,
                                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                                },
                            );
                        }
                    } else {
                        for node in persisted.iter() {
                            runtime.restore_node_state(
                                &node.id,
                                rhythm_core::RestoredNodeState {
                                    rhythm_enabled: node.rhythm_enabled,
                                    disabled: node.disabled,
                                    time_offset_minutes: node.time_offset_minutes,
                                    brightness_offset: node.brightness_offset,
                                    soft_off: node.soft_off,
                                    mood_active: node.mood_active,
                                    standby_enabled: node.standby_enabled,
                                    hard_off: node.hard_off,
                                    profile_settings: node.profile_settings.clone(),
                                },
                            );
                            debug!(
                                target: "sys",
                                "Restored node '{}': rhythm={} disabled={} time_offset={} bri_offset={} soft_off={} hard_off={} room_profile={}",
                                node.id,
                                node.rhythm_enabled,
                                node.disabled,
                                node.time_offset_minutes,
                                node.brightness_offset,
                                node.soft_off,
                                node.hard_off,
                                !node.profile_settings.is_empty()
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(target: "sys", "No persisted rooms (first boot?): {}", e);
                    for snap in runtime.engine_all_node_snapshots() {
                        runtime.restore_node_state(
                            &snap.id,
                            rhythm_core::RestoredNodeState {
                                rhythm_enabled: true,
                                disabled: false,
                                time_offset_minutes: 0.0,
                                brightness_offset: 0.0,
                                soft_off: false,
                                mood_active: false,
                                standby_enabled: false,
                                hard_off: false,
                                profile_settings: rhythm_core::RoomProfileSettings::default(),
                            },
                        );
                    }
                }
            }
        }
    }

    let runtime = std::sync::Arc::new(runtime);

    // Push settings to engine. If restored nodes were idle and power_save is
    // active, convert them to hard-off and refresh output.
    let (power_save, power_save_refresh_nodes) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (s.power_save, runtime.set_power_save(s.power_save))
    };
    if power_save {
        for node_id in power_save_refresh_nodes {
            if let Err(e) = runtime.lights_off_room(&node_id, None) {
                warn!(
                    target: "sys",
                    "Failed to apply power-save hard-off output for '{}': {}",
                    node_id, e
                );
            }
        }
    }

    // Store runtime and composite controller
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.runtime_config = new_runtime_config;
        {
            s.composite_controller = Some(composite);
        }
        for hub in s.hubs.values_mut() {
            if hub.runtime.is_none() {
                hub.runtime = Some(runtime.clone());
            }
        }
    }

    info!(target: "sys", "Composite Rhythm runtime created and started");
    Ok(())
}

// ============================================================================
// configure_hub
// ============================================================================

/// Shared credential validation and state setup for any hub.
///
/// 1. Build credentials via `build_credentials`
/// 2. Early return if already configured with same creds (`already_configured`)
/// 3. Signal old hub shutdown
/// 4. Save credentials to state + storage
/// 5. Call `connect_fn` closure for platform-specific connection
/// 6. Store active hub + event receiver in state
pub fn configure_hub<B, A, F>(
    state: &SharedState,
    address: &str,
    credentials_json: &str,
    build_credentials: B,
    already_configured: A,
    connect_fn: F,
) -> Result<()>
where
    B: FnOnce(&str, &str) -> Result<HubCredentials>,
    A: FnOnce(&SharedState, &HubCredentials) -> bool,
    F: FnOnce(&SharedState) -> Result<(ActiveHub, Receiver<HubEvent>)>,
{
    let creds = build_credentials(address, credentials_json)?;

    // Early return if already configured with same credentials
    if already_configured(state, &creds) {
        return Ok(());
    }

    // Derive the hub key for this credential set
    let hub_key = creds.hub_key();

    // Credentials are recovery material for authoritative controllers. Commit
    // them durably before any connection callback can capture/clear external
    // state, and compensate the in-memory update if persistence fails.
    {
        let mut s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        if let Some(ref key) = hub_key {
            let previous = s.hub_credentials.insert(key.clone(), creds);
            if let Some(ref storage) = s.storage {
                let all_creds: Vec<_> = s.hub_credentials.values().cloned().collect();
                if let Err(error) = storage.save_all_hub_credentials(&all_creds) {
                    match previous {
                        Some(previous) => {
                            s.hub_credentials.insert(key.clone(), previous);
                        }
                        None => {
                            s.hub_credentials.remove(key);
                        }
                    }
                    return Err(error.context("Failed to durably save hub credentials"));
                }
            }
        }
    }

    // Signal old hub threads to stop (only the matching hub)
    // Take it out of the map but defer drop to a background thread
    // (reqwest::blocking::Client panics if dropped on a tokio worker).
    if let Some(ref key) = hub_key {
        let old_hub = {
            let mut s = state
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
            let old_hub = s.hubs.remove(key);
            s.clear_hub_connected(key);
            old_hub
        };
        if let Some(old_hub) = old_hub {
            info!(target: "sys", "Signaling old hub {} to shut down", key);
            old_hub.shutdown.store(true, Ordering::Relaxed);
            std::thread::Builder::new()
                .name("hub-drop".to_string())
                .spawn(move || drop(old_hub))
                .ok();
        }
    }

    // Platform-specific connection
    let (hub, event_rx) = connect_fn(state)?;

    // Store active hub
    {
        let mut s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        let key = hub.hub_key.clone();
        s.hubs.insert(key.clone(), hub);
        s.set_hub_connected(&key, false);
        s.pending_hub_event_rxs.push(event_rx);
    }

    info!(
        target: "sys",
        "Hub configured (transport starting, runtime deferred)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::Duration;

    use crate::canonical::identity::HubKey;
    use crate::hub::{
        ActiveHub, ExternalLightHubIntegration, HubCredentials, HubEvent, HubProvider, HubType,
    };
    use crate::scenes::StoredScenes;
    use crate::state::AppState;
    use crate::storage::{Storage, StoredLightProfiles, StoredLocation, StoredSettings};
    use crate::topology::{HubRoomBinding, TopologyRoom};
    use anyhow::Result;
    use rhythm_core::room::{Room, RoomManager};
    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_core::{HubLightController, NoOpController, SpyLightController};

    #[derive(Default)]
    struct LifecycleTestStorage {
        rooms: Option<RoomManager>,
        saved_credentials: Arc<Mutex<Vec<Vec<HubCredentials>>>>,
    }

    impl LifecycleTestStorage {
        fn with_rooms(rooms: RoomManager) -> Self {
            Self {
                rooms: Some(rooms),
                saved_credentials: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl Storage for LifecycleTestStorage {
        fn load_rooms(&self) -> Result<RoomManager> {
            self.rooms
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing rooms"))
        }

        fn save_rooms(&self, _rooms: &RoomManager) -> Result<()> {
            Ok(())
        }

        fn load_light_profiles(&self) -> Result<StoredLightProfiles> {
            Err(anyhow::anyhow!("missing light profiles"))
        }

        fn save_light_profiles(&self, _config: &StoredLightProfiles) -> Result<()> {
            Ok(())
        }

        fn load_location(&self) -> Result<StoredLocation> {
            Err(anyhow::anyhow!("missing location"))
        }

        fn save_location(&self, _loc: &StoredLocation) -> Result<()> {
            Ok(())
        }

        fn load_settings(&self) -> Result<StoredSettings> {
            Err(anyhow::anyhow!("missing settings"))
        }

        fn save_settings(&self, _settings: &StoredSettings) -> Result<()> {
            Ok(())
        }

        fn load_scenes(&self) -> Result<Option<StoredScenes>> {
            Ok(None)
        }

        fn save_scenes(&self, _scenes: &StoredScenes) -> Result<()> {
            Ok(())
        }

        fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>> {
            Ok(Vec::new())
        }

        fn save_all_hub_credentials(&self, creds: &[HubCredentials]) -> Result<()> {
            self.saved_credentials.lock().unwrap().push(creds.to_vec());
            Ok(())
        }

        fn load_hub_registry_for(&self, _key: &HubKey) -> Result<Option<serde_json::Value>> {
            Ok(None)
        }

        fn save_hub_registry_for(&self, _key: &HubKey, _data: &serde_json::Value) -> Result<()> {
            Ok(())
        }
    }

    struct TestProvider;

    impl HubProvider for TestProvider {
        fn hub_type(&self) -> HubType {
            HubType::new("test")
        }

        fn configure(
            &self,
            _address: &str,
            _credentials_json: &str,
            _state: &SharedState,
        ) -> Result<()> {
            Ok(())
        }
    }

    static TEST_PROVIDER: TestProvider = TestProvider;

    struct TestIntegration;

    impl ExternalLightHubIntegration for TestIntegration {
        fn hub_type(&self) -> &'static str {
            "test"
        }

        fn provider(&self) -> &'static dyn HubProvider {
            &TEST_PROVIDER
        }

        fn connect_and_start(
            &self,
            _state: SharedState,
            _key: &HubKey,
        ) -> Result<Receiver<HubEvent>> {
            let (_tx, rx) = mpsc::channel();
            Ok(rx)
        }

        fn ensure_runtime(&self, state: &SharedState) -> Result<()> {
            ensure_composite_runtime(state, &[self])
        }

        fn create_controller(
            &self,
            _state: &SharedState,
            _key: &HubKey,
        ) -> Result<Arc<dyn HubLightController>> {
            Ok(Arc::new(NoOpController::new()))
        }
    }

    fn default_room(id: &str) -> Room {
        Room::new(id, id)
    }

    fn shared_state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    fn active_hub(hub_key: HubKey, shutdown: Arc<AtomicBool>) -> ActiveHub {
        ActiveHub {
            hub_type: hub_key.hub_type.clone(),
            hub_key,
            runtime: None,
            hub_data: Box::new(()),
            registry: None,
            discovery: None,
            shutdown,
        }
    }

    fn registry_with_room(
        hub_room_id: &str,
        name: &str,
        control_id: &str,
    ) -> Arc<Mutex<HubDeviceRegistry>> {
        let mut registry = HubDeviceRegistry::with_options(true);
        registry.upsert_room(hub_room_id, name, control_id, &["light-1".to_string()]);
        registry.upsert_device("light-1", Some(hub_room_id), &[], DeviceType::Light);
        Arc::new(Mutex::new(registry))
    }

    fn topology_room_with_binding(
        room_id: &str,
        name: &str,
        hub_key: HubKey,
        hub_room_id: &str,
        control_id: &str,
    ) -> TopologyRoom {
        let mut room = TopologyRoom::new(room_id, name);
        room.upsert_hub_room_binding(HubRoomBinding {
            hub_key,
            hub_room_id: hub_room_id.to_string(),
            control_id: control_id.to_string(),
            light_device_ids: vec!["light-1".to_string()],
        });
        room
    }

    #[test]
    fn empty_persistence_is_not_flagged_as_corrupted() {
        // First boot has no rooms persisted — that's legitimate, not corruption.
        let rooms = RoomManager::new();
        assert!(!persisted_rooms_look_corrupted(&rooms));
    }

    #[test]
    fn single_room_all_defaults_is_corrupted() {
        let mut rooms = RoomManager::new();
        rooms.add_room(default_room("kitchen"));
        assert!(persisted_rooms_look_corrupted(&rooms));
    }

    #[test]
    fn rhythm_enabled_room_is_not_corrupted() {
        let mut rooms = RoomManager::new();
        let mut room = default_room("kitchen");
        room.rhythm_enabled = true;
        rooms.add_room(room);
        assert!(!persisted_rooms_look_corrupted(&rooms));
    }

    #[test]
    fn any_nondefault_field_saves_persistence_from_corruption_flag() {
        // Any user-set persisted field proves the persistence layer is writing
        // real data rather than the historical all-default corruption shape.
        for mutator in [
            |r: &mut Room| r.time_offset_minutes = 10.0,
            |r: &mut Room| r.brightness_offset = -0.3,
            |r: &mut Room| r.disabled = true,
            |r: &mut Room| r.soft_off = true,
            |r: &mut Room| r.mood_active = true,
            |r: &mut Room| r.standby_enabled = true,
            |r: &mut Room| r.hard_off = true,
            |r: &mut Room| r.profile_settings.mood_enabled = Some(false),
            |r: &mut Room| r.rhythm_enabled = true,
        ] {
            let mut rooms = RoomManager::new();
            let mut room = default_room("kitchen");
            mutator(&mut room);
            rooms.add_room(room);
            assert!(
                !persisted_rooms_look_corrupted(&rooms),
                "non-default field should prove non-corruption"
            );
        }
    }

    #[test]
    fn mixed_rooms_only_corrupted_when_all_defaults() {
        // If any room has a non-default field, the persistence is trusted.
        let mut rooms = RoomManager::new();
        rooms.add_room(default_room("kitchen"));
        let mut bedroom = default_room("bedroom");
        bedroom.rhythm_enabled = true;
        rooms.add_room(bedroom);
        assert!(
            !persisted_rooms_look_corrupted(&rooms),
            "presence of one enabled room disproves corruption"
        );
    }

    #[test]
    fn many_default_rooms_still_flagged() {
        let mut rooms = RoomManager::new();
        for i in 0..10 {
            rooms.add_room(default_room(&format!("room-{}", i)));
        }
        assert!(persisted_rooms_look_corrupted(&rooms));
    }

    #[test]
    fn hard_off_only_room_is_not_corrupted_signature() {
        // Hard-off is a valid user state even on an otherwise-defaulted room.
        let mut rooms = RoomManager::new();
        let mut room = default_room("kitchen");
        room.hard_off = true;
        rooms.add_room(room);
        assert!(
            !persisted_rooms_look_corrupted(&rooms),
            "hard_off alone should prove non-corruption"
        );
    }

    #[test]
    fn connect_hub_restores_snapshot_builds_data_and_tags_events() {
        let state = shared_state();
        let hub_key = HubKey::new(HubType::new("test"), "hub-1");
        let mut original = HubDeviceRegistry::with_options(true);
        original.upsert_room("area-1", "Kitchen", "", &["light.kitchen".to_string()]);
        original.upsert_device(
            "button-1",
            Some("area-1"),
            &[("button-event-1".to_string(), 2)],
            DeviceType::Button,
        );
        let snapshot = original.snapshot();

        let (raw_tx, raw_rx) = mpsc::channel();
        let (hub, event_rx) = connect_hub(
            &state,
            HubType::new("test"),
            hub_key.clone(),
            true,
            Some(snapshot),
            |registry| {
                let registry = registry.lock().unwrap();
                assert_eq!(registry.room_name("area-1"), Some("Kitchen"));
                assert_eq!(
                    registry.get_room_for_button("button-event-1"),
                    Some("area-1".to_string())
                );
                Box::new("typed-data".to_string())
            },
            move |_registry, _shutdown| raw_rx,
        )
        .unwrap();

        assert_eq!(hub.hub_key, hub_key);
        assert_eq!(hub.data::<String>().map(String::as_str), Some("typed-data"));
        let registry = hub.registry.as_ref().unwrap().lock().unwrap();
        assert_eq!(
            registry.get_grouped_light_id("area-1"),
            Some("area-1".to_string())
        );
        assert!(registry
            .devices_for_room("area-1")
            .contains(&"button-1".to_string()));
        drop(registry);

        raw_tx.send(HubEvent::Heartbeat { hub_key: None }).unwrap();
        let event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(event.hub_key(), Some(&hub_key));
    }

    #[test]
    fn hub_event_tagger_backpressures_instead_of_dropping_when_full() {
        let hub_key = HubKey::new(HubType::new("test"), "backpressure");
        let (raw_tx, raw_rx) = mpsc::sync_channel(0);
        let event_rx = tag_hub_events_with_capacity(raw_rx, hub_key.clone(), 1);
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        let producer = std::thread::spawn(move || {
            for sequence in 1..=3 {
                raw_tx.send(HubEvent::Heartbeat { hub_key: None }).unwrap();
                accepted_tx.send(sequence).unwrap();
            }
            done_tx.send(()).unwrap();
        });

        // The tagger has accepted the second event but cannot receive the
        // third while its one-slot output queue is full. A lossy try_send
        // implementation would finish the producer by dropping both events.
        assert_eq!(accepted_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        assert_eq!(accepted_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
        assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());

        for _ in 0..3 {
            let event = event_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(event.hub_key(), Some(&hub_key));
        }
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        producer.join().unwrap();
    }

    #[test]
    fn topology_event_backpressures_translator_queue_instead_of_being_dropped() {
        let (hub_tx, hub_rx) = mpsc::sync_channel(1);
        hub_tx.send(HubEvent::Heartbeat { hub_key: None }).unwrap();
        let (done_tx, done_rx) = mpsc::channel();

        let producer = std::thread::spawn(move || {
            let delivered = forward_translated_hub_event(
                &hub_tx,
                HubEvent::TopologyChanged {
                    hub_key: None,
                    resource_id: "light-1".to_string(),
                    resource_type: "light".to_string(),
                },
                1,
            );
            done_tx.send(delivered).unwrap();
        });

        assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(matches!(
            hub_rx.recv_timeout(Duration::from_secs(1)),
            Ok(HubEvent::Heartbeat { .. })
        ));
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        assert!(matches!(
            hub_rx.recv_timeout(Duration::from_secs(1)),
            Ok(HubEvent::TopologyChanged {
                resource_id,
                resource_type,
                ..
            }) if resource_id == "light-1" && resource_type == "light"
        ));
        producer.join().unwrap();
    }

    #[test]
    fn start_event_translator_invokes_activity_and_honors_shutdown() {
        let (raw_tx, raw_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let activity_count = Arc::new(AtomicUsize::new(0));
        let activity = {
            let activity_count = activity_count.clone();
            Arc::new(move || {
                activity_count.fetch_add(1, Ordering::Relaxed);
            })
        };
        let hub_rx = start_event_translator(
            raw_rx,
            |raw: &u8| {
                vec![HubEvent::Disconnected {
                    hub_key: None,
                    reason: format!("raw-{raw}"),
                }]
            },
            shutdown.clone(),
            "test-event-translator",
            Some(activity),
        );

        raw_tx.send(7).unwrap();
        let first = hub_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        match first {
            HubEvent::Disconnected { reason, .. } => assert_eq!(reason, "raw-7"),
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(activity_count.load(Ordering::Relaxed), 1);

        shutdown.store(true, Ordering::Relaxed);
        raw_tx.send(8).unwrap();
        assert!(hub_rx
            .recv_timeout(std::time::Duration::from_millis(150))
            .is_err());
    }

    #[test]
    fn ensure_hub_runtime_restores_persisted_rooms_and_runs_warmup_once() {
        let state = shared_state();
        let hub_key = HubKey::new(HubType::new("test"), "hub-1");
        let registry = registry_with_room("hub-kitchen", "Kitchen", "group-kitchen");

        let mut persisted = RoomManager::new();
        let mut kitchen = Room::new("hub-kitchen", "Kitchen");
        kitchen.rhythm_enabled = true;
        kitchen.disabled = true;
        kitchen.time_offset_minutes = 45.0;
        kitchen.brightness_offset = 0.25;
        kitchen.soft_off = true;
        kitchen.profile_settings.mood_enabled = Some(false);
        persisted.add_room(kitchen);

        {
            let mut guard = state.lock().unwrap();
            guard.latitude = Some(40.71);
            guard.longitude = Some(-74.0);
            guard.utc_offset_hours = 0.0;
            guard.timezone_name = Some("America/New_York".to_string());
            guard.power_save = false;
            guard.storage = Some(std::sync::Arc::new(LifecycleTestStorage::with_rooms(
                persisted,
            )));
            guard.topology.insert_room(topology_room_with_binding(
                "top-kitchen",
                "Kitchen",
                hub_key.clone(),
                "hub-kitchen",
                "group-kitchen",
            ));
            guard.hubs.insert(
                hub_key.clone(),
                active_hub(hub_key.clone(), Arc::new(AtomicBool::new(false))),
            );
        }

        let warmups = Arc::new(AtomicUsize::new(0));
        let spy = Arc::new(SpyLightController::new());
        ensure_hub_runtime(
            &state,
            &hub_key,
            spy.clone(),
            registry.clone(),
            Some(Box::new({
                let warmups = warmups.clone();
                move |_| {
                    warmups.fetch_add(1, Ordering::Relaxed);
                }
            })),
        )
        .unwrap();

        let runtime = state.lock().unwrap().hub_runtime_for(&hub_key).unwrap();
        let snapshot = runtime.engine_node_snapshot("hub-kitchen").unwrap();
        assert!(snapshot.rhythm_enabled);
        assert!(snapshot.disabled);
        assert!(snapshot.soft_off);
        assert!((snapshot.time_offset_minutes - 45.0).abs() < f32::EPSILON);
        assert!((snapshot.brightness_offset - 0.25).abs() < f32::EPSILON);
        assert_eq!(snapshot.profile_settings.mood_enabled, Some(false));
        assert_eq!(warmups.load(Ordering::Relaxed), 1);
        assert!(state.lock().unwrap().utc_offset_hours < 0.0);

        ensure_hub_runtime(&state, &hub_key, NoOpController::new(), registry, None).unwrap();
        assert_eq!(
            warmups.load(Ordering::Relaxed),
            1,
            "existing runtime should short-circuit without warmup"
        );
    }

    #[test]
    fn ensure_hub_runtime_defaults_missing_or_corrupted_rooms_to_rhythm_enabled() {
        for storage in [
            Some(
                std::sync::Arc::new(LifecycleTestStorage::default()) as std::sync::Arc<dyn Storage>
            ),
            Some(std::sync::Arc::new(LifecycleTestStorage::with_rooms({
                let mut rooms = RoomManager::new();
                rooms.add_room(Room::new("hub-kitchen", "Kitchen"));
                rooms
            })) as std::sync::Arc<dyn Storage>),
        ] {
            let state = shared_state();
            let hub_key = HubKey::new(HubType::new("test"), "hub-1");
            let registry = registry_with_room("hub-kitchen", "Kitchen", "group-kitchen");
            {
                let mut guard = state.lock().unwrap();
                guard.power_save = false;
                guard.storage = storage;
                guard.topology.insert_room(topology_room_with_binding(
                    "top-kitchen",
                    "Kitchen",
                    hub_key.clone(),
                    "hub-kitchen",
                    "group-kitchen",
                ));
                guard.hubs.insert(
                    hub_key.clone(),
                    active_hub(hub_key.clone(), Arc::new(AtomicBool::new(false))),
                );
            }

            ensure_hub_runtime(&state, &hub_key, NoOpController::new(), registry, None).unwrap();
            let runtime = state.lock().unwrap().hub_runtime_for(&hub_key).unwrap();
            let snapshot = runtime.engine_node_snapshot("hub-kitchen").unwrap();
            assert!(
                snapshot.rhythm_enabled,
                "missing and corrupted room state should both default to rhythm on"
            );
        }
    }

    #[test]
    fn ensure_composite_runtime_registers_controller_and_restores_topology_nodes() {
        let state = shared_state();
        let hub_key = HubKey::new(HubType::new("test"), "hub-1");
        let mut hub_registry = HubDeviceRegistry::with_options(true);
        hub_registry.upsert_room(
            "hub-kitchen",
            "Kitchen",
            "group-kitchen",
            &["light-1".to_string()],
        );

        let registry: Arc<Mutex<dyn rhythm_core::HubRegistry>> = Arc::new(Mutex::new(hub_registry));
        let mut persisted = RoomManager::new();
        let mut kitchen = Room::new("top-kitchen", "Kitchen");
        kitchen.rhythm_enabled = true;
        kitchen.standby_enabled = true;
        kitchen.brightness_offset = -0.2;
        persisted.add_room(kitchen);

        {
            let mut guard = state.lock().unwrap();
            guard.latitude = Some(35.22);
            guard.longitude = Some(-80.84);
            guard.utc_offset_hours = -5.0;
            guard.power_save = false;
            guard.storage = Some(std::sync::Arc::new(LifecycleTestStorage::with_rooms(
                persisted,
            )));
            guard.topology.insert_room(topology_room_with_binding(
                "top-kitchen",
                "Kitchen",
                hub_key.clone(),
                "hub-kitchen",
                "group-kitchen",
            ));
            let mut hub = active_hub(hub_key.clone(), Arc::new(AtomicBool::new(false)));
            hub.registry = Some(registry);
            guard.hubs.insert(hub_key.clone(), hub);
        }

        let integration = TestIntegration;
        ensure_composite_runtime(&state, &[&integration]).unwrap();

        let guard = state.lock().unwrap();
        let composite = guard.composite_controller.as_ref().unwrap();
        assert_eq!(composite.controller_count(), 1);
        let runtime = guard.hub_runtime_for(&hub_key).unwrap();
        drop(guard);

        let snapshot = runtime.engine_node_snapshot("top-kitchen").unwrap();
        assert!(snapshot.rhythm_enabled);
        assert!(snapshot.standby_enabled);
        assert!((snapshot.brightness_offset + 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn ensure_composite_runtime_reports_when_no_controllers_can_be_registered() {
        let state = shared_state();
        let hub_key = HubKey::new(HubType::new("missing"), "hub-1");
        state.lock().unwrap().hubs.insert(
            hub_key.clone(),
            active_hub(hub_key, Arc::new(AtomicBool::new(false))),
        );

        let err = ensure_composite_runtime(&state, &[]).unwrap_err();
        assert!(err.to_string().contains("No controllers registered"));
        assert!(state.lock().unwrap().hub_runtime().is_none());
    }

    #[test]
    fn configure_hub_saves_credentials_replaces_old_hub_and_stores_receiver() {
        let state = shared_state();
        let hub_key = HubKey::new(HubType::new("test"), "hub-1");
        let old_shutdown = Arc::new(AtomicBool::new(false));
        state.lock().unwrap().hubs.insert(
            hub_key.clone(),
            active_hub(hub_key.clone(), old_shutdown.clone()),
        );

        configure_hub(
            &state,
            "hub-1",
            r#"{"token":"new"}"#,
            |address, raw| {
                let parsed: serde_json::Value = serde_json::from_str(raw)?;
                Ok(HubCredentials::new(
                    "test",
                    address,
                    serde_json::json!({"token": parsed["token"]}),
                ))
            },
            |_state, creds| creds.get_str("token") == Some("same"),
            {
                let hub_key = hub_key.clone();
                move |_state| {
                    let (tx, rx) = mpsc::channel();
                    drop(tx);
                    Ok((active_hub(hub_key, Arc::new(AtomicBool::new(false))), rx))
                }
            },
        )
        .unwrap();

        let guard = state.lock().unwrap();
        assert_eq!(
            guard
                .hub_credentials
                .get(&hub_key)
                .and_then(|creds| creds.get_str("token")),
            Some("new")
        );
        assert!(guard.hubs.contains_key(&hub_key));
        assert_eq!(guard.hub_connection_status.get(&hub_key), Some(&false));
        assert_eq!(guard.pending_hub_event_rxs.len(), 1);
        assert!(old_shutdown.load(Ordering::Relaxed));
    }

    #[test]
    fn configure_hub_skips_connection_when_already_configured() {
        let state = shared_state();
        let called = Arc::new(AtomicBool::new(false));

        configure_hub(
            &state,
            "hub-1",
            r#"{}"#,
            |address, _raw| Ok(HubCredentials::new("test", address, serde_json::json!({}))),
            |_state, _creds| true,
            {
                let called = called.clone();
                move |_state| {
                    called.store(true, Ordering::Relaxed);
                    let (_tx, rx) = mpsc::channel();
                    Ok((
                        active_hub(
                            HubKey::new(HubType::new("test"), "hub-1"),
                            Arc::new(AtomicBool::new(false)),
                        ),
                        rx,
                    ))
                }
            },
        )
        .unwrap();

        assert!(!called.load(Ordering::Relaxed));
        assert!(state.lock().unwrap().hubs.is_empty());
    }
}
