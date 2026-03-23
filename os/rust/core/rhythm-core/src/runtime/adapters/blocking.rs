//! Blocking adapters for std-based platforms (ESP32).
//!
//! This module provides `TimeProvider` and `Scheduler` implementations
//! that use std::thread for scheduling and std::time for time access.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing::debug;

use crate::runtime::error::{RuntimeError, RuntimeResult};
use crate::runtime::scheduler::{ScheduleHandle, Scheduler};
use crate::runtime::time::TimeProvider;

/// Time provider using std::time::SystemTime.
///
/// This provider requires a UTC offset to be set, as SystemTime only
/// provides UTC time. The offset should be set based on the local timezone.
#[derive(Clone)]
pub struct BlockingTimeProvider {
    /// UTC offset in hours (e.g., -5.0 for EST).
    utc_offset_hours: f32,
}

impl BlockingTimeProvider {
    /// Create a new BlockingTimeProvider with the given UTC offset.
    ///
    /// # Arguments
    ///
    /// * `utc_offset_hours` - The UTC offset in hours (e.g., -5.0 for EST, -8.0 for PST)
    pub fn new(utc_offset_hours: f32) -> Self {
        Self { utc_offset_hours }
    }

    /// Get the UTC hour as a float (0.0 - 24.0).
    fn utc_hour(&self) -> f32 {
        if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
            let secs = duration.as_secs();
            let seconds_in_day = secs % 86400;
            let hour = seconds_in_day / 3600;
            let minute = (seconds_in_day % 3600) / 60;
            let second = seconds_in_day % 60;
            return hour as f32 + minute as f32 / 60.0 + second as f32 / 3600.0;
        }
        12.0 // Default to noon if time unavailable
    }

    /// Calculate day of year from Unix timestamp.
    fn day_of_year_from_timestamp(&self, secs: u64) -> u32 {
        // Days since epoch (1970-01-01)
        let days_since_epoch = secs / 86400;

        // This is a simplified calculation that doesn't account for leap years perfectly
        // but is good enough for lighting purposes
        let year = 1970 + (days_since_epoch / 365) as i32;
        let leap_years = ((year - 1970) / 4) as u64;
        let day_in_year = (days_since_epoch - ((year - 1970) as u64 * 365) - leap_years) % 365;

        (day_in_year + 1) as u32
    }
}

impl Default for BlockingTimeProvider {
    fn default() -> Self {
        // Default to UTC (0 hours) — neutral default
        Self::new(0.0)
    }
}

impl TimeProvider for BlockingTimeProvider {
    fn current_hour(&self) -> f32 {
        let utc_hour = self.utc_hour();
        // Apply offset and wrap around 24 hours
        let local_hour = utc_hour + self.utc_offset_hours;
        if local_hour < 0.0 {
            local_hour + 24.0
        } else if local_hour >= 24.0 {
            local_hour - 24.0
        } else {
            local_hour
        }
    }

    fn day_of_year(&self) -> u32 {
        if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
            self.day_of_year_from_timestamp(duration.as_secs())
        } else {
            172 // Default to summer solstice
        }
    }

    fn year(&self) -> i32 {
        if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
            let secs = duration.as_secs();
            let days_since_epoch = secs / 86400;
            1970 + (days_since_epoch / 365) as i32
        } else {
            2024
        }
    }
}

/// Scheduler using std::thread for blocking platforms.
pub struct BlockingScheduler {
    /// Next handle ID.
    next_id: AtomicU32,

    /// Map of handle ID to cancel flag.
    tasks: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,

    /// Thread handles for cleanup.
    handles: Arc<Mutex<HashMap<u64, JoinHandle<()>>>>,

    /// Stack size for spawned threads (bytes). On embedded platforms like ESP32,
    /// the default pthread stack is very small (~3KB usable). Callbacks that
    /// perform TLS, float math, or string formatting need 16KB+.
    stack_size: Option<usize>,
}

impl BlockingScheduler {
    /// Create a new BlockingScheduler with default thread stack size.
    pub fn new() -> Self {
        Self {
            next_id: AtomicU32::new(1),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            handles: Arc::new(Mutex::new(HashMap::new())),
            stack_size: None,
        }
    }

    /// Create a new BlockingScheduler with a specific thread stack size.
    pub fn with_stack_size(stack_size: usize) -> Self {
        Self {
            stack_size: Some(stack_size),
            ..Self::new()
        }
    }
}

impl Default for BlockingScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler for BlockingScheduler {
    fn schedule_periodic<F>(
        &self,
        name: &str,
        interval_secs: u64,
        mut callback: F,
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
        let mut builder = thread::Builder::new().name(format!("sched-{}", task_name));
        if let Some(size) = self.stack_size {
            builder = builder.stack_size(size);
        }
        let handle = builder
            .spawn(move || {
                debug!(
                    "Periodic task '{}' started with {}s interval",
                    task_name, interval_secs
                );

                loop {
                    // Sleep for the interval
                    thread::sleep(interval);

                    // Check if cancelled
                    if cancel.load(Ordering::SeqCst) {
                        debug!("Periodic task '{}' cancelled", task_name);
                        break;
                    }

                    // Execute the callback
                    callback();
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
    use std::sync::atomic::AtomicU32;

    #[test]
    fn test_blocking_time_provider() {
        let provider = BlockingTimeProvider::new(-5.0); // EST

        let hour = provider.current_hour();
        assert!((0.0..24.0).contains(&hour), "Hour {} out of range", hour);

        let day = provider.day_of_year();
        assert!((1..=366).contains(&day), "Day {} out of range", day);

        let year = provider.year();
        assert!(year >= 2024, "Year {} too old", year);
    }

    #[test]
    fn test_blocking_time_provider_offset() {
        // Test that offset is applied correctly
        let est = BlockingTimeProvider::new(-5.0);
        let pst = BlockingTimeProvider::new(-8.0);

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
    fn test_blocking_scheduler_basic() {
        let scheduler = BlockingScheduler::new();
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
