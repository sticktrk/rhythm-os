//! Hub event processing and motion timer management.
//!
//! Shared across platform binaries so the same event loop logic can be reused
//! across the server, appliance, and future targets.
//!
//! The event loop receives [`HubEvent`]s from the SSE stream and
//! dispatches them to the engine via [`RuntimeHandle`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use rhythm_core::{ButtonAction, InputEvent};

use crate::canonical::identity::HubKey;
use crate::commands;
use crate::hub::HubEvent;
use crate::logging;
use crate::server_event::{InputEventResource, InputEventRoute, ServerEvent};
use crate::state::{current_epoch_ms, MotionSeedEntry, MotionSnapshot, SharedState, WorkItem};
use crate::storage::{StoredMotionTimerEntry, StoredMotionTimers};
use crate::topology::NodeControlKind;

/// How many seconds before timeout to start the warning dim.
pub const WARNING_BEFORE_SECS: u64 = 60;

/// Brightness multiplier during warning dim (50% of adaptive).
pub const WARNING_DIM_FACTOR: f32 = 0.5;

/// Default rate-limit for reconnect-triggered full hub resyncs.
const DEFAULT_RECONNECT_RESYNC_COOLDOWN: Duration = Duration::from_secs(120);
/// Hue SSE recycles frequently; full Hue topology/device resync is expensive and noisy.
const HUE_RECONNECT_RESYNC_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
/// Delay app-visible hub loss so brief SSE reconnects do not flash unavailable.
const HUB_DISCONNECT_GRACE: Duration = Duration::from_secs(120);
/// How often the idle event loop wakes to check for new hub events.
const EVENT_LOOP_IDLE_SLEEP: Duration = Duration::from_millis(10);
/// How often to evaluate motion timeouts while sources are active.
const MOTION_TIMER_CHECK_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct MotionSourceState {
    /// Public source node ID when known, otherwise a hub-scoped synthetic key.
    pub source_node_id: String,
    /// Public target node ID currently controlled by this source.
    pub target_node_id: String,
    /// `None` while motion is active, `Some(stopped_at)` once the source clears.
    pub stopped_at: Option<Instant>,
    /// Wall-clock timestamp matching `stopped_at`, persisted across restarts.
    pub stopped_at_epoch_ms: Option<u64>,
}

/// Per-target motion timer state managed by the main loop.
///
/// Hub-agnostic: any hub can emit `HubEvent::Motion` and this struct
/// handles the timeout logic generically. Tracks individual sensors so
/// that multi-source targets don't start countdown until ALL sources clear.
pub struct MotionTimerState {
    /// source tracking key -> motion source state
    pub sensors: HashMap<String, MotionSourceState>,
    /// Targets where motion originally activated the lights.
    /// Manual button press removes a target from here so the
    /// timeout won't turn lights off.
    pub motion_owned: HashSet<String>,
    /// Targets currently in warning dim state (dimmed to 50% before timeout).
    pub warning_active: HashSet<String>,
    /// Last-dispatch time per (node_id, ButtonAction, device_id) used to drop
    /// hardware bounces. Device ID is included when available so two distinct
    /// controllers targeting the same room do not suppress each other.
    pub button_debounce: HashMap<(String, ButtonAction, Option<String>), Instant>,
}

/// Debounce window for repeated identical button presses on the same node.
/// 150ms is comfortably above realistic hardware-bounce intervals while
/// staying below the threshold a human would notice when intentionally
/// double-tapping a button (which is normally measured against a separate
/// short_release / long_release boundary, not raw event arrivals).
pub const BUTTON_DEBOUNCE_WINDOW: Duration = Duration::from_millis(150);

fn input_event_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn hub_event_fields(hub_key: Option<&HubKey>) -> (Option<String>, Option<String>) {
    match hub_key {
        Some(key) => (
            Some(key.hub_type.as_str().to_string()),
            Some(key.address.clone()),
        ),
        None => (None, None),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_button_input_event(
    state: &SharedState,
    hub_key: Option<&HubKey>,
    source_node_id: Option<&str>,
    target_node_id: Option<&str>,
    source_room_id: Option<&str>,
    native_device_id: Option<&str>,
    native_button_id: Option<&str>,
    action: Option<ButtonAction>,
    route: InputEventRoute,
) {
    let (hub_type, address) = hub_event_fields(hub_key);
    crate::state::emit_server_event(
        state,
        ServerEvent::InputEvent(InputEventResource::Button {
            epoch_ms: input_event_epoch_ms(),
            route,
            hub_type,
            address,
            source_node_id: source_node_id.map(str::to_string),
            target_node_id: target_node_id.map(str::to_string),
            source_room_id: source_room_id.map(str::to_string),
            native_device_id: native_device_id.map(str::to_string),
            native_button_id: native_button_id.map(str::to_string),
            button_action: action,
        }),
    );
}

struct MotionInputEventFields<'a> {
    hub_key: Option<&'a HubKey>,
    source_node_id: Option<&'a str>,
    target_node_id: Option<&'a str>,
    source_room_id: Option<&'a str>,
    native_sensor_id: &'a str,
    detected: bool,
    route: InputEventRoute,
}

fn emit_motion_input_event(state: &SharedState, fields: MotionInputEventFields<'_>) {
    let MotionInputEventFields {
        hub_key,
        source_node_id,
        target_node_id,
        source_room_id,
        native_sensor_id,
        detected,
        route,
    } = fields;
    let (hub_type, address) = hub_event_fields(hub_key);
    crate::state::emit_server_event(
        state,
        ServerEvent::InputEvent(InputEventResource::Motion {
            epoch_ms: input_event_epoch_ms(),
            route,
            hub_type,
            address,
            source_node_id: source_node_id.map(str::to_string),
            target_node_id: target_node_id.map(str::to_string),
            source_room_id: source_room_id.map(str::to_string),
            native_sensor_id: native_sensor_id.to_string(),
            detected,
        }),
    );
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
            button_debounce: HashMap::new(),
        }
    }

    /// Record a button dispatch for the given node + action. Returns `true`
    /// if the event should be dispatched, `false` if it falls inside the
    /// debounce window of a prior identical press from the same source.
    ///
    /// On a true return, the table is updated with the current instant. On a
    /// false return, the existing entry is preserved (so a stream of bounces
    /// continues to be suppressed without resetting the window).
    pub fn admit_button(
        &mut self,
        node_id: &str,
        action: ButtonAction,
        device_id: Option<&str>,
    ) -> bool {
        self.admit_button_at(node_id, action, device_id, Instant::now())
    }

    /// Test-friendly variant of [`admit_button`] that takes an explicit
    /// `now` so debounce semantics can be exercised deterministically.
    pub fn admit_button_at(
        &mut self,
        node_id: &str,
        action: ButtonAction,
        device_id: Option<&str>,
        now: Instant,
    ) -> bool {
        let key = (
            node_id.to_string(),
            action,
            device_id.map(std::string::ToString::to_string),
        );
        if let Some(prev) = self.button_debounce.get(&key) {
            if now.duration_since(*prev) < BUTTON_DEBOUNCE_WINDOW {
                return false;
            }
        }
        self.button_debounce.insert(key, now);
        // Garbage-collect entries older than 10× the window so the table
        // doesn't grow unboundedly on a unit running for weeks.
        let cutoff = now.checked_sub(BUTTON_DEBOUNCE_WINDOW * 10);
        if let Some(cutoff) = cutoff {
            self.button_debounce.retain(|_, ts| *ts >= cutoff);
        }
        true
    }

    /// Check if any motion source for this target is currently tracked.
    pub fn has_sources_for_target(&self, target_node_id: &str) -> bool {
        self.sensors
            .values()
            .any(|source| source.target_node_id == target_node_id)
    }

    /// Compute per-target motion snapshots from current source state.
    pub fn snapshots(
        &self,
        timeouts: &HashMap<String, u64>,
        default_timeout: u64,
    ) -> HashMap<String, MotionSnapshot> {
        let now = Instant::now();

        let mut targets: HashMap<&str, Vec<&MotionSourceState>> = HashMap::new();
        for source in self.sensors.values() {
            targets
                .entry(source.target_node_id.as_str())
                .or_default()
                .push(source);
        }

        let mut result = HashMap::new();
        for (target_node_id, sources) in &targets {
            let any_active = sources.iter().any(|source| source.stopped_at.is_none());
            let timeout_secs = timeouts
                .get(*target_node_id)
                .copied()
                .unwrap_or(default_timeout);
            let owned = self.motion_owned.contains(*target_node_id);

            let remaining_secs = if any_active {
                None
            } else {
                let elapsed = sources
                    .iter()
                    .filter_map(|source| source.stopped_at.as_ref())
                    .map(|t| now.duration_since(*t))
                    .min()
                    .unwrap_or(Duration::ZERO);
                Some(timeout_secs.saturating_sub(elapsed.as_secs()))
            };

            result.insert(
                target_node_id.to_string(),
                MotionSnapshot {
                    motion_active: any_active,
                    motion_owned: owned,
                    remaining_secs,
                    timeout_secs,
                    warning_active: self.warning_active.contains(*target_node_id),
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
    node_id: &str,
    action: ButtonAction,
    device_id: Option<&str>,
    command_id: &str,
) -> bool {
    let started = Instant::now();
    let (runtime, has_hub) = {
        let Ok(s) = state.lock() else {
            tracing::warn!(
                target: "evt",
                event = "button_action_dropped",
                command_id = %command_id,
                action = ?action,
                node_id = %node_id,
                device_id = ?device_id,
                source = "inline",
                reason = "state_lock_poisoned",
                "Inline button action dropped"
            );
            return false;
        };
        (s.hub_runtime(), s.has_any_hub())
    };
    let Some(runtime) = runtime else {
        tracing::warn!(
            target: "evt",
            event = "button_action_dropped",
            command_id = %command_id,
            action = ?action,
            node_id = %node_id,
            device_id = ?device_id,
            source = "inline",
            has_hub,
            reason = "no_runtime",
            "Inline button action dropped"
        );
        return false;
    };

    let event = if let Some(dev_id) = device_id {
        InputEvent::with_device(node_id, action, dev_id)
    } else {
        InputEvent::new(node_id, action)
    };

    match runtime.handle_event(&event) {
        Ok(turned_on) => {
            tracing::info!(
                target: "evt",
                event = "button_action_applied",
                command_id = %command_id,
                action = ?action,
                node_id = %node_id,
                device_id = ?device_id,
                source = "inline",
                turned_on,
                latency_ms = started.elapsed().as_millis(),
                "Inline button action applied"
            );
            crate::commands::sync_active_mode_from_runtime(state, &runtime);
            if let Ok(mut s) = state.lock() {
                if s.room_mode_transitions.remove(node_id).is_some() {
                    info!(
                        target: "evt",
                        "Inline: cleared mode transition for node '{}'",
                        node_id
                    );
                }
            }
            crate::commands::update_lights_on_cache_for_runtime_node(
                state, &runtime, node_id, turned_on,
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                target: "evt",
                event = "button_action_failed",
                command_id = %command_id,
                action = ?action,
                node_id = %node_id,
                device_id = ?device_id,
                source = "inline",
                latency_ms = started.elapsed().as_millis(),
                error = %e,
                "Inline button action failed"
            );
            false
        }
    }
}

/// Dim a node's lights to a factor of adaptive brightness inline on the calling thread.
///
/// Same pattern as `process_button_inline`: gets runtime from state, calls
/// `runtime.dim_room()`. Returns true on success.
pub fn dim_node_inline(state: &SharedState, node_id: &str, factor: f32) -> bool {
    let runtime = {
        let Ok(s) = state.lock() else {
            warn!(target: "evt", "dim_node_inline: state lock poisoned");
            return false;
        };
        s.hub_runtime()
    };
    let Some(runtime) = runtime else {
        return false;
    };
    match runtime.dim_room(node_id, factor) {
        Ok(()) => {
            info!(
                target: "evt",
                "dim_node_inline: node '{}' factor={:.1}",
                node_id,
                factor
            );
            true
        }
        Err(e) => {
            warn!(
                target: "evt",
                "dim_node_inline: node '{}' failed: {}",
                node_id,
                e
            );
            false
        }
    }
}

/// Turn on a node with adaptive lighting inline on the calling thread.
/// Used by motion - always turns ON (never toggles), skipping the
/// `any_lights_on` HTTP round-trip that `toggle` would perform.
pub fn turn_on_node_inline(state: &SharedState, node_id: &str) -> bool {
    let command_id = logging::next_command_id("motion");
    let span = tracing::info_span!(
        target: "evt",
        "motion_turn_on",
        command_id = %command_id,
        node_id = %node_id
    );
    let _entered = span.enter();
    let started = Instant::now();
    let runtime = {
        let Ok(s) = state.lock() else {
            warn!(target: "evt", "turn_on_node_inline: state lock poisoned");
            return false;
        };
        s.hub_runtime()
    };
    let Some(runtime) = runtime else {
        return false;
    };
    match runtime.turn_on_room(node_id) {
        Ok(()) => {
            tracing::info!(
                target: "evt",
                event = "motion_turn_on_complete",
                latency_ms = started.elapsed().as_millis(),
                "Motion: turn_on node '{}'",
                node_id
            );
            if let Ok(mut s) = state.lock() {
                if s.room_mode_transitions.remove(node_id).is_some() {
                    info!(
                        target: "evt",
                        "Motion: cleared mode transition for node '{}'",
                        node_id
                    );
                }
            }
            crate::commands::update_lights_on_cache_for_runtime_node(
                state, &runtime, node_id, true,
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                target: "evt",
                event = "motion_turn_on_failed",
                latency_ms = started.elapsed().as_millis(),
                error = %e,
                "Motion: turn_on node '{}' failed",
                node_id
            );
            false
        }
    }
}

fn run_button_ingress_action(
    state: &SharedState,
    node_id: &str,
    action: ButtonAction,
    device_id: Option<&str>,
    command_id: &str,
    persist_after: bool,
) {
    // Physical button ingress is fast lane: execute inline on the ingress
    // dispatch thread and never enqueue behind paced HTTP/system batches.
    if process_button_inline(state, node_id, action, device_id, command_id) && persist_after {
        {
            let runtime = {
                let Ok(s) = state.lock() else { return };
                s.hub_runtime()
            };
            if let Some(runtime) = runtime {
                crate::commands::emit_node_state_event_after_apply(state, &runtime, node_id);
            }
        }

        let work_tx = {
            let Ok(s) = state.lock() else { return };
            s.work_tx.clone()
        };
        if let Some(tx) = work_tx {
            let _ = tx.try_send(WorkItem::DeferredPersist {
                node_id: node_id.to_string(),
            });
        } else {
            commands::persist_rooms(state);
        }
    }
}

fn spawn_button_ingress_action(
    state: &SharedState,
    node_id: String,
    action: ButtonAction,
    device_id: Option<String>,
    command_id: String,
    persist_after: bool,
) {
    let builder = std::thread::Builder::new().name("evt-button".to_string());

    let state_clone = state.clone();
    let node_id_clone = node_id.clone();
    let device_id_clone = device_id.clone();
    let command_id_clone = command_id.clone();

    if let Err(error) = builder.spawn(move || {
        run_button_ingress_action(
            &state_clone,
            &node_id_clone,
            action,
            device_id_clone.as_deref(),
            &command_id_clone,
            persist_after,
        );
    }) {
        warn!(
            target: "evt",
            "Failed to spawn button ingress dispatch thread: {}",
            error
        );
        run_button_ingress_action(
            state,
            &node_id,
            action,
            device_id.as_deref(),
            &command_id,
            persist_after,
        );
    }
}

fn run_input_binding_action(
    state: &SharedState,
    source_node_id: &str,
    action: crate::topology::AutomationAction,
    device_id: Option<&str>,
    command_id: &str,
) {
    let started = Instant::now();
    match crate::commands::do_execute_automation_action(state, &action) {
        Ok(()) => {
            tracing::info!(
                target: "evt",
                event = "input_binding_action_applied",
                command_id = %command_id,
                source_node_id = %source_node_id,
                device_id = ?device_id,
                action = ?action,
                latency_ms = started.elapsed().as_millis(),
                "Input binding action applied"
            );
        }
        Err(error) => {
            tracing::warn!(
                target: "evt",
                event = "input_binding_action_failed",
                command_id = %command_id,
                source_node_id = %source_node_id,
                device_id = ?device_id,
                action = ?action,
                latency_ms = started.elapsed().as_millis(),
                error = %error,
                "Input binding action failed"
            );
        }
    }
}

fn spawn_input_binding_action(
    state: &SharedState,
    source_node_id: String,
    action: crate::topology::AutomationAction,
    device_id: Option<String>,
    command_id: String,
) {
    let builder = std::thread::Builder::new().name("evt-input-binding".to_string());

    let state_clone = state.clone();
    let source_node_id_clone = source_node_id.clone();
    let action_clone = action.clone();
    let device_id_clone = device_id.clone();
    let command_id_clone = command_id.clone();

    if let Err(error) = builder.spawn(move || {
        run_input_binding_action(
            &state_clone,
            &source_node_id_clone,
            action_clone,
            device_id_clone.as_deref(),
            &command_id_clone,
        );
    }) {
        warn!(
            target: "evt",
            "Failed to spawn input binding dispatch thread: {}",
            error
        );
        run_input_binding_action(
            state,
            &source_node_id,
            action,
            device_id.as_deref(),
            &command_id,
        );
    }
}

fn run_motion_turn_on_action(state: &SharedState, node_id: &str) {
    // Motion ingress is fast lane for occupancy responsiveness.
    if !turn_on_node_inline(state, node_id) {
        return;
    }
    let runtime = {
        let Ok(s) = state.lock() else { return };
        s.hub_runtime()
    };
    if let Some(runtime) = runtime {
        crate::commands::emit_node_state_event_after_apply(state, &runtime, node_id);
    }
}

fn spawn_motion_turn_on_action(state: &SharedState, node_id: String) {
    let builder = std::thread::Builder::new().name("evt-motion".to_string());

    let state_clone = state.clone();
    let node_id_clone = node_id.clone();

    if let Err(error) = builder.spawn(move || {
        run_motion_turn_on_action(&state_clone, &node_id_clone);
    }) {
        warn!(
            target: "evt",
            "Failed to spawn motion dispatch thread: {}",
            error
        );
        run_motion_turn_on_action(state, &node_id);
    }
}

fn spawn_motion_dim_action(state: &SharedState, node_id: String, factor: f32) {
    let builder = std::thread::Builder::new().name("evt-motion-dim".to_string());
    let state_clone = state.clone();
    let node_id_clone = node_id.clone();

    if let Err(error) = builder.spawn(move || {
        dim_node_inline(&state_clone, &node_id_clone, factor);
    }) {
        warn!(
            target: "evt",
            "Failed to spawn motion dim dispatch thread for node '{}': {}",
            node_id,
            error
        );
    }
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
                    debug!(
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
                    if let Ok(mut s) = sync_state.lock() {
                        s.clear_hub_reconnect_sync(&sync_hub_key);
                    }
                    warn!(target: "conn", "Hub {} resync failed: {}", sync_hub_key, e);
                }
            }
        });

    if let Err(e) = spawn_result {
        if let Ok(mut s) = state.lock() {
            s.clear_hub_reconnect_sync(hub_key);
        }
        warn!(
            target: "conn",
            "Failed to spawn reconnect sync thread for {}: {}",
            hub_key,
            e
        );
    }
}

