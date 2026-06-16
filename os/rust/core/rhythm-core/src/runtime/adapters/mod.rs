//! Platform-specific adapters for time and scheduling.
//!
//! This module provides the standard thread-based implementations of the
//! `TimeProvider` and `Scheduler` traits used by production binaries.

pub mod thread;

pub use self::thread::{SystemTimeProvider, ThreadScheduler};
