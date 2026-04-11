//! Periodic room tick orchestration.
//!
//! Extracted from ESP32 main.rs so the same periodic update logic can
//! be reused across targets (rhythm-server, future Raspberry Pi, etc.).
//!
//! The periodic updater runs on a fixed interval, ticking each room's
//! engine to update light values based on the current time/solar position.
//!
//! Both `run_periodic_loop` and `check_solar_midnight` use blocking
//! `std::thread::sleep` and `BlockingTimeProvider`. On rhythm-server,
//! this runs inside `tokio::task::spawn_blocking()`.

#[cfg(feature = "blocking")]
use std::thread;
use std::time::Duration;
#[cfg(feature = "blocking")]
use std::time::Instant;

use log::{debug, info, warn};
#[cfg(feature = "blocking")]
use rhythm_core::{BlockingTimeProvider, TimeProvider};

use std::sync::Arc;

use rhythm_core::runtime::RuntimeHandle;

use crate::state::SharedState;
#[cfg(feature = "blocking")]
use crate::state::WorkItem;
#[cfg(feature = "blocking")]
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

fn periodic_room_state(
    room: &rhythm_core::RoomSnapshot,
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
    room_snapshots: &[rhythm_core::RoomSnapshot],
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
        let rooms = if room_suggestions.is_empty() {
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
            "effective cycle math mode={:?} base_hour={:.3} fallback={}s update_interval={}s room_count={} chosen={}s rooms=[{}]",
            profile_registry.active_mode(),
            ctx.current_hour,
            fallback_secs,
            update_interval.as_secs(),
            room_snapshots.len(),
            chosen_secs,
            rooms
        );
    }

    Duration::from_secs(chosen_secs)
}

#[cfg(feature = "blocking")]
fn resolve_periodic_sun_times(
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&str>,
    utc_offset: f32,
) -> Option<rhythm_core::SunTimes> {
    let lat = latitude?;
    let lon = longitude?;
    let tz_name = timezone_name?;

    let local_now =
        chrono::Utc::now().naive_utc() + chrono::Duration::seconds((utc_offset * 3600.0) as i64);
    let (year, month, day) = (
        chrono::Datelike::year(&local_now.date()),
        chrono::Datelike::month(&local_now.date()),
        chrono::Datelike::day(&local_now.date()),
    );
    let tz = rhythm_core::Timezone::new(tz_name);

    Some(rhythm_core::calculate_sun_times(
        lat, lon, year, month, day, &tz,
    ))
}

#[cfg(feature = "blocking")]
fn enqueue_periodic_tick(
    state: &SharedState,
    tx: &std::sync::mpsc::SyncSender<WorkItem>,
    room_id: &str,
    current_hour: f32,
) -> bool {
    let should_enqueue = {
        let Ok(mut s) = state.lock() else {
            return false;
        };
        match s.pending_periodic_ticks.entry(room_id.to_string()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let previous_hour = *entry.get();
                entry.insert(current_hour);
                debug!(
                    target: "sys",
                    "Periodic tick already pending for '{}', coalescing {:.2} -> {:.2}",
                    room_id,
                    previous_hour,
                    current_hour
                );
                false
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(current_hour);
                true
            }
        }
    };

    if !should_enqueue {
        return true;
    }

    match tx.try_send(WorkItem::PeriodicRoomTick {
        room_id: room_id.to_string(),
        current_hour,
    }) {
        Ok(()) => true,
        Err(_) => {
            if let Ok(mut s) = state.lock() {
                s.pending_periodic_ticks.remove(room_id);
            }
            false
        }
    }
}

