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
#[cfg(feature = "blocking")]
use std::time::Duration;

use log::info;
#[cfg(feature = "blocking")]
use log::warn;
#[cfg(feature = "blocking")]
use rhythm_core::{BlockingTimeProvider, LightCurveModule, TimeProvider};

use std::sync::Arc;

use rhythm_core::runtime::RuntimeHandle;

use crate::state::SharedState;
#[cfg(feature = "blocking")]
use crate::state::WorkItem;
#[cfg(feature = "blocking")]
use crate::storage::StoredLocation;

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

    thread::sleep(initial_interval);

    loop {
        let (utc_offset, config, solar_noon, lat, lon, update_interval, timezone_name) = {
            let Ok(s) = state.lock() else {
                thread::sleep(Duration::from_secs(60));
                continue;
            };
            (
                s.utc_offset_hours,
                s.config.clone(),
                s.solar_noon_hour(),
                s.latitude.unwrap_or(35.0),
                s.longitude.unwrap_or(-80.84),
                Duration::from_secs(s.runtime_config.update_interval_secs),
                s.timezone_name.clone(),
            )
        };

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

        // Calculate generic curve values (no offset) for logging
        let solar = rhythm_core::SolarTime::new(solar_noon, lat, doy);
        let ctx = rhythm_core::curve_module::CurveContext::new(current_hour, solar, None);
        let module = rhythm_core::RhythmCurveModule::new(config);
        let values = module.calculate(&ctx);

        // Get rhythm-enabled room IDs, skipping rooms in warning-dim state
        let (room_ids, warning_skipped, work_tx) = {
            let Ok(s) = state.lock() else {
                thread::sleep(update_interval);
                continue;
            };
            let all_ids: Vec<String> = s
                .hub_runtime()
                .map(|rt| {
                    rt.engine_all_room_snapshots()
                        .iter()
                        .filter(|r| r.rhythm_enabled)
                        .map(|r| r.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            let mut skipped = 0usize;
            let ids: Vec<String> = all_ids
                .into_iter()
                .filter(|id| {
                    if s.motion_snapshots
                        .get(id)
                        .is_some_and(|ms| ms.warning_active)
                    {
                        skipped += 1;
                        false
                    } else {
                        true
                    }
                })
                .collect();
            (ids, skipped, s.work_tx.clone())
        };

        if warning_skipped > 0 {
            info!(
                "Periodic tick at hour {:.2} - bri={}% kelvin={} ({} rooms in rhythm, {} skipped: warning-dim)",
                current_hour, values.brightness, values.kelvin, room_ids.len(), warning_skipped
            );
        } else {
            info!(
                "Periodic tick at hour {:.2} - bri={}% kelvin={} ({} rooms in rhythm)",
                current_hour,
                values.brightness,
                values.kelvin,
                room_ids.len()
            );
        }

        if let Some(ref cb) = on_tick {
            cb();
        }

        // Dispatch per-room ticks via work queue or inline
        if let Some(ref tx) = work_tx {
            for room_id in &room_ids {
                if tx
                    .try_send(WorkItem::PeriodicRoomTick {
                        room_id: room_id.clone(),
                        current_hour,
                    })
                    .is_err()
                {
                    warn!(target: "sys", "Work queue full, dropping periodic tick for '{}'", room_id);
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
                for room_id in &room_ids {
                    if let Err(e) = runtime.periodic_tick_room(room_id, current_hour) {
                        warn!(target: "sys", "Periodic room tick '{}' failed: {}", room_id, e);
                    }
                    post_tick_room(&state, &runtime, room_id);
                }
            }
        }

        check_solar_midnight(&state, current_hour);

        thread::sleep(update_interval);
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
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::RoomState {
                rooms: vec![crate::commands::build_room_state_event(state, &snap)],
            },
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
            return;
        };

        let crossed = rhythm_core::crossed_solar_midnight(last, current_hour, solar_midnight);

        if crossed {
            info!(
                "Solar midnight crossed (last={:.2}, now={:.2}, midnight={:.2}) - resetting offsets",
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
    use chrono::{Datelike, Timelike};

    let Some(tz_name) = timezone_name else {
        return (current_offset, current_solar_noon, doy);
    };

    let tz = rhythm_core::Timezone::new(tz_name);
    let now = chrono::Utc::now().naive_utc();
    let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
    let fresh_offset = tz.utc_offset(year, month, day, now.time().hour());

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
    use std::sync::Mutex;

    fn make_state() -> SharedState {
        Arc::new(Mutex::new(crate::state::AppState::default()))
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
        use chrono::{Datelike, Timelike};

        let state = make_state();
        let tz_name = "America/New_York";
        let tz = rhythm_core::Timezone::new(tz_name);
        let now = chrono::Utc::now().naive_utc();
        let current_offset = tz.utc_offset(
            now.date().year(),
            now.date().month(),
            now.date().day(),
            now.time().hour(),
        );

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
        use chrono::{Datelike, Timelike};

        let state = make_state();
        let tz_name = "America/New_York";
        let tz = rhythm_core::Timezone::new(tz_name);
        let now = chrono::Utc::now().naive_utc();
        let current_offset = tz.utc_offset(
            now.date().year(),
            now.date().month(),
            now.date().day(),
            now.time().hour(),
        );

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
        use rhythm_core::{CurveConfig, InputEvent, RoomSnapshot, RuntimeHandle};
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
            fn set_curve_config(&self, _: CurveConfig) -> anyhow::Result<()> {
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
            fn set_soft_off_brightness(&self, _: u8) {}
            fn soft_off_tick_room(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            fn any_lights_on(&self, _: &str) -> anyhow::Result<bool> {
                Ok(false)
            }
            fn current_hour(&self) -> f32 {
                12.0
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
}
