//! Correlates successful Hue light writes with raw SSE activity.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    write_expectations_suppressed_until_activity: bool,
    last_connected_epoch_ms: Option<i64>,
    last_sse_activity_epoch_ms: Option<i64>,
    connection_count: u64,
    reconnect_count: u64,
    last_reconnect_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HueSseLivenessSnapshot {
    pub pending_write_count: usize,
    pub oldest_pending_write_age_secs: Option<f64>,
    pub write_expectations_suppressed_until_activity: bool,
    pub last_connected_epoch_ms: Option<i64>,
    pub last_sse_activity_epoch_ms: Option<i64>,
    pub connection_count: u64,
    pub reconnect_count: u64,
    pub last_reconnect_reason: Option<String>,
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
        if state.write_expectations_suppressed_until_activity {
            return None;
        }
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

    /// Any raw event-stream bytes prove that this bridge's transport is
    /// delivering traffic. One chunk may contain updates for multiple writes.
    ///
    /// Raw traffic satisfies pending writes, but it does not re-arm
    /// write-triggered recovery after a timeout. Hue heartbeat comments can
    /// arrive on an otherwise unreliable event subscription, so only a data
    /// frame is strong enough evidence to leave suppression.
    pub(crate) fn observe_sse_activity(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.pending.clear();
            state.last_sse_activity_epoch_ms = epoch_ms_now();
        }
    }

    /// A complete SSE data frame proves that the replacement subscription is
    /// delivering integration events, so write-triggered recovery may resume.
    pub(crate) fn observe_sse_data_activity(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.pending.clear();
            state.write_expectations_suppressed_until_activity = false;
            state.last_sse_activity_epoch_ms = epoch_ms_now();
        }
    }

    /// A newly established response is a fresh subscription attempt. Old
    /// expectations triggered that recovery and must not immediately kill the
    /// replacement connection.
    pub(crate) fn reset_for_connected_stream(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.pending.clear();
            state.last_connected_epoch_ms = epoch_ms_now();
            state.connection_count = state.connection_count.saturating_add(1);
        }
    }

    pub(crate) fn note_reconnect(&self, reason: impl Into<String>) {
        if let Ok(mut state) = self.state.lock() {
            let reason = reason.into();
            if reason == "expected_activity_timeout" {
                state.pending.clear();
                state.write_expectations_suppressed_until_activity = true;
            }
            state.reconnect_count = state.reconnect_count.saturating_add(1);
            state.last_reconnect_reason = Some(reason);
        }
    }

    pub fn snapshot(&self) -> HueSseLivenessSnapshot {
        let Ok(state) = self.state.lock() else {
            return HueSseLivenessSnapshot {
                pending_write_count: 0,
                oldest_pending_write_age_secs: None,
                write_expectations_suppressed_until_activity: true,
                last_connected_epoch_ms: None,
                last_sse_activity_epoch_ms: None,
                connection_count: 0,
                reconnect_count: 0,
                last_reconnect_reason: Some("liveness_state_lock_poisoned".to_string()),
            };
        };
        HueSseLivenessSnapshot {
            pending_write_count: state.pending.len(),
            oldest_pending_write_age_secs: state
                .pending
                .values()
                .min()
                .map(|oldest| oldest.elapsed().as_secs_f64()),
            write_expectations_suppressed_until_activity: state
                .write_expectations_suppressed_until_activity,
            last_connected_epoch_ms: state.last_connected_epoch_ms,
            last_sse_activity_epoch_ms: state.last_sse_activity_epoch_ms,
            connection_count: state.connection_count,
            reconnect_count: state.reconnect_count,
            last_reconnect_reason: state.last_reconnect_reason.clone(),
        }
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

fn epoch_ms_now() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
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

    #[test]
    fn snapshot_reports_connections_activity_and_reconnect_reason() {
        let liveness = HueSseLiveness::default();
        liveness.reset_for_connected_stream();
        liveness.begin_expected_activity();
        liveness.observe_sse_data_activity();
        liveness.note_reconnect("expected_activity_timeout");

        let snapshot = liveness.snapshot();
        assert_eq!(snapshot.pending_write_count, 0);
        assert!(snapshot.write_expectations_suppressed_until_activity);
        assert!(snapshot.last_connected_epoch_ms.is_some());
        assert!(snapshot.last_sse_activity_epoch_ms.is_some());
        assert_eq!(snapshot.connection_count, 1);
        assert_eq!(snapshot.reconnect_count, 1);
        assert_eq!(
            snapshot.last_reconnect_reason.as_deref(),
            Some("expected_activity_timeout")
        );
    }

    #[test]
    fn timeout_suppresses_repeat_expectations_until_data_activity() {
        let liveness = HueSseLiveness::default();
        assert!(liveness.begin_expected_activity().is_some());

        liveness.note_reconnect("expected_activity_timeout");
        liveness.reset_for_connected_stream();

        assert!(
            liveness.begin_expected_activity().is_none(),
            "a replacement stream without proof-of-life must not trigger another timeout loop"
        );
        let snapshot = liveness.snapshot();
        assert_eq!(snapshot.pending_write_count, 0);
        assert!(snapshot.write_expectations_suppressed_until_activity);

        liveness.observe_sse_activity();

        assert!(
            liveness.begin_expected_activity().is_none(),
            "heartbeat or comment traffic must not re-arm write-based liveness"
        );
        assert!(
            liveness
                .snapshot()
                .write_expectations_suppressed_until_activity
        );

        liveness.observe_sse_data_activity();

        assert!(
            liveness.begin_expected_activity().is_some(),
            "a complete data frame must re-arm write-based liveness detection"
        );
        assert!(
            !liveness
                .snapshot()
                .write_expectations_suppressed_until_activity
        );
    }
}
