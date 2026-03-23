//! Platform-specific adapters for time and scheduling.
//!
//! This module provides implementations of the `TimeProvider` and `Scheduler`
//! traits for different platforms.

#[cfg(feature = "tokio")]
pub mod tokio;

#[cfg(feature = "blocking")]
pub mod blocking;

// Re-exports
#[cfg(feature = "tokio")]
pub use self::tokio::{TokioScheduler, TokioTimeProvider};

#[cfg(feature = "blocking")]
pub use self::blocking::{BlockingScheduler, BlockingTimeProvider};
