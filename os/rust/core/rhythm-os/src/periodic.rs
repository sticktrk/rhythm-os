//! Periodic room tick orchestration.
//!
//! Shared across platform binaries so the same periodic update logic can be
//! reused across the server, appliance, and future targets.
//!
//! The periodic updater runs on a fixed interval, ticking each room's
//! engine to update light values based on the current time/solar position.
//!
//! Both `run_periodic_loop` and `check_solar_midnight` use blocking
//! `std::thread::sleep` and `SystemTimeProvider`. On rhythm-server,
//! this runs inside `tokio::task::spawn_blocking()`.

use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::Duration;
use std::time::Instant;

use log::{debug, info, warn};
use rhythm_core::{SystemTimeProvider, TimeProvider};

use std::sync::Arc;

use rhythm_core::{runtime::RuntimeHandle, RestoredNodeState};

use crate::logging;
use crate::state::WorkItem;
use crate::state::{AppState, SharedState};
use crate::storage::StoredLocation;

pub(crate) fn stable_room_phase_key(room_id: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    room_id.as_bytes().iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

pub(crate) fn dispatch_spacing(cycle_duration: Duration, room_count: usize) -> Duration {
    if room_count == 0 {
        Duration::ZERO
    } else {
        cycle_duration
            .checked_div(room_count as u32)
            .unwrap_or(Duration::ZERO)
    }
}

/// Threshold above which a single periodic tick is considered "slow" and
/// warrants a warning log. A periodic tick that exceeds this on the rpiz
/// (single-core 1 GHz ARM) is the leading indicator that a hub controller
/// is hung — once enough ticks pile up, the periodic queue fills and
/// downstream rooms stop refreshing.
pub const SLOW_TICK_WARNING_THRESHOLD: Duration = Duration::from_secs(5);

/// Tolerance for reconciling wall-clock movement against monotonic elapsed time.
///
/// This intentionally allows civil-time adjustments like DST while still
/// rejecting large discontinuities from cold-boot clocks or manual time steps.
const PERIODIC_TIME_DISCONTINUITY_TOLERANCE: Duration = Duration::from_secs(20 * 60);

/// Classify the outcome of a single periodic tick by elapsed time. Pure so
/// the latency-watchdog logic can be tested without a real engine call.
#[derive(Debug, PartialEq, Eq)]
pub enum TickLatencyOutcome {
    /// Completed under the threshold — no log.
    OnTime,
    /// Exceeded the threshold — emit a warning.
    Slow,
}

/// Classify a tick's elapsed duration against [`SLOW_TICK_WARNING_THRESHOLD`].
pub fn classify_tick_latency(elapsed: Duration) -> TickLatencyOutcome {
    if elapsed >= SLOW_TICK_WARNING_THRESHOLD {
        TickLatencyOutcome::Slow
    } else {
        TickLatencyOutcome::OnTime
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PeriodicTimeCheckResult {
    Seeded,
    Continuous {
        last_hour: f32,
    },
    Discontinuous {
        last_hour: f32,
        adjusted_delta_hours: f32,
        expected_delta_hours: f32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeriodicDispatchNode {
    pub(crate) node_id: String,
    pub(crate) settings_node_id: String,
    pub(crate) emit_node_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PeriodicDispatchSummary {
    eligible_node_count: usize,
    eligible_room_count: usize,
    eligible_device_count: usize,
    dispatch_node_count: usize,
    dispatched_settings_node_count: usize,
    no_dispatch_node_count: usize,
}

fn last_periodic_node_index_by_emit_target(
    nodes: &[PeriodicDispatchNode],
) -> HashMap<String, usize> {
    let mut indices = HashMap::new();
    for (idx, node) in nodes.iter().enumerate() {
        indices.insert(node.emit_node_id.clone(), idx);
    }
    indices
}

fn periodic_settings_node_id(
    snapshot: &rhythm_core::NodeSnapshot,
    collapse_attached_light_nodes: bool,
) -> &str {
    if collapse_attached_light_nodes {
        snapshot.parent_id.as_deref().unwrap_or(&snapshot.id)
    } else {
        &snapshot.id
    }
}

fn periodic_settings_node_kind(
    snapshot: &rhythm_core::NodeSnapshot,
    collapse_attached_light_nodes: bool,
) -> rhythm_core::LightNodeKind {
    if collapse_attached_light_nodes && snapshot.parent_id.is_some() {
        rhythm_core::LightNodeKind::Room
    } else {
        snapshot.kind
    }
}

pub(crate) fn periodic_dispatch_nodes_from_state(
    state: &AppState,
    room_snapshots: &[rhythm_core::NodeSnapshot],
) -> Vec<PeriodicDispatchNode> {
    let has_composite_controller = state.composite_controller.is_some();
    let mut eligible_room_ids: Vec<String> = room_snapshots
        .iter()
        .filter(|node| node.kind.is_light_addressable())
        .map(|node| periodic_settings_node_id(node, has_composite_controller).to_string())
        .collect();
    eligible_room_ids.sort();
    eligible_room_ids.dedup();

    if !has_composite_controller {
        return eligible_room_ids
            .into_iter()
            .map(|room_id| PeriodicDispatchNode {
                node_id: room_id.clone(),
                settings_node_id: room_id.clone(),
                emit_node_id: room_id,
            })
            .collect();
    }

    let eligible_lookup: HashSet<String> = eligible_room_ids.iter().cloned().collect();
    let mut derived_by_room: HashMap<String, Vec<crate::topology::TopologyLightNode>> =
        HashMap::new();
    for node in state
        .topology
        .periodic_light_nodes(&state.canonical_registry)
    {
        if eligible_lookup.contains(&node.source_node_id) {
            derived_by_room
                .entry(node.source_node_id.clone())
                .or_default()
                .push(node);
        }
    }

    let mut dispatch_nodes = Vec::new();
    for room_id in eligible_room_ids {
        match derived_by_room.remove(&room_id) {
            Some(mut nodes) => {
                nodes.sort_by(|left, right| left.id.cmp(&right.id));
                dispatch_nodes.extend(nodes.into_iter().map(|node| PeriodicDispatchNode {
                    node_id: node.id,
                    settings_node_id: room_id.clone(),
                    emit_node_id: node.emit_node_id,
                }));
            }
            None if state.topology.get(&room_id).is_none() => {
                dispatch_nodes.push(PeriodicDispatchNode {
                    node_id: room_id.clone(),
                    settings_node_id: room_id.clone(),
                    emit_node_id: room_id.clone(),
                });
            }
            None => {}
        }
    }

    dispatch_nodes
}

pub(crate) fn preview_dispatch_node_for_target(
    target_snap: &rhythm_core::NodeSnapshot,
) -> Option<PeriodicDispatchNode> {
    if !target_snap.kind.is_light_addressable() {
        return None;
    }

    // Manual/API preview dispatch preserves the explicit target. Topology
    // fan-out belongs to the periodic scheduler, not to single-target edits.
    Some(PeriodicDispatchNode {
        node_id: target_snap.id.clone(),
        settings_node_id: target_snap.id.clone(),
        emit_node_id: target_snap
            .parent_id
            .clone()
            .unwrap_or_else(|| target_snap.id.clone()),
    })
}

fn summarize_periodic_dispatch(
    node_snapshots: &[rhythm_core::NodeSnapshot],
    dispatch_nodes: &[PeriodicDispatchNode],
    collapse_attached_light_nodes: bool,
) -> PeriodicDispatchSummary {
    let mut eligible_settings_node_kinds: HashMap<&str, rhythm_core::LightNodeKind> =
        HashMap::new();
    for node in node_snapshots
        .iter()
        .filter(|node| node.kind.is_light_addressable())
    {
        eligible_settings_node_kinds
            .entry(periodic_settings_node_id(
                node,
                collapse_attached_light_nodes,
            ))
            .or_insert_with(|| periodic_settings_node_kind(node, collapse_attached_light_nodes));
    }

    let eligible_node_count = eligible_settings_node_kinds.len();
    let eligible_room_count = eligible_settings_node_kinds
        .values()
        .filter(|kind| kind.is_room())
        .count();
    let eligible_device_count = eligible_node_count.saturating_sub(eligible_room_count);

    let dispatched_settings_node_ids: HashSet<&str> = dispatch_nodes
        .iter()
        .map(|node| node.settings_node_id.as_str())
        .collect();
    let no_dispatch_node_count = eligible_settings_node_kinds
        .keys()
        .filter(|node_id| !dispatched_settings_node_ids.contains(**node_id))
        .count();

    PeriodicDispatchSummary {
        eligible_node_count,
        eligible_room_count,
        eligible_device_count,
        dispatch_node_count: dispatch_nodes.len(),
        dispatched_settings_node_count: dispatched_settings_node_ids.len(),
        no_dispatch_node_count,
    }
}

fn periodic_room_state(
    room: &rhythm_core::NodeSnapshot,
    power_save: bool,
) -> Option<rhythm_core::RoomModeState> {
    if !room.rhythm_enabled || room.hard_off {
        return None;
    }

    Some(if room.soft_off && !power_save {
        rhythm_core::RoomModeState::Idle
    } else {
        rhythm_core::RoomModeState::Active
    })
}

pub(crate) fn effective_cycle_duration(
    profile_registry: &rhythm_core::LightProfileRegistry,
    ctx: &rhythm_core::CurveContext,
    room_snapshots: &[rhythm_core::NodeSnapshot],
    update_interval: Duration,
    power_save: bool,
) -> Duration {
    let fallback_values = profile_registry.active_profile().calculate(ctx);
    let fallback_secs = fallback_values
        .suggested_tick_interval_secs
        .map(u64::from)
        .unwrap_or(update_interval.as_secs());

    let mut room_suggestions = Vec::new();
    for room in room_snapshots {
        let Some(room_state) = periodic_room_state(room, power_save) else {
            continue;
        };
        let room_ctx = ctx.with_offset(room.time_offset_minutes);
        let suggested_secs = profile_registry
            .profile_for_room_state(
                profile_registry.active_mode(),
                room_state,
                Some(&room.profile_settings),
            )
            .calculate(&room_ctx)
            .suggested_tick_interval_secs
            .map(u64::from);
        room_suggestions.push((
            room.id.clone(),
            room_state,
            room_ctx.current_hour,
            room.time_offset_minutes,
            suggested_secs,
        ));
    }

    let suggested_secs = room_suggestions
        .iter()
        .filter_map(|(_, _, _, _, suggested_secs)| *suggested_secs)
        .min()
        .unwrap_or(fallback_secs);

    let chosen_secs = suggested_secs.max(update_interval.as_secs());

    if log::log_enabled!(log::Level::Debug) {
        let nodes = if room_suggestions.is_empty() {
            "none".to_string()
        } else {
            room_suggestions
                .iter()
                .map(
                    |(room_id, room_state, room_hour, time_offset_minutes, suggested_secs)| {
                        format!(
                            "{}({:?})@{:.3}/offset={:.1}m=>{}",
                            room_id,
                            room_state,
                            room_hour,
                            time_offset_minutes,
                            suggested_secs
                                .map(|secs| format!("{secs}s"))
                                .unwrap_or_else(|| "None".to_string())
                        )
                    },
                )
                .collect::<Vec<_>>()
                .join(", ")
        };

        debug!(
            target: "curve",
            "effective cycle math mode={:?} base_hour={:.3} fallback={}s update_interval={}s node_count={} chosen={}s nodes=[{}]",
            profile_registry.active_mode(),
            ctx.current_hour,
            fallback_secs,
            update_interval.as_secs(),
            room_snapshots.len(),
            chosen_secs,
            nodes
        );
    }

    Duration::from_secs(chosen_secs)
}

fn reserve_dispatch_slot(
    next_dispatch_at: &mut Option<Instant>,
    spacing: Duration,
) -> Option<Duration> {
    let now = Instant::now();
    match *next_dispatch_at {
        Some(next) if next > now => {
            let sleep_for = next.duration_since(now);
            *next_dispatch_at = Some(next.checked_add(spacing).unwrap_or(next));
            Some(sleep_for)
        }
        _ => {
            *next_dispatch_at = Some(now.checked_add(spacing).unwrap_or(now));
            None
        }
    }
}

pub(crate) fn wait_for_interactive_node_dispatch_slot(
    state: &SharedState,
    command_id: &str,
    node_id: &str,
    spacing: Duration,
) {
    let sleep_for = {
        let Ok(mut s) = state.lock() else {
            return;
        };
        if spacing.is_zero() {
            return;
        }

        reserve_dispatch_slot(&mut s.next_interactive_node_dispatch_at, spacing)
    };

    if let Some(sleep_for) = sleep_for {
        tracing::debug!(
            target: "sys",
            event = "interactive_node_dispatch_paced",
            command_id = %command_id,
            node_id,
            sleep_ms = sleep_for.as_millis(),
            "Queued interactive node dispatch paced"
        );
        thread::sleep(sleep_for);
    }
}

pub(crate) fn light_dispatch_generation_current(
    state: &SharedState,
    dispatch_generation: u64,
) -> bool {
    state
        .lock()
        .ok()
        .is_some_and(|s| s.light_dispatch_generation == dispatch_generation)
}

pub(crate) fn wait_for_node_dispatch_slot_if_current(
    state: &SharedState,
    command_id: &str,
    node_id: &str,
    spacing: Duration,
    dispatch_generation: u64,
) -> bool {
    let sleep_for = {
        let Ok(mut s) = state.lock() else {
            return false;
        };
        if s.light_dispatch_generation != dispatch_generation {
            return false;
        }
        if spacing.is_zero() {
            return true;
        }

        reserve_dispatch_slot(&mut s.next_node_dispatch_at, spacing)
    };

    if let Some(sleep_for) = sleep_for {
        tracing::debug!(
            target: "sys",
            event = "node_dispatch_paced",
            command_id = %command_id,
            node_id,
            sleep_ms = sleep_for.as_millis(),
            dispatch_generation,
            "Queued node dispatch paced"
        );

        let started = Instant::now();
        while started.elapsed() < sleep_for {
            if !light_dispatch_generation_current(state, dispatch_generation) {
                return false;
            }
            let remaining = sleep_for.saturating_sub(started.elapsed());
            thread::sleep(remaining.min(Duration::from_millis(100)));
        }
    }

    light_dispatch_generation_current(state, dispatch_generation)
}

pub(crate) struct PeriodicTickEnqueue<'a> {
    pub(crate) command_id: &'a str,
    pub(crate) node_id: &'a str,
    pub(crate) settings_node_id: &'a str,
    pub(crate) dispatch_generation: u64,
    pub(crate) current_hour: f32,
    pub(crate) emit_parent_node_id: Option<&'a str>,
    pub(crate) dispatch_spacing: Duration,
}

pub(crate) fn enqueue_periodic_tick(
    state: &SharedState,
    tx: &std::sync::mpsc::SyncSender<WorkItem>,
    tick: PeriodicTickEnqueue<'_>,
) -> bool {
    let should_enqueue = {
        let Ok(mut s) = state.lock() else {
            return false;
        };
        if s.light_dispatch_generation != tick.dispatch_generation {
            tracing::debug!(
                target: "sys",
                event = "periodic_tick_skipped",
                command_id = %tick.command_id,
                node_id = %tick.node_id,
                settings_node_id = %tick.settings_node_id,
                dispatch_generation = tick.dispatch_generation,
                current_generation = s.light_dispatch_generation,
                reason = "stale_dispatch_generation",
                "Periodic tick skipped before enqueue"
            );
            return true;
        }
        match s.pending_periodic_ticks.entry(tick.node_id.to_string()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let previous_hour = *entry.get();
                entry.insert(tick.current_hour);
                tracing::debug!(
                    target: "sys",
                    event = "periodic_tick_coalesced",
                    command_id = %tick.command_id,
                    node_id = %tick.node_id,
                    settings_node_id = %tick.settings_node_id,
                    previous_hour,
                    current_hour = tick.current_hour,
                    "Periodic tick already pending"
                );
                false
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(tick.current_hour);
                true
            }
        }
    };

    if !should_enqueue {
        return true;
    }

    match tx.try_send(WorkItem::PeriodicNodeTick {
        command_id: tick.command_id.to_string(),
        node_id: tick.node_id.to_string(),
        settings_node_id: tick.settings_node_id.to_string(),
        current_hour: tick.current_hour,
        emit_parent_node_id: tick.emit_parent_node_id.map(str::to_string),
        dispatch_spacing: tick.dispatch_spacing,
        dispatch_generation: tick.dispatch_generation,
    }) {
        Ok(()) => true,
        Err(_) => {
            if let Ok(mut s) = state.lock() {
                s.pending_periodic_ticks.remove(tick.node_id);
            }
            false
        }
    }
}

/// Run the blocking periodic update loop.
///
/// Sleeps `update_interval_secs` between iterations. Each iteration:
/// 1. Calculates current hour via `SystemTimeProvider`
/// 2. Logs curve values for the current time
/// 3. Gets rhythm-enabled runtime nodes, skipping warning-dimmed nodes
/// 4. Dispatches `PeriodicNodeTick` via `work_tx` if `Some`, or calls
///    `periodic_tick_room` inline if `None`
/// 5. Calls `check_solar_midnight()`
///
/// The `on_tick` callback is for platform-specific per-tick actions
/// (for example diagnostic hooks or appliance-side helpers).
pub fn run_periodic_loop<F: Fn()>(state: SharedState, on_tick: Option<F>) {
    let initial_interval = {
        let Ok(s) = state.lock() else { return };
        Duration::from_secs(s.runtime_config.update_interval_secs)
    };

    info!(
        "Periodic updater started with {}s interval",
        initial_interval.as_secs()
    );

    loop {
        let (
            utc_offset,
            profile_registry,
            solar_noon,
            latitude,
            longitude,
            update_interval,
            timezone_name,
            power_save,
            dispatch_generation,
        ) = {
            let Ok(s) = state.lock() else {
                thread::sleep(Duration::from_secs(60));
                continue;
            };
            let active_profile_id = s.active_mode_profile_id();
            let mut profile_registry = rhythm_core::LightProfileRegistry::with_profiles(
                s.light_profile_configs
                    .values()
                    .cloned()
                    .collect::<Vec<_>>(),
                &active_profile_id,
            );
            profile_registry.set_mode_configs(s.mode_configs());
            (
                s.utc_offset_hours,
                profile_registry,
                s.solar_noon_hour(),
                s.latitude,
                s.longitude,
                Duration::from_secs(s.runtime_config.update_interval_secs),
                s.timezone_name.clone(),
                s.power_save,
                s.light_dispatch_generation,
            )
        };

        let lat = latitude.unwrap_or(35.0);
        let lon = longitude.unwrap_or(-80.84);

        let doy = SystemTimeProvider::new(utc_offset).day_of_year();

        // Check for DST transition and refresh UTC offset if needed
        let (utc_offset, solar_noon, _doy) = refresh_dst_offset(
            &state,
            utc_offset,
            solar_noon,
            lat,
            lon,
            doy,
            timezone_name.as_deref(),
        );
        let (solar_noon, _doy, _sun_times) = refresh_runtime_solar_context(
            &state,
            solar_noon,
            lat,
            lon,
            utc_offset,
            timezone_name.as_deref(),
        );

        let time_provider = SystemTimeProvider::new(utc_offset);
        let current_hour = time_provider.current_hour();
        let local_now = chrono::Utc::now().naive_utc()
            + chrono::Duration::seconds((utc_offset * 3600.0) as i64);

        // Calculate generic curve values (no offset) for logging
        let ctx = rhythm_core::curve_context_for_local_date_and_hour(
            solar_noon,
            latitude,
            longitude,
            timezone_name.as_deref(),
            local_now.date(),
            current_hour,
        );
        let module = profile_registry.active_profile();
        let values = module.calculate(&ctx);

        // Get rhythm-enabled light-addressable runtime nodes, skipping warning-dimmed nodes
        let (
            mut room_snapshots,
            mut periodic_nodes,
            has_composite_controller,
            warning_skipped,
            transition_skipped,
            rhythm_disabled_skipped,
            hard_off_rooms,
            expired_transitions,
            periodic_work_tx,
            work_tx,
        ) = {
            let Ok(mut s) = state.lock() else {
                thread::sleep(update_interval);
                continue;
            };
            let now = Instant::now();
            let transitions_before = s.room_mode_transitions.len();
            s.room_mode_transitions
                .retain(|_, transition| transition.periodic_resume_at > now);
            let expired_transitions = transitions_before - s.room_mode_transitions.len();
            let all_rooms: Vec<rhythm_core::NodeSnapshot> = s
                .hub_runtime()
                .map(|rt| rt.engine_all_effective_node_snapshots())
                .unwrap_or_default();
            let mut warning_skipped = 0usize;
            let mut transition_skipped = 0usize;
            let mut rhythm_disabled_skipped = 0usize;
            let mut hard_off_rooms = 0usize;
            let rooms: Vec<rhythm_core::NodeSnapshot> = all_rooms
                .into_iter()
                .filter(|room| {
                    if !room.kind.is_light_addressable() {
                        return false;
                    }
                    let event_room_id = room.parent_id.as_deref().unwrap_or(&room.id);
                    if !room.rhythm_enabled {
                        rhythm_disabled_skipped += 1;
                        return false;
                    }
                    if room.hard_off {
                        hard_off_rooms += 1;
                    }
                    if s.room_mode_transitions
                        .get(event_room_id)
                        .is_some_and(|transition| transition.periodic_resume_at > now)
                    {
                        transition_skipped += 1;
                        return false;
                    }
                    if s.motion_snapshots
                        .get(event_room_id)
                        .is_some_and(|ms| ms.warning_active)
                    {
                        warning_skipped += 1;
                        false
                    } else {
                        true
                    }
                })
                .collect();
            let periodic_nodes = periodic_dispatch_nodes_from_state(&s, &rooms);
            (
                rooms,
                periodic_nodes,
                s.composite_controller.is_some(),
                warning_skipped,
                transition_skipped,
                rhythm_disabled_skipped,
                hard_off_rooms,
                expired_transitions,
                s.periodic_work_tx.clone(),
                s.work_tx.clone(),
            )
        };

        room_snapshots.sort_by_key(|room| stable_room_phase_key(&room.id));
        periodic_nodes.sort_by_key(|node| stable_room_phase_key(&node.node_id));
        let dispatch_summary =
            summarize_periodic_dispatch(&room_snapshots, &periodic_nodes, has_composite_controller);
        let last_node_index_by_emit_target =
            last_periodic_node_index_by_emit_target(&periodic_nodes);
        let cycle_duration = effective_cycle_duration(
            &profile_registry,
            &ctx,
            &room_snapshots,
            update_interval,
            power_save,
        );
        let phase_gap = dispatch_spacing(cycle_duration, periodic_nodes.len());
        let command_id = logging::next_command_id("periodic");

        if transition_skipped > 0
            || rhythm_disabled_skipped > 0
            || hard_off_rooms > 0
            || expired_transitions > 0
            || dispatch_summary.no_dispatch_node_count > 0
        {
            tracing::debug!(
                target: "sys",
                event = "periodic_cycle_detail",
                command_id = %command_id,
                dispatch_count = dispatch_summary.dispatch_node_count,
                eligible_node_count = dispatch_summary.eligible_node_count,
                eligible_room_count = dispatch_summary.eligible_room_count,
                eligible_device_count = dispatch_summary.eligible_device_count,
                dispatched_settings_count = dispatch_summary.dispatched_settings_node_count,
                no_dispatch_count = dispatch_summary.no_dispatch_node_count,
                warning_skipped,
                transition_skipped,
                rhythm_disabled_skipped,
                hard_off_rooms,
                expired_transitions,
                "Periodic cycle detail"
            );
        }

        tracing::info!(
            target: "sys",
            event = "periodic_cycle",
            command_id = %command_id,
            local_hour = current_hour,
            solar_time = values.solar_time,
            brightness_pct = values.brightness,
            kelvin = values.kelvin,
            dispatch_count = dispatch_summary.dispatch_node_count,
            eligible_node_count = dispatch_summary.eligible_node_count,
            eligible_room_count = dispatch_summary.eligible_room_count,
            eligible_device_count = dispatch_summary.eligible_device_count,
            dispatched_settings_count = dispatch_summary.dispatched_settings_node_count,
            no_dispatch_count = dispatch_summary.no_dispatch_node_count,
            warning_skipped,
            transition_skipped,
            rhythm_disabled_skipped,
            hard_off_rooms,
            expired_transitions,
            cycle_secs = cycle_duration.as_secs_f32(),
            phase_gap_ms = phase_gap.as_millis(),
            "Periodic cycle"
        );

        // Record tick timestamp and sync curve-computed motion timeout
        if let Ok(mut s) = state.lock() {
            s.last_tick_epoch_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            s.default_motion_timeout_secs = values.motion_timeout_secs as u64;
            s.default_fade_ms = values.transition_ms;
        }

        if let Some(ref cb) = on_tick {
            cb();
        }

        let cycle_started = Instant::now();

        // Dispatch per-node ticks with stable staggering across the cycle.
        if let Some(ref tx) = periodic_work_tx {
            for (idx, node) in periodic_nodes.iter().enumerate() {
                if !light_dispatch_generation_current(&state, dispatch_generation) {
                    tracing::debug!(
                        target: "sys",
                        event = "periodic_cycle_invalidated",
                        command_id = %command_id,
                        dispatch_generation,
                        "Stopping stale periodic cycle"
                    );
                    break;
                }
                let room_hour = SystemTimeProvider::new(utc_offset).current_hour();
                let emit_parent_node_id = last_node_index_by_emit_target
                    .get(&node.emit_node_id)
                    .is_some_and(|last_idx| *last_idx == idx)
                    .then_some(node.emit_node_id.as_str())
                    .filter(|emit_id| *emit_id != node.settings_node_id);
                if !enqueue_periodic_tick(
                    &state,
                    tx,
                    PeriodicTickEnqueue {
                        command_id: &command_id,
                        node_id: &node.node_id,
                        settings_node_id: &node.settings_node_id,
                        dispatch_generation,
                        current_hour: room_hour,
                        emit_parent_node_id,
                        dispatch_spacing: phase_gap,
                    },
                ) {
                    tracing::warn!(
                        target: "sys",
                        event = "periodic_tick_dropped",
                        command_id = %command_id,
                        node_id = %node.node_id,
                        settings_node_id = %node.settings_node_id,
                        queue = "periodic",
                        "Periodic queue full, dropping tick"
                    );
                }
                if idx + 1 < periodic_nodes.len() && !phase_gap.is_zero() {
                    thread::sleep(phase_gap);
                }
            }
        } else if let Some(ref tx) = work_tx {
            for (idx, node) in periodic_nodes.iter().enumerate() {
                if !light_dispatch_generation_current(&state, dispatch_generation) {
                    tracing::debug!(
                        target: "sys",
                        event = "periodic_cycle_invalidated",
                        command_id = %command_id,
                        dispatch_generation,
                        "Stopping stale periodic cycle"
                    );
                    break;
                }
                let room_hour = SystemTimeProvider::new(utc_offset).current_hour();
                let emit_parent_node_id = last_node_index_by_emit_target
                    .get(&node.emit_node_id)
                    .is_some_and(|last_idx| *last_idx == idx)
                    .then_some(node.emit_node_id.as_str())
                    .filter(|emit_id| *emit_id != node.settings_node_id);
                if !enqueue_periodic_tick(
                    &state,
                    tx,
                    PeriodicTickEnqueue {
                        command_id: &command_id,
                        node_id: &node.node_id,
                        settings_node_id: &node.settings_node_id,
                        dispatch_generation,
                        current_hour: room_hour,
                        emit_parent_node_id,
                        dispatch_spacing: phase_gap,
                    },
                ) {
                    tracing::warn!(
                        target: "sys",
                        event = "periodic_tick_dropped",
                        command_id = %command_id,
                        node_id = %node.node_id,
                        settings_node_id = %node.settings_node_id,
                        queue = "work",
                        "Work queue full, dropping periodic tick"
                    );
                }
                if idx + 1 < periodic_nodes.len() && !phase_gap.is_zero() {
                    thread::sleep(phase_gap);
                }
            }
        } else {
            // No work queue (e.g. rhythm-server) - tick rooms inline
            let runtime = {
                let Ok(s) = state.lock() else {
                    thread::sleep(update_interval);
                    continue;
                };
                s.hub_runtime()
            };
            if let Some(runtime) = runtime {
                for (idx, node) in periodic_nodes.iter().enumerate() {
                    let room_hour = SystemTimeProvider::new(utc_offset).current_hour();
                    let started = Instant::now();
                    if let Err(e) =
                        runtime.periodic_tick_node(&node.node_id, &node.settings_node_id, room_hour)
                    {
                        tracing::warn!(
                            target: "sys",
                            event = "periodic_node_tick_failed",
                            command_id = %command_id,
                            node_id = %node.node_id,
                            settings_node_id = %node.settings_node_id,
                            current_hour = room_hour,
                            latency_ms = started.elapsed().as_millis(),
                            error = %e,
                            "Periodic node tick failed"
                        );
                    } else {
                        tracing::debug!(
                            target: "sys",
                            event = "periodic_node_tick_applied",
                            command_id = %command_id,
                            node_id = %node.node_id,
                            settings_node_id = %node.settings_node_id,
                            current_hour = room_hour,
                            latency_ms = started.elapsed().as_millis(),
                            dispatch = "inline",
                            "Periodic node tick applied"
                        );
                    }
                    post_tick_node(&state, &runtime, &node.settings_node_id);
                    if last_node_index_by_emit_target
                        .get(&node.emit_node_id)
                        .is_some_and(|last_idx| *last_idx == idx)
                        && node.emit_node_id != node.settings_node_id
                    {
                        post_tick_node(&state, &runtime, &node.emit_node_id);
                    }
                    if idx + 1 < periodic_nodes.len() && !phase_gap.is_zero() {
                        thread::sleep(phase_gap);
                    }
                }
            } else if !periodic_nodes.is_empty() {
                debug!(
                    target: "sys",
                    "Periodic tick skipped {} queued node(s): no runtime available",
                    periodic_nodes.len()
                );
            }
        }

        let current_hour = SystemTimeProvider::new(utc_offset).current_hour();
        match check_solar_midnight_at(&state, current_hour, utc_offset, Instant::now()) {
            PeriodicTimeCheckResult::Continuous { last_hour } => {
                check_mode_transitions(&state, last_hour, current_hour);
            }
            PeriodicTimeCheckResult::Seeded | PeriodicTimeCheckResult::Discontinuous { .. } => {
                replay_missed_mode_transitions(&state);
            }
        }

        let elapsed = cycle_started.elapsed();
        if elapsed < cycle_duration {
            thread::sleep(cycle_duration - elapsed);
        }
    }
}

/// Process post-tick side effects for a single public node.
///
/// Emits an SSE event with the node's current state on desktop.
pub fn post_tick_node(state: &SharedState, runtime: &Arc<dyn RuntimeHandle>, node_id: &str) {
    let Some(snap) = runtime.engine_effective_node_snapshot(node_id) else {
        return;
    };

    crate::commands::refresh_lights_on_cache_for_runtime_snapshot_with_source(
        state,
        runtime,
        &snap,
        crate::state::ObservedPowerSource::Periodic,
    );

    {
        let mut event = crate::commands::build_node_state_event(state, &snap);
        event.tick = true;
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::NodeState { nodes: vec![event] },
        );
    }

    // Suppress unused variable warnings on non-desktop builds
    let _ = (&state, &snap);
}

/// Choose the modulo-24 local-hour delta that best matches expected wall-clock movement.
fn adjusted_local_hour_delta(last_hour: f32, current_hour: f32, expected_delta_hours: f32) -> f32 {
    let raw_delta = current_hour - last_hour;
    let mut best_delta = raw_delta;
    let mut best_diff = (raw_delta - expected_delta_hours).abs();

    for candidate in [raw_delta - 24.0, raw_delta + 24.0] {
        let diff = (candidate - expected_delta_hours).abs();
        if diff < best_diff {
            best_delta = candidate;
            best_diff = diff;
        }
    }

    best_delta
}

fn advance_periodic_time_check(
    state: &SharedState,
    current_hour: f32,
    current_utc_offset_hours: f32,
    observed_at: Instant,
) -> PeriodicTimeCheckResult {
    let (last_hour, last_check_instant, last_utc_offset_hours) = {
        let Ok(mut s) = state.lock() else {
            return PeriodicTimeCheckResult::Seeded;
        };

        let last_hour = s.last_check_hour;
        let last_check_instant = s.last_check_instant;
        let last_utc_offset_hours = s.last_check_utc_offset_hours;

        s.last_check_hour = Some(current_hour);
        s.last_check_instant = Some(observed_at);
        s.last_check_utc_offset_hours = Some(current_utc_offset_hours);

        (last_hour, last_check_instant, last_utc_offset_hours)
    };

    let (Some(last_hour), Some(last_check_instant), Some(last_utc_offset_hours)) =
        (last_hour, last_check_instant, last_utc_offset_hours)
    else {
        debug!(
            target: "sys",
            "Solar midnight check seeded at local_hour {:.2}",
            current_hour
        );
        return PeriodicTimeCheckResult::Seeded;
    };

    let elapsed_hours = observed_at
        .saturating_duration_since(last_check_instant)
        .as_secs_f32()
        / 3600.0;
    let expected_delta_hours = elapsed_hours + (current_utc_offset_hours - last_utc_offset_hours);
    let adjusted_delta_hours =
        adjusted_local_hour_delta(last_hour, current_hour, expected_delta_hours);
    let tolerance_hours = PERIODIC_TIME_DISCONTINUITY_TOLERANCE.as_secs_f32() / 3600.0;

    if (adjusted_delta_hours - expected_delta_hours).abs() > tolerance_hours {
        return PeriodicTimeCheckResult::Discontinuous {
            last_hour,
            adjusted_delta_hours,
            expected_delta_hours,
        };
    }

    PeriodicTimeCheckResult::Continuous { last_hour }
}

fn maybe_reset_on_solar_midnight(state: &SharedState, last_hour: f32, current_hour: f32) {
    let (crossed, runtime) = {
        let Ok(s) = state.lock() else { return };

        let solar_midnight = s.solar_midnight_hour();
        let crossed = rhythm_core::crossed_solar_midnight(last_hour, current_hour, solar_midnight);

        if crossed {
            info!(
                "Solar midnight crossed (last_local={:.2}, now_local={:.2}, trigger_local={:.2}) - resetting offsets",
                last_hour, current_hour, solar_midnight
            );
        }

        (crossed, s.hub_runtime())
    };

    if crossed {
        if let Some(runtime) = runtime {
            for snap in runtime.engine_all_node_snapshots() {
                runtime.restore_node_state(
                    &snap.id,
                    RestoredNodeState {
                        time_offset_minutes: 0.0,
                        brightness_offset: 0.0,
                        ..RestoredNodeState::from(&snap)
                    },
                );
            }

            {
                let events: Vec<_> = runtime
                    .engine_all_room_snapshots()
                    .iter()
                    .map(|s| {
                        crate::commands::build_node_state_event(
                            state,
                            &rhythm_core::NodeSnapshot::from_room_snapshot(s.clone()),
                        )
                    })
                    .collect();
                if !events.is_empty() {
                    crate::state::emit_server_event(
                        state,
                        crate::server_event::ServerEvent::NodeState { nodes: events },
                    );
                }
            }

            info!("Reset room offsets for new solar day");
        }
    }
}

fn check_solar_midnight_at(
    state: &SharedState,
    current_hour: f32,
    current_utc_offset_hours: f32,
    observed_at: Instant,
) -> PeriodicTimeCheckResult {
    let result =
        advance_periodic_time_check(state, current_hour, current_utc_offset_hours, observed_at);

    match result {
        PeriodicTimeCheckResult::Continuous { last_hour } => {
            maybe_reset_on_solar_midnight(state, last_hour, current_hour);
        }
        PeriodicTimeCheckResult::Discontinuous {
            last_hour,
            adjusted_delta_hours,
            expected_delta_hours,
        } => {
            warn!(
                target: "sys",
                "Wall-clock jump detected during periodic checks (last_local={:.2}, now_local={:.2}, adjusted_delta_hours={:.2}, expected_delta_hours={:.2}) - reseeding time-based checks",
                last_hour,
                current_hour,
                adjusted_delta_hours,
                expected_delta_hours
            );
        }
        PeriodicTimeCheckResult::Seeded => {}
    }

    result
}

/// Check for solar midnight crossing and reset room offsets.
///
/// Solar midnight is when the sun is at its lowest point (opposite of solar noon).
/// At this crossing, per-room time and brightness offsets are reset to zero,
/// giving each day a clean start.
pub fn check_solar_midnight(state: &SharedState, current_hour: f32) {
    let current_utc_offset_hours = state
        .lock()
        .ok()
        .map(|s| s.utc_offset_hours)
        .unwrap_or_default();
    let _ = check_solar_midnight_at(
        state,
        current_hour,
        current_utc_offset_hours,
        Instant::now(),
    );
}

fn fallback_solar_trigger_hour(
    trigger: rhythm_core::ModeTransitionTrigger,
    target_mode: rhythm_core::RhythmMode,
    sunrise: f32,
    sunset: f32,
) -> Option<f32> {
    let toward_day = target_mode == rhythm_core::RhythmMode::Day;
    match trigger {
        rhythm_core::ModeTransitionTrigger::Manual => None,
        rhythm_core::ModeTransitionTrigger::Scheduled(time) => Some(time.local_hour()),
        rhythm_core::ModeTransitionTrigger::Sunrise => Some(sunrise),
        rhythm_core::ModeTransitionTrigger::Sunset => Some(sunset),
        rhythm_core::ModeTransitionTrigger::CivilTwilight => Some(if toward_day {
            (sunrise - 0.5).rem_euclid(24.0)
        } else {
            (sunset + 0.5).rem_euclid(24.0)
        }),
        rhythm_core::ModeTransitionTrigger::NauticalTwilight => Some(if toward_day {
            (sunrise - 1.0).rem_euclid(24.0)
        } else {
            (sunset + 1.0).rem_euclid(24.0)
        }),
        rhythm_core::ModeTransitionTrigger::AstronomicalTwilight => Some(if toward_day {
            (sunrise - 1.5).rem_euclid(24.0)
        } else {
            (sunset + 1.5).rem_euclid(24.0)
        }),
    }
}

#[derive(Clone, Copy)]
struct SolarTriggerContext<'a> {
    solar_noon: f32,
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct ReplayTransitionContext<'a> {
    solar: SolarTriggerContext<'a>,
    utc_offset: f32,
    configs: &'a [rhythm_core::ModeTransitionConfig],
}

fn trigger_hour_for_local_date(
    trigger: rhythm_core::ModeTransitionTrigger,
    target_mode: rhythm_core::RhythmMode,
    ctx: SolarTriggerContext<'_>,
    date: chrono::NaiveDate,
) -> Option<f32> {
    if let rhythm_core::ModeTransitionTrigger::Scheduled(time) = trigger {
        return Some(time.local_hour());
    }

    let estimated_sunrise = if ctx.latitude.is_some() && ctx.longitude.is_some() {
        (ctx.solar_noon - 6.0).rem_euclid(24.0)
    } else {
        rhythm_core::config::FALLBACK_SUNRISE_HOUR
    };
    let estimated_sunset = if ctx.latitude.is_some() && ctx.longitude.is_some() {
        (ctx.solar_noon + 6.0).rem_euclid(24.0)
    } else {
        rhythm_core::config::FALLBACK_SUNSET_HOUR
    };

    let Some(lat) = ctx.latitude else {
        return fallback_solar_trigger_hour(
            trigger,
            target_mode,
            estimated_sunrise,
            estimated_sunset,
        );
    };
    let Some(lon) = ctx.longitude else {
        return fallback_solar_trigger_hour(
            trigger,
            target_mode,
            estimated_sunrise,
            estimated_sunset,
        );
    };
    let Some(tz_name) = ctx.timezone_name else {
        return fallback_solar_trigger_hour(
            trigger,
            target_mode,
            estimated_sunrise,
            estimated_sunset,
        );
    };

    let tz = rhythm_core::Timezone::new(tz_name);
    let year = chrono::Datelike::year(&date);
    let month = chrono::Datelike::month(&date);
    let day = chrono::Datelike::day(&date);
    let sun = rhythm_core::calculate_sun_times(lat, lon, year, month, day, &tz);
    let twilight = rhythm_core::calculate_twilight_times(lat, lon, year, month, day, &tz);

    match trigger {
        rhythm_core::ModeTransitionTrigger::Manual => None,
        rhythm_core::ModeTransitionTrigger::Scheduled(time) => Some(time.local_hour()),
        rhythm_core::ModeTransitionTrigger::Sunrise => Some(sun.sunrise),
        rhythm_core::ModeTransitionTrigger::Sunset => Some(sun.sunset),
        rhythm_core::ModeTransitionTrigger::CivilTwilight => {
            Some(if target_mode == rhythm_core::RhythmMode::Day {
                twilight
                    .dawn
                    .civil
                    .unwrap_or((sun.sunrise - 0.5).rem_euclid(24.0))
            } else {
                twilight
                    .dusk
                    .civil
                    .unwrap_or((sun.sunset + 0.5).rem_euclid(24.0))
            })
        }
        rhythm_core::ModeTransitionTrigger::NauticalTwilight => {
            Some(if target_mode == rhythm_core::RhythmMode::Day {
                twilight
                    .dawn
                    .nautical
                    .unwrap_or((sun.sunrise - 1.0).rem_euclid(24.0))
            } else {
                twilight
                    .dusk
                    .nautical
                    .unwrap_or((sun.sunset + 1.0).rem_euclid(24.0))
            })
        }
        rhythm_core::ModeTransitionTrigger::AstronomicalTwilight => {
            Some(if target_mode == rhythm_core::RhythmMode::Day {
                twilight
                    .dawn
                    .astronomical
                    .unwrap_or((sun.sunrise - 1.5).rem_euclid(24.0))
            } else {
                twilight
                    .dusk
                    .astronomical
                    .unwrap_or((sun.sunset + 1.5).rem_euclid(24.0))
            })
        }
    }
}

fn trigger_hour(
    trigger: rhythm_core::ModeTransitionTrigger,
    target_mode: rhythm_core::RhythmMode,
    ctx: SolarTriggerContext<'_>,
) -> Option<f32> {
    if let Some(tz_name) = ctx.timezone_name {
        let tz = rhythm_core::Timezone::new(tz_name);
        let (year, month, day) = tz.local_date_from_utc(chrono::Utc::now().naive_utc());
        let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
        trigger_hour_for_local_date(trigger, target_mode, ctx, date)
    } else {
        let fallback_date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)?;
        trigger_hour_for_local_date(trigger, target_mode, ctx, fallback_date)
    }
}

fn local_datetime_from_utc(
    utc: chrono::NaiveDateTime,
    utc_offset: f32,
    timezone_name: Option<&str>,
) -> chrono::NaiveDateTime {
    if let Some(tz_name) = timezone_name {
        rhythm_core::Timezone::new(tz_name).local_datetime_from_utc(utc)
    } else {
        utc + chrono::Duration::seconds((utc_offset * 3600.0) as i64)
    }
}

fn utc_datetime_from_local(
    local: chrono::NaiveDateTime,
    utc_offset: f32,
    timezone_name: Option<&str>,
) -> Option<chrono::NaiveDateTime> {
    if let Some(tz_name) = timezone_name {
        rhythm_core::Timezone::new(tz_name).utc_datetime_from_local(local)
    } else {
        Some(local - chrono::Duration::seconds((utc_offset * 3600.0) as i64))
    }
}

fn resolved_replayed_mode_transition(
    start_mode: rhythm_core::RhythmMode,
    start_utc: chrono::NaiveDateTime,
    end_utc: chrono::NaiveDateTime,
    ctx: ReplayTransitionContext<'_>,
) -> Option<rhythm_core::ModeTransitionConfig> {
    if end_utc <= start_utc {
        return None;
    }

    let start_local = local_datetime_from_utc(start_utc, ctx.utc_offset, ctx.solar.timezone_name);
    let end_local = local_datetime_from_utc(end_utc, ctx.utc_offset, ctx.solar.timezone_name);
    let mut date = start_local.date();
    let end_date = end_local.date();
    let mut events = Vec::new();

    while date <= end_date {
        let Some(local_midnight) = date.and_hms_opt(0, 0, 0) else {
            break;
        };

        for config in ctx.configs.iter().filter(|config| {
            config.trigger_enabled && config.trigger != rhythm_core::ModeTransitionTrigger::Manual
        }) {
            let Some(trigger_hour) =
                trigger_hour_for_local_date(config.trigger, config.to_mode, ctx.solar, date)
            else {
                continue;
            };

            let trigger_seconds =
                ((trigger_hour.rem_euclid(24.0)) * 3600.0).round() as i64 % 86_400;
            let event_local = local_midnight + chrono::Duration::seconds(trigger_seconds);
            let Some(event_utc) =
                utc_datetime_from_local(event_local, ctx.utc_offset, ctx.solar.timezone_name)
            else {
                continue;
            };

            if event_utc > start_utc && event_utc <= end_utc {
                events.push((event_utc, config.clone()));
            }
        }

        let Some(next_date) = date.succ_opt() else {
            break;
        };
        date = next_date;
    }

    if events.is_empty() {
        return None;
    }

    events.sort_by_key(|(event_utc, _)| *event_utc);

    let mut mode = start_mode;
    let mut final_transition = None;
    for (_, transition) in events {
        if mode == transition.from_mode {
            mode = transition.to_mode;
            final_transition = Some(transition);
        }
    }

    let transition = final_transition?;
    if mode == start_mode {
        return None;
    }

    Some(transition)
}

fn replay_missed_mode_transitions(state: &SharedState) {
    let (
        start_mode,
        start_cause,
        start_change_utc_ms,
        solar_noon,
        utc_offset,
        latitude,
        longitude,
        timezone_name,
        configs,
    ) = {
        let Ok(s) = state.lock() else { return };
        (
            s.active_mode,
            s.last_active_mode_cause,
            s.last_active_mode_change_utc_ms,
            s.solar_noon_hour(),
            s.utc_offset_hours,
            s.latitude,
            s.longitude,
            s.timezone_name.clone(),
            s.mode_transition_configs(),
        )
    };

    let Some(start_change_utc_ms) = start_change_utc_ms else {
        return;
    };
    let Some(start_utc) =
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start_change_utc_ms)
            .map(|dt| dt.naive_utc())
    else {
        return;
    };

    let now_utc = chrono::Utc::now().naive_utc();
    let Some(transition) = resolved_replayed_mode_transition(
        start_mode,
        start_utc,
        now_utc,
        ReplayTransitionContext {
            solar: SolarTriggerContext {
                solar_noon,
                latitude,
                longitude,
                timezone_name: timezone_name.as_deref(),
            },
            utc_offset,
            configs: &configs,
        },
    ) else {
        return;
    };

    info!(
        "Replaying missed mode transition {:?} -> {:?} on {:?} after restart/downtime (last_change_cause={:?})",
        start_mode, transition.to_mode, transition.trigger, start_cause
    );

    if let Err(e) = crate::commands::do_set_active_mode_with_trigger(
        state,
        transition.to_mode,
        transition.trigger,
    ) {
        warn!("Failed to replay missed mode transition: {}", e);
    }
}

