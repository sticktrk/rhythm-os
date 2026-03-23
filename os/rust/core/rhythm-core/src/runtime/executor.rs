//! Async/sync execution helpers.
//!
//! This module provides utilities for bridging async and sync code,
//! allowing the same runtime logic to work on both tokio (addon) and
//! blocking (ESP32) platforms.

use std::future::Future;

/// Execute a future in a blocking context.
///
/// On tokio platforms, this uses the current runtime handle.
/// On blocking platforms, this uses futures::executor::block_on.
///
/// Uses `block_in_place` when called from within a tokio multi-thread
/// runtime (e.g., axum handlers) to avoid the "Cannot start a runtime
/// from within a runtime" panic that `Handle::block_on` would trigger.
#[cfg(feature = "tokio")]
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // block_in_place moves the current worker thread out of the
            // async pool, allowing handle.block_on to work safely.
            tokio::task::block_in_place(|| handle.block_on(future))
        }
        Err(_) => {
            // No runtime — create a blocking executor
            futures::executor::block_on(future)
        }
    }
}

/// Execute a future in a blocking context.
///
/// Uses futures::executor::block_on for blocking platforms.
#[cfg(all(feature = "blocking", not(feature = "tokio")))]
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    futures::executor::block_on(future)
}

/// Execute a future in a blocking context.
///
/// Fallback for when neither feature is enabled (should rarely happen).
#[cfg(all(not(feature = "tokio"), not(feature = "blocking")))]
pub fn block_on<F>(_future: F) -> F::Output
where
    F: Future,
{
    panic!("No async executor available. Enable the 'tokio' or 'blocking' feature.");
}
