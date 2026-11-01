//! Thread-based adapters for std-based platforms.
//!
//! This module provides `TimeProvider` and `Scheduler` implementations
//! that use std::thread for scheduling and std::time for time access.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{DateTime, Datelike, FixedOffset, Timelike, Utc};
use tracing::debug;

use crate::runtime::error::{RuntimeError, RuntimeResult};
use crate::runtime::scheduler::{ScheduleHandle, Scheduler};
use crate::runtime::time::TimeProvider;

/// Time provider using the system UTC clock.
///
/// A named timezone resolves its current offset on every read so a long-lived
/// runtime follows daylight saving changes. Fixed-offset installations retain
/// the offset supplied to [`Self::new`]. Clones share location changes.
#[derive(Clone)]
pub struct SystemTimeProvider {
    config: Arc<RwLock<SystemTimeConfig>>,
}

struct SystemTimeConfig {
    utc_offset_hours: f32,
    timezone: Option<crate::Timezone>,
}

impl SystemTimeProvider {
    /// Create a new SystemTimeProvider with the given UTC offset.
    ///
    /// # Arguments
    ///
    /// * `utc_offset_hours` - The UTC offset in hours (e.g., -5.0 for EST, -8.0 for PST)
    pub fn new(utc_offset_hours: f32) -> Self {
        Self {
            config: Arc::new(RwLock::new(SystemTimeConfig {
                utc_offset_hours,
                timezone: None,
            })),
        }
    }

    /// Resolve local time using an IANA timezone, including future DST changes.
    pub fn with_timezone(self, timezone: crate::Timezone) -> Self {
        self.config
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .timezone = Some(timezone);
        self
    }

    fn local_datetime_at_utc(&self, utc: DateTime<Utc>) -> DateTime<FixedOffset> {
        let config = self
            .config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(timezone) = &config.timezone {
            timezone.local_datetime_with_offset_from_utc(utc.naive_utc())
        } else {
            let offset = FixedOffset::east_opt((config.utc_offset_hours * 3600.0) as i32)
                .unwrap_or_else(|| FixedOffset::east_opt(0).expect("valid UTC offset"));
            utc.with_timezone(&offset)
        }
    }
}

impl Default for SystemTimeProvider {
    fn default() -> Self {
        // Default to UTC (0 hours) — neutral default
        Self::new(0.0)
    }
}

impl TimeProvider for SystemTimeProvider {
    fn current_hour(&self) -> f32 {
        let local = self.local_datetime_at_utc(Utc::now());
        local.hour() as f32 + local.minute() as f32 / 60.0 + local.second() as f32 / 3600.0
    }

    fn day_of_year(&self) -> u32 {
        self.local_datetime_at_utc(Utc::now()).ordinal()
    }

    fn year(&self) -> i32 {
        self.local_datetime_at_utc(Utc::now()).year()
    }

    fn set_timezone(&self, timezone_name: Option<&str>, utc_offset_hours: f32) {
        let mut config = self
            .config
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        config.utc_offset_hours = utc_offset_hours;
        config.timezone = timezone_name.map(crate::Timezone::new);
    }
}

/// Scheduler using std::thread for server-class platforms.
pub struct ThreadScheduler {
    /// Next handle ID.
    next_id: AtomicU32,

    /// Map of handle ID to cancel flag.
    tasks: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,

    /// Thread handles for cleanup.
    handles: Arc<Mutex<HashMap<u64, JoinHandle<()>>>>,
}

impl ThreadScheduler {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU32::new(1),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for ThreadScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler for ThreadScheduler {
    fn schedule_periodic<F>(
        &self,
        name: &str,
        interval_secs: u64,
        callback: F,
    ) -> RuntimeResult<ScheduleHandle>
    where
        F: FnMut() + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) as u64;
        let cancel_flag = Arc::new(AtomicBool::new(false));

        // Store the cancel flag
        {
            let mut tasks = self.tasks.lock().map_err(|e| {
                RuntimeError::SchedulerError(format!("Failed to lock tasks: {}", e))
            })?;
            tasks.insert(id, cancel_flag.clone());
        }

        // Clone for the spawned thread
        let cancel = cancel_flag.clone();
        let task_name = name.to_string();
        let interval = Duration::from_secs(interval_secs);