/// Trigger configured scheduled mode transitions when their event time is crossed.
pub fn check_mode_transitions(state: &SharedState, last_hour: f32, current_hour: f32) {
    let candidate = {
        let Ok(s) = state.lock() else { return };

        let active_mode = s.active_mode;
        let solar_noon = s.solar_noon_hour();
        let latitude = s.latitude;
        let longitude = s.longitude;
        let timezone_name = s.timezone_name.clone();

        s.mode_transition_configs().into_iter().find_map(|config| {
            if config.from_mode != active_mode
                || !config.trigger_enabled
                || config.trigger == rhythm_core::ModeTransitionTrigger::Manual
            {
                return None;
            }

            let trigger_hour = trigger_hour(
                config.trigger,
                config.to_mode,
                SolarTriggerContext {
                    solar_noon,
                    latitude,
                    longitude,
                    timezone_name: timezone_name.as_deref(),
                },
            )?;

            if rhythm_core::crossed_solar_midnight(last_hour, current_hour, trigger_hour) {
                Some((config, trigger_hour))
            } else {
                None
            }
        })
    };

    let Some((transition, trigger_hour)) = candidate else {
        return;
    };

    info!(
        "Mode transition {:?} -> {:?} on {:?} at trigger_local {:.2} (last_local={:.2}, now_local={:.2})",
        state
            .lock()
            .ok()
            .map(|s| s.active_mode)
            .unwrap_or(transition.to_mode),
        transition.to_mode,
        transition.trigger,
        trigger_hour,
        last_hour,
        current_hour
    );

    if let Err(e) = crate::commands::do_set_active_mode_with_trigger(
        state,
        transition.to_mode,
        transition.trigger,
    ) {
        warn!("Failed to apply mode transition: {}", e);
    }
}

