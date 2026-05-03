//! Shared test harness for scenario integration tests.
//!
//! Provides `TestHarness` — a headless Rhythm system with a real `RhythmRuntime`
//! (using `NoOpController`) so engine state mutations flow between steps, unlike
//! the `MockRuntime` in unit tests which returns static snapshots.
//!
//! ## Design principles
//!
//! 1. **Real engine, fake I/O** — `NoOpController` swallows light commands;
//!    `MockTimeProvider` gives deterministic time. Engine state is real.
//! 2. **Same lifecycle as production** — runtime starts as `None`, created lazily
//!    on first room via `ensure_runtime_fn`.
//! 3. **Call public APIs only** — `commands::do_*`, `room_sync::sync_from_hub_for_key`.
//! 4. **Assert on observable state** — observed power, engine snapshots, return values.
//! 5. **Each test is self-contained** — fresh `TestHarness` per test.
//! 6. **Read top-to-bottom as a scenario** — setup → action → assertion.

#![allow(dead_code)] // Helpers may not be used by every test file

use std::sync::{Arc, Mutex};

use anyhow::Result;
use rhythm_core::controller::NoOpController;
use rhythm_core::runtime::handle::{RoomSnapshot, RuntimeHandle};
use rhythm_core::runtime::orchestrator::RhythmRuntime;
use rhythm_core::runtime::registry::SimpleDeviceRegistry;
use rhythm_core::runtime::scheduler::NoOpScheduler;
use rhythm_core::runtime::time::MockTimeProvider;
use rhythm_core::runtime::RuntimeConfig;
use rhythm_core::spy_controller::SpyLightController;
use rhythm_core::HubRegistry;
use rhythm_core::{
    default_day_idle_profile, default_rhythm_profile, default_sleep_idle_profile,
    default_sleep_profile, LightProfileConfig, ModeConfig, DAY_IDLE_PROFILE_ID, RHYTHM_PROFILE_ID,
    SLEEP_IDLE_PROFILE_ID, SLEEP_PROFILE_ID,
};

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::commands;
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};
use rhythm_os::hub::{ActiveHub, HubType};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::room_sync::{self, SyncReport};
use rhythm_os::state::{
    AppState, MotionSnapshot, ObservedPowerSource, ObservedPowerState, SharedState,
};

fn default_profile_for_id(profile_id: &str) -> LightProfileConfig {
    match profile_id {
        RHYTHM_PROFILE_ID => default_rhythm_profile(),
        SLEEP_PROFILE_ID => default_sleep_profile(),
        DAY_IDLE_PROFILE_ID => default_day_idle_profile(),
        SLEEP_IDLE_PROFILE_ID => default_sleep_idle_profile(),
        _ => panic!("unknown built-in profile: {}", profile_id),
    }
}

use rhythm_core::runtime::hub_registry::DeviceType;

// ============================================================================
// TestHarness
// ============================================================================

/// Headless Rhythm system for scenario integration tests.
///
/// Wraps `SharedState` with a real `RhythmRuntime<NoOpController, ...>` so
/// engine state mutations (room add, action dispatch, fix) all flow through
/// the same code paths as production.
///
/// Supports multi-hub scenarios via `add_hub()` and `sync_hub()`.
pub struct TestHarness {
    pub state: SharedState,
    pub hub_key: HubKey,
    /// Additional hub keys for multi-hub scenarios.
    pub extra_hub_keys: Vec<HubKey>,
}

impl Default for TestHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl TestHarness {
    /// Create a new harness with an empty hub (runtime created on first room).
    ///
    /// Mirrors production `connect_hub()`:
    /// - `AppState::default()` (no I/O)
    /// - `ActiveHub` with `runtime: None`, `registry: Some(HubDeviceRegistry)`
    /// - `ensure_runtime_fn` creates real `RhythmRuntime<NoOpController, ...>`
    ///
    /// Mock time defaults to 2 PM on June 21, 2026. Use `new_at` to vary.
    pub fn new() -> Self {
        Self::new_at(14.0, 172)
    }

    /// Like `new()` but with a configurable mock time-of-day.
    ///
    /// `hour` is in 24h decimal (e.g. 6.5 = 6:30 AM); `day_of_year` is 1-365.
    /// Use this to test time-sensitive behavior (sleep boundaries, idle profile,
    /// low-sun curves) that the default afternoon time hides.
    pub fn new_at(hour: f32, day_of_year: u32) -> Self {
        let hub_type = HubType::new("mock");
        let hub_key = HubKey::new(hub_type.clone(), "192.168.1.100");

        let registry = HubDeviceRegistry::new();
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(registry));

