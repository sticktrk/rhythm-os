//! Hub event processing and motion timer management.
//!
//! Extracted from ESP32 main.rs so the same event loop logic can be
//! reused across targets (rhythm-server, future Raspberry Pi, etc.).
//!
//! The event loop receives [`HubEvent`]s from the SSE stream and
//! dispatches them to the engine via [`RuntimeHandle`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use rhythm_core::{ButtonAction, InputEvent};

use crate::commands;
use crate::hub::HubEvent;
use crate::state::{MotionSnapshot, SharedState, WorkItem};

/// How many seconds before timeout to start the warning dim.
pub const WARNING_BEFORE_SECS: u64 = 60;

/// Brightness multiplier during warning dim (50% of adaptive).
pub const WARNING_DIM_FACTOR: f32 = 0.5;

/// Per-room motion timer state managed by the main loop.
///
/// Hub-agnostic: any hub can emit `HubEvent::Motion` and this struct
/// handles the timeout logic generically. Tracks individual sensors so
/// that multi-sensor rooms don't start countdown until ALL sensors clear.
pub struct MotionTimerState {
    /// sensor_id -> (room_id, None=active | Some(stopped_at))
    pub sensors: HashMap<String, (String, Option<Instant>)>,
    /// Rooms where motion originally activated the lights.
    /// Manual button press removes a room from here so the
    /// timeout won't turn lights off.
    pub motion_owned: HashSet<String>,
    /// Rooms currently in warning dim state (dimmed to 50% before timeout).
    pub warning_active: HashSet<String>,
}

impl Default for MotionTimerState {
    fn default() -> Self {
        Self::new()
    }
}

impl MotionTimerState {
    pub fn new() -> Self {
        Self {
            sensors: HashMap::new(),
            motion_owned: HashSet::new(),
            warning_active: HashSet::new(),
        }
    }

    /// Check if any sensor for this room is currently tracked.
    pub fn has_sensors_for_room(&self, room_id: &str) -> bool {
        self.sensors.values().any(|(rid, _)| rid == room_id)
    }

    /// Compute per-room motion snapshots from current sensor state.
    pub fn snapshots(
        &self,
        timeouts: &HashMap<String, u64>,
        default_timeout: u64,
    ) -> HashMap<String, MotionSnapshot> {
        let now = Instant::now();

        // Group sensors by room
        let mut rooms: HashMap<&str, Vec<&Option<Instant>>> = HashMap::new();
        for (room_id, stopped_at) in self.sensors.values() {
            rooms.entry(room_id.as_str()).or_default().push(stopped_at);
        }

        let mut result = HashMap::new();
        for (room_id, sensors) in &rooms {
            let any_active = sensors.iter().any(|s| s.is_none());
            let timeout_secs = timeouts.get(*room_id).copied().unwrap_or(default_timeout);
            let owned = self.motion_owned.contains(*room_id);

            let remaining_secs = if any_active {
                None
            } else {
                // All sensors cleared - find elapsed since latest stop
                let elapsed = sensors
                    .iter()
                    .filter_map(|s| s.as_ref())
                    .map(|t| now.duration_since(*t))
                    .min()
                    .unwrap_or(Duration::ZERO);
                Some(timeout_secs.saturating_sub(elapsed.as_secs()))
            };

            result.insert(
                room_id.to_string(),
                MotionSnapshot {
                    motion_active: any_active,
                    motion_owned: owned,
                    remaining_secs,
                    timeout_secs,
                    warning_active: self.warning_active.contains(*room_id),
                },
            );
        }
        result
    }
}

/// Process a button action inline on the calling thread (zero queue delay).
///
/// Calls `runtime.handle_event()` directly. Returns `true` if a deferred
/// persist should be enqueued.
pub fn process_button_inline(
    state: &SharedState,
    room_id: &str,
    action: ButtonAction,
    device_id: Option<&str>,
) -> bool {
    let (runtime, has_hub) = {
        let Ok(s) = state.lock() else {
            warn!(target: "evt", "Inline: state lock poisoned, dropping {:?}", action);
            return false;
        };
        (s.hub_runtime(), s.has_any_hub())
    };
    let Some(runtime) = runtime else {
        warn!(target: "evt", "Inline: no runtime (hub={}) - dropping {:?} for room '{}'",
            has_hub, action, room_id);
        return false;
    };

    let event = if let Some(dev_id) = device_id {
        InputEvent::with_device(room_id, action, dev_id)
    } else {
        InputEvent::new(room_id, action)
    };

    match runtime.handle_event(&event) {
        Ok(turned_on) => {
            info!(target: "evt", "Inline: {:?} room '{}' -> on={}", action, room_id, turned_on);
            crate::commands::sync_active_mode_from_runtime(state, &runtime);
            // Track lights_on state
            if let Ok(mut s) = state.lock() {
                if s.room_mode_transitions.remove(room_id).is_some() {
                    debug!(
                        target: "evt",
                        "Inline: cleared mode transition for room '{}'",
                        room_id
                    );
                }
                s.room_lights_on.insert(room_id.to_string(), turned_on);
            }
            true
        }
        Err(e) => {
            warn!(target: "evt", "Inline: {:?} room '{}' failed: {}", action, room_id, e);
            false
        }
    }
}

/// Dim a room's lights to a factor of adaptive brightness inline on the calling thread.
///
/// Same pattern as `process_button_inline`: gets runtime from state, calls
/// `runtime.dim_room()`. Returns true on success.
pub fn dim_room_inline(state: &SharedState, room_id: &str, factor: f32) -> bool {
    let runtime = {
        let Ok(s) = state.lock() else {
            warn!(target: "evt", "dim_room_inline: state lock poisoned");
            return false;
        };
        s.hub_runtime()
    };
    let Some(runtime) = runtime else {
        return false;
    };
    match runtime.dim_room(room_id, factor) {
        Ok(()) => {
            info!(target: "evt", "dim_room_inline: room '{}' factor={:.1}", room_id, factor);
            true
        }
        Err(e) => {
            warn!(target: "evt", "dim_room_inline: room '{}' failed: {}", room_id, e);
            false
        }
    }
}

