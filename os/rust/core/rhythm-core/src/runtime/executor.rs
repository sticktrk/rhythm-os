//! Async/sync execution helpers.
//!
//! This module provides utilities for bridging async and sync code.

use std::future::Future;

/// Execute a future in a blocking context.
///
/// Uses `block_in_place` when called from within a tokio multi-thread
/// runtime (e.g., axum handlers) to avoid the "Cannot start a runtime
/// from within a runtime" panic that `Handle::block_on` would trigger.
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