        let mut app = AppState {
            latitude: Some(35.0),
            longitude: Some(-120.0),
            utc_offset_hours: -8.0,
            ..Default::default()
        };

        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key: hub_key.clone(),
                runtime: None, // Deferred — created on first room, just like production
                hub_data: Box::new(()),
                registry: Some(registry),
                discovery: None,
                shutdown: Default::default(),
            },
        );

        // Set up the runtime creation callback (mirrors platform crate startup)
        let ensure_key = hub_key.clone();
        app.ensure_runtime_fn = Some(Arc::new(move |state: &SharedState| {
            // Check if runtime already exists
            {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                if s.hub_runtime().is_some() {
                    return Ok(());
                }
            }

            let runtime = RhythmRuntime::new(
                Arc::new(NoOpController::new()),
                MockTimeProvider::new(hour, day_of_year, 2026),
                NoOpScheduler::new(),
                SimpleDeviceRegistry::new(),
                RuntimeConfig::default(),
            );

            let runtime: Arc<dyn RuntimeHandle> = Arc::new(runtime);

            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            if let Some(hub) = s.hubs.get_mut(&ensure_key) {
                hub.runtime = Some(runtime);
            }
            Ok(())
        }));

        let state: SharedState = Arc::new(Mutex::new(app));
        Self {
            state,
            hub_key,
            extra_hub_keys: Vec::new(),
        }
    }

    /// Create a harness with a `SpyLightController` so tests can verify
    /// what `LightingCommand` values the engine dispatches.
    ///
    /// Returns `(harness, spy)` where `spy` can be inspected for recorded calls.
    /// Mock time defaults to 2 PM on June 21, 2026; use `with_spy_controller_at`
    /// to vary.
    pub fn with_spy_controller() -> (Self, Arc<SpyLightController>) {
        Self::with_spy_controller_at(14.0, 172)
    }

    /// Like `with_spy_controller()` but with a configurable mock time-of-day.
    pub fn with_spy_controller_at(hour: f32, day_of_year: u32) -> (Self, Arc<SpyLightController>) {
        let hub_type = HubType::new("mock");
        let hub_key = HubKey::new(hub_type.clone(), "192.168.1.100");

        let registry = HubDeviceRegistry::new();
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(registry));

        let mut app = AppState {
            latitude: Some(35.0),
            longitude: Some(-120.0),
            utc_offset_hours: -8.0,
            ..Default::default()
        };

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

        let spy = Arc::new(SpyLightController::new());
        let spy_for_closure = spy.clone();
        let ensure_key = hub_key.clone();
        app.ensure_runtime_fn = Some(Arc::new(move |state: &SharedState| {
            {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                if s.hub_runtime().is_some() {
                    return Ok(());
                }
            }

            let runtime = RhythmRuntime::new(
                spy_for_closure.clone(),
                MockTimeProvider::new(hour, day_of_year, 2026),
                NoOpScheduler::new(),
                SimpleDeviceRegistry::new(),
                RuntimeConfig::default(),
            );

            let runtime: Arc<dyn RuntimeHandle> = Arc::new(runtime);

            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            if let Some(hub) = s.hubs.get_mut(&ensure_key) {
                hub.runtime = Some(runtime);
            }
            Ok(())
        }));

        let state: SharedState = Arc::new(Mutex::new(app));
        let harness = Self {
            state,
            hub_key,
            extra_hub_keys: Vec::new(),
        };
        (harness, spy)
    }

    /// Set up hub discovery with rooms and optional devices.
    ///
    /// Call before `sync()` to configure what the hub will "discover".
    pub fn with_discovery(
        self,
        rooms: Vec<DiscoveredRoom>,
        devices: Vec<DiscoveredDevice>,
    ) -> Self {
        let discovery = MockDiscovery::from_discovered(rooms, devices);
        {
            let mut s = self.state.lock().unwrap();
            if let Some(hub) = s.hubs.get_mut(&self.hub_key) {
                hub.discovery = Some(Arc::new(discovery));
            }
        }
        self
    }

    /// Run room sync from the hub's discovery.
    ///
    /// Calls `sync_from_hub_for_key` — the same function production uses.
    /// First call triggers runtime creation via `ensure_runtime_fn`.
    pub fn sync(&self) -> SyncReport {
        room_sync::sync_from_hub_for_key(&self.state, &self.hub_key, true)
            .expect("sync_from_hub_for_key failed")
    }

    /// Resolve a primary-hub room or node alias to the topology node ID used by the engine.
    ///
    /// Tests use short hub-native IDs like `"kitchen"` for readability, but
    /// the engine uses topology UUIDs. Hub-native aliases are scoped to the
    /// primary test hub; multi-hub scenarios should call `resolve_for_hub`.
    pub fn resolve(&self, room_id: &str) -> String {
        self.resolve_for_hub(&self.hub_key, room_id)
    }

    /// Resolve a hub-scoped room or node alias to the topology node ID used by the engine.
    pub fn resolve_for_hub(&self, hub_key: &HubKey, room_id: &str) -> String {
        let s = self.state.lock().unwrap();
        s.topology
            .resolve_room_alias(Some(hub_key), room_id)
            .unwrap_or_else(|| room_id.to_string())
    }

    /// Mark a room's lights as on or off.
    ///
    /// In production, this is set by event handlers (button press, motion)
    /// and action dispatch. Use this to set up initial state for scenarios
    /// that test downstream behavior.
    /// Translates hub-native IDs to topology IDs automatically.
    pub fn set_lights_on(&self, room_id: &str, on: bool) {
        let resolved = self.resolve(room_id);
        let mut state = self.state.lock().unwrap();
        state.room_observed_power.insert(
            resolved,
            ObservedPowerState::new(on, ObservedPowerSource::Command),
        );
    }

    /// Add an active motion snapshot for a room.
    ///
    /// Simulates what the event loop does when motion is detected.
    /// Translates hub-native IDs to topology IDs automatically.
    pub fn set_motion_active(&self, room_id: &str) {
        let resolved = self.resolve(room_id);
        self.state.lock().unwrap().motion_snapshots.insert(
            resolved,
            MotionSnapshot {
                motion_active: true,
                motion_owned: true,
                remaining_secs: None,
                timeout_secs: 300,
                warning_active: false,
            },
        );
    }

    /// Dispatch a named action to a room.
    ///
    /// Actions: "on", "off", "toggle", "reset", "rhythm_on", "rhythm_off",
    /// "step_up", "step_down", "dim_up", "dim_down", "lights_off".
    /// Translates hub-native IDs to topology IDs automatically.
    pub fn action(&self, room_id: &str, action: &str) -> Result<String> {
        let resolved = self.resolve(room_id);
        commands::do_node_action(&self.state, &resolved, action, false)
    }

    /// Read a room's engine snapshot (returns `None` if room doesn't exist).
    /// Translates hub-native IDs to topology IDs automatically.
    pub fn snapshot(&self, room_id: &str) -> Option<RoomSnapshot> {
        let resolved = self.resolve(room_id);
        let s = self.state.lock().unwrap();
        s.hub_runtime()?.engine_room_snapshot(&resolved)
    }

    /// Read all engine room snapshots.
    pub fn all_snapshots(&self) -> Vec<RoomSnapshot> {
        let s = self.state.lock().unwrap();
        s.hub_runtime()
            .map(|rt| rt.engine_all_room_snapshots())
            .unwrap_or_default()
    }

    /// Check if a room's lights are tracked as on.
    /// Translates hub-native IDs to topology IDs automatically.
    pub fn lights_on(&self, room_id: &str) -> bool {
        let resolved = self.resolve(room_id);
        self.state
            .lock()
            .unwrap()
            .room_observed_power
            .get(&resolved)
            .map(|observed| observed.lights_on)
            .unwrap_or(false)
    }

    /// Get the list of room IDs pending motion clear.
    pub fn pending_motion_clears(&self) -> Vec<String> {
        self.state.lock().unwrap().pending_motion_clear.clone()
    }

    // ========================================================================
    // Multi-hub support
    // ========================================================================

    /// Add a second (or third) hub to the harness.
    ///
    /// Returns the `HubKey` for the new hub. The new hub shares the same
    /// `ensure_runtime_fn` — runtime is only created once (first room from
    /// any hub triggers it).
    pub fn add_hub(&mut self, hub_type_str: &str, address: &str) -> HubKey {
        let hub_type = HubType::new(hub_type_str);
        let hub_key = HubKey::new(hub_type.clone(), address);

        let registry = HubDeviceRegistry::new();
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(registry));

        let mut s = self.state.lock().unwrap();
        s.hubs.insert(
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

        // Share runtime with the new hub once it exists (deferred)
        let primary_key = self.hub_key.clone();
        let new_key = hub_key.clone();
        let old_fn = s.ensure_runtime_fn.clone();
        s.ensure_runtime_fn = Some(Arc::new(move |state: &SharedState| {
            // Run original ensure_runtime_fn
            if let Some(ref f) = old_fn {
                f(state)?;
            }
            // Copy runtime to new hub
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let runtime = s.hubs.get(&primary_key).and_then(|h| h.runtime.clone());
            if let (Some(runtime), Some(hub)) = (runtime, s.hubs.get_mut(&new_key)) {
                if hub.runtime.is_none() {
                    hub.runtime = Some(runtime);
                }
            }
            Ok(())
        }));
        drop(s);

        self.extra_hub_keys.push(hub_key.clone());
        hub_key
    }

    /// Set up discovery data for a specific hub.
    pub fn set_hub_discovery(
        &self,
        hub_key: &HubKey,
        rooms: Vec<DiscoveredRoom>,
        devices: Vec<DiscoveredDevice>,
    ) {
        let discovery = MockDiscovery::from_discovered(rooms, devices);
        let mut s = self.state.lock().unwrap();
        if let Some(hub) = s.hubs.get_mut(hub_key) {
            hub.discovery = Some(Arc::new(discovery));
        }
    }

    /// Run room sync for a specific hub.
    pub fn sync_hub(&self, hub_key: &HubKey) -> SyncReport {
        room_sync::sync_from_hub_for_key(&self.state, hub_key, true)
            .expect("sync_from_hub_for_key failed")
    }

    /// Sync ALL connected hubs.
    pub fn sync_all(&self) -> SyncReport {
        room_sync::sync_all_hubs(&self.state).expect("sync_all_hubs failed")
    }

    // ========================================================================
    // Triage helpers
    // ========================================================================

    /// Get the number of pending triage entries.
    pub fn triage_pending_count(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .canonical_registry
            .triage()
            .pending_count()
    }

    /// Get the number of pending room binding entries.
    pub fn triage_pending_room_count(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .canonical_registry
            .triage()
            .pending_room_count()
    }

    /// Get the number of pending device merge entries.
    pub fn triage_pending_device_count(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .canonical_registry
            .triage()
            .pending_device_count()
    }

    /// Get all pending triage entry IDs.
    pub fn triage_pending_ids(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .canonical_registry
            .triage()
            .pending()
            .iter()
            .map(|e| e.id.clone())
            .collect()
    }

    /// Get a pending room binding entry: returns (entry_id, hub_room_name, target_rhythm_room_id).
    pub fn triage_room_binding(&self, index: usize) -> Option<(String, String, String)> {
        let s = self.state.lock().unwrap();
        let bindings: Vec<_> = s
            .canonical_registry
            .triage()
            .pending_by_kind(rhythm_os::canonical::triage::TriageKind::RoomBinding)
            .into_iter()
            .collect();
        bindings.get(index).map(|e| {
            let rb = e.room_binding.as_ref().unwrap();
            (
                e.id.clone(),
                rb.hub_room_name.clone(),
                rb.target_rhythm_room_id.clone(),
            )
        })
    }

    /// Resolve a triage room binding (merge rooms).
    pub fn triage_bind(&self, entry_id: &str) -> Result<()> {
        commands::do_triage_bind_room(&self.state, entry_id)
    }

    /// Resolve a triage room binding to a specific target room.
    pub fn triage_bind_to(&self, entry_id: &str, target_room_id: &str) -> Result<()> {
        commands::do_triage_bind_room_to(&self.state, entry_id, Some(target_room_id))
    }

    /// Resolve a triage entry as new device (keep separate).
    pub fn triage_new(&self, entry_id: &str) -> Result<String> {
        commands::do_triage_new_device(&self.state, entry_id)
    }

    /// Dismiss a triage entry.
    pub fn triage_dismiss(&self, entry_id: &str) -> Result<()> {
        commands::do_triage_dismiss(&self.state, entry_id)
    }

    // ========================================================================
    // Topology helpers
    // ========================================================================

    /// Count the number of rooms in topology.
    pub fn topology_room_count(&self) -> usize {
        self.state.lock().unwrap().topology.room_count()
    }

    /// Get the number of hub targets for a topology room (by hub-native ID).
    pub fn hub_target_count(&self, room_id: &str) -> usize {
        let resolved = self.resolve(room_id);
        self.hub_target_count_by_topology_id(&resolved)
    }

    /// Get the number of hub targets for a hub-scoped topology room alias.
    pub fn hub_target_count_for_hub(&self, hub_key: &HubKey, room_id: &str) -> usize {
        let resolved = self.resolve_for_hub(hub_key, room_id);
        self.hub_target_count_by_topology_id(&resolved)
    }

    fn hub_target_count_by_topology_id(&self, room_id: &str) -> usize {
        let s = self.state.lock().unwrap();
        s.topology
            .get(room_id)
            .map(|r| r.hub_room_bindings.len())
            .unwrap_or(0)
    }

    /// Set room preferences (rhythm_enabled, disabled, soft_off).
    pub fn set_room_preferences(
        &self,
        room_id: &str,
        rhythm_enabled: Option<bool>,
        disabled: Option<bool>,
        soft_off: Option<bool>,
    ) {
        let resolved = self.resolve(room_id);
        let target_state = soft_off.map(|soft_off| {
            if soft_off {
                rhythm_core::RoomModeState::Idle
            } else {
                rhythm_core::RoomModeState::Active
            }
        });
        commands::do_node_preferences_set(
            &self.state,
            &resolved,
            rhythm_enabled,
            disabled,
            target_state,
            None,
            false,
        )
        .expect("do_node_preferences_set failed");
    }

    /// Set brightness on a room (0-100).
    pub fn set_brightness(&self, room_id: &str, brightness: u8) {
        let resolved = self.resolve(room_id);
        commands::do_set_node_brightness(&self.state, &resolved, brightness, false)
            .expect("do_set_node_brightness failed");
    }

    // ========================================================================
    // Settings & config helpers
    // ========================================================================

    /// Update global settings (mirrors "Settings → Preferences" screen).
    pub fn set_settings(&self, power_save: Option<bool>) -> String {
        commands::do_settings_set(&self.state, power_save, None, None, None)
            .expect("do_settings_set failed")
    }

    /// Replace the persisted mode profile mappings.
    pub fn set_mode_configs(&self, configs: Vec<ModeConfig>) {
        commands::do_settings_set(&self.state, None, None, Some(configs), None)
            .expect("do_settings_set failed");
    }

    /// Push a new light profile config to the active profile (mirrors "Designer → Save").
    pub fn set_config(&self, config: LightProfileConfig) {
        let active_id = self.state.lock().unwrap().active_mode_profile_id();
        self.set_config_for(&active_id, config);
    }

    /// Push a new light profile config to the requested stored profile.
    pub fn set_config_for(&self, profile_id: &str, mut config: LightProfileConfig) {
        let default = default_profile_for_id(profile_id);
        config.id = default.id;
        if config.name.is_empty() {
            config.name = default.name;
        }
        commands::do_config_set(&self.state, config).expect("do_config_set failed");
    }

    /// Reset the active light profile config to built-in defaults.
    pub fn reset_config(&self) {
        let active_id = self.state.lock().unwrap().active_mode_profile_id();
        self.set_config(default_profile_for_id(&active_id));
    }

    /// Absorb a time offset into the active light profile config.
    pub fn absorb_offset(&self, offset_minutes: f32) {
        commands::do_absorb_time_offset(&self.state, None, offset_minutes)
            .expect("do_absorb_time_offset failed");
    }

    /// Set a room's time offset directly.
    pub fn set_room_offset(&self, room_id: &str, offset: f32) {
        let resolved = self.resolve(room_id);
        commands::do_set_node_time_offset(&self.state, &resolved, offset, false)
            .expect("do_set_node_time_offset failed");
    }

    /// Read the current active light profile config from state.
    pub fn config(&self) -> LightProfileConfig {
        let active_id = self.state.lock().unwrap().active_mode_profile_id();
        self.config_for(&active_id)
    }

    /// Read a stored light profile config by ID.
    pub fn config_for(&self, profile_id: &str) -> LightProfileConfig {
        self.state
            .lock()
            .unwrap()
            .light_profile_config(profile_id)
            .expect("light profile missing")
            .clone()
    }
}