/// Turn on a room with adaptive lighting inline on the calling thread.
/// Used by motion - always turns ON (never toggles), skipping the
/// `any_lights_on` HTTP round-trip that `toggle` would perform.
pub fn turn_on_room_inline(state: &SharedState, room_id: &str) -> bool {
    let runtime = {
        let Ok(s) = state.lock() else {
            warn!(target: "evt", "turn_on_room_inline: state lock poisoned");
            return false;
        };
        s.hub_runtime()
    };
    let Some(runtime) = runtime else {
        return false;
    };
    match runtime.turn_on_room(room_id) {
        Ok(()) => {
            info!(target: "evt", "Motion: turn_on room '{}'", room_id);
            if let Ok(mut s) = state.lock() {
                if s.room_mode_transitions.remove(room_id).is_some() {
                    debug!(
                        target: "evt",
                        "Motion: cleared mode transition for room '{}'",
                        room_id
                    );
                }
                s.room_lights_on.insert(room_id.to_string(), true);
            }
            true
        }
        Err(e) => {
            warn!(target: "evt", "Motion: turn_on room '{}' failed: {}", room_id, e);
            false
        }
    }
}

/// Translate a hub-native room ID to the topology room ID if available.
///
/// Used at the event boundary so all downstream processing (engine, motion
/// timers, SSE) uses topology room IDs consistently.
///
/// Tries hub-key-specific lookup first, then falls back to searching all
/// hubs (for events where hub_key is None, e.g., button_resolve).
fn translate_room_id(
    state: &SharedState,
    hub_key: Option<&crate::canonical::identity::HubKey>,
    room_id: &str,
) -> String {
    state
        .lock()
        .ok()
        .and_then(|s| {
            // Try key-specific lookup first
            if let Some(k) = hub_key {
                if let Some(id) = s.topology.translate_room_id(k, room_id) {
                    return Some(id.to_string());
                }
            }
            // Fallback: search all hubs (hub-native IDs are globally unique)
            s.topology
                .translate_room_id_any_hub(room_id)
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| room_id.to_string())
}

fn spawn_reconnect_sync(state: &SharedState, hub_key: &crate::canonical::identity::HubKey) {
    let discover_devices = state
        .lock()
        .ok()
        .map(|s| s.platform.full_device_discovery)
        .unwrap_or(false);
    let sync_state = state.clone();
    let sync_hub_key = hub_key.clone();
    let thread_name = format!("hub-resync-{}", sync_hub_key.hub_type.as_str());

    let spawn_result = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            match crate::room_sync::sync_from_hub_for_key(
                &sync_state,
                &sync_hub_key,
                discover_devices,
            ) {
                Ok(report) => {
                    info!(
                        target: "conn",
                        "Hub {} resync: +{} ~{} -{} devices={}",
                        sync_hub_key,
                        report.rooms_added,
                        report.rooms_updated,
                        report.rooms_removed,
                        report.devices_synced
                    );
                    crate::room_sync::poll_initial_light_state(&sync_state);
                }
                Err(e) => {
                    warn!(target: "conn", "Hub {} resync failed: {}", sync_hub_key, e);
                }
            }
        });

    if let Err(e) = spawn_result {
        warn!(
            target: "conn",
            "Failed to spawn reconnect sync thread for {}: {}",
            hub_key,
            e
        );
    }
}

