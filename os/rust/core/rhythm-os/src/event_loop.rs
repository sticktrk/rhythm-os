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
use crate::logging;
use crate::state::{MotionSnapshot, SharedState, WorkItem};
use crate::topology::NodeControlKind;

/// How many seconds before timeout to start the warning dim.
pub const WARNING_BEFORE_SECS: u64 = 60;

/// Brightness multiplier during warning dim (50% of adaptive).
pub const WARNING_DIM_FACTOR: f32 = 0.5;

/// Rate-limit reconnect-triggered full hub resyncs during SSE flapping.
const RECONNECT_RESYNC_COOLDOWN: Duration = Duration::from_secs(120);

#[derive(Clone, Debug)]
pub struct MotionSourceState {
    /// Public source node ID when known, otherwise a hub-scoped synthetic key.
    pub source_node_id: String,
    /// Public target node ID currently controlled by this source.
    pub target_node_id: String,
    /// `None` while motion is active, `Some(stopped_at)` once the source clears.
    pub stopped_at: Option<Instant>,
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
                    debug!(
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
                    debug!(
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

/// Translate a hub-native room/device ID to the public topology node ID if available.
///
/// Used at the event boundary so all downstream processing uses public node
/// IDs consistently.
///
/// Tries hub-key-specific lookup first, then falls back to searching all
/// hubs (for events where hub_key is None, e.g., button_resolve).
fn resolve_public_node_id(
    state: &SharedState,
    hub_key: Option<&crate::canonical::identity::HubKey>,
    node_id: &str,
) -> String {
    state
        .lock()
        .ok()
        .and_then(|s| {
            s.topology
                .resolve_room_alias(&s.canonical_registry, hub_key, node_id)
        })
        .unwrap_or_else(|| node_id.to_string())
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

fn should_run_reconnect_resync(
    state: &SharedState,
    hub_key: &crate::canonical::identity::HubKey,
) -> bool {
    let Ok(mut s) = state.lock() else {
        return true;
    };

    if s.reconnect_sync_recently_ran(hub_key, RECONNECT_RESYNC_COOLDOWN) {
        return false;
    }

    s.note_hub_reconnect_sync(hub_key);
    true
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
                    if should_run_reconnect_resync(state, key) {
                        spawn_reconnect_sync(state, key);
                    } else {
                        info!(
                            target: "conn",
                            "Hub {} reconnected within {}s; skipping full resync",
                            key,
                            RECONNECT_RESYNC_COOLDOWN.as_secs()
                        );
                        spawn_light_state_poll(state, key);
                    }
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
            let node_id = resolve_public_node_id(state, hub_key.as_ref(), room_id);
            let command_id = logging::next_command_id("button");
            tracing::info!(
                target: "evt",
                event = "button_ingress",
                command_id = %command_id,
                action = ?action,
                node_id = %node_id,
                source_room_id = %room_id,
                device_id = ?device_id.as_deref(),
                hub_present = hub_key.is_some(),
                "Button event received"
            );
            motion.motion_owned.remove(&node_id);
            motion.warning_active.remove(&node_id);
            motion
                .sensors
                .retain(|_, source| source.target_node_id != node_id);

            if process_button_inline(state, &node_id, action, device_id.as_deref(), &command_id) {
                #[cfg(feature = "desktop")]
                {
                    let runtime = {
                        let Ok(s) = state.lock() else { return };
                        s.hub_runtime()
                    };
                    if let Some(runtime) = runtime {
                        crate::commands::emit_node_state_event_after_apply(
                            state, &runtime, &node_id,
                        );
                    }
                }

                let work_tx = {
                    let Ok(s) = state.lock() else { return };
                    s.work_tx.clone()
                };
                if let Some(tx) = work_tx {
                    let _ = tx.try_send(WorkItem::DeferredPersist {
                        node_id: node_id.clone(),
                    });
                } else {
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
            let Some((source_node_id, target_node_id)) = commands::resolve_node_control_target(
                state,
                hub_key.as_ref(),
                sensor_id,
                room_id,
                &NodeControlKind::Motion,
            ) else {
                info!(
                    target: "evt",
                    "Motion: sensor {} has no control target, ignoring",
                    sensor_id
                );
                return;
            };
            if detected {
                if motion.warning_active.remove(&target_node_id) {
                    info!(
                        target: "evt",
                        "Motion: restoring full brightness in node {} (was warning-dimmed)",
                        target_node_id
                    );
                    dim_node_inline(state, &target_node_id, 1.0);
                }

                let is_new_target = !motion.has_sources_for_target(&target_node_id);
                motion.sensors.insert(
                    source_node_id.clone(),
                    MotionSourceState {
                        source_node_id: source_node_id.clone(),
                        target_node_id: target_node_id.clone(),
                        stopped_at: None,
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

                    turn_on_node_inline(state, &target_node_id);
                } else {
                    info!(
                        target: "evt",
                        "Motion: continued/refreshed source {} -> target {}",
                        source_node_id,
                        target_node_id
                    );
                }
            } else {
                if let Some(source) = motion.sensors.get_mut(&source_node_id) {
                    source.target_node_id = target_node_id.clone();
                    source.stopped_at = Some(Instant::now());

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

        HubEvent::UnroutableButton {
            ref hub_key,
            ref device_id,
            ref button_id,
        } => {
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
                #[cfg(feature = "desktop")]
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
    debug!(
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
                dim_node_inline(state, target_node_id, WARNING_DIM_FACTOR);
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
            if let Some(tx) = work_tx {
                if tx
                    .try_send(WorkItem::ButtonAction {
                        command_id: command_id.clone(),
                        node_id: target_node_id.clone(),
                        action: ButtonAction::OffPress,
                        device_id: None,
                    })
                    .is_err()
                {
                    tracing::warn!(
                        target: "evt",
                        event = "motion_timeout_queue_full",
                        command_id = %command_id,
                        node_id = %target_node_id,
                        "Motion timeout queue full, applying inline"
                    );
                    process_button_inline(
                        state,
                        target_node_id,
                        ButtonAction::OffPress,
                        None,
                        &command_id,
                    );
                }
            } else {
                process_button_inline(
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
    let mut motion_tick: u32 = 0;
    let mut motion_dirty = false;

    // Seed motion state from startup prefetch of binary_sensor states.
    if let Ok(mut s) = state.lock() {
        if !s.pending_motion_seed.is_empty() {
            let seeds = std::mem::take(&mut s.pending_motion_seed);
            for (source_node_id, target_node_id) in seeds {
                motion_state.sensors.insert(
                    source_node_id.clone(),
                    MotionSourceState {
                        source_node_id,
                        target_node_id: target_node_id.clone(),
                        stopped_at: None,
                    },
                );
                motion_state.motion_owned.insert(target_node_id);
            }
            motion_dirty = true;
            info!(
                target: "evt",
                "Motion: seeded {} active sources from startup prefetch",
                motion_state.sensors.len()
            );
        }
    }

    loop {
        // Pick up new hub event receivers from reconfiguration
        if let Ok(mut s) = state.lock() {
            if !s.pending_hub_event_rxs.is_empty() {
                let new_rxs = std::mem::take(&mut s.pending_hub_event_rxs);
                hub_event_rxs.extend(new_rxs);
                motion_state = MotionTimerState::new();
            }

            if !s.pending_motion_clear.is_empty() {
                let target_node_ids = std::mem::take(&mut s.pending_motion_clear);
                drop(s);
                for target_node_id in &target_node_ids {
                    motion_state
                        .sensors
                        .retain(|_, source| &source.target_node_id != target_node_id);
                    motion_state.motion_owned.remove(target_node_id);
                    motion_state.warning_active.remove(target_node_id);
                }
                if !target_node_ids.is_empty() {
                    info!(
                        target: "evt",
                        "Motion: cleared timers for {} targets",
                        target_node_ids.len()
                    );
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
        WorkItem::ButtonAction {
            command_id,
            node_id,
            action,
            device_id,
        } => {
            let started = Instant::now();
            let (runtime, has_hub) = {
                let Ok(s) = state.lock() else {
                    tracing::warn!(
                        target: "evt",
                        event = "button_action_dropped",
                        command_id = %command_id,
                        action = ?action,
                        node_id = %node_id,
                        device_id = ?device_id.as_deref(),
                        source = "worker",
                        reason = "state_lock_poisoned",
                        "Worker button action dropped"
                    );
                    return;
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
                    device_id = ?device_id.as_deref(),
                    source = "worker",
                    has_hub,
                    reason = "no_runtime",
                    "Worker button action dropped"
                );
                return;
            };

            let event = if let Some(ref dev_id) = device_id {
                InputEvent::with_device(&node_id, action, dev_id)
            } else {
                InputEvent::new(&node_id, action)
            };

            match runtime.handle_event(&event) {
                Ok(turned_on) => {
                    tracing::info!(
                        target: "evt",
                        event = "button_action_applied",
                        command_id = %command_id,
                        action = ?action,
                        node_id = %node_id,
                        device_id = ?device_id.as_deref(),
                        source = "worker",
                        turned_on,
                        latency_ms = started.elapsed().as_millis(),
                        "Worker button action applied"
                    );
                    crate::commands::sync_active_mode_from_runtime(state, &runtime);
                    if let Ok(mut s) = state.lock() {
                        if s.room_mode_transitions.remove(&node_id).is_some() {
                            debug!(
                                target: "evt",
                                "Worker: cleared mode transition for node '{}'",
                                node_id
                            );
                        }
                    }
                    crate::commands::update_lights_on_cache_for_runtime_node(
                        state, &runtime, &node_id, turned_on,
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "evt",
                        event = "button_action_failed",
                        command_id = %command_id,
                        action = ?action,
                        node_id = %node_id,
                        device_id = ?device_id.as_deref(),
                        source = "worker",
                        latency_ms = started.elapsed().as_millis(),
                        error = %e,
                        "Worker button action failed"
                    );
                }
            }
        }
        WorkItem::ApplyNodeCommand { node_id, command } => {
            let runtime = {
                let Ok(s) = state.lock() else { return };
                s.hub_runtime()
            };
            let Some(runtime) = runtime else { return };

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

            #[cfg(feature = "desktop")]
            crate::commands::emit_node_state_event_after_apply(state, &runtime, &node_id);
        }
        WorkItem::PeriodicNodeTick {
            command_id,
            node_id,
            settings_node_id,
            current_hour,
            emit_parent_node_id,
        } => {
            let current_hour = state
                .lock()
                .ok()
                .and_then(|mut s| s.pending_periodic_ticks.remove(&node_id))
                .unwrap_or(current_hour);

            let runtime = {
                let Ok(s) = state.lock() else { return };
                s.hub_runtime()
            };
            let Some(runtime) = runtime else { return };

            if runtime.engine_node_snapshot(&node_id).is_none()
                || runtime.engine_node_snapshot(&settings_node_id).is_none()
            {
                tracing::debug!(
                    target: "sys",
                    event = "periodic_node_tick_skipped",
                    command_id = %command_id,
                    node_id = %node_id,
                    settings_node_id = %settings_node_id,
                    reason = "stale",
                    "Skipping stale periodic tick"
                );
                return;
            }

            if let Err(e) = runtime.periodic_tick_node(&node_id, &settings_node_id, current_hour) {
                tracing::warn!(
                    target: "sys",
                    event = "periodic_node_tick_failed",
                    command_id = %command_id,
                    node_id = %node_id,
                    settings_node_id = %settings_node_id,
                    current_hour,
                    error = %e,
                    "Periodic node tick failed"
                );
            }

            crate::periodic::post_tick_node(state, &runtime, &settings_node_id);
            if let Some(parent_node_id) = emit_parent_node_id {
                if parent_node_id != settings_node_id {
                    crate::periodic::post_tick_node(state, &runtime, &parent_node_id);
                }
            }
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

    use crate::canonical::identity::HubKey;
    use crate::hub::{ActiveHub, HubType};
    use crate::registry::{RegistrySnapshot, SnapshotRoom};

    fn motion_source(
        source_node_id: &str,
        target_node_id: &str,
        stopped_at: Option<Instant>,
    ) -> MotionSourceState {
        MotionSourceState {
            source_node_id: source_node_id.to_string(),
            target_node_id: target_node_id.to_string(),
            stopped_at,
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

    // ========================================================================
    // check_motion_timers tests
    // ========================================================================

    fn make_state() -> SharedState {
        std::sync::Arc::new(std::sync::Mutex::new(crate::state::AppState::default()))
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

        let after = after.expect("connected event should remap the room to a topology node id");
        let after_room = after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == "Room A" && node["kind"] == "room")
            .unwrap();
        assert_ne!(after_room["id"], before_room_id);
    }

    #[test]
    fn rapid_reconnect_skips_full_resync_but_polls_light_state() {
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
            "first reconnect sync should poll light state"
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
}