/// Get the configured update interval in seconds.
pub fn update_interval_secs(state: &SharedState) -> u64 {
    state
        .lock()
        .ok()
        .map(|s| s.runtime_config.update_interval_secs)
        .unwrap_or(60)
}

/// Check if the UTC offset has changed due to a DST transition.
///
/// If `timezone_name` is set, recomputes the current UTC offset via chrono-tz.
/// When the offset differs (DST spring-forward or fall-back), updates AppState,
/// recomputes solar noon, pushes the new SolarTime to the engine, persists the
/// location, and emits an SSE event.
///
/// Returns the (possibly updated) `(utc_offset, solar_noon, day_of_year)`.
pub fn refresh_dst_offset(
    state: &SharedState,
    current_offset: f32,
    current_solar_noon: f32,
    lat: f32,
    lon: f32,
    doy: u32,
    timezone_name: Option<&str>,
) -> (f32, f32, u32) {
    let Some(tz_name) = timezone_name else {
        return (current_offset, current_solar_noon, doy);
    };

    let tz = rhythm_core::Timezone::new(tz_name);
    let (year, month, day, hour) = tz.local_date_hour_from_utc(chrono::Utc::now().naive_utc());
    let fresh_offset = tz.utc_offset(year, month, day, hour);

    if (fresh_offset - current_offset).abs() < 0.01 {
        return (current_offset, current_solar_noon, doy);
    }

    // DST transition detected
    info!(
        "DST transition detected: utc_offset {:.1} → {:.1} ({})",
        current_offset, fresh_offset, tz_name
    );

    let fresh_solar_noon = rhythm_core::calculate_solar_noon(lon, year, month, day, &tz);
    let fresh_sun_times = rhythm_core::calculate_sun_times(lat, lon, year, month, day, &tz);
    let fresh_doy = rhythm_core::timezone::day_of_year(year, month, day);

    // Update state, engine, and persist
    {
        let Ok(mut s) = state.lock() else {
            return (fresh_offset, fresh_solar_noon, fresh_doy);
        };

        s.utc_offset_hours = fresh_offset;
        s.runtime_config.solar_noon_hour = fresh_solar_noon;

        if let Some(runtime) = s.hub_runtime() {
            let solar_time = rhythm_core::SolarTime::new(fresh_solar_noon, lat, fresh_doy);
            if let Err(e) = runtime.set_solar(solar_time) {
                warn!("Failed to update solar time after DST change: {}", e);
            }
            if let Err(e) = runtime.set_sun_times(fresh_sun_times) {
                warn!("Failed to update sun times after DST change: {}", e);
            }
        }

        if let Some(ref storage) = s.storage {
            let loc = StoredLocation {
                latitude: Some(lat),
                longitude: Some(lon),
                utc_offset_hours: fresh_offset,
                timezone_name: Some(tz_name.to_string()),
            };
            if let Err(e) = storage.save_location(&loc) {
                warn!("Failed to persist location after DST change: {}", e);
            }
        }

        s.emit_event(crate::server_event::ServerEvent::ConfigChanged);
    }

    (fresh_offset, fresh_solar_noon, fresh_doy)
}