fn spawn_light_state_poll(state: &SharedState, hub_key: &crate::canonical::identity::HubKey) {
    let poll_state = state.clone();
    let poll_hub_key = hub_key.clone();
    let thread_name = format!("hub-light-poll-{}", poll_hub_key.hub_type.as_str());

    let spawn_result = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || crate::room_sync::poll_initial_light_state(&poll_state));

    if let Err(e) = spawn_result {
        warn!(
            target: "conn",
            "Failed to spawn reconnect light-state poll for {}: {}",
            hub_key,
            e
        );
    }
}

fn emit_hub_status(
    state: &SharedState,
    hub_key: Option<&crate::canonical::identity::HubKey>,
    connected: bool,
) {
    crate::state::emit_server_event(
        state,
        crate::server_event::ServerEvent::HubStatus {
            hub_type: hub_key.map(|k| k.hub_type.as_str().to_string()),
            address: hub_key.map(|k| k.address.clone()),
            connected,
        },
    );
}

fn notify_hub_disconnect(state: &SharedState) {
    let on_disconnect = {
        let Ok(s) = state.lock() else { return };
        s.on_hub_disconnect.clone()
    };
    if let Some(cb) = on_disconnect {
        cb();
    }
}

fn spawn_pending_disconnect(
    state: &SharedState,
    hub_key: &crate::canonical::identity::HubKey,
    deadline: Instant,
    reason: String,
) {
    let disconnect_state = state.clone();
    let disconnect_hub_key = hub_key.clone();
    let thread_name = format!(
        "hub-disconnect-grace-{}",
        disconnect_hub_key.hub_type.as_str()
    );

    let spawn_result = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }

            let should_emit = {
                let Ok(mut s) = disconnect_state.lock() else {
                    return;
                };
                if !s.hub_pending_disconnect_matches(&disconnect_hub_key, deadline) {
                    return;
                }
                s.clear_hub_pending_disconnect(&disconnect_hub_key);
                if !s.hub_is_connected(&disconnect_hub_key) {
                    return;
                }
                s.set_hub_connected(&disconnect_hub_key, false);
                true
            };

            if should_emit {
                warn!(
                    target: "conn",
                    "Hub {} still disconnected after {}s grace: {}",
                    disconnect_hub_key,
                    HUB_DISCONNECT_GRACE.as_secs(),
                    reason
                );
                emit_hub_status(&disconnect_state, Some(&disconnect_hub_key), false);
                notify_hub_disconnect(&disconnect_state);
            }
        });

    if let Err(e) = spawn_result {
        warn!(
            target: "conn",
            "Failed to spawn disconnect grace thread for {}: {}",
            hub_key,
            e
        );
        let should_emit = {
            let Ok(mut s) = state.lock() else { return };
            s.clear_hub_pending_disconnect(hub_key);
            if !s.hub_is_connected(hub_key) {
                return;
            }
            s.set_hub_connected(hub_key, false);
            true
        };
        if should_emit {
            emit_hub_status(state, Some(hub_key), false);
            notify_hub_disconnect(state);
        }
    }
}

fn should_run_reconnect_resync(
    state: &SharedState,
    hub_key: &crate::canonical::identity::HubKey,
) -> bool {
    let Ok(mut s) = state.lock() else {
        return true;
    };

    if s.reconnect_sync_recently_ran(hub_key, reconnect_resync_cooldown(hub_key)) {
        return false;
    }

    s.note_hub_reconnect_sync(hub_key);
    true
}

fn reconnect_resync_cooldown(hub_key: &crate::canonical::identity::HubKey) -> Duration {
    if hub_key.hub_type.as_str() == crate::hub::HubType::HUE {
        HUE_RECONNECT_RESYNC_COOLDOWN
    } else {
        DEFAULT_RECONNECT_RESYNC_COOLDOWN
    }
}