/// Run the blocking periodic update loop.
#[cfg(feature = "blocking")]
///
/// Sleeps `update_interval_secs` between iterations. Each iteration:
/// 1. Calculates current hour via `BlockingTimeProvider`
/// 2. Logs curve values for the current time
/// 3. Gets rhythm-enabled room IDs, skipping rooms in warning-dim state
/// 4. Dispatches `PeriodicRoomTick` via `work_tx` if `Some`, or calls
///    `periodic_tick_room` inline if `None`
/// 5. Calls `check_solar_midnight()`
///
/// The `on_tick` callback is for platform-specific per-tick actions
/// (e.g., `led::led_periodic()` on ESP32).
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
            )
        };

        let lat = latitude.unwrap_or(35.0);
        let lon = longitude.unwrap_or(-80.84);

        let doy = BlockingTimeProvider::new(utc_offset).day_of_year();

        // Check for DST transition and refresh UTC offset if needed
        let (utc_offset, solar_noon, doy) = refresh_dst_offset(
            &state,
            utc_offset,
            solar_noon,
            lat,
            lon,
            doy,
            timezone_name.as_deref(),
        );

        let time_provider = BlockingTimeProvider::new(utc_offset);
        let current_hour = time_provider.current_hour();
        let sun_times =
            resolve_periodic_sun_times(latitude, longitude, timezone_name.as_deref(), utc_offset);

        // Calculate generic curve values (no offset) for logging
        let solar = rhythm_core::SolarTime::new(solar_noon, lat, doy);
        let ctx = rhythm_core::light_profile::CurveContext::new(current_hour, solar, sun_times);
        let module = profile_registry.active_profile();
        let values = module.calculate(&ctx);

        // Get rhythm-enabled room IDs, skipping rooms in warning-dim state
        let (
            mut room_snapshots,
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
                .retain(|_, transition| transition.ends_at > now);
            let expired_transitions = transitions_before - s.room_mode_transitions.len();
            let all_rooms: Vec<rhythm_core::RoomSnapshot> = s
                .hub_runtime()
                .map(|rt| rt.engine_all_room_snapshots())
                .unwrap_or_default();
            let mut warning_skipped = 0usize;
            let mut transition_skipped = 0usize;
            let mut rhythm_disabled_skipped = 0usize;
            let mut hard_off_rooms = 0usize;
            let rooms: Vec<rhythm_core::RoomSnapshot> = all_rooms
                .into_iter()
                .filter(|room| {
                    if !room.rhythm_enabled {
                        rhythm_disabled_skipped += 1;
                        return false;
                    }
                    if room.hard_off {
                        hard_off_rooms += 1;
                    }
                    if s.room_mode_transitions.contains_key(&room.id) {
                        transition_skipped += 1;
                        return false;
                    }
                    if s.motion_snapshots
                        .get(&room.id)
                        .is_some_and(|ms| ms.warning_active)
                    {
                        warning_skipped += 1;
                        false
                    } else {
                        true
                    }
                })
                .collect();
            (
                rooms,
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
        let room_ids: Vec<String> = room_snapshots.iter().map(|room| room.id.clone()).collect();

        if transition_skipped > 0
            || rhythm_disabled_skipped > 0
            || hard_off_rooms > 0
            || expired_transitions > 0
        {
            debug!(
                target: "sys",
                "Periodic tick detail: dispatch={} warning_skipped={} transition_skipped={} rhythm_disabled={} hard_off={} expired_transitions={}",
                room_ids.len(),
                warning_skipped,
                transition_skipped,
                rhythm_disabled_skipped,
                hard_off_rooms,
                expired_transitions
            );
        }

        if warning_skipped > 0 {
            info!(
                "Periodic tick at local_hour {:.2} (solar_time {:.2}) - bri={}% kelvin={} ({} rooms in rhythm, {} skipped: warning-dim)",
                current_hour,
                values.solar_time,
                values.brightness,
                values.kelvin,
                room_ids.len(),
                warning_skipped
            );
        } else {
            info!(
                "Periodic tick at local_hour {:.2} (solar_time {:.2}) - bri={}% kelvin={} ({} rooms in rhythm)",
                current_hour,
                values.solar_time,
                values.brightness,
                values.kelvin,
                room_ids.len()
            );
        }

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

        let cycle_duration = effective_cycle_duration(
            &profile_registry,
            &ctx,
            &room_snapshots,
            update_interval,
            power_save,
        );
        let phase_gap = dispatch_spacing(cycle_duration, room_ids.len());
        let cycle_started = Instant::now();

        // Dispatch per-room ticks with stable staggering across the cycle.
        if let Some(ref tx) = periodic_work_tx {
            for (idx, room_id) in room_ids.iter().enumerate() {
                let room_hour = BlockingTimeProvider::new(utc_offset).current_hour();
                if !enqueue_periodic_tick(&state, tx, room_id, room_hour) {
                    warn!(target: "sys", "Periodic queue full, dropping periodic tick for '{}'", room_id);
                }
                if idx + 1 < room_ids.len() && !phase_gap.is_zero() {
                    thread::sleep(phase_gap);
                }
            }
        } else if let Some(ref tx) = work_tx {
            for (idx, room_id) in room_ids.iter().enumerate() {
                let room_hour = BlockingTimeProvider::new(utc_offset).current_hour();
                if !enqueue_periodic_tick(&state, tx, room_id, room_hour) {
                    warn!(target: "sys", "Work queue full, dropping periodic tick for '{}'", room_id);
                }
                if idx + 1 < room_ids.len() && !phase_gap.is_zero() {
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
                for (idx, room_id) in room_ids.iter().enumerate() {
                    let room_hour = BlockingTimeProvider::new(utc_offset).current_hour();
                    if let Err(e) = runtime.periodic_tick_room(room_id, room_hour) {
                        warn!(target: "sys", "Periodic room tick '{}' failed: {}", room_id, e);
                    }
                    post_tick_room(&state, &runtime, room_id);
                    if idx + 1 < room_ids.len() && !phase_gap.is_zero() {
                        thread::sleep(phase_gap);
                    }
                }
            } else if !room_ids.is_empty() {
                debug!(
                    target: "sys",
                    "Periodic tick skipped {} queued room(s): no runtime available",
                    room_ids.len()
                );
            }
        }

        // Read last_check_hour before check_solar_midnight updates it
        let last_hour = state.lock().ok().and_then(|s| s.last_check_hour);
        let current_hour = BlockingTimeProvider::new(utc_offset).current_hour();
        check_solar_midnight(&state, current_hour);
        if let Some(last) = last_hour {
            check_mode_transitions(&state, last, current_hour);
        } else {
            replay_missed_mode_transitions(&state);
        }

        let elapsed = cycle_started.elapsed();
        if elapsed < cycle_duration {
            thread::sleep(cycle_duration - elapsed);
        }
    }
}

/// Process post-tick side effects for a single room.
///
/// Emits an SSE event with the room's current state on desktop.
pub fn post_tick_room(state: &SharedState, runtime: &Arc<dyn RuntimeHandle>, room_id: &str) {
    let Some(snap) = runtime.engine_room_snapshot(room_id) else {
        return;
    };

    #[cfg(feature = "desktop")]
    {
        let mut event = crate::commands::build_room_state_event(state, &snap);
        event.tick = true;
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::RoomState { rooms: vec![event] },
        );
    }

    // Suppress unused variable warnings on non-desktop builds
    let _ = (&state, &snap);
}