/// Refresh the runtime's date-specific solar context.
///
/// Solar noon, sunrise, and sunset drift every day. The periodic loop keeps the
/// engine's stored context current so actual room ticks use the same solar data
/// as API/rendering paths.
pub fn refresh_runtime_solar_context(
    state: &SharedState,
    current_solar_noon: f32,
    lat: f32,
    lon: f32,
    utc_offset: f32,
    timezone_name: Option<&str>,
) -> (f32, u32, Option<rhythm_core::SunTimes>) {
    let Some(tz_name) = timezone_name else {
        let day_of_year = SystemTimeProvider::new(utc_offset).day_of_year();
        let runtime = state.lock().ok().and_then(|s| s.hub_runtime());
        if let Some(runtime) = runtime {
            let solar_time = rhythm_core::SolarTime::new(current_solar_noon, lat, day_of_year);
            if let Err(e) = runtime.set_solar(solar_time) {
                warn!("Failed to refresh fallback solar time: {}", e);
            }
            if let Err(e) = runtime.clear_sun_times() {
                warn!("Failed to clear fallback sun times: {}", e);
            }
        }
        return (current_solar_noon, day_of_year, None);
    };

    let tz = rhythm_core::Timezone::new(tz_name);
    let local_now = tz.local_datetime_from_utc(chrono::Utc::now().naive_utc());
    let year = chrono::Datelike::year(&local_now.date());
    let month = chrono::Datelike::month(&local_now.date());
    let day = chrono::Datelike::day(&local_now.date());
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);
    let solar_noon = rhythm_core::calculate_solar_noon(lon, year, month, day, &tz);
    let sun_times = rhythm_core::calculate_sun_times(lat, lon, year, month, day, &tz);

    let runtime = {
        let Ok(mut s) = state.lock() else {
            return (solar_noon, day_of_year, Some(sun_times));
        };
        if (s.runtime_config.solar_noon_hour - solar_noon).abs() > 0.001 {
            s.runtime_config.solar_noon_hour = solar_noon;
        }
        s.hub_runtime()
    };

    if let Some(runtime) = runtime {
        let solar_time = rhythm_core::SolarTime::new(solar_noon, lat, day_of_year);
        if let Err(e) = runtime.set_solar(solar_time) {
            warn!("Failed to refresh solar time: {}", e);
        }
        if let Err(e) = runtime.set_sun_times(sun_times) {
            warn!("Failed to refresh sun times: {}", e);
        }
    }

    (solar_noon, day_of_year, Some(sun_times))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ObservedPowerSource, ObservedPowerState};
    use chrono::{NaiveDate, Timelike};
    use rhythm_core::CompositeController;
    use rhythm_core::{
        default_rhythm_profile, CurveContext, LightProfileRegistry, RoomProfileSettings,
    };
    use std::sync::Mutex;

    fn make_state() -> SharedState {
        Arc::new(Mutex::new(crate::state::AppState::default()))
    }

    #[test]
    fn post_tick_node_refreshes_lights_on_cache_before_emitting_event() {
        use rhythm_core::controller::NoOpController;
        use rhythm_core::runtime::orchestrator::RhythmRuntime;
        use rhythm_core::runtime::registry::SimpleDeviceRegistry;
        use rhythm_core::runtime::scheduler::NoOpScheduler;
        use rhythm_core::runtime::time::MockTimeProvider;
        use rhythm_core::RuntimeConfig;

        let state = make_state();
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        {
            let mut s = state.lock().unwrap();
            s.event_tx = Some(tx);
            s.room_observed_power.insert(
                "room1".into(),
                ObservedPowerState::new(true, ObservedPowerSource::Command),
            );
        }

        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RhythmRuntime::new(
            Arc::new(NoOpController::new()),
            MockTimeProvider::new(14.0, 172, 2026),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        ));
        runtime.add_room("room1", "Room 1");

        post_tick_node(&state, &runtime, "room1");

        let cached = state
            .lock()
            .unwrap()
            .room_observed_power
            .get("room1")
            .map(|observed| observed.lights_on);
        assert_eq!(cached, Some(false));

        match rx.try_recv().expect("post-tick should emit a node event") {
            crate::server_event::ServerEvent::NodeState { nodes } => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].id, "room1");
                assert!(!nodes[0].lights_on);
                assert!(nodes[0].tick);
            }
            _ => panic!("unexpected event type"),
        }
    }

    // ========================================================================
    // dispatch_spacing edge cases
    // ========================================================================

    #[test]
    fn dispatch_spacing_handles_more_rooms_than_seconds() {
        // 60 rooms in a 1-second cycle: spacing collapses to ~16ms each.
        let cycle = Duration::from_secs(1);
        let spacing = dispatch_spacing(cycle, 60);
        assert!(spacing.as_millis() <= 17 && spacing.as_millis() >= 16);
    }

    // ========================================================================
    // Tick latency classifier
    // ========================================================================

    #[test]
    fn classify_tick_latency_below_threshold_is_on_time() {
        assert_eq!(
            classify_tick_latency(Duration::from_millis(100)),
            TickLatencyOutcome::OnTime
        );
        assert_eq!(
            classify_tick_latency(Duration::from_secs(1)),
            TickLatencyOutcome::OnTime
        );
        // Just under the threshold.
        assert_eq!(
            classify_tick_latency(SLOW_TICK_WARNING_THRESHOLD - Duration::from_millis(1)),
            TickLatencyOutcome::OnTime
        );
    }

    #[test]
    fn classify_tick_latency_at_or_above_threshold_is_slow() {
        assert_eq!(
            classify_tick_latency(SLOW_TICK_WARNING_THRESHOLD),
            TickLatencyOutcome::Slow
        );
        assert_eq!(
            classify_tick_latency(SLOW_TICK_WARNING_THRESHOLD + Duration::from_millis(1)),
            TickLatencyOutcome::Slow
        );
        assert_eq!(
            classify_tick_latency(Duration::from_secs(60)),
            TickLatencyOutcome::Slow
        );
    }

    #[test]
    fn slow_tick_threshold_is_finite_and_reasonable() {
        // Anchor: the warning threshold must stay well below the default
        // periodic cycle so we get warnings before queue saturation kicks
        // in. 5 seconds matches the engineering intent at the time of
        // writing; if it changes, this test should be updated alongside.
        assert!(SLOW_TICK_WARNING_THRESHOLD >= Duration::from_secs(1));
        assert!(SLOW_TICK_WARNING_THRESHOLD <= Duration::from_secs(30));
    }

    #[test]
    fn stable_room_phase_key_handles_empty_string() {
        let _ = stable_room_phase_key("");
        // Should not panic; value not asserted (FNV offset basis is fine).
    }

    fn make_room(id: &str, time_offset_minutes: f32) -> rhythm_core::NodeSnapshot {
        rhythm_core::NodeSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            kind: rhythm_core::LightNodeKind::Room,
            parent_id: None,
            rhythm_enabled: true,
            disabled: false,
            time_offset_minutes,
            brightness_offset: 0.0,
            soft_off: false,
            hard_off: false,
            profile_settings: RoomProfileSettings::default(),
        }
    }

    fn make_node(
        id: &str,
        kind: rhythm_core::LightNodeKind,
        parent_id: Option<&str>,
    ) -> rhythm_core::NodeSnapshot {
        rhythm_core::NodeSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            kind,
            parent_id: parent_id.map(str::to_string),
            rhythm_enabled: true,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off: false,
            hard_off: false,
            profile_settings: RoomProfileSettings::default(),
        }
    }

    #[test]
    fn stable_room_phase_key_is_deterministic() {
        assert_eq!(
            stable_room_phase_key("room-a"),
            stable_room_phase_key("room-a")
        );
        assert_ne!(
            stable_room_phase_key("room-a"),
            stable_room_phase_key("room-b")
        );
    }

    #[test]
    fn dispatch_spacing_spreads_rooms_across_cycle() {
        assert_eq!(dispatch_spacing(Duration::from_secs(60), 0), Duration::ZERO);
        assert_eq!(
            dispatch_spacing(Duration::from_secs(60), 1),
            Duration::from_secs(60)
        );
        assert_eq!(
            dispatch_spacing(Duration::from_secs(60), 5),
            Duration::from_secs(12)
        );
    }

    #[test]
    fn effective_cycle_duration_uses_fastest_room_offset() {
        let registry = LightProfileRegistry::with_profiles(
            vec![default_rhythm_profile()],
            rhythm_core::RHYTHM_PROFILE_ID,
        );
        let ctx = CurveContext::new(12.0, rhythm_core::SolarTime::new(12.0, 35.0, 172), None);
        let midday_only = vec![make_room("midday", 0.0)];
        let mixed_offsets = vec![make_room("midday", 0.0), make_room("morning", -300.0)];

        let midday_cycle = effective_cycle_duration(
            &registry,
            &ctx,
            &midday_only,
            Duration::from_secs(60),
            false,
        );
        let mixed_cycle = effective_cycle_duration(
            &registry,
            &ctx,
            &mixed_offsets,
            Duration::from_secs(60),
            false,
        );

        assert!(
            mixed_cycle < midday_cycle,
            "A room still on its ramp should force a faster shared cadence"
        );
    }

    #[test]
    fn enqueue_periodic_tick_coalesces_latest_hour() {
        let state = make_state();
        let (tx, rx) = std::sync::mpsc::sync_channel::<WorkItem>(4);
        let dispatch_generation = state.lock().unwrap().light_dispatch_generation;

        assert!(enqueue_periodic_tick(
            &state,
            &tx,
            PeriodicTickEnqueue {
                command_id: "periodic-test-1",
                node_id: "node-1",
                settings_node_id: "room1",
                dispatch_generation,
                current_hour: 10.0,
                emit_parent_node_id: Some("room-parent"),
                dispatch_spacing: Duration::from_millis(250),
            },
        ));
        assert!(enqueue_periodic_tick(
            &state,
            &tx,
            PeriodicTickEnqueue {
                command_id: "periodic-test-2",
                node_id: "node-1",
                settings_node_id: "room1",
                dispatch_generation,
                current_hour: 10.5,
                emit_parent_node_id: Some("room-parent"),
                dispatch_spacing: Duration::from_millis(250),
            },
        ));

        let item = rx.try_recv().expect("first periodic item should be queued");
        match item {
            WorkItem::PeriodicNodeTick {
                command_id: _,
                node_id,
                settings_node_id,
                current_hour,
                emit_parent_node_id,
                ..
            } => {
                assert_eq!(node_id, "node-1");
                assert_eq!(settings_node_id, "room1");
                assert!((current_hour - 10.0).abs() < f32::EPSILON);
                assert_eq!(emit_parent_node_id.as_deref(), Some("room-parent"));
            }
            _ => panic!("unexpected work item"),
        }

        assert!(
            rx.try_recv().is_err(),
            "duplicate periodic item should be coalesced"
        );

        let pending = state.lock().unwrap();
        let latest = pending
            .pending_periodic_ticks
            .get("node-1")
            .copied()
            .expect("latest hour should be retained");
        assert!((latest - 10.5).abs() < f32::EPSILON);
    }

    #[test]
    fn enqueue_periodic_tick_skips_stale_generation() {
        let state = make_state();
        let (tx, rx) = std::sync::mpsc::sync_channel::<WorkItem>(4);
        let stale_generation = state.lock().unwrap().light_dispatch_generation;
        state.lock().unwrap().invalidate_queued_light_dispatches();

        assert!(enqueue_periodic_tick(
            &state,
            &tx,
            PeriodicTickEnqueue {
                command_id: "periodic-test-stale",
                node_id: "node-1",
                settings_node_id: "room1",
                dispatch_generation: stale_generation,
                current_hour: 10.0,
                emit_parent_node_id: Some("room-parent"),
                dispatch_spacing: Duration::from_millis(250),
            },
        ));

        assert!(
            rx.try_recv().is_err(),
            "stale generation should not enqueue a worker item"
        );
        assert!(
            state.lock().unwrap().pending_periodic_ticks.is_empty(),
            "stale generation should not reserve pending tick state"
        );
    }

    #[test]
    fn periodic_dispatch_nodes_fall_back_to_room_ids_without_composite() {
        let state = crate::state::AppState::default();
        let snapshots = vec![make_room("room-a", 0.0)];

        assert_eq!(
            periodic_dispatch_nodes_from_state(&state, &snapshots),
            vec![PeriodicDispatchNode {
                node_id: "room-a".to_string(),
                settings_node_id: "room-a".to_string(),
                emit_node_id: "room-a".to_string(),
            }]
        );
    }

    #[test]
    fn periodic_dispatch_nodes_ignore_non_light_addressable_nodes() {
        let mut state = crate::state::AppState {
            composite_controller: Some(Arc::new(CompositeController::new())),
            ..Default::default()
        };

        let room_id = state.topology.create_room("Kitchen");
        state
            .topology
            .get_mut(&room_id)
            .unwrap()
            .upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: crate::canonical::identity::HubKey::new(
                    crate::hub::HubType::new("hue"),
                    "192.168.1.10",
                ),
                hub_room_id: "hue-room-1".to_string(),
                control_id: "gl-1".to_string(),
                light_device_ids: vec!["hue-light-1".to_string()],
            });

        let snapshots = vec![
            make_room(&room_id, 0.0),
            make_node(
                "button-1",
                rhythm_core::LightNodeKind::Button,
                Some(&room_id),
            ),
            make_node(
                "motion-1",
                rhythm_core::LightNodeKind::MotionSensor,
                Some(&room_id),
            ),
        ];
        let expected = state
            .topology
            .periodic_light_nodes(&state.canonical_registry);
        assert_eq!(expected.len(), 1);

        assert_eq!(
            periodic_dispatch_nodes_from_state(&state, &snapshots),
            vec![PeriodicDispatchNode {
                node_id: expected[0].id.clone(),
                settings_node_id: room_id,
                emit_node_id: expected[0].emit_node_id.clone(),
            }]
        );
    }

    #[test]
    fn last_periodic_node_index_tracks_last_node_per_emit_target_when_interleaved() {
        let nodes = vec![
            PeriodicDispatchNode {
                node_id: "node-a1".to_string(),
                settings_node_id: "room-a".to_string(),
                emit_node_id: "room-a".to_string(),
            },
            PeriodicDispatchNode {
                node_id: "node-b1".to_string(),
                settings_node_id: "room-b".to_string(),
                emit_node_id: "room-b".to_string(),
            },
            PeriodicDispatchNode {
                node_id: "node-a2".to_string(),
                settings_node_id: "room-a".to_string(),
                emit_node_id: "room-a".to_string(),
            },
        ];

        let indices = last_periodic_node_index_by_emit_target(&nodes);
        assert_eq!(indices.get("room-a"), Some(&2));
        assert_eq!(indices.get("room-b"), Some(&1));
    }

    #[test]
    fn periodic_dispatch_nodes_follow_topology_light_nodes_with_composite() {
        let mut state = crate::state::AppState {
            composite_controller: Some(Arc::new(CompositeController::new())),
            ..Default::default()
        };

        let room_id = state.topology.create_room("Kitchen");
        state
            .topology
            .get_mut(&room_id)
            .unwrap()
            .upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: crate::canonical::identity::HubKey::new(
                    crate::hub::HubType::new("hue"),
                    "192.168.1.10",
                ),
                hub_room_id: "hue-room-1".to_string(),
                control_id: "gl-1".to_string(),
                light_device_ids: vec!["hue-light-1".to_string()],
            });

        let snapshots = vec![make_room(&room_id, 0.0)];
        let expected = state
            .topology
            .periodic_light_nodes(&state.canonical_registry);
        assert_eq!(expected.len(), 1);

        assert_eq!(
            periodic_dispatch_nodes_from_state(&state, &snapshots),
            vec![PeriodicDispatchNode {
                node_id: expected[0].id.clone(),
                settings_node_id: room_id,
                emit_node_id: expected[0].emit_node_id.clone(),
            }]
        );
    }

    #[test]
    fn periodic_dispatch_nodes_collapse_attached_light_snapshots_with_composite() {
        let mut state = crate::state::AppState {
            composite_controller: Some(Arc::new(CompositeController::new())),
            ..Default::default()
        };

        let room_id = state.topology.create_room("Kitchen");
        state
            .topology
            .get_mut(&room_id)
            .unwrap()
            .upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: crate::canonical::identity::HubKey::new(
                    crate::hub::HubType::new("hue"),
                    "192.168.1.10",
                ),
                hub_room_id: "hue-room-1".to_string(),
                control_id: "gl-1".to_string(),
                light_device_ids: vec!["hue-light-1".to_string()],
            });

        let snapshots = vec![
            make_room(&room_id, 0.0),
            make_node(
                "attached-light-1",
                rhythm_core::LightNodeKind::LightDevice,
                Some(&room_id),
            ),
        ];
        let expected = state
            .topology
            .periodic_light_nodes(&state.canonical_registry);
        assert_eq!(expected.len(), 1);

        assert_eq!(
            periodic_dispatch_nodes_from_state(&state, &snapshots),
            vec![PeriodicDispatchNode {
                node_id: expected[0].id.clone(),
                settings_node_id: room_id,
                emit_node_id: expected[0].emit_node_id.clone(),
            }]
        );
    }

    #[test]
    fn periodic_dispatch_nodes_include_attached_device_routes_without_group_binding() {
        let mut state = crate::state::AppState {
            composite_controller: Some(Arc::new(CompositeController::new())),
            ..Default::default()
        };

        let room_id = state.topology.create_room("Bookcase");
        let matter_key =
            crate::canonical::identity::HubKey::new(crate::hub::HubType::new("matter"), "local");
        let identity = crate::canonical::identity::DiscoveredIdentity {
            native_id: "matter-102".to_string(),
            room_id: None,
            room_name: None,
            name: "Bookcase bulb".to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            hardware_ids: vec![],
            manufacturer: None,
            model: None,
        };
        let light_id = match state
            .canonical_registry
            .resolve(&identity, &matter_key, 1000)
        {
            crate::canonical::registry::ResolveResult::Created { canonical_id }
            | crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id }
            | crate::canonical::registry::ResolveResult::ReApproved { canonical_id } => {
                canonical_id
            }
            crate::canonical::registry::ResolveResult::Queued { .. } => {
                panic!("unexpected triage for test identity")
            }
        };
        assert!(state
            .topology
            .attach_device_user_override(&room_id, &light_id));

        let snapshots = vec![
            make_room(&room_id, 0.0),
            make_node(
                &light_id,
                rhythm_core::LightNodeKind::LightDevice,
                Some(&room_id),
            ),
        ];

        assert_eq!(
            periodic_dispatch_nodes_from_state(&state, &snapshots),
            vec![PeriodicDispatchNode {
                node_id: light_id,
                settings_node_id: room_id.clone(),
                emit_node_id: room_id,
            }]
        );
    }

    #[test]
    fn preview_dispatch_preserves_explicit_light_node_with_composite() {
        let target = make_node(
            "attached-light-1",
            rhythm_core::LightNodeKind::LightDevice,
            Some("room-a"),
        );

        assert_eq!(
            preview_dispatch_node_for_target(&target),
            Some(PeriodicDispatchNode {
                node_id: "attached-light-1".to_string(),
                settings_node_id: "attached-light-1".to_string(),
                emit_node_id: "room-a".to_string(),
            })
        );
    }

    #[test]
    fn preview_dispatch_preserves_explicit_room_with_composite() {
        let mut state = crate::state::AppState {
            composite_controller: Some(Arc::new(CompositeController::new())),
            ..Default::default()
        };

        let room_id = state.topology.create_room("Kitchen");
        state
            .topology
            .get_mut(&room_id)
            .unwrap()
            .upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: crate::canonical::identity::HubKey::new(
                    crate::hub::HubType::new("hue"),
                    "192.168.1.10",
                ),
                hub_room_id: "hue-room-1".to_string(),
                control_id: "gl-1".to_string(),
                light_device_ids: vec!["hue-light-1".to_string()],
            });

        assert_eq!(
            preview_dispatch_node_for_target(&make_room(&room_id, 0.0)),
            Some(PeriodicDispatchNode {
                node_id: room_id.clone(),
                settings_node_id: room_id.clone(),
                emit_node_id: room_id,
            })
        );
    }

    #[test]
    fn summarize_periodic_dispatch_counts_rooms_devices_and_missing_routes() {
        let node_snapshots = vec![
            make_node("room-a", rhythm_core::LightNodeKind::Room, None),
            make_node(
                "light-a",
                rhythm_core::LightNodeKind::LightDevice,
                Some("room-a"),
            ),
            make_node("room-b", rhythm_core::LightNodeKind::Room, None),
        ];
        let dispatch_nodes = vec![
            PeriodicDispatchNode {
                node_id: "dispatch-room-a".to_string(),
                settings_node_id: "room-a".to_string(),
                emit_node_id: "room-a".to_string(),
            },
            PeriodicDispatchNode {
                node_id: "dispatch-light-a".to_string(),
                settings_node_id: "light-a".to_string(),
                emit_node_id: "room-a".to_string(),
            },
        ];

        assert_eq!(
            summarize_periodic_dispatch(&node_snapshots, &dispatch_nodes, false),
            PeriodicDispatchSummary {
                eligible_node_count: 3,
                eligible_room_count: 2,
                eligible_device_count: 1,
                dispatch_node_count: 2,
                dispatched_settings_node_count: 2,
                no_dispatch_node_count: 1,
            }
        );
    }

    #[test]
    fn summarize_periodic_dispatch_collapses_attached_lights_in_composite_mode() {
        let node_snapshots = vec![
            make_node("room-a", rhythm_core::LightNodeKind::Room, None),
            make_node(
                "light-a",
                rhythm_core::LightNodeKind::LightDevice,
                Some("room-a"),
            ),
            make_node("room-b", rhythm_core::LightNodeKind::Room, None),
        ];
        let dispatch_nodes = vec![PeriodicDispatchNode {
            node_id: "dispatch-room-a".to_string(),
            settings_node_id: "room-a".to_string(),
            emit_node_id: "room-a".to_string(),
        }];

        assert_eq!(
            summarize_periodic_dispatch(&node_snapshots, &dispatch_nodes, true),
            PeriodicDispatchSummary {
                eligible_node_count: 2,
                eligible_room_count: 2,
                eligible_device_count: 0,
                dispatch_node_count: 1,
                dispatched_settings_node_count: 1,
                no_dispatch_node_count: 1,
            }
        );
    }

    /// When no timezone is set, refresh_dst_offset is a no-op.
    #[test]
    fn test_refresh_no_timezone_is_noop() {
        let state = make_state();
        let (offset, noon, doy) = refresh_dst_offset(&state, -5.0, 12.5, 35.0, -80.0, 100, None);
        assert_eq!(offset, -5.0);
        assert_eq!(noon, 12.5);
        assert_eq!(doy, 100);
    }

    /// When the offset already matches the current timezone offset,
    /// the function returns the same values (no state mutation).
    #[test]
    fn test_refresh_same_offset_is_noop() {
        let state = make_state();
        let tz_name = "America/New_York";
        let tz = rhythm_core::Timezone::new(tz_name);
        let (year, month, day, hour) = tz.local_date_hour_from_utc(chrono::Utc::now().naive_utc());
        let current_offset = tz.utc_offset(year, month, day, hour);

        let (offset, noon, doy) = refresh_dst_offset(
            &state,
            current_offset,
            12.5,
            35.0,
            -80.0,
            100,
            Some(tz_name),
        );

        // Offset should be unchanged
        assert!((offset - current_offset).abs() < 0.01);
        // Solar noon and doy should also be unchanged (no DST transition)
        assert_eq!(noon, 12.5);
        assert_eq!(doy, 100);
    }

    /// When the stored offset is stale (simulating a DST transition),
    /// the function updates offset, solar noon, and state.
    #[test]
    fn test_refresh_detects_stale_offset() {
        let state = make_state();
        let tz_name = "America/New_York";
        let tz = rhythm_core::Timezone::new(tz_name);
        let (year, month, day, hour) = tz.local_date_hour_from_utc(chrono::Utc::now().naive_utc());
        let current_offset = tz.utc_offset(year, month, day, hour);

        // Deliberately pass a wrong offset (off by 1 hour, simulating a DST miss)
        let stale_offset = current_offset + 1.0;

        let (offset, noon, _doy) =
            refresh_dst_offset(&state, stale_offset, 12.5, 35.0, -80.0, 100, Some(tz_name));

        // Should have corrected to the actual offset
        assert!(
            (offset - current_offset).abs() < 0.01,
            "Expected offset {:.1}, got {:.1}",
            current_offset,
            offset
        );

        // Solar noon should have been recomputed (not the original 12.5)
        assert!(
            (noon - 12.5).abs() > 0.01,
            "Solar noon should have been recomputed, still got 12.5"
        );

        // State should have been updated
        let s = state.lock().unwrap();
        assert!(
            (s.utc_offset_hours - current_offset).abs() < 0.01,
            "State offset should be updated to {:.1}, got {:.1}",
            current_offset,
            s.utc_offset_hours
        );
    }

    /// Verify that the Spring forward DST boundary produces different offsets
    /// when queried for the day before vs after (using rhythm_core directly).
    #[test]
    fn test_dst_spring_forward_offset_changes() {
        let tz = rhythm_core::Timezone::new("America/New_York");

        // 2025 spring forward: March 9
        let before = tz.utc_offset(2025, 3, 8, 12); // March 8 = EST
        let after = tz.utc_offset(2025, 3, 10, 12); // March 10 = EDT

        assert_eq!(before, -5.0, "Before spring forward should be EST (-5)");
        assert_eq!(after, -4.0, "After spring forward should be EDT (-4)");

        // If the periodic loop stored -5.0 and spring forward happened,
        // refresh_dst_offset should detect the 1-hour difference
        assert!(
            (after - before).abs() > 0.01,
            "DST transition should produce a different offset"
        );
    }

    /// Verify that the Fall back DST boundary produces different offsets.
    #[test]
    fn test_dst_fall_back_offset_changes() {
        let tz = rhythm_core::Timezone::new("America/New_York");

        // 2025 fall back: November 2
        let before = tz.utc_offset(2025, 11, 1, 12); // Nov 1 = EDT
        let after = tz.utc_offset(2025, 11, 3, 12); // Nov 3 = EST

        assert_eq!(before, -4.0, "Before fall back should be EDT (-4)");
        assert_eq!(after, -5.0, "After fall back should be EST (-5)");
    }

    // ========================================================================
    // check_solar_midnight tests
    // ========================================================================

    #[test]
    fn solar_midnight_first_call_is_noop() {
        let state = make_state();
        // last_check_hour starts as None, so first call just sets it
        check_solar_midnight(&state, 14.0);
        let s = state.lock().unwrap();
        assert_eq!(s.last_check_hour, Some(14.0));
        assert!(s.last_check_instant.is_some());
        assert_eq!(s.last_check_utc_offset_hours, Some(0.0));
    }

    #[test]
    fn solar_midnight_no_crossing_same_side() {
        let state = make_state();
        // Set initial last_check_hour
        state.lock().unwrap().last_check_hour = Some(14.0);
        // Default solar noon = 12.5, so midnight = 0.5
        // 14.0 → 15.0 doesn't cross 0.5
        check_solar_midnight(&state, 15.0);
        let s = state.lock().unwrap();
        assert_eq!(s.last_check_hour, Some(15.0));
    }

    #[test]
    fn solar_midnight_crossing_resets_offsets() {
        // We need a runtime to verify offset reset
        use rhythm_core::{
            InputEvent, LightNodeKind, LightProfileConfig, RestoredRoomState, RoomSnapshot,
            RuntimeHandle,
        };
        use std::sync::Mutex as StdMutex;

        struct MockRuntime {
            snapshots: StdMutex<Vec<RoomSnapshot>>,
            restore_calls: StdMutex<Vec<(String, f32, f32)>>,
        }

        impl RuntimeHandle for MockRuntime {
            fn handle_event(&self, _: &InputEvent) -> anyhow::Result<bool> {
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
            fn engine_room_snapshot(&self, id: &str) -> Option<RoomSnapshot> {
                self.snapshots
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|s| s.id == id)
                    .cloned()
            }
            fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
                self.snapshots.lock().unwrap().clone()
            }
            fn restore_room_state(&self, room_id: &str, state: RestoredRoomState) {
                self.restore_calls.lock().unwrap().push((
                    room_id.to_string(),
                    state.time_offset_minutes,
                    state.brightness_offset,
                ));
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
                "rhythm".into()
            }
            fn available_light_profiles(&self) -> Vec<(String, String)> {
                vec![
                    ("rhythm".into(), "Rhythm Curve".into()),
                    ("sleep".into(), "Sleep Curve".into()),
                ]
            }
        }

        let runtime = Arc::new(MockRuntime {
            snapshots: StdMutex::new(vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                kind: LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 30.0,
                brightness_offset: 10.0,
                soft_off: false,
                hard_off: false,
                profile_settings: rhythm_core::RoomProfileSettings::default(),
            }]),
            restore_calls: StdMutex::new(Vec::new()),
        });

        let state = make_state();
        let observed_at = Instant::now();
        {
            let mut s = state.lock().unwrap();
            // Solar noon = 12.5, midnight = 0.5
            s.runtime_config.solar_noon_hour = 12.5;
            s.last_check_hour = Some(0.4); // just before midnight
            s.last_check_instant = Some(observed_at - Duration::from_secs(12 * 60));
            s.last_check_utc_offset_hours = Some(0.0);
            let hub_type = crate::hub::HubType::parse("mock").unwrap();
            let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
            s.hubs.insert(
                hub_key.clone(),
                crate::hub::ActiveHub {
                    hub_type,
                    hub_key,
                    runtime: Some(runtime.clone() as Arc<dyn RuntimeHandle>),
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: None,
                    shutdown: Default::default(),
                },
            );
        }

        // Cross solar midnight: 0.4 → 0.6 crosses 0.5
        let result = check_solar_midnight_at(&state, 0.6, 0.0, observed_at);
        assert_eq!(
            result,
            PeriodicTimeCheckResult::Continuous { last_hour: 0.4 }
        );

        let calls = runtime.restore_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "room1");
        assert!(
            (calls[0].1 - 0.0).abs() < f32::EPSILON,
            "time_offset should be reset to 0"
        );
        assert!(
            (calls[0].2 - 0.0).abs() < f32::EPSILON,
            "bri_offset should be reset to 0"
        );
    }

    #[test]
    fn solar_midnight_no_crossing_when_no_runtime() {
        let state = make_state();
        let observed_at = Instant::now();
        {
            let mut s = state.lock().unwrap();
            s.runtime_config.solar_noon_hour = 12.5;
            s.last_check_hour = Some(0.4);
            s.last_check_instant = Some(observed_at - Duration::from_secs(12 * 60));
            s.last_check_utc_offset_hours = Some(0.0);
        }
        // This should not panic even with no runtime
        let _ = check_solar_midnight_at(&state, 0.6, 0.0, observed_at);
    }

    #[test]
    fn solar_midnight_large_clock_jump_is_reseeded() {
        use rhythm_core::{
            InputEvent, LightNodeKind, LightProfileConfig, RestoredRoomState, RoomSnapshot,
            RuntimeHandle,
        };
        use std::sync::Mutex as StdMutex;

        struct MockRuntime {
            snapshots: StdMutex<Vec<RoomSnapshot>>,
            restore_calls: StdMutex<Vec<(String, f32, f32)>>,
        }

        impl RuntimeHandle for MockRuntime {
            fn handle_event(&self, _: &InputEvent) -> anyhow::Result<bool> {
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
            fn engine_room_snapshot(&self, id: &str) -> Option<RoomSnapshot> {
                self.snapshots
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|s| s.id == id)
                    .cloned()
            }
            fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
                self.snapshots.lock().unwrap().clone()
            }
            fn restore_room_state(&self, room_id: &str, state: RestoredRoomState) {
                self.restore_calls.lock().unwrap().push((
                    room_id.to_string(),
                    state.time_offset_minutes,
                    state.brightness_offset,
                ));
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
                "rhythm".into()
            }
            fn available_light_profiles(&self) -> Vec<(String, String)> {
                vec![
                    ("rhythm".into(), "Rhythm Curve".into()),
                    ("sleep".into(), "Sleep Curve".into()),
                ]
            }
        }

        let runtime = Arc::new(MockRuntime {
            snapshots: StdMutex::new(vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                kind: LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 30.0,
                brightness_offset: 10.0,
                soft_off: false,
                hard_off: false,
                profile_settings: rhythm_core::RoomProfileSettings::default(),
            }]),
            restore_calls: StdMutex::new(Vec::new()),
        });

        let state = make_state();
        let observed_at = Instant::now();
        {
            let mut s = state.lock().unwrap();
            s.runtime_config.solar_noon_hour = 13.226_456;
            s.last_check_hour = Some(0.03);
            s.last_check_instant = Some(observed_at - Duration::from_secs(3 * 60));
            s.last_check_utc_offset_hours = Some(-5.0);
            let hub_type = crate::hub::HubType::parse("mock").unwrap();
            let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
            s.hubs.insert(
                hub_key.clone(),
                crate::hub::ActiveHub {
                    hub_type,
                    hub_key,
                    runtime: Some(runtime.clone() as Arc<dyn RuntimeHandle>),
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: None,
                    shutdown: Default::default(),
                },
            );
        }

        let result = check_solar_midnight_at(&state, 15.67, -4.0, observed_at);
        assert_eq!(
            result,
            PeriodicTimeCheckResult::Discontinuous {
                last_hour: 0.03,
                adjusted_delta_hours: -8.36,
                expected_delta_hours: 1.05,
            }
        );
        assert!(runtime.restore_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn solar_midnight_allows_dst_fall_back_without_reseed() {
        let state = make_state();
        let observed_at = Instant::now();
        {
            let mut s = state.lock().unwrap();
            s.runtime_config.solar_noon_hour = 12.5;
            s.last_check_hour = Some(1.9);
            s.last_check_instant = Some(observed_at - Duration::from_secs(3 * 60));
            s.last_check_utc_offset_hours = Some(-4.0);
        }

        let result = check_solar_midnight_at(&state, 1.1, -5.0, observed_at);
        assert_eq!(
            result,
            PeriodicTimeCheckResult::Continuous { last_hour: 1.9 }
        );
    }

    // ========================================================================
    // update_interval_secs tests
    // ========================================================================

    #[test]
    fn update_interval_reads_from_state() {
        let state = make_state();
        state.lock().unwrap().runtime_config.update_interval_secs = 120;
        assert_eq!(update_interval_secs(&state), 120);
    }

    #[test]
    fn update_interval_default() {
        let state = make_state();
        // Default RuntimeConfig has 60s interval
        assert_eq!(update_interval_secs(&state), 60);
    }

    #[test]
    fn mode_transition_requires_matching_trigger() {
        let state = make_state();
        {
            let mut s = state.lock().unwrap();
            s.active_mode = rhythm_core::RhythmMode::Sleep;
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                rhythm_core::RhythmMode::Sleep,
                rhythm_core::RhythmMode::Day,
                1000,
            )]);
        }

        let sunrise = rhythm_core::config::FALLBACK_SUNRISE_HOUR;
        check_mode_transitions(&state, sunrise - 0.1, sunrise + 0.1);

        assert_eq!(
            state.lock().unwrap().active_mode,
            rhythm_core::RhythmMode::Sleep
        );
    }

    #[test]
    fn astronomical_twilight_sleep_transition_switches_mode_when_trigger_configured() {
        let state = make_state();
        state.lock().unwrap().active_mode = rhythm_core::RhythmMode::Sleep;

        let astronomical_dawn = (rhythm_core::config::FALLBACK_SUNRISE_HOUR - 1.5).rem_euclid(24.0);
        check_mode_transitions(&state, astronomical_dawn - 0.1, astronomical_dawn + 0.1);

        assert_eq!(
            state.lock().unwrap().active_mode,
            rhythm_core::RhythmMode::Day
        );
    }

    #[test]
    fn nautical_twilight_day_to_sleep_transition_switches_mode() {
        let state = make_state();
        state.lock().unwrap().active_mode = rhythm_core::RhythmMode::Day;

        let nautical_dusk = (rhythm_core::config::FALLBACK_SUNSET_HOUR + 1.0).rem_euclid(24.0);
        check_mode_transitions(&state, nautical_dusk - 0.1, nautical_dusk + 0.1);

        assert_eq!(
            state.lock().unwrap().active_mode,
            rhythm_core::RhythmMode::Sleep
        );
    }

    #[test]
    fn scheduled_day_to_sleep_transition_switches_mode() {
        let state = make_state();
        {
            let mut s = state.lock().unwrap();
            s.active_mode = rhythm_core::RhythmMode::Day;
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                rhythm_core::RhythmMode::Day,
                rhythm_core::RhythmMode::Sleep,
                1_000,
            )
            .with_trigger(rhythm_core::ModeTransitionTrigger::Scheduled(
                rhythm_core::ModeTransitionTime::from_hour_minute(22, 0).unwrap(),
            ))]);
        }

        check_mode_transitions(&state, 21.9, 22.1);

        assert_eq!(
            state.lock().unwrap().active_mode,
            rhythm_core::RhythmMode::Sleep
        );
    }

    #[test]
    fn disabled_trigger_does_not_switch_mode() {
        let state = make_state();
        {
            let mut s = state.lock().unwrap();
            s.active_mode = rhythm_core::RhythmMode::Day;
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                rhythm_core::RhythmMode::Day,
                rhythm_core::RhythmMode::Sleep,
                1_000,
            )
            .with_trigger(rhythm_core::ModeTransitionTrigger::Scheduled(
                rhythm_core::ModeTransitionTime::from_hour_minute(22, 0).unwrap(),
            ))
            .with_trigger_enabled(false)]);
        }

        check_mode_transitions(&state, 21.9, 22.1);

        assert_eq!(
            state.lock().unwrap().active_mode,
            rhythm_core::RhythmMode::Day
        );
    }

    #[test]
    fn resolved_replay_applies_missed_astronomical_dawn_transition() {
        let tz_name = "America/New_York";
        let tz = rhythm_core::Timezone::new(tz_name);
        let start_local = NaiveDate::from_ymd_opt(2026, 4, 8)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let end_local = NaiveDate::from_ymd_opt(2026, 4, 9)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let start_utc = tz.utc_datetime_from_local(start_local).unwrap();
        let end_utc = tz.utc_datetime_from_local(end_local).unwrap();

        let resolved = resolved_replayed_mode_transition(
            rhythm_core::RhythmMode::Sleep,
            start_utc,
            end_utc,
            ReplayTransitionContext {
                solar: SolarTriggerContext {
                    solar_noon: 12.5,
                    latitude: Some(35.804102),
                    longitude: Some(-78.799_3),
                    timezone_name: Some(tz_name),
                },
                utc_offset: -4.0,
                configs: &rhythm_core::default_mode_transition_configs(),
            },
        );

        let transition = resolved.expect("expected replayed solar transition");
        assert_eq!(transition.to_mode, rhythm_core::RhythmMode::Day);
        assert_eq!(
            transition.trigger,
            rhythm_core::ModeTransitionTrigger::AstronomicalTwilight
        );
    }

    #[test]
    fn resolved_replay_skips_when_mode_returns_to_starting_state() {
        let tz_name = "America/New_York";
        let tz = rhythm_core::Timezone::new(tz_name);
        let start_local = NaiveDate::from_ymd_opt(2026, 4, 8)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let end_local = NaiveDate::from_ymd_opt(2026, 4, 9)
            .unwrap()
            .and_hms_opt(23, 0, 0)
            .unwrap();
        let start_utc = tz.utc_datetime_from_local(start_local).unwrap();
        let end_utc = tz.utc_datetime_from_local(end_local).unwrap();

        let resolved = resolved_replayed_mode_transition(
            rhythm_core::RhythmMode::Sleep,
            start_utc,
            end_utc,
            ReplayTransitionContext {
                solar: SolarTriggerContext {
                    solar_noon: 12.5,
                    latitude: Some(35.804102),
                    longitude: Some(-78.799_3),
                    timezone_name: Some(tz_name),
                },
                utc_offset: -4.0,
                configs: &rhythm_core::default_mode_transition_configs(),
            },
        );

        assert_eq!(resolved, None);
    }

    #[test]
    fn resolved_replay_applies_missed_scheduled_transition() {
        let tz_name = "America/New_York";
        let tz = rhythm_core::Timezone::new(tz_name);
        let start_local = NaiveDate::from_ymd_opt(2026, 4, 8)
            .unwrap()
            .and_hms_opt(21, 0, 0)
            .unwrap();
        let end_local = NaiveDate::from_ymd_opt(2026, 4, 8)
            .unwrap()
            .and_hms_opt(23, 0, 0)
            .unwrap();
        let start_utc = tz.utc_datetime_from_local(start_local).unwrap();
        let end_utc = tz.utc_datetime_from_local(end_local).unwrap();

        let configs = vec![rhythm_core::ModeTransitionConfig::new(
            rhythm_core::RhythmMode::Day,
            rhythm_core::RhythmMode::Sleep,
            1_000,
        )
        .with_trigger(rhythm_core::ModeTransitionTrigger::Scheduled(
            rhythm_core::ModeTransitionTime::from_hour_minute(22, 0).unwrap(),
        ))];

        let resolved = resolved_replayed_mode_transition(
            rhythm_core::RhythmMode::Day,
            start_utc,
            end_utc,
            ReplayTransitionContext {
                solar: SolarTriggerContext {
                    solar_noon: 12.5,
                    latitude: Some(35.804102),
                    longitude: Some(-78.799_3),
                    timezone_name: Some(tz_name),
                },
                utc_offset: -4.0,
                configs: &configs,
            },
        );

        let transition = resolved.expect("expected replayed scheduled transition");
        assert_eq!(transition.to_mode, rhythm_core::RhythmMode::Sleep);
        assert_eq!(
            transition.trigger,
            rhythm_core::ModeTransitionTrigger::Scheduled(
                rhythm_core::ModeTransitionTime::from_hour_minute(22, 0).unwrap(),
            )
        );
    }

    #[test]
    fn resolved_replay_skips_disabled_transition() {
        let tz_name = "America/New_York";
        let tz = rhythm_core::Timezone::new(tz_name);
        let start_local = NaiveDate::from_ymd_opt(2026, 4, 8)
            .unwrap()
            .and_hms_opt(21, 0, 0)
            .unwrap();
        let end_local = NaiveDate::from_ymd_opt(2026, 4, 8)
            .unwrap()
            .and_hms_opt(23, 0, 0)
            .unwrap();
        let start_utc = tz.utc_datetime_from_local(start_local).unwrap();
        let end_utc = tz.utc_datetime_from_local(end_local).unwrap();

        let configs = vec![rhythm_core::ModeTransitionConfig::new(
            rhythm_core::RhythmMode::Day,
            rhythm_core::RhythmMode::Sleep,
            1_000,
        )
        .with_trigger(rhythm_core::ModeTransitionTrigger::Scheduled(
            rhythm_core::ModeTransitionTime::from_hour_minute(22, 0).unwrap(),
        ))
        .with_trigger_enabled(false)];

        let resolved = resolved_replayed_mode_transition(
            rhythm_core::RhythmMode::Day,
            start_utc,
            end_utc,
            ReplayTransitionContext {
                solar: SolarTriggerContext {
                    solar_noon: 12.5,
                    latitude: Some(35.804102),
                    longitude: Some(-78.799_3),
                    timezone_name: Some(tz_name),
                },
                utc_offset: -4.0,
                configs: &configs,
            },
        );

        assert_eq!(resolved, None);
    }

    #[test]
    fn replay_wrapper_applies_scheduled_transition_after_manual_mode_change() {
        let state = make_state();
        let now_utc = chrono::Utc::now().naive_utc();
        let start_utc = now_utc - chrono::Duration::hours(3);
        let trigger_utc = now_utc - chrono::Duration::hours(1);
        let trigger_time = rhythm_core::ModeTransitionTime::from_hour_minute(
            trigger_utc.hour() as u8,
            trigger_utc.minute() as u8,
        )
        .unwrap();
        {
            let mut s = state.lock().unwrap();
            s.active_mode = rhythm_core::RhythmMode::Day;
            s.last_active_mode_cause = rhythm_core::ModeChangeCause::Manual;
            s.last_active_mode_change_utc_ms = Some(start_utc.and_utc().timestamp_millis());
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                rhythm_core::RhythmMode::Day,
                rhythm_core::RhythmMode::Sleep,
                1_000,
            )
            .with_trigger(rhythm_core::ModeTransitionTrigger::Scheduled(trigger_time))]);
        }

        replay_missed_mode_transitions(&state);

        assert_eq!(
            state.lock().unwrap().active_mode,
            rhythm_core::RhythmMode::Sleep
        );
    }

    #[test]
    fn replay_wrapper_preserves_manual_override_after_scheduled_transition() {
        let state = make_state();
        let now_utc = chrono::Utc::now().naive_utc();
        let start_utc = now_utc - chrono::Duration::minutes(30);
        let trigger_utc = now_utc - chrono::Duration::hours(1);
        let trigger_time = rhythm_core::ModeTransitionTime::from_hour_minute(
            trigger_utc.hour() as u8,
            trigger_utc.minute() as u8,
        )
        .unwrap();
        {
            let mut s = state.lock().unwrap();
            s.active_mode = rhythm_core::RhythmMode::Day;
            s.last_active_mode_cause = rhythm_core::ModeChangeCause::Manual;
            s.last_active_mode_change_utc_ms = Some(start_utc.and_utc().timestamp_millis());
            s.set_mode_transition_configs(vec![rhythm_core::ModeTransitionConfig::new(
                rhythm_core::RhythmMode::Day,
                rhythm_core::RhythmMode::Sleep,
                1_000,
            )
            .with_trigger(rhythm_core::ModeTransitionTrigger::Scheduled(trigger_time))]);
        }

        replay_missed_mode_transitions(&state);

        assert_eq!(
            state.lock().unwrap().active_mode,
            rhythm_core::RhythmMode::Day
        );
    }
}