// ============================================================================
// MockDiscovery
// ============================================================================

/// Fake hub discovery for integration tests.
///
/// Stores room/device data as tuples and reconstructs `DiscoveredRoom` /
/// `DiscoveredDevice` on each call (they don't derive Clone).
#[allow(clippy::type_complexity)]
struct MockDiscovery {
    /// (id, name, grouped_light_id)
    rooms: Vec<(String, String, String)>,
    /// (device_id, room_id, buttons, device_type)
    devices: Vec<(String, String, Vec<(String, u8)>, DeviceType)>,
}

impl MockDiscovery {
    fn from_discovered(rooms: Vec<DiscoveredRoom>, devices: Vec<DiscoveredDevice>) -> Self {
        // Build room name index for identity discovery
        let room_names: std::collections::HashMap<String, String> = rooms
            .iter()
            .map(|r| (r.id.clone(), r.name.clone()))
            .collect();
        let _ = room_names; // used in discover_identities via self.rooms

        Self {
            rooms: rooms
                .into_iter()
                .map(|r| (r.id, r.name, r.grouped_light_id))
                .collect(),
            devices: devices
                .into_iter()
                .map(|d| (d.device_id, d.room_id, d.buttons, d.device_type))
                .collect(),
        }
    }
}

impl HubDiscovery for MockDiscovery {
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        Ok(self
            .rooms
            .iter()
            .map(|(id, name, gl)| DiscoveredRoom {
                id: id.clone(),
                name: name.clone(),
                grouped_light_id: gl.clone(),
                device_ids: vec![],
            })
            .collect())
    }

    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        Ok(self
            .devices
            .iter()
            .map(|(did, rid, buttons, dt)| DiscoveredDevice {
                device_id: did.clone(),
                room_id: rid.clone(),
                buttons: buttons.clone(),
                device_type: dt.clone(),
            })
            .collect())
    }

    fn discover_identities(
        &self,
    ) -> Result<Vec<rhythm_os::canonical::identity::DiscoveredIdentity>> {
        // Build room name lookup from rooms
        let room_names: std::collections::HashMap<&str, &str> = self
            .rooms
            .iter()
            .map(|(id, name, _)| (id.as_str(), name.as_str()))
            .collect();

        Ok(self
            .devices
            .iter()
            .map(
                |(did, rid, _, dt)| rhythm_os::canonical::identity::DiscoveredIdentity {
                    native_id: did.clone(),
                    room_id: rid.clone(),
                    room_name: room_names.get(rid.as_str()).unwrap_or(&"").to_string(),
                    name: did.clone(),
                    device_type: dt.clone(),
                    hardware_ids: vec![],
                    manufacturer: None,
                    model: None,
                },
            )
            .collect())
    }
}