/// Check for solar midnight crossing and reset room offsets.
///
/// Solar midnight is when the sun is at its lowest point (opposite of solar noon).
/// At this crossing, per-room time and brightness offsets are reset to zero,
/// giving each day a clean start.
pub fn check_solar_midnight(state: &SharedState, current_hour: f32) {
    let (crossed, runtime) = {
        let Ok(mut s) = state.lock() else { return };

        let solar_midnight = s.solar_midnight_hour();
        let last_hour = s.last_check_hour;
        s.last_check_hour = Some(current_hour);

        let Some(last) = last_hour else {
            debug!(
                target: "sys",
                "Solar midnight check seeded at local_hour {:.2}",
                current_hour
            );
            return;
        };

        let crossed = rhythm_core::crossed_solar_midnight(last, current_hour, solar_midnight);

        if crossed {
            info!(
                "Solar midnight crossed (last_local={:.2}, now_local={:.2}, trigger_local={:.2}) - resetting offsets",
                last, current_hour, solar_midnight
            );
        }

        (crossed, s.hub_runtime())
    };

    if crossed {
        if let Some(runtime) = runtime {
            for snap in runtime.engine_all_room_snapshots() {
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

            #[cfg(feature = "desktop")]
            {
                let events: Vec<_> = runtime
                    .engine_all_room_snapshots()
                    .iter()
                    .map(|s| crate::commands::build_room_state_event(state, s))
                    .collect();
                if !events.is_empty() {
                    crate::state::emit_server_event(
                        state,
                        crate::server_event::ServerEvent::RoomState { rooms: events },
                    );
                }
            }
        }
    }
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

fn trigger_hour_for_local_date(
    trigger: rhythm_core::ModeTransitionTrigger,
    target_mode: rhythm_core::RhythmMode,
    solar_noon: f32,
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&str>,
    year: i32,
    month: u32,
    day: u32,
) -> Option<f32> {
    if let rhythm_core::ModeTransitionTrigger::Scheduled(time) = trigger {
        return Some(time.local_hour());
    }

    let estimated_sunrise = if latitude.is_some() && longitude.is_some() {
        (solar_noon - 6.0).rem_euclid(24.0)
    } else {
        rhythm_core::config::FALLBACK_SUNRISE_HOUR
    };
    let estimated_sunset = if latitude.is_some() && longitude.is_some() {
        (solar_noon + 6.0).rem_euclid(24.0)
    } else {
        rhythm_core::config::FALLBACK_SUNSET_HOUR
    };

    let Some(lat) = latitude else {
        return fallback_solar_trigger_hour(
            trigger,
            target_mode,
            estimated_sunrise,
            estimated_sunset,
        );
    };
    let Some(lon) = longitude else {
        return fallback_solar_trigger_hour(
            trigger,
            target_mode,
            estimated_sunrise,
            estimated_sunset,
        );
    };
    let Some(tz_name) = timezone_name else {
        return fallback_solar_trigger_hour(
            trigger,
            target_mode,
            estimated_sunrise,
            estimated_sunset,
        );
    };

    let tz = rhythm_core::Timezone::new(tz_name);
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
    solar_noon: f32,
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&str>,
) -> Option<f32> {
    if let Some(tz_name) = timezone_name {
        let tz = rhythm_core::Timezone::new(tz_name);
        let (year, month, day) = tz.local_date_from_utc(chrono::Utc::now().naive_utc());
        trigger_hour_for_local_date(
            trigger,
            target_mode,
            solar_noon,
            latitude,
            longitude,
            Some(tz_name),
            year,
            month,
            day,
        )
    } else {
        trigger_hour_for_local_date(
            trigger,
            target_mode,
            solar_noon,
            latitude,
            longitude,
            None,
            1970,
            1,
            1,
        )
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
    solar_noon: f32,
    utc_offset: f32,
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&str>,
    configs: &[rhythm_core::ModeTransitionConfig],
) -> Option<rhythm_core::ModeTransitionConfig> {
    if end_utc <= start_utc {
        return None;
    }

    let start_local = local_datetime_from_utc(start_utc, utc_offset, timezone_name);
    let end_local = local_datetime_from_utc(end_utc, utc_offset, timezone_name);
    let mut date = start_local.date();
    let end_date = end_local.date();
    let mut events = Vec::new();

    while date <= end_date {
        let Some(local_midnight) = date.and_hms_opt(0, 0, 0) else {
            break;
        };

        for config in configs
            .iter()
            .filter(|config| config.trigger != rhythm_core::ModeTransitionTrigger::Manual)
        {
            let Some(trigger_hour) = trigger_hour_for_local_date(
                config.trigger,
                config.to_mode,
                solar_noon,
                latitude,
                longitude,
                timezone_name,
                chrono::Datelike::year(&date),
                chrono::Datelike::month(&date),
                chrono::Datelike::day(&date),
            ) else {
                continue;
            };

            let trigger_seconds =
                ((trigger_hour.rem_euclid(24.0)) * 3600.0).round() as i64 % 86_400;
            let event_local = local_midnight + chrono::Duration::seconds(trigger_seconds);
            let Some(event_utc) = utc_datetime_from_local(event_local, utc_offset, timezone_name)
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

    let Some(transition) = final_transition else {
        return None;
    };
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

    if start_cause == rhythm_core::ModeChangeCause::Manual {
        return;
    }

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
        solar_noon,
        utc_offset,
        latitude,
        longitude,
        timezone_name.as_deref(),
        &configs,
    ) else {
        return;
    };

    info!(
        "Replaying missed mode transition {:?} -> {:?} on {:?} after restart/downtime",
        start_mode, transition.to_mode, transition.trigger
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
                || config.trigger == rhythm_core::ModeTransitionTrigger::Manual
            {
                return None;
            }

            let trigger_hour = trigger_hour(
                config.trigger,
                config.to_mode,
                solar_noon,
                latitude,
                longitude,
                timezone_name.as_deref(),
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
#[cfg(feature = "blocking")]
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

        #[cfg(feature = "desktop")]
        s.emit_event(crate::server_event::ServerEvent::ConfigChanged);
    }

    (fresh_offset, fresh_solar_noon, fresh_doy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use rhythm_core::{
        default_rhythm_profile, CurveContext, LightProfileRegistry, RoomProfileSettings,
    };
    use std::sync::Mutex;

    fn make_state() -> SharedState {
        Arc::new(Mutex::new(crate::state::AppState::default()))
    }

    fn make_room(id: &str, time_offset_minutes: f32) -> rhythm_core::RoomSnapshot {
        rhythm_core::RoomSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            rhythm_enabled: true,
            disabled: false,
            time_offset_minutes,
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

        assert!(enqueue_periodic_tick(&state, &tx, "room1", 10.0));
        assert!(enqueue_periodic_tick(&state, &tx, "room1", 10.5));

        let item = rx.try_recv().expect("first periodic item should be queued");
        match item {
            WorkItem::PeriodicRoomTick {
                room_id,
                current_hour,
            } => {
                assert_eq!(room_id, "room1");
                assert!((current_hour - 10.0).abs() < f32::EPSILON);
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
            .get("room1")
            .copied()
            .expect("latest hour should be retained");
        assert!((latest - 10.5).abs() < f32::EPSILON);
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
        use rhythm_core::{InputEvent, LightProfileConfig, RoomSnapshot, RuntimeHandle};
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
            fn restore_room_state(
                &self,
                room_id: &str,
                _: bool,
                _: bool,
                time_offset: f32,
                bri_offset: f32,
                _: bool,
                _: bool,
                _: rhythm_core::RoomProfileSettings,
            ) {
                self.restore_calls.lock().unwrap().push((
                    room_id.to_string(),
                    time_offset,
                    bri_offset,
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
        {
            let mut s = state.lock().unwrap();
            // Solar noon = 12.5, midnight = 0.5
            s.runtime_config.solar_noon_hour = 12.5;
            s.last_check_hour = Some(0.4); // just before midnight
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
        check_solar_midnight(&state, 0.6);

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
        {
            let mut s = state.lock().unwrap();
            s.runtime_config.solar_noon_hour = 12.5;
            s.last_check_hour = Some(0.4);
        }
        // This should not panic even with no runtime
        check_solar_midnight(&state, 0.6);
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
            12.5,
            -4.0,
            Some(35.804102),
            Some(-78.7992983),
            Some(tz_name),
            &rhythm_core::default_mode_transition_configs(),
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
            12.5,
            -4.0,
            Some(35.804102),
            Some(-78.7992983),
            Some(tz_name),
            &rhythm_core::default_mode_transition_configs(),
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
            12.5,
            -4.0,
            Some(35.804102),
            Some(-78.7992983),
            Some(tz_name),
            &configs,
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
    fn replay_wrapper_ignores_manual_mode_changes() {
        let state = make_state();
        {
            let mut s = state.lock().unwrap();
            s.active_mode = rhythm_core::RhythmMode::Sleep;
            s.last_active_mode_cause = rhythm_core::ModeChangeCause::Manual;
            s.last_active_mode_change_utc_ms =
                Some(chrono::Utc::now().timestamp_millis() - 12 * 60 * 60 * 1000);
        }

        replay_missed_mode_transitions(&state);

        assert_eq!(
            state.lock().unwrap().active_mode,
            rhythm_core::RhythmMode::Sleep
        );
    }
}
