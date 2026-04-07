//! Generic hub lifecycle functions.
//!
//! Shared connect, runtime creation, event translation, and configuration
//! logic used by all hub crates. Hub-specific behavior is injected via
//! closures rather than traits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::{info, warn};

use crate::canonical::identity::HubKey;
use crate::hub::{ActiveHub, HubCredentials, HubEvent, HubType};
use crate::registry::{HubDeviceRegistry, RegistrySnapshot};
use crate::state::SharedState;

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

    // Start event stream via hub-specific closure
    let event_rx = start_event_stream(registry.clone(), shutdown.clone());

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

    info!(target: "sys", "Hub connected (runtime deferred until rooms are discovered)");
    Ok((hub, event_rx))
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
#[cfg(feature = "blocking")]
#[allow(clippy::type_complexity)]
pub fn ensure_hub_runtime<C: rhythm_core::LightController + Send + Sync + 'static>(
    state: &SharedState,
    controller: C,
    registry: Arc<Mutex<HubDeviceRegistry>>,
    warmup: Option<Box<dyn FnOnce(&C)>>,
) -> Result<()> {
    use rhythm_core::solar::SolarTime;
    use rhythm_core::{
        BlockingScheduler, BlockingTimeProvider, DeviceRegistry, HubRegistry, RhythmRuntime,
        RuntimeConfig, RuntimeHandle, TimeProvider,
    };

    let (
        light_profiles,
        active_light_profile_id,
        runtime_config,
        utc_offset,
        latitude,
        longitude,
        scheduler_stack,
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
            s.active_light_profile_id.clone(),
            s.runtime_config.clone(),
            s.utc_offset_hours,
            s.latitude,
            s.longitude,
            s.platform.scheduler_stack,
            s.platform.eager_tls_warmup,
            s.timezone_name.clone(),
        )
    };

    info!(target: "sys", "Creating Rhythm runtime...");

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
    let day_of_year = BlockingTimeProvider::new(utc_offset).day_of_year();
    let (utc_offset, solar_noon) = if let Some(ref tz_name) = timezone_name {
        use chrono::{Datelike, Timelike};
        let tz = rhythm_core::Timezone::new(tz_name);
        let now = chrono::Utc::now().naive_utc();
        let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
        let fresh_offset = tz.utc_offset(year, month, day, now.time().hour());
        let noon = rhythm_core::calculate_solar_noon(lon, year, month, day, &tz);
        (fresh_offset, noon)
    } else {
        let noon = rhythm_core::calculate_solar_noon_from_offset(lon, utc_offset, day_of_year);
        (utc_offset, noon)
    };
    let time_provider = BlockingTimeProvider::new(utc_offset);

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

    let scheduler = match scheduler_stack {
        Some(size) => BlockingScheduler::with_stack_size(size),
        None => BlockingScheduler::new(),
    };

    let runtime = RhythmRuntime::new(
        controller,
        time_provider,
        scheduler,
        {
            let reg = registry
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock registry"))?;
            let mut rt_reg = HubDeviceRegistry::default();
            for room_id in reg.list_rooms() {
                for device_id in HubRegistry::devices_for_room(&*reg, &room_id) {
                    rt_reg.register_device(&device_id, &room_id);
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
    for profile in light_profiles {
        runtime
            .set_light_profile_config(profile)
            .map_err(|e| anyhow::anyhow!("Failed to set profile config: {}", e))?;
    }
    if !runtime.set_light_profile(&active_light_profile_id) {
        return Err(anyhow::anyhow!(
            "Failed to activate light profile '{}'",
            active_light_profile_id
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
                    let all_defaults = !persisted.is_empty()
                        && persisted.iter().all(|r| {
                            !r.rhythm_enabled
                                && !r.disabled
                                && r.time_offset_minutes == 0.0
                                && r.brightness_offset == 0.0
                                && !r.soft_off
                        });

                    if all_defaults {
                        warn!(target: "sys",
                            "All {} rooms at default state — likely corrupted. Defaulting to rhythm_enabled=true.",
                            persisted.len()
                        );
                        for snap in runtime.engine_all_room_snapshots() {
                            runtime.restore_room_state(
                                &snap.id,
                                true,
                                false,
                                0.0,
                                0.0,
                                false,
                                rhythm_core::RoomProfileSettings::default(),
                            );
                        }
                    } else {
                        for room in persisted.iter() {
                            runtime.restore_room_state(
                                &room.id,
                                room.rhythm_enabled,
                                room.disabled,
                                room.time_offset_minutes,
                                room.brightness_offset,
                                room.soft_off,
                                room.profile_settings.clone(),
                            );
                            info!(target: "sys",
                                "Restored room '{}': rhythm={}, disabled={}, time_offset={}, bri_offset={}, soft_off={}",
                                room.id, room.rhythm_enabled, room.disabled,
                                room.time_offset_minutes, room.brightness_offset, room.soft_off
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(target: "sys", "No persisted rooms (first boot?): {}", e);
                    for snap in runtime.engine_all_room_snapshots() {
                        runtime.restore_room_state(
                            &snap.id,
                            true,
                            false,
                            0.0,
                            0.0,
                            false,
                            rhythm_core::RoomProfileSettings::default(),
                        );
                    }
                }
            }
        }
    }

    let runtime = Arc::new(runtime);

    // Push runtime settings to engine
    {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        runtime.set_power_save(s.power_save);
        info!(target: "sys", "Active light profile: {}", s.active_light_profile_id);
        info!(target: "sys", "Power save: {}", s.power_save);
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
    stack_size: Option<usize>,
    on_activity: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Receiver<HubEvent> {
    let (hub_tx, hub_rx) = std::sync::mpsc::sync_channel::<HubEvent>(32);

    let mut builder = std::thread::Builder::new().name(thread_name.to_string());
    if let Some(size) = stack_size {
        builder = builder.stack_size(size);
    }

    builder
        .spawn(move || {
            while let Ok(raw_event) = raw_rx.recv() {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(ref cb) = on_activity {
                    cb();
                }
                for hub_event in translate(&raw_event) {
                    match hub_tx.try_send(hub_event) {
                        Ok(()) => {}
                        Err(std::sync::mpsc::TrySendError::Full(_)) => {
                            warn!(target: "evt", "Hub event channel full, dropping event");
                        }
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                            return;
                        }
                    }
                }
            }
        })
        .expect("Failed to spawn event translator thread");

    hub_rx
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
#[cfg(feature = "blocking")]
pub fn ensure_composite_runtime(
    state: &SharedState,
    integrations: &[&dyn crate::hub::ExternalLightHubIntegration],
) -> Result<()> {
    use rhythm_core::solar::SolarTime;
    use rhythm_core::{
        BlockingScheduler, BlockingTimeProvider, CompositeController, DeviceRegistry, HubRegistry,
        RhythmRuntime, RuntimeConfig, RuntimeHandle, TimeProvider,
    };

    let (
        light_profiles,
        active_light_profile_id,
        runtime_config,
        utc_offset,
        latitude,
        longitude,
        scheduler_stack,
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
            s.active_light_profile_id.clone(),
            s.runtime_config.clone(),
            s.utc_offset_hours,
            s.latitude,
            s.longitude,
            s.platform.scheduler_stack,
            s.timezone_name.clone(),
        )
    };

    info!(target: "sys", "Creating composite Rhythm runtime...");

    // Build CompositeController and register per-hub controllers.
    // Collect hub keys first (brief lock), then create controllers outside the lock.
    let composite = std::sync::Arc::new(CompositeController::new());
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

    // Build a merged registry from all hubs for the runtime's DeviceRegistry
    let merged_registry = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let mut rt_reg = crate::registry::HubDeviceRegistry::default();
        for hub in s.hubs.values() {
            if let Some(ref reg) = hub.registry {
                if let Ok(reg) = reg.lock() {
                    for room in reg.rooms() {
                        for device_id in HubRegistry::devices_for_room(&*reg, &room.id) {
                            rt_reg.register_device(&device_id, &room.id);
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
    let day_of_year = BlockingTimeProvider::new(utc_offset).day_of_year();
    let (utc_offset, solar_noon) = if let Some(ref tz_name) = timezone_name {
        use chrono::{Datelike, Timelike};
        let tz = rhythm_core::Timezone::new(tz_name);
        let now = chrono::Utc::now().naive_utc();
        let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
        let fresh_offset = tz.utc_offset(year, month, day, now.time().hour());
        let noon = rhythm_core::calculate_solar_noon(lon, year, month, day, &tz);
        (fresh_offset, noon)
    } else {
        let noon = rhythm_core::calculate_solar_noon_from_offset(lon, utc_offset, day_of_year);
        (utc_offset, noon)
    };
    let time_provider = BlockingTimeProvider::new(utc_offset);

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

    let scheduler = match scheduler_stack {
        Some(size) => BlockingScheduler::with_stack_size(size),
        None => BlockingScheduler::new(),
    };

    // Create the shared runtime with the composite controller.
    // Arc::clone gives the runtime a shared ref; AppState holds another for dynamic registration.
    let runtime = RhythmRuntime::new(
        composite.clone(),
        time_provider,
        scheduler,
        merged_registry,
        new_runtime_config.clone(),
    );

    let solar_time = SolarTime::new(solar_noon, lat, day_of_year);
    runtime
        .set_solar(solar_time)
        .map_err(|e| anyhow::anyhow!("set solar: {}", e))?;
    for profile in light_profiles {
        runtime
            .set_light_profile_config(profile)
            .map_err(|e| anyhow::anyhow!("set profile config: {}", e))?;
    }
    if !runtime.set_light_profile(&active_light_profile_id) {
        return Err(anyhow::anyhow!(
            "Failed to activate light profile '{}'",
            active_light_profile_id
        ));
    }

    // Add rooms from all hubs' registries (using topology IDs to avoid
    // duplicates when do_room_set later adds the same room under its
    // proper topology ID).
    {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        for (key, hub) in &s.hubs {
            if let Some(ref reg) = hub.registry {
                if let Ok(reg) = reg.lock() {
                    for room in reg.rooms() {
                        // Use topology ID if mapped, otherwise skip — do_room_set
                        // will add it with a proper topology ID during sync.
                        if let Some(topo_id) = s.topology.translate_room_id(key, &room.id) {
                            runtime.add_room(topo_id, &room.name);
                        }
                    }
                }
            }
        }
    }

    // Restore persisted room state
    {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if let Some(ref storage) = s.storage {
            match storage.load_rooms() {
                Ok(persisted) => {
                    let all_defaults = !persisted.is_empty()
                        && persisted.iter().all(|r| {
                            !r.rhythm_enabled
                                && !r.disabled
                                && r.time_offset_minutes == 0.0
                                && r.brightness_offset == 0.0
                                && !r.soft_off
                        });

                    if all_defaults {
                        warn!(target: "sys", "All {} rooms at default state — defaulting to rhythm_enabled=true", persisted.len());
                        for snap in runtime.engine_all_room_snapshots() {
                            runtime.restore_room_state(
                                &snap.id,
                                true,
                                false,
                                0.0,
                                0.0,
                                false,
                                rhythm_core::RoomProfileSettings::default(),
                            );
                        }
                    } else {
                        for room in persisted.iter() {
                            runtime.restore_room_state(
                                &room.id,
                                room.rhythm_enabled,
                                room.disabled,
                                room.time_offset_minutes,
                                room.brightness_offset,
                                room.soft_off,
                                room.profile_settings.clone(),
                            );
                        }
                    }
                }
                Err(_) => {
                    for snap in runtime.engine_all_room_snapshots() {
                        runtime.restore_room_state(
                            &snap.id,
                            true,
                            false,
                            0.0,
                            0.0,
                            false,
                            rhythm_core::RoomProfileSettings::default(),
                        );
                    }
                }
            }
        }
    }

    let runtime = std::sync::Arc::new(runtime);

    // Push settings to engine
    {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        runtime.set_power_save(s.power_save);
    }

    // Store runtime and composite controller
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.runtime_config = new_runtime_config;
        #[cfg(feature = "desktop")]
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

    // Signal old hub threads to stop (only the matching hub)
    // Take it out of the map but defer drop to a background thread
    // (reqwest::blocking::Client panics if dropped on a tokio worker).
    if let Some(ref key) = hub_key {
        let old_hub = {
            let mut s = state
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
            s.hubs.remove(key)
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

    // Save credentials to state and storage (additive)
    {
        let mut s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        if let Some(ref key) = hub_key {
            s.hub_credentials.insert(key.clone(), creds);
        }
        if let Some(ref storage) = s.storage {
            let all_creds: Vec<_> = s.hub_credentials.values().cloned().collect();
            if let Err(e) = storage.save_all_hub_credentials(&all_creds) {
                warn!(target: "sys", "Failed to save credentials: {}", e);
            }
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
        s.hubs.insert(key, hub);
        s.pending_hub_event_rxs.push(event_rx);
    }

    info!(target: "sys", "Hub configured (connected, runtime deferred)");
    Ok(())
}
