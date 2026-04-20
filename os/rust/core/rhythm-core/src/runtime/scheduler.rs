//! Scheduler trait for periodic task execution.
//!
//! This module defines the `Scheduler` trait which abstracts how the runtime
//! schedules periodic tasks. Different platforms can implement this differently:
//! - Tokio-based systems use tokio::time::interval
//! - Blocking runtimes can use std::thread with sleep
//! - Test environments can use manual triggering

use crate::runtime::error::RuntimeResult;

/// Handle for a scheduled task, used for cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScheduleHandle(pub u64);

impl ScheduleHandle {
    /// Create a new schedule handle with the given ID.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Get the underlying ID.
    pub fn id(&self) -> u64 {
        self.0
    }
}

/// Trait for scheduling periodic tasks.
///
/// Implementations should handle task execution in a platform-appropriate way:
/// - Tokio: spawn async tasks
/// - Blocking: spawn threads or use a timer loop
pub trait Scheduler: Send + Sync {
    /// Schedule a periodic callback.
    ///
    /// The callback will be invoked every `interval_secs` seconds.
    /// Returns a handle that can be used to cancel the schedule.
    ///
    /// # Arguments
    ///
    /// * `name` - A descriptive name for the task (for logging)
    /// * `interval_secs` - Interval between invocations in seconds
    /// * `callback` - The callback to invoke
    fn schedule_periodic<F>(
        &self,
        name: &str,
        interval_secs: u64,
        callback: F,
    ) -> RuntimeResult<ScheduleHandle>
    where
        F: FnMut() + Send + 'static;

    /// Cancel a scheduled task.
    ///
    /// # Arguments
    ///
    /// * `handle` - The handle returned from `schedule_periodic`
    fn cancel(&self, handle: ScheduleHandle) -> RuntimeResult<()>;
}

/// A no-op scheduler for testing.
pub struct NoOpScheduler;

impl NoOpScheduler {
    /// Create a new no-op scheduler.
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoOpScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler for NoOpScheduler {
    fn schedule_periodic<F>(
        &self,
        _name: &str,
        _interval_secs: u64,
        _callback: F,
    ) -> RuntimeResult<ScheduleHandle>
    where
        F: FnMut() + Send + 'static,
    {
        Ok(ScheduleHandle::new(0))
    }

    fn cancel(&self, _handle: ScheduleHandle) -> RuntimeResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_handle() {
        let handle = ScheduleHandle::new(42);
        assert_eq!(handle.id(), 42);
    }

    #[test]
    fn test_noop_scheduler() {
        let scheduler = NoOpScheduler::new();
        let handle = scheduler.schedule_periodic("test", 60, || {}).unwrap();
        assert!(scheduler.cancel(handle).is_ok());
    }
}
