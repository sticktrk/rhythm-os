//! Tokio-based adapters for async runtimes.
//!
//! This module provides `TimeProvider` and `Scheduler` implementations
//! that use tokio for async time operations and chrono for time access.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{Datelike, Local, Timelike};
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
use tracing::debug;

use crate::runtime::error::{RuntimeError, RuntimeResult};
use crate::runtime::scheduler::{ScheduleHandle, Scheduler};
use crate::runtime::time::TimeProvider;

/// Time provider using chrono::Local for system time.
#[derive(Clone, Copy, Default)]
pub struct TokioTimeProvider;

impl TokioTimeProvider {
    /// Create a new TokioTimeProvider.
    pub fn new() -> Self {
        Self
    }
}

impl TimeProvider for TokioTimeProvider {
    fn current_hour(&self) -> f32 {
        let now = Local::now();
        now.hour() as f32 + now.minute() as f32 / 60.0 + now.second() as f32 / 3600.0
    }

    fn day_of_year(&self) -> u32 {
        Local::now().ordinal()
    }

    fn year(&self) -> i32 {
        Local::now().year()
    }
}

/// Scheduler using tokio for async task scheduling.
pub struct TokioScheduler {
    /// Next handle ID.
    next_id: AtomicU64,

    /// Map of handle ID to cancel flag.
    /// The inner bool is set to true when cancelled.
    tasks: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,

    /// Task handles for cleanup (not strictly needed but nice for debugging).
    handles: Arc<Mutex<HashMap<u64, JoinHandle<()>>>>,
}

impl TokioScheduler {
    /// Create a new TokioScheduler.
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for TokioScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler for TokioScheduler {
    fn schedule_periodic<F>(
        &self,
        name: &str,
        interval_secs: u64,
        mut callback: F,
    ) -> RuntimeResult<ScheduleHandle>
    where
        F: FnMut() + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let cancel_flag = Arc::new(AtomicBool::new(false));

        // Store the cancel flag
        {
            let mut tasks = self.tasks.lock().map_err(|e| {
                RuntimeError::SchedulerError(format!("Failed to lock tasks: {}", e))
            })?;
            tasks.insert(id, cancel_flag.clone());
        }

        // Clone for the spawned task
        let cancel = cancel_flag.clone();
        let task_name = name.to_string();

        // Spawn the periodic task
        let handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_secs));

            // Skip the first immediate tick
            ticker.tick().await;

            debug!(
                "Periodic task '{}' started with {}s interval",
                task_name, interval_secs
            );

            loop {
                ticker.tick().await;

                // Check if cancelled
                if cancel.load(Ordering::SeqCst) {
                    debug!("Periodic task '{}' cancelled", task_name);
                    break;
                }

                // Execute the callback
                callback();
            }
        });

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

        // Abort the task handle (in case it's waiting on tick)
        {
            let mut handles = self.handles.lock().map_err(|e| {
                RuntimeError::SchedulerError(format!("Failed to lock handles: {}", e))
            })?;
            if let Some(handle) = handles.remove(&id) {
                handle.abort();
            }
        }

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
    fn test_tokio_time_provider() {
        let provider = TokioTimeProvider::new();

        let hour = provider.current_hour();
        assert!((0.0..24.0).contains(&hour));

        let day = provider.day_of_year();
        assert!((1..=366).contains(&day));

        let year = provider.year();
        assert!(year >= 2024);
    }

    #[tokio::test]
    async fn test_tokio_scheduler_basic() {
        let scheduler = TokioScheduler::new();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let handle = scheduler
            .schedule_periodic("test", 1, move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();

        // Wait for a couple ticks
        tokio::time::sleep(Duration::from_millis(2500)).await;

        // Cancel
        scheduler.cancel(handle).unwrap();

        // Should have ticked at least once
        let count = counter.load(Ordering::SeqCst);
        assert!(count >= 1, "Counter should be >= 1, got {}", count);
    }

    #[tokio::test]
    async fn test_tokio_scheduler_cancel() {
        let scheduler = TokioScheduler::new();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let handle = scheduler
            .schedule_periodic("test", 1, move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();

        // Cancel immediately
        scheduler.cancel(handle).unwrap();

        // Wait to ensure no ticks occur
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let count = counter.load(Ordering::SeqCst);
        assert_eq!(count, 0, "Counter should be 0 after cancellation");
    }
}