/// Handle a hub-agnostic event from the event stream.
///
/// Routes events through `RuntimeHandle` regardless of which hub produced them.
/// Button/motion events are processed inline (zero queue delay), with persist
/// deferred to the cmd-worker thread.
pub fn handle_hub_event(state: &SharedState, event: HubEvent, motion: &mut MotionTimerState) {
    let hub_key = event.hub_key().cloned();
    let connected = !matches!(&event, HubEvent::Disconnected { .. });
    let became_connected = if let Some(ref key) = hub_key {
        if let Ok(mut s) = state.lock() {
            let was_connected = s.hub_is_connected(key);
            s.set_hub_connected(key, connected);
            connected && !was_connected
        } else {
            false
        }
    } else {
        false
    };

    match event {
        HubEvent::Connected { .. } => {
            info!(target: "conn", "Hub connected");
            if became_connected {
                if let Some(ref key) = hub_key {
                    spawn_reconnect_sync(state, key);
                }
            }

            #[cfg(feature = "desktop")]
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::HubStatus {
                    hub_type: hub_key.as_ref().map(|k| k.hub_type.as_str().to_string()),
                    address: hub_key.as_ref().map(|k| k.address.clone()),
                    connected: true,
                },
            );
        }

        HubEvent::Button {
            ref hub_key,
            ref room_id,
            action,
            ref device_id,
        } => {
            let room_id = translate_room_id(state, hub_key.as_ref(), room_id);
            // Manual button press clears motion state - user has taken control
            motion.motion_owned.remove(&room_id);
            motion.warning_active.remove(&room_id);
            motion.sensors.retain(|_, (rid, _)| *rid != room_id);

            // Process inline (zero queue delay)
            if process_button_inline(state, &room_id, action, device_id.as_deref()) {
                // Emit SSE event for the affected room
                #[cfg(feature = "desktop")]
                {
                    let runtime = {
                        let Ok(s) = state.lock() else { return };
                        s.hub_runtime()
                    };
                    if let Some(runtime) = runtime {
                        if let Some(snap) = runtime.engine_room_snapshot(&room_id) {
                            crate::state::emit_server_event(
                                state,
                                crate::server_event::ServerEvent::RoomState {
                                    rooms: vec![crate::commands::build_room_state_event(
                                        state, &snap,
                                    )],
                                },
                            );
                        }
                    }
                }

                let work_tx = {
                    let Ok(s) = state.lock() else { return };
                    s.work_tx.clone()
                };
                if let Some(tx) = work_tx {
                    let _ = tx.try_send(WorkItem::DeferredPersist {
                        room_id: room_id.clone(),
                    });
                } else {
                    // No work queue (e.g. rhythm-server) - persist inline
                    commands::persist_rooms(state);
                }
            }
        }

        HubEvent::Motion {
            ref hub_key,
            ref room_id,
            ref sensor_id,
            detected,
        } => {
            let room_id = translate_room_id(state, hub_key.as_ref(), room_id);
            let sensor_id = sensor_id.clone();
            if detected {
                // If warning dim was active, restore full brightness immediately
                if motion.warning_active.remove(&room_id) {
                    info!(target: "evt", "Motion: restoring full brightness in room {} (was warning-dimmed)", room_id);
                    dim_room_inline(state, &room_id, 1.0);
                }

                let is_new_room = !motion.has_sensors_for_room(&room_id);
                // None = motion ongoing, don't countdown
                motion
                    .sensors
                    .insert(sensor_id.clone(), (room_id.clone(), None));

                if is_new_room {
                    // Motion always takes ownership - timeout will turn lights off
                    // (manual button press clears ownership via HubEvent::Button handler)
                    motion.motion_owned.insert(room_id.clone());

                    info!(target: "evt", "Motion: new activation in room {} sensor {} (owned=true)",
                        room_id, sensor_id);

                    // Always turn ON with adaptive values - never toggle.
                    turn_on_room_inline(state, &room_id);
                } else {
                    info!(target: "evt", "Motion: continued/refreshed in room {} sensor {}", room_id, sensor_id);
                }
            } else {
                // Motion stopped for this sensor - mark with timestamp
                if motion.sensors.contains_key(&sensor_id) {
                    motion
                        .sensors
                        .insert(sensor_id.clone(), (room_id.clone(), Some(Instant::now())));

                    // Log whether all sensors for this room are now cleared
                    let all_cleared = motion
                        .sensors
                        .values()
                        .filter(|(rid, _)| rid == &room_id)
                        .all(|(_, stopped)| stopped.is_some());
                    info!(target: "evt", "Motion: sensor {} stopped in room {} - all_cleared={}", sensor_id, room_id, all_cleared);
                } else {
                    info!(target: "evt", "Motion: detected=false for sensor {} room {} but not tracked, ignoring", sensor_id, room_id);
                }
            }
        }

        HubEvent::Heartbeat { .. } => {
            let on_heartbeat = {
                let Ok(s) = state.lock() else { return };
                s.on_hub_heartbeat.clone()
            };
            if let Some(cb) = on_heartbeat {
                cb();
            }
        }

        HubEvent::Disconnected { reason, .. } => {
            warn!(target: "conn", "Hub disconnected: {}", reason);

            // Motion timers are local state (Instant timestamps) — they keep
            // counting regardless of hub connectivity.  Clearing them here
            // means a transient SSE reconnection permanently prevents the
            // timeout from firing, so lights never turn off.

            #[cfg(feature = "desktop")]
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::HubStatus {
                    hub_type: hub_key.as_ref().map(|k| k.hub_type.as_str().to_string()),
                    address: hub_key.as_ref().map(|k| k.address.clone()),
                    connected: false,
                },
            );

            let on_disconnect = {
                let Ok(s) = state.lock() else { return };
                s.on_hub_disconnect.clone()
            };
            if let Some(cb) = on_disconnect {
                cb();
            }
        }

        HubEvent::DevicePaired {
            ref device_id,
            ref name,
            ..
        } => {
            info!(target: "evt", "Device paired: {} ({})",
                name, device_id);
        }
    }
}