        // Spawn the periodic thread
        let handle = thread::Builder::new()
            .name(format!("sched-{}", task_name))
            .spawn(move || {
                debug!(
                    "Periodic task '{}' started with {}s interval",
                    task_name, interval_secs
                );

                // The callback is not statically UnwindSafe, but we never
                // reuse state owned by the closure after a panic — each tick
                // is independent, and the alternative is worse: an unwinding
                // panic silently kills this scheduler thread and adaptive
                // lighting stops forever while the server looks healthy.
                let mut callback = std::panic::AssertUnwindSafe(callback);

                loop {
                    // Sleep for the interval
                    thread::sleep(interval);

                    // Check if cancelled
                    if cancel.load(Ordering::SeqCst) {
                        debug!("Periodic task '{}' cancelled", task_name);
                        break;
                    }

                    // Execute the callback; survive panics so one bad tick
                    // (bad config, poisoned downstream lock) doesn't
                    // permanently stop the periodic light updates.
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (callback.0)()));
                    if let Err(panic) = result {
                        let msg = panic
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "<non-string panic>".to_string());
                        log::error!(
                            "Periodic task '{}' panicked (continuing): {}",
                            task_name,
                            msg
                        );
                    }
                }
            })
            .map_err(|e| RuntimeError::SchedulerError(format!("Failed to spawn thread: {}", e)))?;

        // Store the handle
        {
            let mut handles = self.handles.lock().map_err(|e| {
                RuntimeError::SchedulerError(format!("Failed to lock handles: {}", e))
            })?;
            handles.insert(id, handle);
        }

        Ok(ScheduleHandle::new(id))
    }

    fn cancel(&self, handle: ScheduleHandle) -> RuntimeResult<()> {
        let id = handle.id();

        // Set the cancel flag
        {
            let tasks = self.tasks.lock().map_err(|e| {
                RuntimeError::SchedulerError(format!("Failed to lock tasks: {}", e))
            })?;
            if let Some(cancel_flag) = tasks.get(&id) {
                cancel_flag.store(true, Ordering::SeqCst);
            }
        }

        // Note: We don't join the thread here as it would block.
        // The thread will exit on its next iteration when it checks the cancel flag.

        // Clean up the cancel flag
        {
            let mut tasks = self.tasks.lock().map_err(|e| {
                RuntimeError::SchedulerError(format!("Failed to lock tasks: {}", e))
            })?;
            tasks.remove(&id);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spy_controller::SpyLightController;
    use crate::{NoOpScheduler, RhythmRuntime, RuntimeConfig, RuntimeHandle, SimpleDeviceRegistry};
    use chrono::TimeZone;
    use std::sync::atomic::AtomicU32;

    #[derive(Clone)]
    struct ControlledSystemTime {
        provider: SystemTimeProvider,
        utc: Arc<Mutex<DateTime<Utc>>>,
    }

    impl ControlledSystemTime {
        fn new(provider: SystemTimeProvider, utc: DateTime<Utc>) -> Self {
            Self {
                provider,
                utc: Arc::new(Mutex::new(utc)),
            }
        }

        fn set_utc(&self, utc: DateTime<Utc>) {
            *self.utc.lock().unwrap() = utc;
        }

        fn local(&self) -> DateTime<FixedOffset> {
            self.provider
                .local_datetime_at_utc(*self.utc.lock().unwrap())
        }
    }

    impl TimeProvider for ControlledSystemTime {
        fn current_hour(&self) -> f32 {
            let local = self.local();
            local.hour() as f32 + local.minute() as f32 / 60.0 + local.second() as f32 / 3600.0
        }

        fn day_of_year(&self) -> u32 {
            self.local().ordinal()
        }

        fn year(&self) -> i32 {
            self.local().year()
        }

        fn set_timezone(&self, timezone_name: Option<&str>, utc_offset_hours: f32) {
            self.provider.set_timezone(timezone_name, utc_offset_hours);
        }
    }

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
    }

    fn assert_runtime_actions_follow_dst(
        startup_offset: f32,
        observations: &[(DateTime<Utc>, f32)],
    ) {
        let clock = ControlledSystemTime::new(
            SystemTimeProvider::new(startup_offset)
                .with_timezone(crate::Timezone::new("America/New_York")),
            observations[0].0,
        );
        let spy = Arc::new(SpyLightController::new());
        spy.set_any_lights_on(true);
        let runtime = RhythmRuntime::new(
            spy.clone(),
            clock.clone(),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        );
        let handle: &dyn RuntimeHandle = &runtime;

        for (instant, expected_hour) in observations {
            clock.set_utc(*instant);
            assert!((handle.current_hour() - expected_hour).abs() < 0.0001);
        }

        let expected_hour = observations.last().unwrap().1;
        handle.add_room("motion-room", "Motion Room");
        handle.add_room("periodic-room", "Periodic Room");
        handle.turn_on_room("motion-room").unwrap();
        handle.turn_on_room("periodic-room").unwrap();
        handle
            .periodic_tick_room("periodic-room", expected_hour)
            .unwrap();
        let action = spy.last_command_for("motion-room").unwrap();
        let periodic = spy.last_command_for("periodic-room").unwrap();
        assert_eq!(
            (action.brightness, action.kelvin),
            (periodic.brightness, periodic.kelvin),
            "a long-lived runtime must use the same local hour for motion and periodic lighting"
        );

        // The scenario must distinguish the stale startup offset from the
        // correct clock, rather than passing because both times look identical.
        let stale_hour = expected_hour + if startup_offset == -4.0 { 1.0 } else { -1.0 };
        handle.add_room("stale-clock-room", "Stale Clock Room");
        handle.turn_on_room("stale-clock-room").unwrap();
        handle
            .periodic_tick_room("stale-clock-room", stale_hour)
            .unwrap();
        let stale = spy.last_command_for("stale-clock-room").unwrap();
        assert_ne!(
            (action.brightness, action.kelvin),
            (stale.brightness, stale.kelvin),
            "the morning curve should expose an hour of clock skew"
        );
    }

    #[test]
    fn runtime_motion_and_periodic_lighting_agree_after_dst_fall_back() {
        assert_runtime_actions_follow_dst(
            -4.0,
            &[
                (utc(2026, 11, 1, 5, 30), 1.5),
                (utc(2026, 11, 1, 5, 59), 1.0 + 59.0 / 60.0),
                (utc(2026, 11, 1, 6, 0), 1.0),
                (utc(2026, 11, 1, 6, 30), 1.5),
                (utc(2026, 11, 1, 12, 3), 7.05),
            ],
        );
    }

    #[test]
    fn runtime_motion_and_periodic_lighting_agree_after_dst_spring_forward() {
        assert_runtime_actions_follow_dst(
            -5.0,
            &[
                (utc(2026, 3, 8, 6, 59), 1.0 + 59.0 / 60.0),
                (utc(2026, 3, 8, 7, 0), 3.0),
                (utc(2026, 3, 8, 11, 3), 7.05),
            ],
        );
    }

    #[test]
    fn runtime_clock_follows_location_updates_and_fixed_offset_fallback() {
        let clock = ControlledSystemTime::new(
            SystemTimeProvider::new(-4.0).with_timezone(crate::Timezone::new("America/New_York")),
            utc(2026, 11, 1, 12, 3),
        );
        let runtime = RhythmRuntime::new(
            Arc::new(crate::NoOpController),
            clock.clone(),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        );
        let handle: &dyn RuntimeHandle = &runtime;
        assert!((handle.current_hour() - 7.05).abs() < 0.0001);

        handle
            .set_timezone(Some("America/Los_Angeles"), -8.0)
            .unwrap();
        assert!((handle.current_hour() - 4.05).abs() < 0.0001);
        assert!((clock.current_hour() - 4.05).abs() < 0.0001);

        handle.set_timezone(None, 5.5).unwrap();
        assert!((handle.current_hour() - 17.55).abs() < 0.0001);
        handle.set_timezone(Some("America/New_York"), -4.0).unwrap();
        assert!((handle.current_hour() - 7.05).abs() < 0.0001);
    }

    #[test]
    fn named_and_fixed_clock_dates_follow_local_midnight_and_leap_days() {
        let named = ControlledSystemTime::new(
            SystemTimeProvider::new(0.0).with_timezone(crate::Timezone::new("America/New_York")),
            utc(2026, 1, 1, 0, 30),
        );
        assert_eq!((named.year(), named.day_of_year()), (2025, 365));
        assert!((named.current_hour() - 19.5).abs() < 0.0001);
        named.set_utc(utc(2024, 3, 1, 0, 30));
        assert_eq!((named.year(), named.day_of_year()), (2024, 60));

        let fixed =
            ControlledSystemTime::new(SystemTimeProvider::new(5.5), utc(2024, 2, 29, 20, 0));
        assert_eq!((fixed.year(), fixed.day_of_year()), (2024, 61));
        assert!((fixed.current_hour() - 1.5).abs() < 0.0001);
        fixed.set_utc(utc(2026, 1, 1, 20, 0));
        assert_eq!((fixed.year(), fixed.day_of_year()), (2026, 2));
        assert!((fixed.current_hour() - 1.5).abs() < 0.0001);
    }

    #[test]
    fn test_system_time_provider() {
        let provider = SystemTimeProvider::new(-5.0); // EST

        let hour = provider.current_hour();
        assert!((0.0..24.0).contains(&hour), "Hour {} out of range", hour);

        let day = provider.day_of_year();
        assert!((1..=366).contains(&day), "Day {} out of range", day);

        let year = provider.year();
        assert!(year >= 2024, "Year {} too old", year);
    }

    #[test]
    fn test_system_time_provider_offset() {
        // Test that offset is applied correctly
        let est = SystemTimeProvider::new(-5.0);
        let pst = SystemTimeProvider::new(-8.0);

        // PST should be 3 hours behind EST
        let est_hour = est.current_hour();
        let pst_hour = pst.current_hour();

        // Account for day wrap-around
        let diff = if est_hour >= pst_hour {
            est_hour - pst_hour
        } else {
            est_hour + 24.0 - pst_hour
        };

        assert!(
            (diff - 3.0).abs() < 0.1,
            "EST ({}) should be 3 hours ahead of PST ({})",
            est_hour,
            pst_hour
        );
    }

    #[test]
    fn test_thread_scheduler_basic() {
        let scheduler = ThreadScheduler::new();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let handle = scheduler
            .schedule_periodic("test", 1, move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();

        // Wait for a couple ticks
        thread::sleep(Duration::from_millis(2500));

        // Cancel
        scheduler.cancel(handle).unwrap();

        // Should have ticked at least once
        let count = counter.load(Ordering::SeqCst);
        assert!(count >= 1, "Counter should be >= 1, got {}", count);
    }
}