// ============================================================================
// Factory helpers — readable test setup
// ============================================================================

/// Create a `DiscoveredRoom` with sensible defaults.
pub fn room(id: &str, name: &str) -> DiscoveredRoom {
    DiscoveredRoom {
        id: id.to_string(),
        name: name.to_string(),
        grouped_light_id: format!("{}_grouped", id),
        device_ids: vec![],
    }
}

/// Create a motion sensor `DiscoveredDevice`.
pub fn motion_sensor(device_id: &str, room_id: &str) -> DiscoveredDevice {
    DiscoveredDevice {
        device_id: device_id.to_string(),
        room_id: room_id.to_string(),
        buttons: vec![],
        device_type: DeviceType::Motion,
    }
}

/// Create a light `DiscoveredDevice`.
pub fn light(device_id: &str, room_id: &str) -> DiscoveredDevice {
    DiscoveredDevice {
        device_id: device_id.to_string(),
        room_id: room_id.to_string(),
        buttons: vec![],
        device_type: DeviceType::Light,
    }
}

/// Create rooms with one light device each (needed for identity/topology sync).
///
/// Returns (rooms, devices) ready for `with_discovery()` or `set_hub_discovery()`.
pub fn rooms_with_lights(specs: &[(&str, &str)]) -> (Vec<DiscoveredRoom>, Vec<DiscoveredDevice>) {
    let rooms: Vec<DiscoveredRoom> = specs.iter().map(|(id, name)| room(id, name)).collect();
    let devices: Vec<DiscoveredDevice> = specs
        .iter()
        .map(|(id, _)| light(&format!("light-{}", id), id))
        .collect();
    (rooms, devices)
}