/// Check motion timers and turn off lights in expired motion-owned rooms.
///
/// Groups sensors by room. For each room, countdown only starts when ALL
/// sensors have cleared. Uses the latest sensor stop time as the countdown
/// start. Per-room timeout is resolved from the room's selected base profile
/// plus any persisted `room_profile.motion_timeout_secs` override. Timeout=0
/// disables auto-off.
pub fn check_motion_timers(state: &SharedState, motion: &mut MotionTimerState) {
    let now = Instant::now();

    let (timeouts, _) = commands::resolved_room_motion_timeout_map(state);
    let default_timeout_secs = state
        .lock()
        .ok()
        .map(|s| s.default_motion_timeout_secs)
        .unwrap_or(0);

    // Group sensors by room_id
    let mut rooms: HashMap<String, Vec<(&String, &Option<Instant>)>> = HashMap::new();
    for (sensor_id, (room_id, stopped_at)) in &motion.sensors {
        rooms
            .entry(room_id.clone())
            .or_default()
            .push((sensor_id, stopped_at));
    }

    // Log timer status
    let status: Vec<String> = rooms
        .iter()
        .map(|(room_id, sensors)| {
            let room_timeout = timeouts
                .get(room_id)
                .copied()
                .unwrap_or(default_timeout_secs);
            let sensor_status: Vec<String> = sensors
                .iter()
                .map(|(sid, stopped)| match stopped {
                    None => format!("{}=active", sid),
                    Some(t) => format!(
                        "{}={}s/{}s",
                        sid,
                        now.duration_since(*t).as_secs(),
                        room_timeout
                    ),
                })
                .collect();
            format!("{}:[{}]", room_id, sensor_status.join(","))
        })
        .collect();
    info!(target: "evt", "Motion: checking timers - {} rooms: {}", rooms.len(), status.join(" "));

    let mut expired_rooms: Vec<String> = Vec::new();

    for (room_id, sensors) in &rooms {
        // If ANY sensor is still active (None), skip this room
        if sensors.iter().any(|(_, stopped)| stopped.is_none()) {
            continue;
        }

        // All sensors cleared - find elapsed time since the LATEST stop
        // (minimum elapsed = most recent stop time)
        let Some(elapsed) = sensors
            .iter()
            .filter_map(|(_, stopped)| stopped.as_ref())
            .map(|t| now.duration_since(*t))
            .min()
        else {
            continue;
        };

        // Get per-room timeout (fallback to default)
        let timeout_secs = timeouts
            .get(room_id)
            .copied()
            .unwrap_or(default_timeout_secs);

        // Timeout=0 means auto-off disabled for this room
        if timeout_secs == 0 {
            continue;
        }

        let timeout = Duration::from_secs(timeout_secs);
        if elapsed > timeout {
            expired_rooms.push(room_id.clone());
        } else {
            // Warning dim: if within WARNING_BEFORE_SECS of timeout, timeout is long
            // enough, room is motion-owned, and not already warned -> dim to 50%
            let remaining = timeout_secs.saturating_sub(elapsed.as_secs());
            if remaining <= WARNING_BEFORE_SECS
                && timeout_secs > WARNING_BEFORE_SECS
                && motion.motion_owned.contains(room_id.as_str())
                && !motion.warning_active.contains(room_id.as_str())
            {
                info!(target: "evt", "Motion: warning dim for room {} ({}s remaining)", room_id, remaining);
                dim_room_inline(state, room_id, WARNING_DIM_FACTOR);
                motion.warning_active.insert(room_id.clone());
            }
        }
    }

    for room_id in &expired_rooms {
        // Remove all sensors for this room
        motion.sensors.retain(|_, (rid, _)| rid != room_id);
        motion.warning_active.remove(room_id);

        if motion.motion_owned.remove(room_id) {
            info!(target: "evt", "Motion: timeout expired for room {} - sending OffPress", room_id);

            // Dispatch via work queue if available, otherwise execute inline
            let work_tx = {
                let Ok(s) = state.lock() else { continue };
                s.work_tx.clone()
            };
            if let Some(tx) = work_tx {
                let _ = tx.try_send(WorkItem::ButtonAction {
                    room_id: room_id.clone(),
                    action: ButtonAction::OffPress,
                    device_id: None,
                });
            } else {
                // No work queue - execute inline
                process_button_inline(state, room_id, ButtonAction::OffPress, None);
            }
        } else {
            info!(target: "evt", "Motion: timeout expired for room {} - not owned, skipping off", room_id);
        }
    }
}

