//! Correlates successful Hue light writes with raw SSE activity.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ExpectedActivityToken(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExpectedActivityFailure {
    pub pending_count: usize,
    pub oldest_age: Duration,
}

#[derive(Debug, Default)]
struct LivenessState {
    next_token: u64,
    pending: BTreeMap<ExpectedActivityToken, Instant>,
}

/// Per-bridge expectation tracker shared by the light controller and SSE
/// reader. A write is registered before transport I/O so an event arriving
/// before the HTTP response completes still satisfies the expectation.
#[derive(Debug, Default)]
pub struct HueSseLiveness {
    state: Mutex<LivenessState>,
}

impl HueSseLiveness {
    pub(crate) fn begin_expected_activity(&self) -> Option<ExpectedActivityToken> {
        let mut state = self.state.lock().ok()?;
        state.next_token = state.next_token.wrapping_add(1).max(1);
        let token = ExpectedActivityToken(state.next_token);
        state.pending.insert(token, Instant::now());
        Some(token)
    }

    pub(crate) fn cancel_expected_activity(&self, token: Option<ExpectedActivityToken>) {
        let Some(token) = token else { return };
        if let Ok(mut state) = self.state.lock() {
            state.pending.remove(&token);
        }
    }

    /// Any raw event-stream bytes prove that this bridge's subscription is
    /// delivering traffic. One chunk may contain updates for multiple writes.
    pub(crate) fn observe_sse_activity(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.pending.clear();
        }
    }

    /// A newly established response is a fresh subscription attempt. Old
    /// expectations triggered that recovery and must not immediately kill the
    /// replacement connection.
    pub(crate) fn reset_for_connected_stream(&self) {
        self.observe_sse_activity();
    }

    pub(crate) fn expected_activity_failure(
        &self,
        timeout: Duration,
    ) -> Option<ExpectedActivityFailure> {
        let state = self.state.lock().ok()?;
        let oldest = state.pending.values().min()?;
        let oldest_age = oldest.elapsed();
        (oldest_age >= timeout).then_some(ExpectedActivityFailure {
            pending_count: state.pending.len(),
            oldest_age,
        })
    }

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.state
            .lock()
            .map(|state| state.pending.len())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_activity_satisfies_all_pending_writes() {
        let liveness = HueSseLiveness::default();
        liveness.begin_expected_activity();
        liveness.begin_expected_activity();
        assert_eq!(liveness.pending_count(), 2);

        liveness.observe_sse_activity();

        assert_eq!(liveness.pending_count(), 0);
    }

    #[test]
    fn failed_write_cancels_only_its_expectation() {
        let liveness = HueSseLiveness::default();
        let first = liveness.begin_expected_activity();
        let second = liveness.begin_expected_activity();

        liveness.cancel_expected_activity(first);

        assert_eq!(liveness.pending_count(), 1);
        liveness.cancel_expected_activity(second);
        assert_eq!(liveness.pending_count(), 0);
    }
}