/// Handle a hub-agnostic event from the event stream.
///
/// Routes events through `RuntimeHandle` regardless of which hub produced them.
/// Button/motion events are dispatched onto short-lived worker threads so a
/// slow light command cannot block the shared hub-event ingress loop.
pub fn handle_hub_event(state: &SharedState, event: HubEvent, motion: &mut MotionTimerState) {
    let hub_key = event.hub_key().cloned();

    match event {
        HubEvent::Connected { .. } => {
            debug!(target: "conn", "Hub connected");
            let (app_became_connected, first_connected_event, reconnect_after_gap) =
                if let Some(ref key) = hub_key {
                    if let Ok(mut s) = state.lock() {
                        let was_connected = s.hub_is_connected(key);
                        let pending_disconnect = s.clear_hub_pending_disconnect(key);
                        let first_connected_event = s.note_hub_connected_event(key);
                        s.set_hub_connected(key, true);
                        (
                            !was_connected,
                            first_connected_event,
                            !first_connected_event && (!was_connected || pending_disconnect),
                        )
                    } else {
                        (false, false, false)
                    }
                } else {
                    (true, false, false)
                };

            if first_connected_event {
                if let Some(ref key) = hub_key {
                    debug!(
                        target: "conn",
                        "Skipping reconnect resync for initial connect of {}",
                        key
                    );
                }
            } else if reconnect_after_gap {
                if let Some(ref key) = hub_key {
                    if should_run_reconnect_resync(state, key) {
                        spawn_reconnect_sync(state, key);
                    } else {
                        debug!(
                            target: "conn",
                            "Hub {} reconnected within {}s; skipping full resync",
                            key,
                            reconnect_resync_cooldown(key).as_secs()
                        );
                        spawn_light_state_poll(state, key);
                    }
                }
            }

            if app_became_connected {
                emit_hub_status(state, hub_key.as_ref(), true);
            }
        }

        HubEvent::Button {
            ref hub_key,
            ref room_id,
            action,
            ref device_id,
        } => {
            let Some((key, native_device_id)) = hub_key.as_ref().zip(device_id.as_deref()) else {
                emit_button_input_event(
                    state,
                    hub_key.as_ref(),
                    None,
                    None,
                    Some(room_id.as_str()),
                    device_id.as_deref(),
                    None,
                    Some(action),
                    InputEventRoute::Unresolved,
                );
                info!(
                    target: "evt",
                    "Button event from {:?} has no hub key/native device id, ignoring",
                    device_id.as_deref()
                );
                return;
            };
            let Some(source_node_id) =
                commands::resolve_input_source_node_id(state, key, native_device_id)
            else {
                emit_button_input_event(
                    state,
                    hub_key.as_ref(),
                    None,
                    None,
                    Some(room_id.as_str()),
                    Some(native_device_id),
                    None,
                    Some(action),
                    InputEventRoute::Unresolved,
                );
                info!(
                    target: "evt",
                    "Button event from {:?} could not resolve canonical source node, ignoring",
                    device_id.as_deref()
                );
                return;
            };
            let command_id = logging::next_command_id("button");

            if let Some(binding_action) =
                commands::matching_button_input_binding_action(state, &source_node_id, action)
            {
                emit_button_input_event(
                    state,
                    hub_key.as_ref(),
                    Some(source_node_id.as_str()),
                    None,
                    Some(room_id.as_str()),
                    Some(native_device_id),
                    None,
                    Some(action),
                    InputEventRoute::InputBinding,
                );
                tracing::info!(
                    target: "evt",
                    event = "button_binding_ingress",
                    command_id = %command_id,
                    action = ?action,
                    source_node_id = %source_node_id,
                    source_room_id = %room_id,
                    device_id = ?device_id.as_deref(),
                    "Button event matched input binding"
                );
                if !motion.admit_button(&source_node_id, action, device_id.as_deref()) {
                    tracing::info!(
                        target: "evt",
                        event = "button_binding_debounced",
                        command_id = %command_id,
                        action = ?action,
                        source_node_id = %source_node_id,
                        device_id = ?device_id.as_deref(),
                        debounce_ms = BUTTON_DEBOUNCE_WINDOW.as_millis() as u64,
                        "Dropping bounce-duplicate input binding event"
                    );
                    return;
                }
                spawn_input_binding_action(
                    state,
                    source_node_id,
                    binding_action,
                    device_id.clone(),
                    command_id,
                );
                return;
            }

            let Some(node_id) = commands::resolve_node_control_target_for_source(
                state,
                &source_node_id,
                &crate::topology::NodeControlKind::Button,
            ) else {
                emit_button_input_event(
                    state,
                    hub_key.as_ref(),
                    Some(source_node_id.as_str()),
                    None,
                    Some(room_id.as_str()),
                    Some(native_device_id),
                    None,
                    Some(action),
                    InputEventRoute::Unroutable,
                );
                info!(
                    target: "evt",
                    "Button event from {:?} could not resolve canonical topology target, ignoring",
                    device_id.as_deref()
                );
                return;
            };
            emit_button_input_event(
                state,
                hub_key.as_ref(),
                Some(source_node_id.as_str()),
                Some(node_id.as_str()),
                Some(room_id.as_str()),
                Some(native_device_id),
                None,
                Some(action),
                InputEventRoute::NodeControl,
            );
            tracing::info!(
                target: "evt",
                event = "button_ingress",
                command_id = %command_id,
                action = ?action,
                node_id = %node_id,
                source_node_id = %source_node_id,
                source_room_id = %room_id,
                device_id = ?device_id.as_deref(),
                hub_present = hub_key.is_some(),
                "Button event received"
            );
            if !motion.admit_button(&node_id, action, device_id.as_deref()) {
                tracing::info!(
                    target: "evt",
                    event = "button_debounced",
                    command_id = %command_id,
                    action = ?action,
                    node_id = %node_id,
                    device_id = ?device_id.as_deref(),
                    debounce_ms = BUTTON_DEBOUNCE_WINDOW.as_millis() as u64,
                    "Dropping bounce-duplicate button event"
                );
                return;
            }
            motion.motion_owned.remove(&node_id);
            motion.warning_active.remove(&node_id);
            motion
                .sensors
                .retain(|_, source| source.target_node_id != node_id);

            spawn_button_ingress_action(
                state,
                node_id,
                action,
                device_id.clone(),
                command_id,
                true,
            );
        }

        HubEvent::Motion {
            ref hub_key,
            ref room_id,
            ref sensor_id,
            detected,
        } => {
            let Some(hub_key) = hub_key.as_ref() else {
                emit_motion_input_event(
                    state,
                    MotionInputEventFields {
                        hub_key: None,
                        source_node_id: None,
                        target_node_id: None,
                        source_room_id: Some(room_id.as_str()),
                        native_sensor_id: sensor_id,
                        detected,
                        route: InputEventRoute::Unresolved,
                    },
                );
                info!(
                    target: "evt",
                    "Motion: sensor {} has no hub key, ignoring",
                    sensor_id
                );
                return;
            };
            let Some(source_node_id) =
                commands::resolve_input_source_node_id(state, hub_key, sensor_id)
            else {
                emit_motion_input_event(
                    state,
                    MotionInputEventFields {
                        hub_key: Some(hub_key),
                        source_node_id: None,
                        target_node_id: None,
                        source_room_id: Some(room_id.as_str()),
                        native_sensor_id: sensor_id,
                        detected,
                        route: InputEventRoute::Unresolved,
                    },
                );
                info!(
                    target: "evt",
                    "Motion: sensor {} could not resolve canonical source node, ignoring",
                    sensor_id
                );
                return;
            };
            let Some(target_node_id) = commands::resolve_node_control_target_for_source(
                state,
                &source_node_id,
                &NodeControlKind::Motion,
            ) else {
                emit_motion_input_event(
                    state,
                    MotionInputEventFields {
                        hub_key: Some(hub_key),
                        source_node_id: Some(source_node_id.as_str()),
                        target_node_id: None,
                        source_room_id: Some(room_id.as_str()),
                        native_sensor_id: sensor_id,
                        detected,
                        route: InputEventRoute::Unroutable,
                    },
                );
                info!(
                    target: "evt",
                    "Motion: sensor {} has no control target, ignoring",
                    sensor_id
                );
                return;
            };
            emit_motion_input_event(
                state,
                MotionInputEventFields {
                    hub_key: Some(hub_key),
                    source_node_id: Some(source_node_id.as_str()),
                    target_node_id: Some(target_node_id.as_str()),
                    source_room_id: Some(room_id.as_str()),
                    native_sensor_id: sensor_id,
                    detected,
                    route: InputEventRoute::NodeControl,
                },
            );
            if detected {
                if motion.warning_active.remove(&target_node_id) {
                    info!(
                        target: "evt",
                        "Motion: restoring full brightness in node {} (was warning-dimmed)",
                        target_node_id
                    );
                    spawn_motion_dim_action(state, target_node_id.clone(), 1.0);
                }

                let is_new_target = !motion.has_sources_for_target(&target_node_id);
                motion.sensors.insert(
                    source_node_id.clone(),
                    MotionSourceState {
                        source_node_id: source_node_id.clone(),
                        target_node_id: target_node_id.clone(),
                        stopped_at: None,
                        stopped_at_epoch_ms: None,
                    },
                );

                if is_new_target {
                    motion.motion_owned.insert(target_node_id.clone());

                    info!(
                        target: "evt",
                        "Motion: new activation source {} -> target {} (owned=true)",
                        source_node_id,
                        target_node_id
                    );

                    spawn_motion_turn_on_action(state, target_node_id.clone());
                } else {
                    info!(
                        target: "evt",
                        "Motion: continued/refreshed source {} -> target {}",
                        source_node_id,
                        target_node_id
                    );
                }
            } else if let Some(source) = motion.sensors.get_mut(&source_node_id) {
                source.target_node_id = target_node_id.clone();
                source.stopped_at = Some(Instant::now());
                source.stopped_at_epoch_ms = Some(current_epoch_ms());

                let all_cleared = motion
                    .sensors
                    .values()
                    .filter(|source| source.target_node_id == target_node_id)
                    .all(|source| source.stopped_at.is_some());
                info!(
                    target: "evt",
                    "Motion: source {} stopped on target {} - all_cleared={}",
                    source_node_id,
                    target_node_id,
                    all_cleared
                );
            } else {
                info!(
                    target: "evt",
                    "Motion: detected=false for source {} target {} but not tracked, ignoring",
                    source_node_id,
                    target_node_id
                );
            }
        }

        HubEvent::LightPower {
            ref hub_key,
            ref device_id,
            lights_on,
        } => {
            let Some(hub_key) = hub_key.as_ref() else {
                info!(
                    target: "evt",
                    "Light power report for {} has no hub key, ignoring",
                    device_id
                );
                return;
            };
            let runtime = {
                let Ok(s) = state.lock() else { return };
                s.hub_runtime()
            };
            let Some(runtime) = runtime else {
                return;
            };
            let Some(node_id) = commands::update_lights_on_cache_for_native_light_report(
                state,
                &runtime,
                hub_key,
                device_id,
                lights_on,
                crate::state::ObservedPowerSource::LiveSubscription,
            ) else {
                info!(
                    target: "evt",
                    "Light power report from {:?}:{} could not resolve canonical topology target, ignoring",
                    hub_key,
                    device_id
                );
                return;
            };
            info!(
                target: "evt",
                "Light power report from {:?}:{} resolved to {} lights_on={}",
                hub_key,
                device_id,
                node_id,
                lights_on
            );
            commands::emit_node_state_event_after_apply(state, &runtime, &node_id);
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

            if let Some(ref key) = hub_key {
                let pending_deadline = {
                    let Ok(mut s) = state.lock() else { return };
                    if s.hub_is_connected(key) && s.hub_seen_connected_once(key) {
                        Some(s.note_hub_pending_disconnect(key, HUB_DISCONNECT_GRACE))
                    } else if s.hub_is_connected(key) {
                        s.set_hub_connected(key, false);
                        None
                    } else {
                        None
                    }
                };

                if let Some(deadline) = pending_deadline {
                    info!(
                        target: "conn",
                        "Hub {} disconnect pending for {}s before app-visible unavailable",
                        key,
                        HUB_DISCONNECT_GRACE.as_secs()
                    );
                    spawn_pending_disconnect(state, key, deadline, reason);
                } else {
                    emit_hub_status(state, hub_key.as_ref(), false);
                    notify_hub_disconnect(state);
                }
            } else {
                emit_hub_status(state, None, false);
                notify_hub_disconnect(state);
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

        HubEvent::UnroutableButton {
            ref hub_key,
            ref device_id,
            ref button_id,
        } => {
            emit_button_input_event(
                state,
                hub_key.as_ref(),
                None,
                None,
                None,
                device_id.as_deref(),
                Some(button_id.as_str()),
                None,
                InputEventRoute::Unroutable,
            );
            // Try to create an UnassignedDevice triage entry so the user knows
            // a switch needs attention. Requires the device to be in the
            // canonical registry (populated during room sync).
            let canonical_id_to_queue = if let (Some(key), Some(dev_id)) = (hub_key, device_id) {
                let Ok(s) = state.lock() else { return };
                s.canonical_registry
                    .find_by_native_id(key, dev_id)
                    .filter(|cd| !s.canonical_registry.triage().has_unassigned_device(&cd.id))
                    .map(|cd| cd.id.clone())
            } else {
                warn!(target: "evt",
                    "Unroutable button {} with no hub_key or device_id — \
                     will surface at next sync", button_id);
                None
            };

            if let Some(cid) = canonical_id_to_queue {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if let Ok(mut s) = state.lock() {
                    s.canonical_registry.queue_unassigned(&cid, now);
                    info!(target: "evt",
                        "Created UnassignedDevice triage entry for button {} (canonical={})",
                        button_id, cid);
                }
                commands::emit_triage_changed(state);
            }
        }
    }
}

/// Check motion timers and turn off lights in expired motion-owned targets.
///
/// Groups sources by target node. For each target, countdown only starts when
/// ALL sources have cleared. Uses the latest source stop time as the countdown
/// start. Per-target timeout is resolved from the target node's effective
/// profile settings. Timeout=0 disables auto-off.
pub fn check_motion_timers(state: &SharedState, motion: &mut MotionTimerState) {
    let now = Instant::now();

    let (timeouts, _) = commands::resolved_motion_timeout_map(state);
    let default_timeout_secs = state
        .lock()
        .ok()
        .map(|s| s.default_motion_timeout_secs)
        .unwrap_or(0);

    let mut targets: HashMap<String, Vec<(&String, &MotionSourceState)>> = HashMap::new();
    for (source_key, source) in &motion.sensors {
        targets
            .entry(source.target_node_id.clone())
            .or_default()
            .push((source_key, source));
    }

    let status: Vec<String> = targets
        .iter()
        .map(|(target_node_id, sources)| {
            let timeout_secs = timeouts
                .get(target_node_id)
                .copied()
                .unwrap_or(default_timeout_secs);
            let source_status: Vec<String> = sources
                .iter()
                .map(|(_, source)| match source.stopped_at {
                    None => format!("{}=active", source.source_node_id),
                    Some(t) => format!(
                        "{}={}s/{}s",
                        source.source_node_id,
                        now.duration_since(t).as_secs(),
                        timeout_secs
                    ),
                })
                .collect();
            format!("{}:[{}]", target_node_id, source_status.join(","))
        })
        .collect();
    info!(
        target: "evt",
        "Motion: checking timers - {} targets: {}",
        targets.len(),
        status.join(" ")
    );

    let mut expired_targets: Vec<String> = Vec::new();

    for (target_node_id, sources) in &targets {
        if sources
            .iter()
            .any(|(_, source)| source.stopped_at.is_none())
        {
            continue;
        }

        let Some(elapsed) = sources
            .iter()
            .filter_map(|(_, source)| source.stopped_at.as_ref())
            .map(|t| now.duration_since(*t))
            .min()
        else {
            continue;
        };

        let timeout_secs = timeouts
            .get(target_node_id)
            .copied()
            .unwrap_or(default_timeout_secs);

        if timeout_secs == 0 {
            continue;
        }

        let timeout = Duration::from_secs(timeout_secs);
        if elapsed > timeout {
            expired_targets.push(target_node_id.clone());
        } else {
            let remaining = timeout_secs.saturating_sub(elapsed.as_secs());
            if remaining <= WARNING_BEFORE_SECS
                && timeout_secs > WARNING_BEFORE_SECS
                && motion.motion_owned.contains(target_node_id.as_str())
                && !motion.warning_active.contains(target_node_id.as_str())
            {
                info!(
                    target: "evt",
                    "Motion: warning dim for node {} ({}s remaining)",
                    target_node_id,
                    remaining
                );
                spawn_motion_dim_action(state, target_node_id.clone(), WARNING_DIM_FACTOR);
                motion.warning_active.insert(target_node_id.clone());
            }
        }
    }

    for target_node_id in &expired_targets {
        motion
            .sensors
            .retain(|_, source| &source.target_node_id != target_node_id);
        motion.warning_active.remove(target_node_id);

        if motion.motion_owned.remove(target_node_id) {
            info!(
                target: "evt",
                "Motion: timeout expired for node {} - sending OffPress",
                target_node_id
            );

            let work_tx = {
                let Ok(s) = state.lock() else { continue };
                s.work_tx.clone()
            };
            let command_id = logging::next_command_id("motion-timeout");
            if work_tx.is_some() {
                spawn_button_ingress_action(
                    state,
                    target_node_id.clone(),
                    ButtonAction::OffPress,
                    None,
                    command_id,
                    false,
                );
            } else {
                let _ = process_button_inline(
                    state,
                    target_node_id,
                    ButtonAction::OffPress,
                    None,
                    &command_id,
                );
            }
        } else {
            info!(
                target: "evt",
                "Motion: timeout expired for node {} - not owned, skipping off",
                target_node_id
            );
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
    let mut motion_persistence = MotionTimerPersistence::new();
    let mut last_motion_check = Instant::now();
    let mut motion_dirty = false;

    loop {
        let mut processed_hub_event = false;
        let mut force_motion_persist = false;
        let mut motion_persist_acks = Vec::new();
        let mut pending_motion_clear = Vec::new();

        // Pick up new hub event receivers from reconfiguration
        if let Ok(mut s) = state.lock() {
            if !s.pending_hub_event_rxs.is_empty() {
                let new_rxs = std::mem::take(&mut s.pending_hub_event_rxs);
                hub_event_rxs.extend(new_rxs);
                motion_state = MotionTimerState::new();
                last_motion_check = Instant::now();
                motion_dirty = true;
                motion_persistence.mark_dirty();
                force_motion_persist = true;
            }

            if !s.pending_motion_clear.is_empty() {
                pending_motion_clear = std::mem::take(&mut s.pending_motion_clear);
            }

            if !s.pending_motion_timer_persist_acks.is_empty() {
                motion_persist_acks = std::mem::take(&mut s.pending_motion_timer_persist_acks);
            }
        }

        for target_node_id in &pending_motion_clear {
            motion_state
                .sensors
                .retain(|_, source| &source.target_node_id != target_node_id);
            motion_state.motion_owned.remove(target_node_id);
            motion_state.warning_active.remove(target_node_id);
        }
        if !pending_motion_clear.is_empty() {
            info!(
                target: "evt",
                "Motion: cleared timers for {} targets",
                pending_motion_clear.len()
            );
            motion_dirty = true;
            motion_persistence.mark_dirty();
            force_motion_persist = true;
        }

        // Apply any motion seeds queued by startup prefetch. Hub bootstrap
        // populates the queue asynchronously, so this must be inside the
        // loop — checking once before the loop drops every seed.
        if apply_pending_motion_seeds(&state, &mut motion_state, Instant::now()) {
            motion_dirty = true;
            motion_persistence.mark_dirty();
            force_motion_persist = true;
        }

        // Process hub events from all receivers, removing disconnected ones
        hub_event_rxs.retain(|rx| loop {
            match rx.try_recv() {
                Ok(event) => {
                    processed_hub_event = true;
                    handle_hub_event(&state, event, &mut motion_state);
                    motion_dirty = true;
                    motion_persistence.mark_dirty();
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

        // Check motion timers periodically without tying the cadence to the
        // event-loop sleep interval.
        if last_motion_check.elapsed() >= MOTION_TIMER_CHECK_INTERVAL {
            last_motion_check = Instant::now();
            if !motion_state.sensors.is_empty() {
                check_motion_timers(&state, &mut motion_state);
                motion_dirty = true;
                motion_persistence.mark_dirty();
                force_motion_persist = true;
            }
        }

        // Sync motion snapshots when dirty
        if motion_dirty {
            motion_dirty = false;
            sync_motion_snapshots(&state, &motion_state);
        }
        if !motion_persist_acks.is_empty() {
            motion_persistence.mark_dirty();
            motion_persistence.persist_now(&state, &motion_state);
            for ack in motion_persist_acks {
                let _ = ack.send(());
            }
        } else if force_motion_persist {
            motion_persistence.persist_now(&state, &motion_state);
        } else {
            motion_persistence.persist_if_due(&state, &motion_state);
        }

        if processed_hub_event {
            std::thread::yield_now();
        } else {
            std::thread::sleep(EVENT_LOOP_IDLE_SLEEP);
        }
    }
}

/// Ask the event loop to persist its owned motion timer map before a planned
/// restart. Returns `true` if there was nothing to flush or the event loop
/// acknowledged the request before `timeout`.
pub fn request_motion_timer_persist(state: &SharedState, timeout: Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    {
        let Ok(mut s) = state.lock() else {
            return false;
        };
        if s.motion_snapshots.is_empty() && s.motion_timer_restores.is_empty() {
            return true;
        }
        s.pending_motion_timer_persist_acks.push(tx);
    }

    rx.recv_timeout(timeout).is_ok()
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

/// Drain `pending_motion_seed` into the live motion state.
///
/// Called from inside the event loop because hub bootstrap finishes
/// asynchronously — the seed queue can be empty when the loop starts and
/// populated seconds later. Returns `true` if any seed was applied.
///
/// Active sensors land with `stopped_at = None` so the room stays on while
/// occupied. Inactive sensors wait for observed power before landing with
/// `stopped_at = Some(now)`: claiming ownership before the initial light poll
/// can misclassify lit rooms as idle. Seeds for sources that already received
/// live events are drained without overwriting the live event-stream state.
pub fn apply_pending_motion_seeds(
    state: &SharedState,
    motion: &mut MotionTimerState,
    now: Instant,
) -> bool {
    let seeds: Vec<MotionSeedEntry> = match state.lock() {
        Ok(mut s) if !s.pending_motion_seed.is_empty() => {
            std::mem::take(&mut s.pending_motion_seed)
        }
        _ => return false,
    };

    let mut active_count = 0usize;
    let mut owned_inactive_count = 0usize;
    let mut idle_count = 0usize;
    let mut live_count = 0usize;
    let mut hard_off_dropped = 0usize;
    let mut soft_off_dropped = 0usize;
    let mut deferred = Vec::new();

    for seed in seeds {
        let MotionSeedEntry {
            source_node_id,
            target_node_id,
            is_active,
            stopped_at_epoch_ms,
            motion_owned,
            warning_active,
        } = seed;
        let restore_too_stale = stopped_at_epoch_ms.is_some_and(|epoch_ms| {
            restored_motion_timer_too_stale(state, &target_node_id, epoch_ms)
        });
        let stopped_at_epoch_ms = if restore_too_stale {
            None
        } else {
            stopped_at_epoch_ms
        };
        let motion_owned = if restore_too_stale {
            None
        } else {
            motion_owned
        };
        let warning_active = warning_active && !restore_too_stale && !is_active;

        if motion.sensors.contains_key(&source_node_id) {
            live_count += 1;
            continue;
        }

        if let Some((hard_off, soft_off)) = target_room_flags(state, &target_node_id) {
            // Hard-off rooms have no motion timers — live mutations clear them
            // via queue_motion_timer_clear, so boot-time seeding must do the
            // same or timers reappear after a power cycle (issue #53).
            if hard_off {
                hard_off_dropped += 1;
                continue;
            }

            // Idle/soft-off is also an explicit persisted room state. For
            // inactive startup-prefetch sensors, the room's semantic
            // lights_on=true should not be mistaken for motion-owned light.
            if soft_off && !is_active {
                soft_off_dropped += 1;
                continue;
            }
        }

        let observed_lights_on = if is_active || motion_owned.is_some() {
            None
        } else {
            match observed_room_lights_on(state, &target_node_id) {
                Some(on) => Some(on),
                None => {
                    deferred.push(MotionSeedEntry {
                        source_node_id,
                        target_node_id,
                        is_active,
                        stopped_at_epoch_ms,
                        motion_owned,
                        warning_active,
                    });
                    continue;
                }
            }
        };

        let (stopped_at, stopped_at_epoch_ms) = if is_active {
            (None, None)
        } else if let Some(epoch_ms) = stopped_at_epoch_ms {
            (Some(instant_from_epoch_ms(now, epoch_ms)), Some(epoch_ms))
        } else {
            (Some(now), Some(current_epoch_ms()))
        };
        let should_own = if is_active {
            true
        } else {
            motion_owned.unwrap_or_else(|| observed_lights_on.unwrap_or(false))
        };
        motion.sensors.insert(
            source_node_id.clone(),
            MotionSourceState {
                source_node_id,
                target_node_id: target_node_id.clone(),
                stopped_at,
                stopped_at_epoch_ms,
            },
        );
        if should_own {
            motion.motion_owned.insert(target_node_id.clone());
        }
        if warning_active && should_own {
            motion.warning_active.insert(target_node_id.clone());
        }
        if is_active {
            active_count += 1;
        } else if should_own {
            owned_inactive_count += 1;
        } else {
            idle_count += 1;
        }
    }

    if !deferred.is_empty() {
        if let Ok(mut s) = state.lock() {
            s.pending_motion_seed.extend(deferred);
        }
    }

    let applied_count = active_count + owned_inactive_count + idle_count;
    if applied_count > 0 || live_count > 0 || hard_off_dropped > 0 || soft_off_dropped > 0 {
        info!(
            target: "evt",
            "Motion: seeded {} active, {} cleared-but-owned, {} idle from startup prefetch ({} live sources kept, {} hard_off dropped, {} soft_off dropped)",
            active_count, owned_inactive_count, idle_count, live_count, hard_off_dropped, soft_off_dropped
        );
    }
    applied_count > 0
}

fn instant_from_epoch_ms(now: Instant, epoch_ms: u64) -> Instant {
    let age_ms = current_epoch_ms().saturating_sub(epoch_ms);
    now.checked_sub(Duration::from_millis(age_ms))
        .unwrap_or(now)
}

fn restored_motion_timer_too_stale(
    state: &SharedState,
    target_node_id: &str,
    stopped_at_epoch_ms: u64,
) -> bool {
    let timeout_secs = motion_timeout_secs_for_target(state, target_node_id);
    if timeout_secs == 0 {
        return false;
    }

    let age_ms = current_epoch_ms().saturating_sub(stopped_at_epoch_ms);
    age_ms > timeout_secs.saturating_mul(2).saturating_mul(1_000)
}

fn motion_timeout_secs_for_target(state: &SharedState, target_node_id: &str) -> u64 {
    let (timeouts, _) = commands::resolved_motion_timeout_map(state);
    timeouts.get(target_node_id).copied().unwrap_or_else(|| {
        state
            .lock()
            .ok()
            .map(|s| s.default_motion_timeout_secs)
            .unwrap_or(0)
    })
}

fn observed_room_lights_on(state: &SharedState, target_node_id: &str) -> Option<bool> {
    state.lock().ok().and_then(|s| {
        s.room_observed_power
            .get(target_node_id)
            .map(|observed| observed.lights_on)
    })
}

fn target_room_flags(state: &SharedState, target_node_id: &str) -> Option<(bool, bool)> {
    let runtime = state.lock().ok().and_then(|s| s.hub_runtime())?;
    runtime
        .engine_effective_node_snapshot(target_node_id)
        .map(|snap| (snap.hard_off, snap.soft_off))
}

fn motion_timers_for_storage(motion: &MotionTimerState) -> StoredMotionTimers {
    let mut entries: Vec<_> = motion
        .sensors
        .values()
        .map(|source| StoredMotionTimerEntry {
            source_node_id: source.source_node_id.clone(),
            target_node_id: source.target_node_id.clone(),
            stopped_at_epoch_ms: stopped_at_epoch_ms_for_storage(source),
            motion_owned: motion.motion_owned.contains(&source.target_node_id),
            warning_active: motion.warning_active.contains(&source.target_node_id),
        })
        .collect();
    entries.sort_by(|a, b| a.source_node_id.cmp(&b.source_node_id));
    StoredMotionTimers {
        schema_version: 1,
        entries,
    }
}

fn stopped_at_epoch_ms_for_storage(source: &MotionSourceState) -> Option<u64> {
    source.stopped_at.map(|stopped_at| {
        source.stopped_at_epoch_ms.unwrap_or_else(|| {
            let elapsed_ms: u64 = Instant::now()
                .checked_duration_since(stopped_at)
                .unwrap_or(Duration::ZERO)
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);
            current_epoch_ms().saturating_sub(elapsed_ms)
        })
    })
}

#[derive(Default)]
struct MotionTimerPersistence {
    dirty: bool,
    last_attempt_at: Option<Instant>,
    last_saved: Option<StoredMotionTimers>,
}

impl MotionTimerPersistence {
    fn new() -> Self {
        Self::default()
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn persist_if_due(&mut self, state: &SharedState, motion: &MotionTimerState) {
        if !self.dirty {
            return;
        }
        if self
            .last_attempt_at
            .is_some_and(|last| last.elapsed() < MOTION_TIMER_CHECK_INTERVAL)
        {
            return;
        }
        self.persist_now(state, motion);
    }

    fn persist_now(&mut self, state: &SharedState, motion: &MotionTimerState) {
        if !self.dirty {
            return;
        }

        self.last_attempt_at = Some(Instant::now());
        let timers = motion_timers_for_storage(motion);
        if self.last_saved.as_ref() == Some(&timers) {
            self.dirty = false;
            return;
        }

        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        let Some(ref storage) = s.storage else {
            self.dirty = false;
            return;
        };
        match storage.save_motion_timers(&timers) {
            Ok(()) => {
                self.last_saved = Some(timers);
                self.dirty = false;
            }
            Err(e) => {
                warn!(target: "evt", "Failed to save motion timers: {}", e);
            }
        }
    }
}

/// Sync motion timer snapshots into AppState for API visibility.
pub fn sync_motion_snapshots(state: &SharedState, motion: &MotionTimerState) {
    let (timeouts, _) = commands::resolved_motion_timeout_map(state);
    let default_timeout_secs = state
        .lock()
        .ok()
        .map(|s| s.default_motion_timeout_secs)
        .unwrap_or(0);

    if let Ok(mut s) = state.lock() {
        if motion.sensors.is_empty() {
            if !s.motion_snapshots.is_empty() {
                s.motion_snapshots.clear();
                s.emit_event(crate::server_event::ServerEvent::MotionTimer { timers: vec![] });
            }
        } else {
            s.motion_snapshots = motion.snapshots(&timeouts, default_timeout_secs);
            {
                let timers: Vec<_> = s
                    .motion_snapshots
                    .iter()
                    .map(|(node_id, snap)| {
                        crate::server_event::MotionTimerEvent::from_snapshot(node_id, snap)
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
        WorkItem::QueuedNodeAction {
            command_id,
            node_id,
            action,
            device_id,
            dispatch_spacing,
            persist_after,
        } => {
            let started = Instant::now();
            let (runtime, has_hub) = {
                let Ok(s) = state.lock() else {
                    tracing::warn!(
                        target: "evt",
                        event = "queued_node_action_dropped",
                        command_id = %command_id,
                        action = ?action,
                        node_id = %node_id,
                        device_id = ?device_id.as_deref(),
                        source = "worker",
                        reason = "state_lock_poisoned",
                        "Worker queued node action dropped"
                    );
                    return;
                };
                (s.hub_runtime(), s.has_any_hub())
            };
            let Some(runtime) = runtime else {
                tracing::warn!(
                    target: "evt",
                    event = "queued_node_action_dropped",
                    command_id = %command_id,
                    action = ?action,
                    node_id = %node_id,
                    device_id = ?device_id.as_deref(),
                    source = "worker",
                    has_hub,
                    reason = "no_runtime",
                    "Worker queued node action dropped"
                );
                return;
            };

            let event = if let Some(ref dev_id) = device_id {
                InputEvent::with_device(&node_id, action, dev_id)
            } else {
                InputEvent::new(&node_id, action)
            };

            crate::periodic::wait_for_interactive_node_dispatch_slot(
                state,
                &command_id,
                &node_id,
                dispatch_spacing,
            );
            match runtime.handle_event(&event) {
                Ok(turned_on) => {
                    tracing::info!(
                        target: "evt",
                        event = "queued_node_action_applied",
                        command_id = %command_id,
                        action = ?action,
                        node_id = %node_id,
                        device_id = ?device_id.as_deref(),
                        source = "worker",
                        turned_on,
                        latency_ms = started.elapsed().as_millis(),
                        "Worker queued node action applied"
                    );
                    crate::commands::sync_active_mode_from_runtime(state, &runtime);
                    if let Ok(mut s) = state.lock() {
                        if s.room_mode_transitions.remove(&node_id).is_some() {
                            info!(
                                target: "evt",
                                "Worker: cleared mode transition for node '{}'",
                                node_id
                            );
                        }
                    }
                    crate::commands::update_lights_on_cache_for_runtime_node(
                        state, &runtime, &node_id, turned_on,
                    );
                    crate::commands::emit_node_state_event_after_apply(state, &runtime, &node_id);
                    if persist_after {
                        crate::commands::persist_rooms(state);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        target: "evt",
                        event = "queued_node_action_failed",
                        command_id = %command_id,
                        action = ?action,
                        node_id = %node_id,
                        device_id = ?device_id.as_deref(),
                        source = "worker",
                        latency_ms = started.elapsed().as_millis(),
                        error = %e,
                        "Worker queued node action failed"
                    );
                }
            }
        }
        WorkItem::SetNodeBrightness {
            command_id,
            node_id,
            brightness,
            dispatch_spacing,
            persist_after,
        } => {
            crate::periodic::wait_for_interactive_node_dispatch_slot(
                state,
                &command_id,
                &node_id,
                dispatch_spacing,
            );
            if let Err(e) =
                crate::commands::do_set_node_brightness(state, &node_id, brightness, persist_after)
            {
                tracing::warn!(
                    target: "cmd",
                    event = "set_node_brightness_failed",
                    command_id = %command_id,
                    node_id = %node_id,
                    brightness,
                    source = "worker",
                    error = %e,
                    "Worker set_node_brightness failed"
                );
            }
        }
        WorkItem::SetNodePreferences {
            command_id,
            node_id,
            rhythm_enabled,
            disabled,
            target_state,
            room_profile,
            dispatch_spacing,
            persist_after,
        } => {
            crate::periodic::wait_for_interactive_node_dispatch_slot(
                state,
                &command_id,
                &node_id,
                dispatch_spacing,
            );
            if let Err(e) = crate::commands::do_node_preferences_set(
                state,
                &node_id,
                rhythm_enabled,
                disabled,
                target_state,
                room_profile.as_ref(),
                persist_after,
            ) {
                tracing::warn!(
                    target: "cmd",
                    event = "set_node_preferences_failed",
                    command_id = %command_id,
                    node_id = %node_id,
                    source = "worker",
                    error = %e,
                    "Worker set_node_preferences failed"
                );
            }
        }
        WorkItem::LightsOffRoom {
            command_id,
            node_id,
            transition_ms,
            dispatch_spacing,
            dispatch_generation,
        } => {
            let started = Instant::now();
            if !crate::periodic::light_dispatch_generation_current(state, dispatch_generation) {
                tracing::debug!(
                    target: "cmd",
                    event = "lights_off_room_skipped",
                    command_id = %command_id,
                    node_id = %node_id,
                    dispatch_generation,
                    reason = "stale_dispatch_generation",
                    "Skipping stale lights_off_room"
                );
                return;
            }
            let runtime = {
                let Ok(s) = state.lock() else { return };
                s.hub_runtime()
            };
            let Some(runtime) = runtime else { return };

            if !crate::periodic::wait_for_node_dispatch_slot_if_current(
                state,
                &command_id,
                &node_id,
                dispatch_spacing,
                dispatch_generation,
            ) {
                tracing::debug!(
                    target: "cmd",
                    event = "lights_off_room_skipped",
                    command_id = %command_id,
                    node_id = %node_id,
                    dispatch_generation,
                    reason = "stale_dispatch_generation_after_pace",
                    "Skipping stale lights_off_room"
                );
                return;
            }
            match runtime.lights_off_room(&node_id, transition_ms) {
                Ok(()) => {
                    tracing::info!(
                        target: "cmd",
                        event = "lights_off_room_applied",
                        command_id = %command_id,
                        node_id = %node_id,
                        transition_ms = ?transition_ms,
                        source = "worker",
                        latency_ms = started.elapsed().as_millis(),
                        "Worker lights_off_room applied"
                    );
                    crate::commands::update_lights_on_cache_for_runtime_node(
                        state, &runtime, &node_id, false,
                    );
                    crate::commands::emit_node_state_event_after_apply(state, &runtime, &node_id);
                }
                Err(e) => {
                    tracing::warn!(
                        target: "cmd",
                        event = "lights_off_room_failed",
                        command_id = %command_id,
                        node_id = %node_id,
                        transition_ms = ?transition_ms,
                        source = "worker",
                        latency_ms = started.elapsed().as_millis(),
                        error = %e,
                        "Worker lights_off_room failed"
                    );
                }
            }
        }
        WorkItem::ApplyNodeCommand {
            command_id,
            node_id,
            command,
            dispatch_spacing: _,
            dispatch_generation,
        } => {
            if !crate::periodic::light_dispatch_generation_current(state, dispatch_generation) {
                tracing::debug!(
                    target: "cmd",
                    event = "apply_node_command_skipped",
                    command_id = %command_id,
                    node_id = %node_id,
                    dispatch_generation,
                    reason = "stale_dispatch_generation",
                    "Skipping stale apply_node_command"
                );
                return;
            }
            let runtime = {
                let Ok(s) = state.lock() else { return };
                s.hub_runtime()
            };
            let Some(runtime) = runtime else { return };

            // Manual mode-apply commands are paced producer-side by
            // dispatch_room_commands. We intentionally do not wait on
            // next_node_dispatch_at here — that slot is shared with periodic
            // ticks, and a single periodic tick with a long phase_gap could
            // otherwise stall every queued ApplyNodeCommand behind it.
            crate::commands::log_room_command_dispatch(runtime.as_ref(), &node_id, &command);
            if let Err(e) = runtime.apply_room_command(&node_id, command) {
                warn!(
                    target: "cmd",
                    "Worker: apply_node_command for '{}' failed: {}",
                    node_id,
                    e
                );
                return;
            }

            crate::commands::update_lights_on_cache_for_runtime_node(
                state, &runtime, &node_id, true,
            );
            crate::commands::emit_node_state_event_after_apply(state, &runtime, &node_id);
        }
        WorkItem::PeriodicNodeTick {
            command_id,
            node_id,
            settings_node_id,
            current_hour,
            emit_parent_node_id,
            dispatch_spacing,
            dispatch_generation,
        } => {
            let current_hour = {
                let Ok(s) = state.lock() else { return };
                if s.light_dispatch_generation != dispatch_generation {
                    tracing::debug!(
                        target: "sys",
                        event = "periodic_node_tick_skipped",
                        command_id = %command_id,
                        node_id = %node_id,
                        settings_node_id = %settings_node_id,
                        dispatch_generation,
                        current_generation = s.light_dispatch_generation,
                        reason = "stale_dispatch_generation",
                        "Skipping stale periodic tick"
                    );
                    return;
                }
                s.pending_periodic_ticks
                    .get(&node_id)
                    .filter(|pending| pending.dispatch_generation == dispatch_generation)
                    .map(|pending| pending.current_hour)
                    .unwrap_or(current_hour)
            };

            let runtime = {
                let Ok(s) = state.lock() else { return };
                s.hub_runtime()
            };
            let Some(runtime) = runtime else {
                crate::periodic::clear_pending_periodic_tick_generation(
                    state,
                    &node_id,
                    dispatch_generation,
                );
                return;
            };

            if runtime.engine_node_snapshot(&settings_node_id).is_none() {
                tracing::debug!(
                    target: "sys",
                    event = "periodic_node_tick_skipped",
                    command_id = %command_id,
                    node_id = %node_id,
                    settings_node_id = %settings_node_id,
                    reason = "stale_settings_node",
                    "Skipping stale periodic tick"
                );
                crate::periodic::clear_pending_periodic_tick_generation(
                    state,
                    &node_id,
                    dispatch_generation,
                );
                return;
            }

            if !crate::periodic::wait_for_node_dispatch_slot_if_current(
                state,
                &command_id,
                &node_id,
                dispatch_spacing,
                dispatch_generation,
            ) {
                tracing::debug!(
                    target: "sys",
                    event = "periodic_node_tick_skipped",
                    command_id = %command_id,
                    node_id = %node_id,
                    settings_node_id = %settings_node_id,
                    dispatch_generation,
                    reason = "stale_dispatch_generation_after_pace",
                    "Skipping stale periodic tick"
                );
                crate::periodic::clear_pending_periodic_tick_generation(
                    state,
                    &node_id,
                    dispatch_generation,
                );
                return;
            }
            let tick_started = Instant::now();
            let tick_result = runtime.periodic_tick_node(&node_id, &settings_node_id, current_hour);
            let elapsed = tick_started.elapsed();
            if let Err(e) = tick_result {
                tracing::warn!(
                    target: "sys",
                    event = "periodic_node_tick_failed",
                    command_id = %command_id,
                    node_id = %node_id,
                    settings_node_id = %settings_node_id,
                    current_hour,
                    elapsed_ms = elapsed.as_millis() as u64,
                    error = %e,
                    "Periodic node tick failed"
                );
            }
            if matches!(
                crate::periodic::classify_tick_latency(elapsed),
                crate::periodic::TickLatencyOutcome::Slow
            ) {
                tracing::warn!(
                    target: "sys",
                    event = "periodic_node_tick_slow",
                    command_id = %command_id,
                    node_id = %node_id,
                    settings_node_id = %settings_node_id,
                    elapsed_ms = elapsed.as_millis() as u64,
                    threshold_ms = crate::periodic::SLOW_TICK_WARNING_THRESHOLD.as_millis() as u64,
                    "Periodic node tick exceeded slow-tick threshold (controller may be stalled)"
                );
            }

            crate::periodic::post_tick_node(state, &runtime, &settings_node_id);
            if let Some(parent_node_id) = emit_parent_node_id {
                if parent_node_id != settings_node_id {
                    crate::periodic::post_tick_node(state, &runtime, &parent_node_id);
                }
            }
            crate::periodic::clear_pending_periodic_tick_generation(
                state,
                &node_id,
                dispatch_generation,
            );
        }
        WorkItem::DeferredPersist { node_id: _ } => {
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crate::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
    use crate::canonical::registry::ResolveResult;
    use crate::hub::{ActiveHub, HubType};
    use crate::registry::{RegistrySnapshot, SnapshotRoom};
    use crate::topology::{DevicePlacement, InputBinding, TopologyRoom};
    use rhythm_core::runtime::hub_registry::DeviceType;

    fn motion_source(
        source_node_id: &str,
        target_node_id: &str,
        stopped_at: Option<Instant>,
    ) -> MotionSourceState {
        MotionSourceState {
            source_node_id: source_node_id.to_string(),
            target_node_id: target_node_id.to_string(),
            stopped_at,
            stopped_at_epoch_ms: stopped_at.map(|_| current_epoch_ms()),
        }
    }

    #[test]
    fn new_is_empty() {
        let state = MotionTimerState::new();
        assert!(state.sensors.is_empty());
        assert!(state.motion_owned.is_empty());
        assert!(state.warning_active.is_empty());
    }

    #[test]
    fn has_sources_for_target_true() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), motion_source("sensor_1", "room_a", None));
        assert!(state.has_sources_for_target("room_a"));
    }

    #[test]
    fn has_sources_for_target_false() {
        let state = MotionTimerState::new();
        assert!(!state.has_sources_for_target("room_a"));

        let mut state2 = MotionTimerState::new();
        state2
            .sensors
            .insert("sensor_1".into(), motion_source("sensor_1", "room_b", None));
        assert!(!state2.has_sources_for_target("room_a"));
    }

    #[test]
    fn snapshot_active_sensor() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), motion_source("sensor_1", "room_a", None));

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
        state.sensors.insert(
            "sensor_1".into(),
            motion_source("sensor_1", "room_a", Some(stopped_at)),
        );

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
            .insert("sensor_1".into(), motion_source("sensor_1", "room_a", None));
        state.sensors.insert(
            "sensor_2".into(),
            motion_source("sensor_2", "room_a", Some(stopped_at)),
        );

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
            .insert("sensor_1".into(), motion_source("sensor_1", "room_a", None));

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
            .insert("sensor_1".into(), motion_source("sensor_1", "room_a", None));

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
            .insert("sensor_1".into(), motion_source("sensor_1", "room_a", None));
        state.motion_owned.insert("room_a".into());

        let timeouts = HashMap::new();
        let snaps = state.snapshots(&timeouts, 300);

        let snap = snaps.get("room_a").expect("room_a should have a snapshot");
        assert!(snap.motion_owned);

        // Also verify a room without ownership
        let mut state2 = MotionTimerState::new();
        state2
            .sensors
            .insert("sensor_2".into(), motion_source("sensor_2", "room_b", None));

        let snaps2 = state2.snapshots(&timeouts, 300);
        let snap2 = snaps2.get("room_b").expect("room_b should have a snapshot");
        assert!(!snap2.motion_owned);
    }

    #[test]
    fn snapshot_warning_active() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), motion_source("sensor_1", "room_a", None));
        state.warning_active.insert("room_a".into());

        let timeouts = HashMap::new();
        let snaps = state.snapshots(&timeouts, 300);

        let snap = snaps.get("room_a").expect("room_a should have a snapshot");
        assert!(snap.warning_active);

        // Also verify a room without warning
        let mut state2 = MotionTimerState::new();
        state2
            .sensors
            .insert("sensor_2".into(), motion_source("sensor_2", "room_b", None));

        let snaps2 = state2.snapshots(&timeouts, 300);
        let snap2 = snaps2.get("room_b").expect("room_b should have a snapshot");
        assert!(!snap2.warning_active);
    }

    #[test]
    fn snapshot_multiple_rooms() {
        let mut state = MotionTimerState::new();
        state
            .sensors
            .insert("sensor_1".into(), motion_source("sensor_1", "room_a", None));
        let stopped_at = Instant::now() - Duration::from_secs(20);
        state.sensors.insert(
            "sensor_2".into(),
            motion_source("sensor_2", "room_b", Some(stopped_at)),
        );
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

    #[test]
    fn hue_reconnect_resync_cooldown_is_24_hours() {
        let hue_key = HubKey::new(HubType::new(HubType::HUE), "192.168.1.2:443");
        let other_key = HubKey::new(HubType::new("test"), "hub.local");

        assert_eq!(
            reconnect_resync_cooldown(&hue_key),
            Duration::from_secs(24 * 60 * 60)
        );
        assert_eq!(
            reconnect_resync_cooldown(&other_key),
            DEFAULT_RECONNECT_RESYNC_COOLDOWN
        );
    }

    // ========================================================================
    // check_motion_timers tests
    // ========================================================================

    fn make_state() -> SharedState {
        std::sync::Arc::new(std::sync::Mutex::new(crate::state::AppState::default()))
    }

    #[derive(Clone, Default)]
    struct MotionTimerTestStorage {
        saved_motion_timers: Arc<Mutex<Vec<StoredMotionTimers>>>,
    }

    impl crate::storage::Storage for MotionTimerTestStorage {
        fn load_rooms(&self) -> anyhow::Result<rhythm_core::room::RoomManager> {
            Ok(rhythm_core::room::RoomManager::new())
        }

        fn save_rooms(&self, _rooms: &rhythm_core::room::RoomManager) -> anyhow::Result<()> {
            Ok(())
        }

        fn load_light_profiles(&self) -> anyhow::Result<crate::storage::StoredLightProfiles> {
            Err(anyhow::anyhow!("missing light profiles"))
        }

        fn save_light_profiles(
            &self,
            _config: &crate::storage::StoredLightProfiles,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn load_location(&self) -> anyhow::Result<crate::storage::StoredLocation> {
            Err(anyhow::anyhow!("missing location"))
        }

        fn save_location(&self, _loc: &crate::storage::StoredLocation) -> anyhow::Result<()> {
            Ok(())
        }

        fn load_settings(&self) -> anyhow::Result<crate::storage::StoredSettings> {
            Err(anyhow::anyhow!("missing settings"))
        }

        fn save_settings(&self, _settings: &crate::storage::StoredSettings) -> anyhow::Result<()> {
            Ok(())
        }

        fn save_motion_timers(&self, timers: &StoredMotionTimers) -> anyhow::Result<()> {
            self.saved_motion_timers
                .lock()
                .unwrap()
                .push(timers.clone());
            Ok(())
        }

        fn load_all_hub_credentials(&self) -> anyhow::Result<Vec<crate::hub::HubCredentials>> {
            Ok(Vec::new())
        }

        fn save_all_hub_credentials(
            &self,
            _creds: &[crate::hub::HubCredentials],
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn load_hub_registry_for(
            &self,
            _key: &HubKey,
        ) -> anyhow::Result<Option<serde_json::Value>> {
            Ok(None)
        }

        fn save_hub_registry_for(
            &self,
            _key: &HubKey,
            _data: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn make_state_with_runtime(runtime: Arc<dyn RuntimeHandle>) -> SharedState {
        let mut app = crate::state::AppState::default();
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

    fn only_hub_key(state: &SharedState) -> HubKey {
        state.lock().unwrap().hubs.keys().next().cloned().unwrap()
    }

    fn subscribe_events(
        state: &SharedState,
    ) -> tokio::sync::broadcast::Receiver<crate::server_event::ServerEvent> {
        let (event_tx, event_rx) = tokio::sync::broadcast::channel(16);
        state.lock().unwrap().event_tx = Some(event_tx);
        event_rx
    }

    fn add_canonical_control_source(
        state: &SharedState,
        hub_key: &HubKey,
        native_id: &str,
        room_id: &str,
        device_type: DeviceType,
    ) -> String {
        let identity = DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: Some(format!("{room_id}_native")),
            room_name: Some(room_id.to_string()),
            name: native_id.to_string(),
            device_type,
            hardware_ids: vec![HardwareId::matter(native_id)],
            manufacturer: None,
            model: None,
        };

        let mut s = state.lock().unwrap();
        if s.topology.get(room_id).is_none() {
            s.topology.insert_room(TopologyRoom::new(room_id, room_id));
        }
        let canonical_id = match s.canonical_registry.resolve(&identity, hub_key, 1) {
            ResolveResult::Created { canonical_id }
            | ResolveResult::AlreadyKnown { canonical_id } => canonical_id,
            other => panic!("unexpected resolve result: {:?}", other),
        };
        assert!(s
            .canonical_registry
            .assign_room(&canonical_id, Some(room_id)));
        s.topology.ensure_standalone_device(&canonical_id);
        assert!(s.topology.assign_device(
            &canonical_id,
            Some(room_id),
            DevicePlacement::UserOverride,
        ));
        canonical_id
    }

    fn add_canonical_standalone_control_source(
        state: &SharedState,
        hub_key: &HubKey,
        native_id: &str,
        device_type: DeviceType,
    ) -> String {
        let identity = DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: None,
            room_name: None,
            name: native_id.to_string(),
            device_type,
            hardware_ids: vec![HardwareId::matter(native_id)],
            manufacturer: None,
            model: None,
        };

        let mut s = state.lock().unwrap();
        let canonical_id = match s.canonical_registry.resolve(&identity, hub_key, 1) {
            ResolveResult::Created { canonical_id }
            | ResolveResult::AlreadyKnown { canonical_id } => canonical_id,
            other => panic!("unexpected resolve result: {:?}", other),
        };
        s.topology.ensure_standalone_device(&canonical_id);
        canonical_id
    }

    struct MotionTestRuntime {
        snapshots: Vec<RoomSnapshot>,
        any_lights_on_calls: Option<Arc<AtomicUsize>>,
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
        fn restore_room_state(&self, _: &str, _: rhythm_core::RestoredRoomState) {}
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
        fn lights_off_room(&self, _: &str, _: Option<u32>) -> anyhow::Result<()> {
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
            if let Some(ref calls) = self.any_lights_on_calls {
                calls.fetch_add(1, Ordering::SeqCst);
                return Ok(true);
            }
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

    struct SlowDispatchRuntime {
        handle_event_delay: Duration,
        turn_on_room_delay: Duration,
        handle_event_calls: Arc<AtomicUsize>,
        turn_on_room_calls: Arc<AtomicUsize>,
    }

    impl RuntimeHandle for SlowDispatchRuntime {
        fn handle_event(&self, _: &rhythm_core::InputEvent) -> anyhow::Result<bool> {
            self.handle_event_calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.handle_event_delay);
            Ok(true)
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
        fn engine_room_snapshot(&self, _: &str) -> Option<RoomSnapshot> {
            None
        }
        fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
            vec![]
        }
        fn restore_room_state(&self, _: &str, _: rhythm_core::RestoredRoomState) {}
        fn add_room(&self, _: &str, _: &str) {}
        fn remove_room(&self, _: &str) {}
        fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }
        fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
            self.turn_on_room_calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.turn_on_room_delay);
            Ok(())
        }
        fn apply_room_command(
            &self,
            _: &str,
            _: rhythm_core::LightingCommand,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn lights_off_room(&self, _: &str, _: Option<u32>) -> anyhow::Result<()> {
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
            vec![]
        }
    }

    struct PeriodicWorkerTestRuntime {
        snapshots: Vec<RoomSnapshot>,
        periodic_tick_node_calls: Arc<Mutex<Vec<(String, String, f32)>>>,
        apply_room_command_calls: Arc<AtomicUsize>,
        handle_event_calls: Arc<AtomicUsize>,
        turn_on_room_calls: Arc<AtomicUsize>,
        set_room_brightness_calls: Arc<AtomicUsize>,
        periodic_entered_tx: Option<std::sync::mpsc::Sender<()>>,
        periodic_release_rx: Option<Arc<Mutex<std::sync::mpsc::Receiver<()>>>>,
    }

    impl RuntimeHandle for PeriodicWorkerTestRuntime {
        fn handle_event(&self, _: &rhythm_core::InputEvent) -> anyhow::Result<bool> {
            self.handle_event_calls.fetch_add(1, Ordering::SeqCst);
            Ok(true)
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
        fn periodic_tick_node(
            &self,
            node_id: &str,
            source_room_id: &str,
            current_hour: f32,
        ) -> anyhow::Result<()> {
            self.periodic_tick_node_calls.lock().unwrap().push((
                node_id.to_string(),
                source_room_id.to_string(),
                current_hour,
            ));
            if let Some(tx) = &self.periodic_entered_tx {
                let _ = tx.send(());
            }
            if let Some(rx) = &self.periodic_release_rx {
                let _ = rx.lock().unwrap().recv_timeout(Duration::from_secs(2));
            }
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
        fn restore_room_state(&self, _: &str, _: rhythm_core::RestoredRoomState) {}
        fn add_room(&self, _: &str, _: &str) {}
        fn remove_room(&self, _: &str) {}
        fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }
        fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
            self.turn_on_room_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn apply_room_command(
            &self,
            _: &str,
            _: rhythm_core::LightingCommand,
        ) -> anyhow::Result<()> {
            self.apply_room_command_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn lights_off_room(&self, _: &str, _: Option<u32>) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_power_save(&self, _: bool) -> Vec<String> {
            vec![]
        }
        fn is_power_save(&self) -> bool {
            false
        }
        fn set_room_brightness(&self, _: &str, _: u8) -> anyhow::Result<()> {
            self.set_room_brightness_calls
                .fetch_add(1, Ordering::SeqCst);
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
            vec![]
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
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
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
            any_lights_on_calls: None,
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

    struct CountingReconnectDiscovery {
        discover_rooms_calls: Arc<AtomicUsize>,
    }

    impl HubDiscovery for CountingReconnectDiscovery {
        fn discover_rooms(&self) -> anyhow::Result<Vec<DiscoveredRoom>> {
            self.discover_rooms_calls.fetch_add(1, Ordering::SeqCst);
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
    fn transient_disconnect_does_not_make_hub_api_unavailable() {
        let state = make_state();
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(16);
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
            s.event_tx = Some(event_tx);
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

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Connected {
                hub_key: Some(hub_key.clone()),
            },
            &mut MotionTimerState::new(),
        );
        while event_rx.try_recv().is_ok() {}

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Disconnected {
                hub_key: Some(hub_key.clone()),
                reason: "SSE dropped".into(),
            },
            &mut MotionTimerState::new(),
        );

        {
            let s = state.lock().unwrap();
            assert!(
                s.hub_is_connected(&hub_key),
                "transient disconnect should remain app-visible as connected"
            );
            assert!(
                s.hub_pending_disconnect_at.contains_key(&hub_key),
                "disconnect should be pending during the grace window"
            );
        }

        let mut saw_unavailable = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(
                event,
                crate::server_event::ServerEvent::HubStatus {
                    connected: false,
                    ..
                }
            ) {
                saw_unavailable = true;
            }
        }
        assert!(
            !saw_unavailable,
            "transient disconnect should not emit app-visible unavailable"
        );

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Connected {
                hub_key: Some(hub_key.clone()),
            },
            &mut MotionTimerState::new(),
        );

        let s = state.lock().unwrap();
        assert!(s.hub_is_connected(&hub_key));
        assert!(!s.hub_pending_disconnect_at.contains_key(&hub_key));
    }

    #[test]
    fn reconnected_event_resyncs_room_devices_for_api_state() {
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
        let before_room = before["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == "Room A" && node["kind"] == "room")
            .unwrap();
        let before_room_id = before_room["id"].as_str().unwrap().to_string();

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Connected {
                hub_key: Some(hub_key.clone()),
            },
            &mut MotionTimerState::new(),
        );

        let after_initial: serde_json::Value =
            serde_json::from_str(&crate::commands::build_state_snapshot(&state).unwrap()).unwrap();
        let initial_room = after_initial["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == "Room A" && node["kind"] == "room")
            .unwrap();
        assert_eq!(
            initial_room["id"].as_str().unwrap(),
            before_room_id,
            "initial connect should not trigger reconnect-style resync"
        );

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Disconnected {
                hub_key: Some(hub_key.clone()),
                reason: "SSE dropped".into(),
            },
            &mut MotionTimerState::new(),
        );

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
            let room = parsed["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|candidate| candidate["name"] == "Room A" && candidate["kind"] == "room")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            if room["id"] != before_room_id {
                after = Some(parsed);
                break;
            }
        }

        let after = after.expect("reconnected event should remap the room to a topology node id");
        let after_room = after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == "Room A" && node["kind"] == "room")
            .unwrap();
        assert_ne!(after_room["id"], before_room_id);
    }

    #[test]
    fn button_event_broadcasts_normalized_input_event() {
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::ZERO,
            turn_on_room_delay: Duration::ZERO,
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        let source_id = add_canonical_control_source(
            &state,
            &hub_key,
            "button_a",
            "room_a",
            DeviceType::Button,
        );
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
        state.lock().unwrap().event_tx = Some(event_tx);

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Button {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                action: ButtonAction::OnPress,
                device_id: Some("button_a".into()),
            },
            &mut MotionTimerState::new(),
        );

        let event = event_rx.try_recv().expect("expected input event broadcast");
        match event {
            crate::server_event::ServerEvent::InputEvent(InputEventResource::Button {
                route,
                source_node_id,
                target_node_id,
                native_device_id,
                button_action,
                ..
            }) => {
                assert_eq!(route, InputEventRoute::NodeControl);
                assert_eq!(source_node_id.as_deref(), Some(source_id.as_str()));
                assert_eq!(target_node_id.as_deref(), Some("room_a"));
                assert_eq!(native_device_id.as_deref(), Some("button_a"));
                assert_eq!(button_action, Some(ButtonAction::OnPress));
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn button_event_broadcasts_input_binding_route_before_automation_dispatch() {
        let handle_event_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::ZERO,
            turn_on_room_delay: Duration::ZERO,
            handle_event_calls: handle_event_calls.clone(),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        let source_id = add_canonical_control_source(
            &state,
            &hub_key,
            "button_a",
            "room_a",
            DeviceType::Button,
        );
        {
            let mut s = state.lock().unwrap();
            s.topology.set_input_binding(InputBinding::day_sleep_toggle(
                source_id.clone(),
                Some(ButtonAction::OnPress),
            ));
        }
        let mut event_rx = subscribe_events(&state);

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Button {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                action: ButtonAction::OnPress,
                device_id: Some("button_a".into()),
            },
            &mut MotionTimerState::new(),
        );

        let event = event_rx.try_recv().expect("expected input event broadcast");
        match event {
            crate::server_event::ServerEvent::InputEvent(InputEventResource::Button {
                epoch_ms,
                route,
                hub_type,
                address,
                source_node_id,
                target_node_id,
                source_room_id,
                native_device_id,
                native_button_id,
                button_action,
            }) => {
                assert!(epoch_ms > 0);
                assert_eq!(route, InputEventRoute::InputBinding);
                assert_eq!(hub_type.as_deref(), Some("test"));
                assert_eq!(address.as_deref(), Some("hub.local"));
                assert_eq!(source_node_id.as_deref(), Some(source_id.as_str()));
                assert_eq!(target_node_id, None);
                assert_eq!(source_room_id.as_deref(), Some("room_a"));
                assert_eq!(native_device_id.as_deref(), Some("button_a"));
                assert_eq!(native_button_id, None);
                assert_eq!(button_action, Some(ButtonAction::OnPress));
            }
            other => panic!("unexpected event: {:?}", other),
        }
        assert_eq!(
            handle_event_calls.load(Ordering::SeqCst),
            0,
            "input binding should not fall through to ordinary node control"
        );
    }

    #[test]
    fn button_event_for_known_source_without_target_broadcasts_unroutable() {
        let handle_event_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::ZERO,
            turn_on_room_delay: Duration::ZERO,
            handle_event_calls: handle_event_calls.clone(),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        let source_id = add_canonical_standalone_control_source(
            &state,
            &hub_key,
            "button_a",
            DeviceType::Button,
        );
        let mut event_rx = subscribe_events(&state);

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Button {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                action: ButtonAction::OffPress,
                device_id: Some("button_a".into()),
            },
            &mut MotionTimerState::new(),
        );

        let event = event_rx.try_recv().expect("expected input event broadcast");
        match event {
            crate::server_event::ServerEvent::InputEvent(InputEventResource::Button {
                route,
                hub_type,
                address,
                source_node_id,
                target_node_id,
                source_room_id,
                native_device_id,
                button_action,
                ..
            }) => {
                assert_eq!(route, InputEventRoute::Unroutable);
                assert_eq!(hub_type.as_deref(), Some("test"));
                assert_eq!(address.as_deref(), Some("hub.local"));
                assert_eq!(source_node_id.as_deref(), Some(source_id.as_str()));
                assert_eq!(target_node_id, None);
                assert_eq!(source_room_id.as_deref(), Some("room_a"));
                assert_eq!(native_device_id.as_deref(), Some("button_a"));
                assert_eq!(button_action, Some(ButtonAction::OffPress));
            }
            other => panic!("unexpected event: {:?}", other),
        }
        assert_eq!(handle_event_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn button_event_for_unknown_source_broadcasts_unresolved() {
        let handle_event_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::ZERO,
            turn_on_room_delay: Duration::ZERO,
            handle_event_calls: handle_event_calls.clone(),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        let mut event_rx = subscribe_events(&state);

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Button {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                action: ButtonAction::Reset,
                device_id: Some("missing_button".into()),
            },
            &mut MotionTimerState::new(),
        );

        let event = event_rx.try_recv().expect("expected input event broadcast");
        match event {
            crate::server_event::ServerEvent::InputEvent(InputEventResource::Button {
                route,
                hub_type,
                address,
                source_node_id,
                target_node_id,
                source_room_id,
                native_device_id,
                button_action,
                ..
            }) => {
                assert_eq!(route, InputEventRoute::Unresolved);
                assert_eq!(hub_type.as_deref(), Some("test"));
                assert_eq!(address.as_deref(), Some("hub.local"));
                assert_eq!(source_node_id, None);
                assert_eq!(target_node_id, None);
                assert_eq!(source_room_id.as_deref(), Some("room_a"));
                assert_eq!(native_device_id.as_deref(), Some("missing_button"));
                assert_eq!(button_action, Some(ButtonAction::Reset));
            }
            other => panic!("unexpected event: {:?}", other),
        }
        assert_eq!(handle_event_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unroutable_button_event_broadcasts_native_button_id() {
        let state = make_state();
        let hub_key = HubKey::new(HubType::new("test"), "hub.local");
        let mut event_rx = subscribe_events(&state);

        handle_hub_event(
            &state,
            crate::hub::HubEvent::UnroutableButton {
                hub_key: Some(hub_key),
                device_id: Some("button_device".into()),
                button_id: "button_resource".into(),
            },
            &mut MotionTimerState::new(),
        );

        let event = event_rx.try_recv().expect("expected input event broadcast");
        match event {
            crate::server_event::ServerEvent::InputEvent(InputEventResource::Button {
                route,
                hub_type,
                address,
                native_device_id,
                native_button_id,
                button_action,
                ..
            }) => {
                assert_eq!(route, InputEventRoute::Unroutable);
                assert_eq!(hub_type.as_deref(), Some("test"));
                assert_eq!(address.as_deref(), Some("hub.local"));
                assert_eq!(native_device_id.as_deref(), Some("button_device"));
                assert_eq!(native_button_id.as_deref(), Some("button_resource"));
                assert_eq!(button_action, None);
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn button_event_dispatches_without_blocking_event_ingress() {
        let handle_event_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::from_millis(250),
            turn_on_room_delay: Duration::ZERO,
            handle_event_calls: handle_event_calls.clone(),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        add_canonical_control_source(&state, &hub_key, "button_a", "room_a", DeviceType::Button);

        let started = Instant::now();
        handle_hub_event(
            &state,
            crate::hub::HubEvent::Button {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                action: ButtonAction::OnPress,
                device_id: Some("button_a".into()),
            },
            &mut MotionTimerState::new(),
        );
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(100),
            "button ingress should return quickly, elapsed {:?}",
            elapsed
        );

        for _ in 0..30 {
            if handle_event_calls.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(handle_event_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn periodic_worker_accepts_internal_light_node_with_live_settings_node() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(PeriodicWorkerTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            }],
            periodic_tick_node_calls: calls.clone(),
            apply_room_command_calls: Arc::new(AtomicUsize::new(0)),
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
            set_room_brightness_calls: Arc::new(AtomicUsize::new(0)),
            periodic_entered_tx: None,
            periodic_release_rx: None,
        });
        let state = make_state_with_runtime(runtime);
        let internal_node_id =
            "__rhythm_light_node__|room=room_a|kind=group|hub=hue@bridge|source=hue-room";
        let dispatch_generation = state.lock().unwrap().light_dispatch_generation;

        process_work_item(
            &state,
            WorkItem::PeriodicNodeTick {
                command_id: "periodic-test".into(),
                node_id: internal_node_id.into(),
                settings_node_id: "room_a".into(),
                current_hour: 20.25,
                emit_parent_node_id: None,
                dispatch_spacing: Duration::ZERO,
                dispatch_generation,
            },
        );

        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[(internal_node_id.to_string(), "room_a".to_string(), 20.25)]
        );
    }

    #[test]
    fn periodic_worker_skips_stale_generation_without_clearing_current_pending_tick() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(PeriodicWorkerTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            }],
            periodic_tick_node_calls: calls.clone(),
            apply_room_command_calls: Arc::new(AtomicUsize::new(0)),
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
            set_room_brightness_calls: Arc::new(AtomicUsize::new(0)),
            periodic_entered_tx: None,
            periodic_release_rx: None,
        });
        let state = make_state_with_runtime(runtime);
        let stale_generation = state.lock().unwrap().light_dispatch_generation;
        let internal_node_id =
            "__rhythm_light_node__|room=room_a|kind=group|hub=hue@bridge|source=hue-room";
        {
            let mut s = state.lock().unwrap();
            s.invalidate_queued_light_dispatches();
            let current_generation = s.light_dispatch_generation;
            s.pending_periodic_ticks.insert(
                internal_node_id.to_string(),
                crate::state::PendingPeriodicTick::new(21.0, current_generation),
            );
        }

        process_work_item(
            &state,
            WorkItem::PeriodicNodeTick {
                command_id: "periodic-stale".into(),
                node_id: internal_node_id.into(),
                settings_node_id: "room_a".into(),
                current_hour: 20.25,
                emit_parent_node_id: None,
                dispatch_spacing: Duration::ZERO,
                dispatch_generation: stale_generation,
            },
        );

        assert!(calls.lock().unwrap().is_empty());
        assert!(state
            .lock()
            .unwrap()
            .pending_periodic_ticks
            .contains_key(internal_node_id));
    }

    #[test]
    fn periodic_worker_keeps_pending_marker_while_tick_is_running() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(PeriodicWorkerTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            }],
            periodic_tick_node_calls: calls.clone(),
            apply_room_command_calls: Arc::new(AtomicUsize::new(0)),
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
            set_room_brightness_calls: Arc::new(AtomicUsize::new(0)),
            periodic_entered_tx: Some(entered_tx),
            periodic_release_rx: Some(Arc::new(Mutex::new(release_rx))),
        });
        let state = make_state_with_runtime(runtime);
        let (tx, rx) = std::sync::mpsc::sync_channel::<WorkItem>(4);
        let dispatch_generation = state.lock().unwrap().light_dispatch_generation;

        assert!(crate::periodic::enqueue_periodic_tick(
            &state,
            &tx,
            crate::periodic::PeriodicTickEnqueue {
                command_id: "periodic-running-1",
                node_id: "room_a",
                settings_node_id: "room_a",
                dispatch_generation,
                current_hour: 20.0,
                emit_parent_node_id: None,
                dispatch_spacing: Duration::ZERO,
            },
        ));
        let item = rx.try_recv().expect("first tick should enqueue");

        let state_for_worker = state.clone();
        let worker = std::thread::spawn(move || {
            process_work_item(&state_for_worker, item);
        });
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("periodic tick should enter runtime");

        assert!(
            state
                .lock()
                .unwrap()
                .pending_periodic_ticks
                .contains_key("room_a"),
            "pending marker must remain while the controller call is in flight"
        );

        assert!(crate::periodic::enqueue_periodic_tick(
            &state,
            &tx,
            crate::periodic::PeriodicTickEnqueue {
                command_id: "periodic-running-2",
                node_id: "room_a",
                settings_node_id: "room_a",
                dispatch_generation,
                current_hour: 21.0,
                emit_parent_node_id: None,
                dispatch_spacing: Duration::ZERO,
            },
        ));
        assert!(
            rx.try_recv().is_err(),
            "a tick that arrives while the previous one runs must coalesce, not queue behind it"
        );
        assert_eq!(
            state
                .lock()
                .unwrap()
                .pending_periodic_ticks
                .get("room_a")
                .map(|pending| pending.current_hour),
            Some(21.0)
        );

        release_tx.send(()).unwrap();
        worker.join().unwrap();

        assert!(
            !state
                .lock()
                .unwrap()
                .pending_periodic_ticks
                .contains_key("room_a"),
            "finished tick should release the coalescing marker"
        );
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[("room_a".to_string(), "room_a".to_string(), 20.0)]
        );
    }

    #[test]
    fn apply_node_command_does_not_wait_on_periodic_dispatch_slot() {
        // Regression: a periodic tick with a long phase_gap used to push the
        // shared next_node_dispatch_at slot minutes into the future, stalling
        // every queued ApplyNodeCommand from a manual mode transition behind
        // it. ApplyNodeCommand is paced producer-side by dispatch_room_commands
        // and must not wait on the shared periodic slot.
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(PeriodicWorkerTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            }],
            periodic_tick_node_calls: Arc::new(Mutex::new(Vec::new())),
            apply_room_command_calls: apply_calls.clone(),
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
            set_room_brightness_calls: Arc::new(AtomicUsize::new(0)),
            periodic_entered_tx: None,
            periodic_release_rx: None,
        });
        let state = make_state_with_runtime(runtime);
        let dispatch_generation = state.lock().unwrap().light_dispatch_generation;

        // Simulate periodic having pushed the shared dispatch slot 3 minutes
        // out (the same scenario as a 1-eligible-node periodic cycle with
        // cycle_secs=180).
        {
            let mut s = state.lock().unwrap();
            s.next_node_dispatch_at = Some(
                Instant::now()
                    .checked_add(Duration::from_secs(180))
                    .unwrap(),
            );
        }

        let started = Instant::now();
        process_work_item(
            &state,
            WorkItem::ApplyNodeCommand {
                command_id: "manual-apply".into(),
                node_id: "room_a".into(),
                command: rhythm_core::LightingCommand::with_transition(50, 4000, 500),
                dispatch_spacing: Duration::from_secs(3),
                dispatch_generation,
            },
        );
        let elapsed = started.elapsed();

        assert_eq!(apply_calls.load(Ordering::SeqCst), 1);
        assert!(
            elapsed < Duration::from_secs(1),
            "ApplyNodeCommand stalled on periodic slot: elapsed {:?}",
            elapsed
        );
    }

    #[test]
    fn queued_node_action_does_not_wait_on_periodic_dispatch_slot() {
        let handle_event_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(PeriodicWorkerTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            }],
            periodic_tick_node_calls: Arc::new(Mutex::new(Vec::new())),
            apply_room_command_calls: Arc::new(AtomicUsize::new(0)),
            handle_event_calls: handle_event_calls.clone(),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
            set_room_brightness_calls: Arc::new(AtomicUsize::new(0)),
            periodic_entered_tx: None,
            periodic_release_rx: None,
        });
        let state = make_state_with_runtime(runtime);
        {
            let mut s = state.lock().unwrap();
            s.next_node_dispatch_at =
                Some(Instant::now().checked_add(Duration::from_secs(2)).unwrap());
        }

        let started = Instant::now();
        process_work_item(
            &state,
            WorkItem::QueuedNodeAction {
                command_id: "app-action".into(),
                node_id: "room_a".into(),
                action: ButtonAction::OnPress,
                device_id: None,
                dispatch_spacing: Duration::from_secs(3),
                persist_after: false,
            },
        );
        let elapsed = started.elapsed();

        assert_eq!(handle_event_calls.load(Ordering::SeqCst), 1);
        assert!(
            elapsed < Duration::from_millis(500),
            "QueuedNodeAction stalled on periodic slot: elapsed {:?}",
            elapsed
        );
    }

    #[test]
    fn set_node_brightness_does_not_wait_on_periodic_dispatch_slot() {
        let brightness_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(PeriodicWorkerTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            }],
            periodic_tick_node_calls: Arc::new(Mutex::new(Vec::new())),
            apply_room_command_calls: Arc::new(AtomicUsize::new(0)),
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
            set_room_brightness_calls: brightness_calls.clone(),
            periodic_entered_tx: None,
            periodic_release_rx: None,
        });
        let state = make_state_with_runtime(runtime);
        {
            let mut s = state.lock().unwrap();
            s.next_node_dispatch_at =
                Some(Instant::now().checked_add(Duration::from_secs(2)).unwrap());
        }

        let started = Instant::now();
        process_work_item(
            &state,
            WorkItem::SetNodeBrightness {
                command_id: "app-brightness".into(),
                node_id: "room_a".into(),
                brightness: 55,
                dispatch_spacing: Duration::from_secs(3),
                persist_after: false,
            },
        );
        let elapsed = started.elapsed();

        assert_eq!(brightness_calls.load(Ordering::SeqCst), 1);
        assert!(
            elapsed < Duration::from_millis(500),
            "SetNodeBrightness stalled on periodic slot: elapsed {:?}",
            elapsed
        );
    }

    #[test]
    fn set_node_preferences_does_not_wait_on_periodic_dispatch_slot() {
        let turn_on_room_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(PeriodicWorkerTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: true,
                profile_settings: RoomProfileSettings::default(),
            }],
            periodic_tick_node_calls: Arc::new(Mutex::new(Vec::new())),
            apply_room_command_calls: Arc::new(AtomicUsize::new(0)),
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: turn_on_room_calls.clone(),
            set_room_brightness_calls: Arc::new(AtomicUsize::new(0)),
            periodic_entered_tx: None,
            periodic_release_rx: None,
        });
        let state = make_state_with_runtime(runtime);
        {
            let mut s = state.lock().unwrap();
            s.next_node_dispatch_at =
                Some(Instant::now().checked_add(Duration::from_secs(2)).unwrap());
        }

        let started = Instant::now();
        process_work_item(
            &state,
            WorkItem::SetNodePreferences {
                command_id: "app-preferences".into(),
                node_id: "room_a".into(),
                rhythm_enabled: None,
                disabled: None,
                target_state: Some(rhythm_core::RoomModeState::Active),
                room_profile: None,
                dispatch_spacing: Duration::from_secs(3),
                persist_after: false,
            },
        );
        let elapsed = started.elapsed();

        assert_eq!(turn_on_room_calls.load(Ordering::SeqCst), 1);
        assert!(
            elapsed < Duration::from_millis(500),
            "SetNodePreferences stalled on periodic slot: elapsed {:?}",
            elapsed
        );
    }

    #[test]
    fn motion_event_broadcasts_normalized_input_event() {
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::ZERO,
            turn_on_room_delay: Duration::ZERO,
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: Arc::new(AtomicUsize::new(0)),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        let source_id = add_canonical_control_source(
            &state,
            &hub_key,
            "sensor_a",
            "room_a",
            DeviceType::Motion,
        );
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
        state.lock().unwrap().event_tx = Some(event_tx);

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Motion {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                sensor_id: "sensor_a".into(),
                detected: true,
            },
            &mut MotionTimerState::new(),
        );

        let event = event_rx.try_recv().expect("expected input event broadcast");
        match event {
            crate::server_event::ServerEvent::InputEvent(InputEventResource::Motion {
                route,
                source_node_id,
                target_node_id,
                native_sensor_id,
                detected,
                ..
            }) => {
                assert_eq!(route, InputEventRoute::NodeControl);
                assert_eq!(source_node_id.as_deref(), Some(source_id.as_str()));
                assert_eq!(target_node_id.as_deref(), Some("room_a"));
                assert_eq!(native_sensor_id, "sensor_a");
                assert!(detected);
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn motion_turn_on_emits_node_state_event() {
        // Regression test for issue #72. Motion-triggered turn-on must
        // broadcast a NodeState SSE event so the app updates without
        // requiring a hard refresh.
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(MotionTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            }],
            any_lights_on_calls: None,
        });
        let state = make_state_with_runtime(runtime);
        let mut event_rx = subscribe_events(&state);

        run_motion_turn_on_action(&state, "room_a");

        let mut saw_node_state = false;
        while let Ok(event) = event_rx.try_recv() {
            if let crate::server_event::ServerEvent::NodeState { nodes } = event {
                assert!(
                    nodes.iter().any(|n| n.id == "room_a"),
                    "NodeState should include room_a, got {:?}",
                    nodes.iter().map(|n| &n.id).collect::<Vec<_>>()
                );
                saw_node_state = true;
                break;
            }
        }
        assert!(
            saw_node_state,
            "expected NodeState SSE event after motion-triggered turn-on"
        );
    }

    #[test]
    fn motion_clear_event_broadcasts_detected_false_before_timer_routing() {
        let turn_on_room_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::ZERO,
            turn_on_room_delay: Duration::ZERO,
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: turn_on_room_calls.clone(),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        let source_id = add_canonical_control_source(
            &state,
            &hub_key,
            "sensor_a",
            "room_a",
            DeviceType::Motion,
        );
        let mut event_rx = subscribe_events(&state);
        let mut motion = MotionTimerState::new();

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Motion {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                sensor_id: "sensor_a".into(),
                detected: false,
            },
            &mut motion,
        );

        let event = event_rx.try_recv().expect("expected input event broadcast");
        match event {
            crate::server_event::ServerEvent::InputEvent(InputEventResource::Motion {
                route,
                hub_type,
                address,
                source_node_id,
                target_node_id,
                source_room_id,
                native_sensor_id,
                detected,
                ..
            }) => {
                assert_eq!(route, InputEventRoute::NodeControl);
                assert_eq!(hub_type.as_deref(), Some("test"));
                assert_eq!(address.as_deref(), Some("hub.local"));
                assert_eq!(source_node_id.as_deref(), Some(source_id.as_str()));
                assert_eq!(target_node_id.as_deref(), Some("room_a"));
                assert_eq!(source_room_id.as_deref(), Some("room_a"));
                assert_eq!(native_sensor_id, "sensor_a");
                assert!(!detected);
            }
            other => panic!("unexpected event: {:?}", other),
        }
        assert!(motion.sensors.is_empty());
        assert_eq!(turn_on_room_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn motion_event_for_known_source_without_target_broadcasts_unroutable() {
        let turn_on_room_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::ZERO,
            turn_on_room_delay: Duration::ZERO,
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: turn_on_room_calls.clone(),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        let source_id = add_canonical_standalone_control_source(
            &state,
            &hub_key,
            "sensor_a",
            DeviceType::Motion,
        );
        let mut event_rx = subscribe_events(&state);
        let mut motion = MotionTimerState::new();

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Motion {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                sensor_id: "sensor_a".into(),
                detected: true,
            },
            &mut motion,
        );

        let event = event_rx.try_recv().expect("expected input event broadcast");
        match event {
            crate::server_event::ServerEvent::InputEvent(InputEventResource::Motion {
                route,
                hub_type,
                address,
                source_node_id,
                target_node_id,
                source_room_id,
                native_sensor_id,
                detected,
                ..
            }) => {
                assert_eq!(route, InputEventRoute::Unroutable);
                assert_eq!(hub_type.as_deref(), Some("test"));
                assert_eq!(address.as_deref(), Some("hub.local"));
                assert_eq!(source_node_id.as_deref(), Some(source_id.as_str()));
                assert_eq!(target_node_id, None);
                assert_eq!(source_room_id.as_deref(), Some("room_a"));
                assert_eq!(native_sensor_id, "sensor_a");
                assert!(detected);
            }
            other => panic!("unexpected event: {:?}", other),
        }
        assert!(motion.sensors.is_empty());
        assert_eq!(turn_on_room_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn motion_event_for_unknown_source_broadcasts_unresolved() {
        let turn_on_room_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::ZERO,
            turn_on_room_delay: Duration::ZERO,
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: turn_on_room_calls.clone(),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        let mut event_rx = subscribe_events(&state);
        let mut motion = MotionTimerState::new();

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Motion {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                sensor_id: "missing_sensor".into(),
                detected: true,
            },
            &mut motion,
        );

        let event = event_rx.try_recv().expect("expected input event broadcast");
        match event {
            crate::server_event::ServerEvent::InputEvent(InputEventResource::Motion {
                route,
                hub_type,
                address,
                source_node_id,
                target_node_id,
                source_room_id,
                native_sensor_id,
                detected,
                ..
            }) => {
                assert_eq!(route, InputEventRoute::Unresolved);
                assert_eq!(hub_type.as_deref(), Some("test"));
                assert_eq!(address.as_deref(), Some("hub.local"));
                assert_eq!(source_node_id, None);
                assert_eq!(target_node_id, None);
                assert_eq!(source_room_id.as_deref(), Some("room_a"));
                assert_eq!(native_sensor_id, "missing_sensor");
                assert!(detected);
            }
            other => panic!("unexpected event: {:?}", other),
        }
        assert!(motion.sensors.is_empty());
        assert_eq!(turn_on_room_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn motion_event_dispatches_without_blocking_event_ingress() {
        let turn_on_room_calls = Arc::new(AtomicUsize::new(0));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(SlowDispatchRuntime {
            handle_event_delay: Duration::ZERO,
            turn_on_room_delay: Duration::from_millis(250),
            handle_event_calls: Arc::new(AtomicUsize::new(0)),
            turn_on_room_calls: turn_on_room_calls.clone(),
        });
        let state = make_state_with_runtime(runtime);
        let hub_key = only_hub_key(&state);
        add_canonical_control_source(&state, &hub_key, "sensor_a", "room_a", DeviceType::Motion);
        let mut motion = MotionTimerState::new();

        let started = Instant::now();
        handle_hub_event(
            &state,
            crate::hub::HubEvent::Motion {
                hub_key: Some(hub_key),
                room_id: "room_a".into(),
                sensor_id: "sensor_a".into(),
                detected: true,
            },
            &mut motion,
        );
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(100),
            "motion ingress should return quickly, elapsed {:?}",
            elapsed
        );

        for _ in 0..30 {
            if turn_on_room_calls.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(turn_on_room_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn initial_connect_skips_resync_then_rapid_reconnect_polls_light_state() {
        let state = make_state();
        let hub_type = HubType::new("test");
        let hub_key = HubKey::new(hub_type.clone(), "hub.local");
        let discover_rooms_calls = Arc::new(AtomicUsize::new(0));
        let light_poll_calls = Arc::new(AtomicUsize::new(0));

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

        let runtime: Arc<dyn RuntimeHandle> = Arc::new(MotionTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            }],
            any_lights_on_calls: Some(light_poll_calls.clone()),
        });

        {
            let mut s = state.lock().unwrap();
            s.hubs.insert(
                hub_key.clone(),
                ActiveHub {
                    hub_type,
                    hub_key: hub_key.clone(),
                    runtime: Some(runtime),
                    hub_data: Box::new(()),
                    registry: Some(registry),
                    discovery: Some(Arc::new(CountingReconnectDiscovery {
                        discover_rooms_calls: discover_rooms_calls.clone(),
                    })),
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
            s.set_hub_connected(&hub_key, false);
        }

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Connected {
                hub_key: Some(hub_key.clone()),
            },
            &mut MotionTimerState::new(),
        );

        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            discover_rooms_calls.load(Ordering::SeqCst),
            0,
            "initial connect should not trigger reconnect resync"
        );
        assert_eq!(
            light_poll_calls.load(Ordering::SeqCst),
            0,
            "initial connect should not poll light state through reconnect path"
        );

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Disconnected {
                hub_key: Some(hub_key.clone()),
                reason: "SSE dropped".into(),
            },
            &mut MotionTimerState::new(),
        );

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Connected {
                hub_key: Some(hub_key.clone()),
            },
            &mut MotionTimerState::new(),
        );

        for _ in 0..50 {
            if discover_rooms_calls.load(Ordering::SeqCst) >= 1
                && light_poll_calls.load(Ordering::SeqCst) >= 1
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(discover_rooms_calls.load(Ordering::SeqCst), 1);
        let first_light_poll_calls = light_poll_calls.load(Ordering::SeqCst);
        assert!(
            first_light_poll_calls >= 1,
            "first real reconnect should poll light state"
        );

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Disconnected {
                hub_key: Some(hub_key.clone()),
                reason: "SSE dropped".into(),
            },
            &mut MotionTimerState::new(),
        );

        handle_hub_event(
            &state,
            crate::hub::HubEvent::Connected {
                hub_key: Some(hub_key.clone()),
            },
            &mut MotionTimerState::new(),
        );

        for _ in 0..50 {
            if light_poll_calls.load(Ordering::SeqCst) > first_light_poll_calls {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(
            discover_rooms_calls.load(Ordering::SeqCst),
            1,
            "rapid reconnect should not trigger a second full resync"
        );
        assert!(
            light_poll_calls.load(Ordering::SeqCst) > first_light_poll_calls,
            "rapid reconnect should still refresh light state"
        );
    }

    #[test]
    fn disconnect_storm_does_not_leak_resync_invocations() {
        // Simulate a flapping hub connection: many disconnect/connect pairs
        // within the cooldown window. The resync machinery must stay bounded
        // — a single resync on the first reconnect, then no extra full
        // resyncs until the cooldown elapses. Light state polls may continue
        // firing (that's the point of the fast path) but the expensive
        // discover-rooms call must be rate-limited.
        let state = make_state();
        let hub_type = HubType::new("test");
        let hub_key = HubKey::new(hub_type.clone(), "hub.local");
        let discover_rooms_calls = Arc::new(AtomicUsize::new(0));
        let light_poll_calls = Arc::new(AtomicUsize::new(0));

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

        let runtime: Arc<dyn RuntimeHandle> = Arc::new(MotionTestRuntime {
            snapshots: vec![RoomSnapshot {
                id: "room_a".into(),
                name: "Room A".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: RoomProfileSettings::default(),
            }],
            any_lights_on_calls: Some(light_poll_calls.clone()),
        });

        {
            let mut s = state.lock().unwrap();
            s.hubs.insert(
                hub_key.clone(),
                ActiveHub {
                    hub_type,
                    hub_key: hub_key.clone(),
                    runtime: Some(runtime),
                    hub_data: Box::new(()),
                    registry: Some(registry),
                    discovery: Some(Arc::new(CountingReconnectDiscovery {
                        discover_rooms_calls: discover_rooms_calls.clone(),
                    })),
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
            s.set_hub_connected(&hub_key, false);
        }

        // Initial connect: no resync.
        handle_hub_event(
            &state,
            crate::hub::HubEvent::Connected {
                hub_key: Some(hub_key.clone()),
            },
            &mut MotionTimerState::new(),
        );

        // Storm: 20 rapid disconnect/connect cycles within the cooldown.
        for _ in 0..20 {
            handle_hub_event(
                &state,
                crate::hub::HubEvent::Disconnected {
                    hub_key: Some(hub_key.clone()),
                    reason: "SSE dropped".into(),
                },
                &mut MotionTimerState::new(),
            );
            handle_hub_event(
                &state,
                crate::hub::HubEvent::Connected {
                    hub_key: Some(hub_key.clone()),
                },
                &mut MotionTimerState::new(),
            );
        }

        // Give spawned resync threads time to finish.
        std::thread::sleep(Duration::from_millis(200));

        // Full discover-rooms resync should have fired at most once during
        // the whole storm — everything after that is suppressed by the
        // reconnect-resync cooldown rate-limiter.
        let discover = discover_rooms_calls.load(Ordering::SeqCst);
        assert!(
            discover <= 1,
            "disconnect storm should trigger at most 1 full resync within cooldown, got {}",
            discover
        );

        // Hub should end up marked connected.
        let s = state.lock().unwrap();
        assert!(
            s.hub_is_connected(&hub_key),
            "after storm settles, hub must be marked connected"
        );
    }

    // ========================================================================
    // Button-debounce tests
    // ========================================================================

    #[test]
    fn admit_button_lets_first_press_through() {
        let mut motion = MotionTimerState::new();
        assert!(motion.admit_button("room_a", ButtonAction::OnPress, None));
    }

    #[test]
    fn admit_button_drops_repeat_within_window() {
        let mut motion = MotionTimerState::new();
        let t0 = Instant::now();
        assert!(motion.admit_button_at("room_a", ButtonAction::OnPress, None, t0));
        // 50ms later — well inside the 150ms window — must be dropped.
        let t1 = t0 + Duration::from_millis(50);
        assert!(!motion.admit_button_at("room_a", ButtonAction::OnPress, None, t1));
    }

    #[test]
    fn admit_button_passes_after_window_elapses() {
        let mut motion = MotionTimerState::new();
        let t0 = Instant::now();
        assert!(motion.admit_button_at("room_a", ButtonAction::OnPress, None, t0));
        let t1 = t0 + BUTTON_DEBOUNCE_WINDOW + Duration::from_millis(5);
        assert!(motion.admit_button_at("room_a", ButtonAction::OnPress, None, t1));
    }

    #[test]
    fn admit_button_distinguishes_actions_on_same_node() {
        // Real Hue Dimmer: OnPress immediately followed by OffPress is a valid
        // user action on different physical buttons. The debounce must NOT
        // collapse them.
        let mut motion = MotionTimerState::new();
        let t0 = Instant::now();
        assert!(motion.admit_button_at("room_a", ButtonAction::OnPress, None, t0));
        let t1 = t0 + Duration::from_millis(20);
        assert!(motion.admit_button_at("room_a", ButtonAction::OffPress, None, t1));
    }

    #[test]
    fn admit_button_distinguishes_nodes_for_same_action() {
        // Two physical buttons in different rooms pressed within 20ms (e.g.,
        // "all on" scene) must each dispatch.
        let mut motion = MotionTimerState::new();
        let t0 = Instant::now();
        assert!(motion.admit_button_at("room_a", ButtonAction::OnPress, None, t0));
        let t1 = t0 + Duration::from_millis(20);
        assert!(motion.admit_button_at("room_b", ButtonAction::OnPress, None, t1));
    }

    #[test]
    fn admit_button_distinguishes_devices_on_same_node_and_action() {
        // Two controllers bound to the same room must not suppress each
        // other just because they trigger the same action close together.
        let mut motion = MotionTimerState::new();
        let t0 = Instant::now();
        assert!(motion.admit_button_at("room_a", ButtonAction::OnPress, Some("device-a"), t0));
        let t1 = t0 + Duration::from_millis(20);
        assert!(motion.admit_button_at("room_a", ButtonAction::OnPress, Some("device-b"), t1));
    }

    #[test]
    fn admit_button_continues_to_drop_during_sustained_bounces() {
        // Bounce stream: 4 events at 30ms spacing should produce one dispatch.
        let mut motion = MotionTimerState::new();
        let t0 = Instant::now();
        let mut admitted = 0;
        for i in 0..4 {
            let t = t0 + Duration::from_millis(i * 30);
            if motion.admit_button_at("room_a", ButtonAction::OnPress, None, t) {
                admitted += 1;
            }
        }
        assert_eq!(
            admitted, 1,
            "only the first of four bouncing events should pass"
        );
    }

    #[test]
    fn admit_button_garbage_collects_old_entries() {
        // Many distinct (node, action) pairs over a long timespan should not
        // grow the table unboundedly — the GC pass keeps it bounded.
        let mut motion = MotionTimerState::new();
        let t0 = Instant::now();
        for i in 0..200 {
            let node = format!("room-{}", i);
            // Spread far apart so each call advances time enough to GC the
            // previous entries.
            let t = t0 + BUTTON_DEBOUNCE_WINDOW * 20 * (i as u32 + 1);
            motion.admit_button_at(&node, ButtonAction::OnPress, None, t);
        }
        assert!(
            motion.button_debounce.len() < 50,
            "GC should keep debounce table bounded, got {}",
            motion.button_debounce.len()
        );
    }

    #[test]
    fn check_motion_timers_all_active_no_expiry() {
        let state = make_state();
        let mut motion = MotionTimerState::new();
        motion
            .sensors
            .insert("s1".into(), motion_source("s1", "room_a", None));
        motion
            .sensors
            .insert("s2".into(), motion_source("s2", "room_a", None));
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
            .insert("s1".into(), motion_source("s1", "room_a", Some(stopped_at)));
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
            .insert("s1".into(), motion_source("s1", "room_a", Some(stopped_at)));
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
            .insert("s1".into(), motion_source("s1", "room_a", Some(stopped_at)));
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
            .insert("s1".into(), motion_source("s1", "room_a", Some(stopped_at)));
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
            .insert("s1".into(), motion_source("s1", "room_a", Some(stopped_at)));
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
            .insert("s1".into(), motion_source("s1", "room_a", Some(stopped_at)));
        motion
            .sensors
            .insert("s2".into(), motion_source("s2", "room_a", None)); // still active
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
        motion.sensors.insert(
            "s1".into(),
            motion_source("s1", "room_a", Some(stopped_early)),
        );
        motion.sensors.insert(
            "s2".into(),
            motion_source("s2", "room_a", Some(stopped_late)),
        );
        motion.motion_owned.insert("room_a".into());

        check_motion_timers(&state, &mut motion);

        // Latest stop was 50s ago, timeout is 120s — should NOT expire
        assert_eq!(motion.sensors.len(), 2);
        assert!(motion.motion_owned.contains("room_a"));
    }

    // ========================================================================
    // apply_pending_motion_seeds tests (issue #38)
    // ========================================================================

    fn push_seed(state: &SharedState, source: &str, target: &str, is_active: bool) {
        state
            .lock()
            .unwrap()
            .pending_motion_seed
            .push(MotionSeedEntry {
                source_node_id: source.into(),
                target_node_id: target.into(),
                is_active,
                stopped_at_epoch_ms: None,
                motion_owned: None,
                warning_active: false,
            });
    }

    #[test]
    fn motion_timers_for_storage_sorts_and_preserves_source_fields() {
        let mut motion = MotionTimerState::new();
        motion.sensors.insert(
            "sensor_b".into(),
            MotionSourceState {
                source_node_id: "sensor_b".into(),
                target_node_id: "room_b".into(),
                stopped_at: None,
                stopped_at_epoch_ms: None,
            },
        );
        motion.sensors.insert(
            "sensor_a".into(),
            MotionSourceState {
                source_node_id: "sensor_a".into(),
                target_node_id: "room_a".into(),
                stopped_at: Some(Instant::now()),
                stopped_at_epoch_ms: Some(1_700_000_010_000),
            },
        );
        motion.motion_owned.insert("room_a".into());
        motion.warning_active.insert("room_a".into());

        let timers = motion_timers_for_storage(&motion);

        assert_eq!(timers.schema_version, 1);
        assert_eq!(
            timers
                .entries
                .iter()
                .map(|entry| entry.source_node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["sensor_a", "sensor_b"]
        );
        assert_eq!(
            timers.entries[0],
            StoredMotionTimerEntry {
                source_node_id: "sensor_a".into(),
                target_node_id: "room_a".into(),
                stopped_at_epoch_ms: Some(1_700_000_010_000),
                motion_owned: true,
                warning_active: true,
            }
        );
        assert_eq!(timers.entries[1].stopped_at_epoch_ms, None);
        assert!(!timers.entries[1].motion_owned);
        assert!(!timers.entries[1].warning_active);
    }

    #[test]
    fn motion_persistence_skips_unchanged_payloads() {
        let saved = Arc::new(Mutex::new(Vec::new()));
        let mut app = crate::state::AppState::default();
        app.storage = Some(Box::new(MotionTimerTestStorage {
            saved_motion_timers: saved.clone(),
        }));
        let state = Arc::new(Mutex::new(app));

        let mut motion = MotionTimerState::new();
        motion.sensors.insert(
            "sensor_a".into(),
            MotionSourceState {
                source_node_id: "sensor_a".into(),
                target_node_id: "room_a".into(),
                stopped_at: None,
                stopped_at_epoch_ms: None,
            },
        );

        let mut persistence = MotionTimerPersistence::new();
        persistence.mark_dirty();
        persistence.persist_now(&state, &motion);
        persistence.mark_dirty();
        persistence.persist_now(&state, &motion);

        assert_eq!(saved.lock().unwrap().len(), 1);
    }

    #[test]
    fn motion_persistence_waits_for_coalesce_interval() {
        let saved = Arc::new(Mutex::new(Vec::new()));
        let mut app = crate::state::AppState::default();
        app.storage = Some(Box::new(MotionTimerTestStorage {
            saved_motion_timers: saved.clone(),
        }));
        let state = Arc::new(Mutex::new(app));

        let mut motion = MotionTimerState::new();
        motion.sensors.insert(
            "sensor_a".into(),
            MotionSourceState {
                source_node_id: "sensor_a".into(),
                target_node_id: "room_a".into(),
                stopped_at: None,
                stopped_at_epoch_ms: None,
            },
        );

        let mut persistence = MotionTimerPersistence::new();
        persistence.last_attempt_at = Some(Instant::now());
        persistence.mark_dirty();
        persistence.persist_if_due(&state, &motion);

        assert!(saved.lock().unwrap().is_empty());
        assert!(persistence.dirty);
    }

    #[test]
    fn request_motion_timer_persist_returns_immediately_when_no_motion_state_exists() {
        let state = Arc::new(Mutex::new(crate::state::AppState::default()));

        assert!(request_motion_timer_persist(
            &state,
            Duration::from_millis(1)
        ));
        assert!(state
            .lock()
            .unwrap()
            .pending_motion_timer_persist_acks
            .is_empty());
    }

    #[test]
    fn request_motion_timer_persist_waits_for_event_loop_ack_when_snapshots_exist() {
        let mut app = crate::state::AppState::default();
        app.motion_snapshots.insert(
            "room_a".to_string(),
            MotionSnapshot {
                motion_active: false,
                motion_owned: true,
                remaining_secs: Some(30),
                timeout_secs: 60,
                warning_active: false,
            },
        );
        let state = Arc::new(Mutex::new(app));
        let request_state = state.clone();

        let handle = std::thread::spawn(move || {
            request_motion_timer_persist(&request_state, Duration::from_secs(1))
        });

        let mut ack = None;
        for _ in 0..100 {
            ack = state
                .lock()
                .unwrap()
                .pending_motion_timer_persist_acks
                .pop();
            if ack.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        ack.expect("queued motion persist ack").send(()).unwrap();

        assert!(handle.join().unwrap());
    }

    fn set_observed_lights_on_with_source(
        state: &SharedState,
        target: &str,
        on: bool,
        source: crate::state::ObservedPowerSource,
    ) {
        let mut s = state.lock().unwrap();
        s.room_observed_power.insert(
            target.into(),
            crate::state::ObservedPowerState::new(on, source),
        );
    }

    fn set_observed_lights_on(state: &SharedState, target: &str, on: bool) {
        set_observed_lights_on_with_source(
            state,
            target,
            on,
            crate::state::ObservedPowerSource::SyncPoll,
        );
    }

    /// Inactive sensor whose target room still has lights on after a restart
    /// must seed with `stopped_at = Some(now)` and claim ownership so the
    /// motion timeout fires the auto-off (issue #38).
    #[test]
    fn apply_seeds_inactive_sensor_with_lights_on_seeds_owned_countdown() {
        let state = make_state();
        set_observed_lights_on(&state, "room_a", true);
        push_seed(&state, "sensor_1", "room_a", false);

        let mut motion = MotionTimerState::new();
        let now = Instant::now();
        let dirty = apply_pending_motion_seeds(&state, &mut motion, now);

        assert!(dirty, "applying a non-empty seed should mark dirty");
        let source = motion
            .sensors
            .get("sensor_1")
            .expect("seed should land in motion state");
        assert_eq!(source.target_node_id, "room_a");
        assert_eq!(
            source.stopped_at,
            Some(now),
            "inactive sensor must start the countdown from boot"
        );
        assert!(
            motion.motion_owned.contains("room_a"),
            "lights-on rooms must be claimed so OffPress fires after the timeout"
        );
        assert!(
            state.lock().unwrap().pending_motion_seed.is_empty(),
            "applying seeds must drain the queue"
        );
    }

    #[test]
    fn apply_seeds_preserves_persisted_inactive_countdown() {
        let state = make_state();
        let stopped_at_epoch_ms = crate::state::current_epoch_ms().saturating_sub(85_000);
        state
            .lock()
            .unwrap()
            .pending_motion_seed
            .push(MotionSeedEntry {
                source_node_id: "sensor_1".into(),
                target_node_id: "room_a".into(),
                is_active: false,
                stopped_at_epoch_ms: Some(stopped_at_epoch_ms),
                motion_owned: Some(true),
                warning_active: false,
            });

        let mut motion = MotionTimerState::new();
        let dirty = apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(dirty);
        assert!(motion.motion_owned.contains("room_a"));
        let snapshots = motion.snapshots(&HashMap::from([("room_a".to_string(), 120)]), 300);
        let remaining = snapshots["room_a"]
            .remaining_secs
            .expect("inactive seed should be counting down");
        assert!(
            (28..=38).contains(&remaining),
            "remaining_secs should preserve elapsed time, got {}",
            remaining
        );
    }

    #[test]
    fn apply_active_seed_keeps_motion_owned_invariant() {
        let state = make_state();
        state
            .lock()
            .unwrap()
            .pending_motion_seed
            .push(MotionSeedEntry {
                source_node_id: "sensor_1".into(),
                target_node_id: "room_a".into(),
                is_active: true,
                stopped_at_epoch_ms: None,
                motion_owned: Some(false),
                warning_active: true,
            });

        let mut motion = MotionTimerState::new();
        let dirty = apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(dirty);
        assert!(motion.motion_owned.contains("room_a"));
        assert!(!motion.warning_active.contains("room_a"));
        assert_eq!(motion.sensors["sensor_1"].stopped_at, None);
        assert_eq!(motion.sensors["sensor_1"].stopped_at_epoch_ms, None);
    }

    #[test]
    fn apply_seed_drops_stale_persisted_countdown() {
        let state = make_state();
        {
            let mut s = state.lock().unwrap();
            s.default_motion_timeout_secs = 120;
            s.pending_motion_seed.push(MotionSeedEntry {
                source_node_id: "sensor_1".into(),
                target_node_id: "room_a".into(),
                is_active: false,
                stopped_at_epoch_ms: Some(
                    crate::state::current_epoch_ms().saturating_sub(10 * 60 * 1_000),
                ),
                motion_owned: Some(true),
                warning_active: true,
            });
        }
        set_observed_lights_on(&state, "room_a", true);

        let mut motion = MotionTimerState::new();
        let dirty = apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(dirty);
        assert!(motion.motion_owned.contains("room_a"));
        assert!(!motion.warning_active.contains("room_a"));
        let snapshots = motion.snapshots(&HashMap::from([("room_a".to_string(), 120)]), 300);
        let remaining = snapshots["room_a"]
            .remaining_secs
            .expect("inactive seed should be counting down");
        assert!(
            remaining >= 115,
            "stale restored countdown should restart near full timeout, got {}",
            remaining
        );
    }

    /// Inactive sensor for a dark room: still tracked so future motion-stop
    /// events aren't ignored, but ownership is not claimed (no manual lights
    /// to surprise-off).
    #[test]
    fn apply_seeds_inactive_sensor_with_lights_off_does_not_claim_owned() {
        let state = make_state();
        set_observed_lights_on(&state, "room_a", false);
        push_seed(&state, "sensor_1", "room_a", false);

        let mut motion = MotionTimerState::new();
        apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(motion.sensors.contains_key("sensor_1"));
        assert!(
            !motion.motion_owned.contains("room_a"),
            "dark rooms must not be claimed at boot — no lights to turn off"
        );
    }

    /// Inactive sensors must wait until the initial observed-power poll has
    /// landed; otherwise lit rooms can be mistaken for dark rooms at startup.
    #[test]
    fn apply_seeds_inactive_sensor_without_observed_power_defers() {
        let state = make_state();
        push_seed(&state, "sensor_1", "room_a", false);

        let mut motion = MotionTimerState::new();
        let dirty_before_poll = apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(
            !dirty_before_poll,
            "unknown observed power should defer inactive seeds"
        );
        assert!(
            motion.sensors.is_empty(),
            "deferred seed must not land in live motion state yet"
        );
        assert_eq!(
            state.lock().unwrap().pending_motion_seed.len(),
            1,
            "deferred seed should stay queued for a later event-loop pass"
        );

        set_observed_lights_on(&state, "room_a", true);
        let now_after_poll = Instant::now();
        let dirty_after_poll = apply_pending_motion_seeds(&state, &mut motion, now_after_poll);

        assert!(dirty_after_poll);
        assert!(state.lock().unwrap().pending_motion_seed.is_empty());
        let source = motion.sensors.get("sensor_1").unwrap();
        assert_eq!(source.stopped_at, Some(now_after_poll));
        assert!(motion.motion_owned.contains("room_a"));
    }

    /// Active sensor at boot keeps the room on (no countdown started yet)
    /// and is claimed for ownership, matching the live motion path.
    #[test]
    fn apply_seeds_active_sensor_seeds_with_no_countdown_and_owned() {
        let state = make_state();
        push_seed(&state, "sensor_1", "room_a", true);

        let mut motion = MotionTimerState::new();
        apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        let source = motion.sensors.get("sensor_1").unwrap();
        assert_eq!(source.stopped_at, None);
        assert!(motion.motion_owned.contains("room_a"));
    }

    /// A stale startup prefetch must not overwrite a source that already
    /// received live event-stream state.
    #[test]
    fn apply_seeds_does_not_overwrite_live_motion_source() {
        let state = make_state();
        set_observed_lights_on(&state, "room_a", true);
        push_seed(&state, "sensor_1", "room_a", false);

        let mut motion = MotionTimerState::new();
        motion
            .sensors
            .insert("sensor_1".into(), motion_source("sensor_1", "room_b", None));
        motion.motion_owned.insert("room_b".into());

        let dirty = apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(
            !dirty,
            "skipping a stale seed over live state should not mark motion dirty"
        );
        let source = motion.sensors.get("sensor_1").unwrap();
        assert_eq!(source.target_node_id, "room_b");
        assert_eq!(source.stopped_at, None);
        assert!(motion.motion_owned.contains("room_b"));
        assert!(
            !motion.motion_owned.contains("room_a"),
            "stale seed must not claim a new target"
        );
        assert!(state.lock().unwrap().pending_motion_seed.is_empty());
    }

    /// Empty queue is a no-op — the loop calls this every iteration and
    /// must not pretend work was done.
    #[test]
    fn apply_seeds_empty_queue_is_noop() {
        let state = make_state();
        let mut motion = MotionTimerState::new();
        let dirty = apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(!dirty);
        assert!(motion.sensors.is_empty());
    }

    /// Seeds appended after the loop has already started (the real-world
    /// flow: hub bootstrap completes asynchronously) must be applied on the
    /// next iteration, not silently dropped. Pre-fix this regressed because
    /// the seed drain ran exactly once before the loop began.
    #[test]
    fn apply_seeds_picks_up_seeds_appended_after_first_call() {
        let state = make_state();
        let mut motion = MotionTimerState::new();

        // First iteration: queue empty (loop starts before hub bootstrap).
        let dirty_first = apply_pending_motion_seeds(&state, &mut motion, Instant::now());
        assert!(!dirty_first);

        // Hub bootstrap finishes and populates the queue.
        set_observed_lights_on(&state, "room_a", true);
        push_seed(&state, "sensor_1", "room_a", false);

        // Next iteration must apply the seed.
        let dirty_second = apply_pending_motion_seeds(&state, &mut motion, Instant::now());
        assert!(
            dirty_second,
            "seeds appended after the loop started must still be applied"
        );
        assert!(motion.sensors.contains_key("sensor_1"));
        assert!(motion.motion_owned.contains("room_a"));
    }

    fn make_state_with_room_snapshot(snapshot: RoomSnapshot) -> SharedState {
        let mut app = crate::state::AppState::default();
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(MotionTestRuntime {
            snapshots: vec![snapshot],
            any_lights_on_calls: None,
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

    fn room_snapshot_with_flags(id: &str, soft_off: bool, hard_off: bool) -> RoomSnapshot {
        RoomSnapshot {
            id: id.into(),
            name: id.into(),
            kind: rhythm_core::LightNodeKind::Room,
            parent_id: None,
            rhythm_enabled: true,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off,
            hard_off,
            profile_settings: RoomProfileSettings::default(),
        }
    }

    fn hard_off_room_snapshot(id: &str) -> RoomSnapshot {
        room_snapshot_with_flags(id, false, true)
    }

    fn soft_off_room_snapshot(id: &str) -> RoomSnapshot {
        room_snapshot_with_flags(id, true, false)
    }

    /// Boot-time motion seeding must skip rooms persisted as hard_off.
    /// Otherwise a motion timer starts for a room the user has explicitly
    /// switched off, and the timer survives across power cycles even though
    /// live mutations clear it via queue_motion_timer_clear (issue #53).
    #[test]
    fn apply_seeds_skips_hard_off_room_with_lights_on() {
        let state = make_state_with_room_snapshot(hard_off_room_snapshot("room_a"));
        // Without the hard_off gate, lights_on=true would force the seed to
        // claim ownership and start a countdown. The gate must drop it first.
        set_observed_lights_on(&state, "room_a", true);
        push_seed(&state, "sensor_1", "room_a", false);

        let mut motion = MotionTimerState::new();
        let dirty = apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(
            !dirty,
            "dropping a hard_off seed must not be reported as a state change"
        );
        assert!(
            motion.sensors.is_empty(),
            "hard_off room must not get a motion sensor entry"
        );
        assert!(
            motion.motion_owned.is_empty(),
            "hard_off room must not be claimed for motion-driven auto-off"
        );
        assert!(
            state.lock().unwrap().pending_motion_seed.is_empty(),
            "hard_off seeds must be dropped, not deferred"
        );
    }

    /// Inactive startup motion prefetch must not turn a persisted Idle room
    /// into a motion-owned countdown. Idle reports semantic lights_on=true for
    /// display purposes, but that does not mean motion owns the room (#58/#59).
    #[test]
    fn apply_seeds_skips_inactive_soft_off_room_with_semantic_lights_on() {
        let state = make_state_with_room_snapshot(soft_off_room_snapshot("room_a"));
        set_observed_lights_on_with_source(
            &state,
            "room_a",
            true,
            crate::state::ObservedPowerSource::SemanticOverride,
        );
        push_seed(&state, "sensor_1", "room_a", false);

        let mut motion = MotionTimerState::new();
        let dirty = apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(
            !dirty,
            "dropping an inactive soft_off seed must not report a state change"
        );
        assert!(
            motion.sensors.is_empty(),
            "idle room must not get a startup motion countdown"
        );
        assert!(
            motion.motion_owned.is_empty(),
            "idle room must not be claimed for motion-driven auto-off"
        );
        assert!(
            state.lock().unwrap().pending_motion_seed.is_empty(),
            "soft_off seeds must be dropped, not deferred"
        );
    }

    /// Active motion sensor on a hard_off room is also dropped — the user
    /// has overridden adaptive control for the room, so motion shouldn't
    /// re-engage timers on boot.
    #[test]
    fn apply_seeds_skips_hard_off_room_with_active_motion() {
        let state = make_state_with_room_snapshot(hard_off_room_snapshot("room_a"));
        push_seed(&state, "sensor_1", "room_a", true);

        let mut motion = MotionTimerState::new();
        apply_pending_motion_seeds(&state, &mut motion, Instant::now());

        assert!(motion.sensors.is_empty());
        assert!(motion.motion_owned.is_empty());
        assert!(state.lock().unwrap().pending_motion_seed.is_empty());
    }
}