/// Main event loop — process hub events and check motion timers.
///
/// Runs forever on a blocking thread. Picks up new hub event receivers
/// when credentials are reconfigured via the HTTP API. Supports multiple
/// simultaneous hub event streams — all feed into one shared runtime.
pub fn run_event_loop(
    state: SharedState,
    initial_event_rxs: Vec<std::sync::mpsc::Receiver<HubEvent>>,
) {
    let mut hub_event_rxs: Vec<std::sync::mpsc::Receiver<HubEvent>> = initial_event_rxs;
    let mut motion_state = MotionTimerState::new();
    let mut motion_tick: u32 = 0;
    let mut motion_dirty = false;

    // Seed motion state from startup prefetch of binary_sensor states.
    if let Ok(mut s) = state.lock() {
        if !s.pending_motion_seed.is_empty() {
            let seeds = std::mem::take(&mut s.pending_motion_seed);
            for (sensor_id, room_id) in seeds {
                // Translate hub-native room ID to topology ID (consistent with
                // live HubEvent::Motion handling which calls translate_room_id).
                let room_id = s
                    .topology
                    .translate_room_id_any_hub(&room_id)
                    .map(|s| s.to_string())
                    .unwrap_or(room_id);
                motion_state
                    .sensors
                    .insert(sensor_id, (room_id.clone(), None));
                motion_state.motion_owned.insert(room_id);
            }
            motion_dirty = true;
            info!(target: "evt", "Motion: seeded {} active sensors from startup prefetch",
                motion_state.sensors.len());
        }
    }

    loop {
        // Pick up new hub event receivers from reconfiguration
        if let Ok(mut s) = state.lock() {
            if !s.pending_hub_event_rxs.is_empty() {
                let new_rxs = std::mem::take(&mut s.pending_hub_event_rxs);
                hub_event_rxs.extend(new_rxs);
                // Reset motion state when hub topology changes
                motion_state = MotionTimerState::new();
            }

            // Clear motion timers requested by fix-my-lights or other commands
            if !s.pending_motion_clear.is_empty() {
                let room_ids = std::mem::take(&mut s.pending_motion_clear);
                drop(s); // release lock before logging
                for room_id in &room_ids {
                    motion_state.sensors.retain(|_, (rid, _)| rid != room_id);
                    motion_state.motion_owned.remove(room_id);
                    motion_state.warning_active.remove(room_id);
                }
                if !room_ids.is_empty() {
                    info!(target: "evt", "Motion: cleared timers for {} rooms (fix-my-lights)", room_ids.len());
                    motion_dirty = true;
                }
            }
        }

        // Process hub events from all receivers, removing disconnected ones
        hub_event_rxs.retain(|rx| loop {
            match rx.try_recv() {
                Ok(event) => {
                    handle_hub_event(&state, event, &mut motion_state);
                    motion_dirty = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    warn!(target: "evt", "Hub event channel disconnected");
                    return false;
                }
            }
        });

        // Persist registry if on-demand discovery registered new sensors/devices
        check_registry_dirty(&state);

        // Check motion timers every ~30s (600 * 50ms)
        motion_tick += 1;
        if motion_tick >= 600 {
            motion_tick = 0;
            if !motion_state.sensors.is_empty() {
                check_motion_timers(&state, &mut motion_state);
                motion_dirty = true;
            }
        }

        // Sync motion snapshots when dirty
        if motion_dirty {
            motion_dirty = false;
            sync_motion_snapshots(&state, &motion_state);
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Persist registry to disk if on-demand discovery marked it dirty.
fn check_registry_dirty(state: &SharedState) {
    let dirty = {
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        s.hubs
            .values()
            .filter_map(|h| h.registry.as_ref())
            .any(|r| r.lock().ok().is_some_and(|mut reg| reg.take_dirty()))
    };
    if dirty {
        info!(target: "evt", "Registry dirty from on-demand discovery, persisting");
        commands::persist_registry(state);
    }
}

/// Sync motion timer snapshots into AppState for API visibility.
pub fn sync_motion_snapshots(state: &SharedState, motion: &MotionTimerState) {
    let (timeouts, _) = commands::resolved_room_motion_timeout_map(state);
    let default_timeout_secs = state
        .lock()
        .ok()
        .map(|s| s.default_motion_timeout_secs)
        .unwrap_or(0);

    if let Ok(mut s) = state.lock() {
        if motion.sensors.is_empty() {
            if !s.motion_snapshots.is_empty() {
                s.motion_snapshots.clear();
                #[cfg(feature = "desktop")]
                s.emit_event(crate::server_event::ServerEvent::MotionTimer { timers: vec![] });
            }
        } else {
            s.motion_snapshots = motion.snapshots(&timeouts, default_timeout_secs);
            #[cfg(feature = "desktop")]
            {
                let timers: Vec<_> = s
                    .motion_snapshots
                    .iter()
                    .map(|(room_id, snap)| {
                        crate::server_event::MotionTimerEvent::from_snapshot(room_id, snap)
                    })
                    .collect();
                s.emit_event(crate::server_event::ServerEvent::MotionTimer { timers });
            }
        }
    }
}

/// Process a single work item on the worker thread.
///
/// Handles app-originated button actions, periodic room ticks,
/// and deferred persist from inline button processing.
pub fn process_work_item(state: &SharedState, item: WorkItem) {
    match item {
        WorkItem::ButtonAction {
            room_id,
            action,
            device_id,
        } => {
            let (runtime, has_hub) = {
                let Ok(s) = state.lock() else {
                    warn!(target: "evt", "Worker: state lock poisoned, dropping {:?}", action);
                    return;
                };
                (s.hub_runtime(), s.has_any_hub())
            };
            let Some(runtime) = runtime else {
                warn!(target: "evt", "Worker: no runtime (hub={}) - dropping {:?} for room '{}'",
                    has_hub, action, room_id);
                return;
            };

            let event = if let Some(ref dev_id) = device_id {
                InputEvent::with_device(&room_id, action, dev_id)
            } else {
                InputEvent::new(&room_id, action)
            };

            match runtime.handle_event(&event) {
                Ok(turned_on) => {
                    info!(target: "evt", "Worker: {:?} room '{}' -> on={}", action, room_id, turned_on);
                    crate::commands::sync_active_mode_from_runtime(state, &runtime);
                    if let Ok(mut s) = state.lock() {
                        if s.room_mode_transitions.remove(&room_id).is_some() {
                            debug!(
                                target: "evt",
                                "Worker: cleared mode transition for room '{}'",
                                room_id
                            );
                        }
                        s.room_lights_on.insert(room_id.clone(), turned_on);
                    }
                }
                Err(e) => {
                    warn!(target: "evt", "Worker: {:?} room '{}' failed: {}", action, room_id, e);
                }
            }
        }
        WorkItem::ApplyRoomCommand { room_id, command } => {
            let runtime = {
                let Ok(s) = state.lock() else { return };
                s.hub_runtime()
            };
            let Some(runtime) = runtime else { return };

            if let Err(e) = runtime.apply_room_command(&room_id, command) {
                warn!(
                    target: "cmd",
                    "Worker: apply_room_command for '{}' failed: {}",
                    room_id,
                    e
                );
                return;
            }

            #[cfg(feature = "desktop")]
            if let Some(snap) = runtime.engine_room_snapshot(&room_id) {
                crate::state::emit_server_event(
                    state,
                    crate::server_event::ServerEvent::RoomState {
                        rooms: vec![crate::commands::build_room_state_event(state, &snap)],
                    },
                );
            }
        }
        WorkItem::PeriodicRoomTick {
            room_id,
            current_hour,
        } => {
            let current_hour = state
                .lock()
                .ok()
                .and_then(|mut s| s.pending_periodic_ticks.remove(&room_id))
                .unwrap_or(current_hour);

            let runtime = {
                let Ok(s) = state.lock() else { return };
                s.hub_runtime()
            };
            let Some(runtime) = runtime else { return };

            if let Err(e) = runtime.periodic_tick_room(&room_id, current_hour) {
                warn!(target: "sys", "Periodic room tick '{}' failed: {}", room_id, e);
            }

            crate::periodic::post_tick_room(state, &runtime, &room_id);
        }
        WorkItem::DeferredPersist { room_id: _ } => {
            commands::persist_rooms(state);
        }
        WorkItem::DeferredPersistState => {
            commands::persist_state(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};
    use rhythm_core::runtime::{RoomSnapshot, RuntimeHandle};
    use rhythm_core::{HubRegistry, LightProfileConfig, RoomProfileSettings, TimerSetting};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crate::canonical::identity::HubKey;
    use crate::hub::{ActiveHub, HubType};
    use crate::registry::{RegistrySnapshot, SnapshotRoom};

    #[test]
    fn new_is_empty() {
        let state = MotionTimerState::new();
        assert!(state.sensors.is_empty());
        assert!(state.motion_owned.is_empty());
        assert!(state.warning_active.is_empty());
    }

    #[test]
    fn has_sensors_for_room_true() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), ("room_a".into(), None));
        assert!(state.has_sensors_for_room("room_a"));
    }

    #[test]
    fn has_sensors_for_room_false() {
        let state = MotionTimerState::new();
        assert!(!state.has_sensors_for_room("room_a"));

        let mut state2 = MotionTimerState::new();
        state2
            .sensors
            .insert("sensor_1".into(), ("room_b".into(), None));
        assert!(!state2.has_sensors_for_room("room_a"));
    }

    #[test]
    fn snapshot_active_sensor() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), ("room_a".into(), None));

        let timeouts = HashMap::new();
        let snaps = state.snapshots(&timeouts, 300);

        let snap = snaps.get("room_a").expect("room_a should have a snapshot");
        assert!(snap.motion_active);
        assert_eq!(snap.remaining_secs, None);
        assert_eq!(snap.timeout_secs, 300);
    }

    #[test]
    fn snapshot_stopped_sensor() {
        let mut state = MotionTimerState::new();
        let stopped_at = Instant::now() - Duration::from_secs(10);
        state
            .sensors
            .insert("sensor_1".into(), ("room_a".into(), Some(stopped_at)));

        let timeouts = HashMap::new();
        let snaps = state.snapshots(&timeouts, 300);

        let snap = snaps.get("room_a").expect("room_a should have a snapshot");
        assert!(!snap.motion_active);
        // remaining should be approximately 300 - 10 = 290, allow some tolerance
        let remaining = snap.remaining_secs.expect("should have remaining_secs");
        assert!(
            (288..=291).contains(&remaining),
            "remaining_secs should be ~290, got {}",
            remaining
        );
        assert_eq!(snap.timeout_secs, 300);
    }

    #[test]
    fn snapshot_mixed_sensors_same_room() {
        let mut state = MotionTimerState::new();
        let stopped_at = Instant::now() - Duration::from_secs(5);
        state
            .sensors
            .insert("sensor_1".into(), ("room_a".into(), None));
        state
            .sensors
            .insert("sensor_2".into(), ("room_a".into(), Some(stopped_at)));

        let timeouts = HashMap::new();
        let snaps = state.snapshots(&timeouts, 300);

        let snap = snaps.get("room_a").expect("room_a should have a snapshot");
        assert!(
            snap.motion_active,
            "any active sensor means motion_active=true"
        );
        assert_eq!(snap.remaining_secs, None);
    }

    #[test]
    fn snapshot_per_room_timeout() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), ("room_a".into(), None));

        let mut timeouts = HashMap::new();
        timeouts.insert("room_a".to_string(), 600u64);

        let snaps = state.snapshots(&timeouts, 300);

        let snap = snaps.get("room_a").expect("room_a should have a snapshot");
        assert_eq!(snap.timeout_secs, 600);
    }

    #[test]
    fn snapshot_default_timeout() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), ("room_a".into(), None));

        let timeouts = HashMap::new();
        let snaps = state.snapshots(&timeouts, 120);

        let snap = snaps.get("room_a").expect("room_a should have a snapshot");
        assert_eq!(snap.timeout_secs, 120);
    }

    #[test]
    fn snapshot_motion_owned() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), ("room_a".into(), None));
        state.motion_owned.insert("room_a".into());

        let timeouts = HashMap::new();
        let snaps = state.snapshots(&timeouts, 300);

        let snap = snaps.get("room_a").expect("room_a should have a snapshot");
        assert!(snap.motion_owned);

        // Also verify a room without ownership
        let mut state2 = MotionTimerState::new();
        state2
            .sensors
            .insert("sensor_2".into(), ("room_b".into(), None));

        let snaps2 = state2.snapshots(&timeouts, 300);
        let snap2 = snaps2.get("room_b").expect("room_b should have a snapshot");
        assert!(!snap2.motion_owned);
    }

    #[test]
    fn snapshot_warning_active() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), ("room_a".into(), None));
        state.warning_active.insert("room_a".into());

        let timeouts = HashMap::new();
        let snaps = state.snapshots(&timeouts, 300);

        let snap = snaps.get("room_a").expect("room_a should have a snapshot");
        assert!(snap.warning_active);

        // Also verify a room without warning
        let mut state2 = MotionTimerState::new();
        state2
            .sensors
            .insert("sensor_2".into(), ("room_b".into(), None));

        let snaps2 = state2.snapshots(&timeouts, 300);
        let snap2 = snaps2.get("room_b").expect("room_b should have a snapshot");
        assert!(!snap2.warning_active);
    }

    #[test]
    fn snapshot_multiple_rooms() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), ("room_a".into(), None));
        let stopped_at = Instant::now() - Duration::from_secs(20);
        state
            .sensors
            .insert("sensor_2".into(), ("room_b".into(), Some(stopped_at)));
        state.motion_owned.insert("room_b".into());

        let mut timeouts = HashMap::new();
        timeouts.insert("room_a".to_string(), 180u64);

        let snaps = state.snapshots(&timeouts, 300);

        assert_eq!(snaps.len(), 2);

        let snap_a = snaps.get("room_a").expect("room_a should have a snapshot");
        assert!(snap_a.motion_active);
        assert_eq!(snap_a.timeout_secs, 180);
        assert!(!snap_a.motion_owned);

        let snap_b = snaps.get("room_b").expect("room_b should have a snapshot");
        assert!(!snap_b.motion_active);
        assert_eq!(snap_b.timeout_secs, 300); // default
        assert!(snap_b.motion_owned);
        let remaining = snap_b.remaining_secs.expect("should have remaining_secs");
        assert!(
            (278..=281).contains(&remaining),
            "remaining_secs should be ~280, got {}",
            remaining
        );
    }

    #[test]
    fn constants_defined() {
        assert_eq!(WARNING_BEFORE_SECS, 60);
        assert!((WARNING_DIM_FACTOR - 0.5).abs() < f32::EPSILON);
    }

    // ========================================================================
    // check_motion_timers tests
    // ========================================================================

    fn make_state() -> SharedState {
        std::sync::Arc::new(std::sync::Mutex::new(crate::state::AppState::default()))
    }

    struct MotionTestRuntime {
        snapshots: Vec<RoomSnapshot>,
    }

    impl RuntimeHandle for MotionTestRuntime {
        fn handle_event(&self, _: &rhythm_core::InputEvent) -> anyhow::Result<bool> {
            Ok(false)
        }
        fn sync_rooms(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_solar(&self, _: rhythm_core::SolarTime) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_light_profile_config(&self, _: LightProfileConfig) -> anyhow::Result<()> {
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
                .iter()
                .find(|snap| snap.id == room_id)
                .cloned()
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
            _: RoomProfileSettings,
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
            _: &str,
            _: rhythm_core::LightingCommand,
        ) -> anyhow::Result<()> {
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
        fn set_room_time_offset(&self, _: &str, _: f32) -> anyhow::Result<()> {
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
            12.0
        }
        fn set_light_profile(&self, _: &str) -> bool {
            true
        }
        fn active_light_profile_id(&self) -> String {
            rhythm_core::RHYTHM_PROFILE_ID.to_string()
        }
        fn available_light_profiles(&self) -> Vec<(String, String)> {
            vec![
                ("rhythm".into(), "Rhythm Curve".into()),
                ("sleep".into(), "Sleep Curve".into()),
            ]
        }
    }

    fn make_state_with_motion_override(timeout_secs: u32) -> SharedState {
        let mut app = crate::state::AppState::default();
        let mut active = app
            .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
            .cloned()
            .unwrap();
        active.motion_timeout_secs = TimerSetting::Fixed { value: 300 };
        app.set_light_profile_config(active);
        app.default_motion_timeout_secs = 300;

        let runtime: Arc<dyn RuntimeHandle> = Arc::new(MotionTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings {
                    motion_timeout_secs: Some(TimerSetting::Fixed {
                        value: timeout_secs,
                    }),
                    ..Default::default()
                },
            }],
        });
        let hub_type = HubType::new("test");
        let hub_key = HubKey::new(hub_type.clone(), "hub.local");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime),
                hub_data: Box::new(()),
                registry: None,
                discovery: None,
                shutdown: Arc::new(AtomicBool::new(false)),
            },
        );

        Arc::new(Mutex::new(app))
    }

    struct ReconnectTestDiscovery;

    impl HubDiscovery for ReconnectTestDiscovery {
        fn discover_rooms(&self) -> anyhow::Result<Vec<DiscoveredRoom>> {
            Ok(vec![DiscoveredRoom {
                id: "room_a".into(),
                name: "Room A".into(),
                grouped_light_id: "gl_room_a".into(),
                device_ids: vec!["light-1".into()],
            }])
        }

        fn discover_devices(&self) -> anyhow::Result<Vec<DiscoveredDevice>> {
            Ok(vec![])
        }
    }

    #[test]
    fn connected_event_resyncs_room_devices_for_api_state() {
        let state = make_state();
        let hub_type = HubType::new("test");
        let hub_key = HubKey::new(hub_type.clone(), "hub.local");

        let mut registry = crate::registry::HubDeviceRegistry::new();
        registry.restore_from_snapshot(RegistrySnapshot {
            rooms: vec![SnapshotRoom {
                id: "room_a".into(),
                name: "Room A".into(),
                grouped_light_id: "gl_room_a".into(),
            }],
            devices: vec![],
            buttons: HashMap::new(),
            area_lights: HashMap::new(),
        });
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(registry));

        {
            let mut s = state.lock().unwrap();
            s.hubs.insert(
                hub_key.clone(),
                ActiveHub {
                    hub_type,
                    hub_key: hub_key.clone(),
                    runtime: None,
                    hub_data: Box::new(()),
                    registry: Some(registry),
                    discovery: Some(Arc::new(ReconnectTestDiscovery)),
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
            s.set_hub_connected(&hub_key, false);
        }

        let before: serde_json::Value =
            serde_json::from_str(&crate::commands::build_state_snapshot(&state).unwrap()).unwrap();
        let before_room = before["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .find(|room| room["name"] == "Room A")
            .unwrap();
        assert_eq!(before_room["device_ids"], serde_json::json!([]));

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Connected {
                hub_key: Some(hub_key.clone()),
            },
            &mut MotionTimerState::new(),
        );

        let mut after: Option<serde_json::Value> = None;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            let parsed: serde_json::Value =
                serde_json::from_str(&crate::commands::build_state_snapshot(&state).unwrap())
                    .unwrap();
            let room = parsed["rooms"]
                .as_array()
                .unwrap()
                .iter()
                .find(|candidate| candidate["name"] == "Room A")
                .cloned()
                .unwrap();
            if room["device_ids"] == serde_json::json!(["light-1"]) {
                after = Some(parsed);
                break;
            }
        }

        let after = after.expect("connected event should repopulate room device_ids");
        let after_room = after["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .find(|room| room["name"] == "Room A")
            .unwrap();
        assert_eq!(after_room["device_ids"], serde_json::json!(["light-1"]));
        assert_eq!(after_room["devices"][0]["id"], "light-1");
        assert_eq!(after_room["devices"][0]["type"], "light");
    }

    #[test]
    fn check_motion_timers_all_active_no_expiry() {
        let state = make_state();
        let mut motion = MotionTimerState::new();
        motion.sensors.insert("s1".into(), ("room_a".into(), None));
        motion.sensors.insert("s2".into(), ("room_a".into(), None));
        motion.motion_owned.insert("room_a".into());

        check_motion_timers(&state, &mut motion);

        // All sensors still active, nothing should change
        assert_eq!(motion.sensors.len(), 2);
        assert!(motion.motion_owned.contains("room_a"));
    }

    #[test]
    fn check_motion_timers_expired_motion_owned() {
        let state = make_state();
        state.lock().unwrap().default_motion_timeout_secs = 120;

        let mut motion = MotionTimerState::new();
        // Sensor stopped 200 seconds ago (> 120s timeout)
        let stopped_at = Instant::now() - Duration::from_secs(200);
        motion
            .sensors
            .insert("s1".into(), ("room_a".into(), Some(stopped_at)));
        motion.motion_owned.insert("room_a".into());

        check_motion_timers(&state, &mut motion);

        // Sensor should be removed and motion_owned cleared
        assert!(motion.sensors.is_empty());
        assert!(!motion.motion_owned.contains("room_a"));
    }

    #[test]
    fn check_motion_timers_expired_not_owned() {
        let state = make_state();
        state.lock().unwrap().default_motion_timeout_secs = 120;

        let mut motion = MotionTimerState::new();
        let stopped_at = Instant::now() - Duration::from_secs(200);
        motion
            .sensors
            .insert("s1".into(), ("room_a".into(), Some(stopped_at)));
        // NOT motion_owned — manual button took control

        check_motion_timers(&state, &mut motion);

        // Sensors removed but no OffPress sent (not owned)
        assert!(motion.sensors.is_empty());
    }

    #[test]
    fn check_motion_timers_timeout_zero_disables() {
        let state = make_state_with_motion_override(0);

        let mut motion = MotionTimerState::new();
        let stopped_at = Instant::now() - Duration::from_secs(9999);
        motion
            .sensors
            .insert("s1".into(), ("room_a".into(), Some(stopped_at)));
        motion.motion_owned.insert("room_a".into());

        check_motion_timers(&state, &mut motion);

        // Should NOT expire — timeout=0 means auto-off disabled
        assert_eq!(motion.sensors.len(), 1);
        assert!(motion.motion_owned.contains("room_a"));
    }

    #[test]
    fn check_motion_timers_per_room_timeout() {
        let state = make_state_with_motion_override(60);

        let mut motion = MotionTimerState::new();
        // Stopped 100 seconds ago — past the 60s per-room timeout but within 300s default
        let stopped_at = Instant::now() - Duration::from_secs(100);
        motion
            .sensors
            .insert("s1".into(), ("room_a".into(), Some(stopped_at)));
        motion.motion_owned.insert("room_a".into());

        check_motion_timers(&state, &mut motion);

        // Should have expired (100s > 60s per-room timeout)
        assert!(motion.sensors.is_empty());
    }

    #[test]
    fn check_motion_timers_warning_dim() {
        let state = make_state();
        state.lock().unwrap().default_motion_timeout_secs = 300;

        let mut motion = MotionTimerState::new();
        // Stopped 250 seconds ago: remaining = 50s < WARNING_BEFORE_SECS (60s)
        let stopped_at = Instant::now() - Duration::from_secs(250);
        motion
            .sensors
            .insert("s1".into(), ("room_a".into(), Some(stopped_at)));
        motion.motion_owned.insert("room_a".into());

        check_motion_timers(&state, &mut motion);

        // Warning should be active (timeout > WARNING_BEFORE_SECS and remaining < WARNING_BEFORE_SECS)
        assert!(motion.warning_active.contains("room_a"));
    }

    #[test]
    fn check_motion_timers_multi_sensor_one_active() {
        let state = make_state();
        state.lock().unwrap().default_motion_timeout_secs = 120;

        let mut motion = MotionTimerState::new();
        let stopped_at = Instant::now() - Duration::from_secs(200);
        motion
            .sensors
            .insert("s1".into(), ("room_a".into(), Some(stopped_at)));
        motion.sensors.insert("s2".into(), ("room_a".into(), None)); // still active
        motion.motion_owned.insert("room_a".into());

        check_motion_timers(&state, &mut motion);

        // One sensor still active — should NOT expire
        assert_eq!(motion.sensors.len(), 2);
        assert!(motion.motion_owned.contains("room_a"));
    }

    #[test]
    fn check_motion_timers_multi_sensor_all_cleared() {
        let state = make_state();
        state.lock().unwrap().default_motion_timeout_secs = 120;

        let mut motion = MotionTimerState::new();
        // s1 stopped 200s ago, s2 stopped 50s ago → countdown from latest (50s, not expired)
        let stopped_early = Instant::now() - Duration::from_secs(200);
        let stopped_late = Instant::now() - Duration::from_secs(50);
        motion
            .sensors
            .insert("s1".into(), ("room_a".into(), Some(stopped_early)));
        motion
            .sensors
            .insert("s2".into(), ("room_a".into(), Some(stopped_late)));
        motion.motion_owned.insert("room_a".into());

        check_motion_timers(&state, &mut motion);

        // Latest stop was 50s ago, timeout is 120s — should NOT expire
        assert_eq!(motion.sensors.len(), 2);
        assert!(motion.motion_owned.contains("room_a"));
    }
}
