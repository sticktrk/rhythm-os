//! Runtime liveness watchdogs shared by native server-class binaries.

use std::sync::TryLockError;
use std::thread;
use std::time::{Duration, Instant};

use log::warn;
use rhythm_os::state::SharedState;

const PERIODIC_WATCHDOG_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const PERIODIC_WATCHDOG_MIN_STALE: Duration = Duration::from_secs(5 * 60);
const PERIODIC_WATCHDOG_INTERVAL_MULTIPLIER: u64 = 5;
const PERIODIC_WATCHDOG_LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
const PERIODIC_WATCHDOG_LOCK_POLL: Duration = Duration::from_millis(100);

pub fn spawn_periodic_watchdog(state: SharedState) {
    crate::boot_diagnostics::watchdog_armed();
    std::thread::Builder::new()
        .name("liveness-watchdog".to_string())
        .spawn(move || {
            let mut consecutive_lock_failures: u64 = 0;
            loop {
                std::thread::sleep(PERIODIC_WATCHDOG_CHECK_INTERVAL);
                crate::boot_diagnostics::watchdog_heartbeat();

                let view = try_acquire_liveness_view(
                    &state,
                    PERIODIC_WATCHDOG_LOCK_ACQUIRE_TIMEOUT,
                    PERIODIC_WATCHDOG_LOCK_POLL,
                );

                let stale = evaluate_liveness(
                    view,
                    &mut consecutive_lock_failures,
                    PERIODIC_WATCHDOG_CHECK_INTERVAL.as_secs(),
                    PERIODIC_WATCHDOG_MIN_STALE.as_secs(),
                );

                if let Some((age_secs, threshold_secs, reason)) = stale {
                    crate::boot_diagnostics::watchdog_triggered(reason);
                    log::error!(
                        target: "sys",
                        "Periodic liveness stale for {}s (threshold {}s, reason {}); restarting appliance",
                        age_secs,
                        threshold_secs,
                        reason
                    );
                    crate::self_update::schedule_liveness_restart();
                    return;
                }
            }
        })
        .expect("Failed to spawn liveness watchdog thread");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LivenessView {
    Acquired {
        last_check_instant: Option<Instant>,
        update_interval_secs: u64,
        oldest_pending_periodic_tick_age_secs: Option<u64>,
    },
    LockUnavailable,
}

fn try_acquire_liveness_view(
    state: &SharedState,
    timeout: Duration,
    poll: Duration,
) -> LivenessView {
    let deadline = Instant::now() + timeout;
    loop {
        match state.try_lock() {
            Ok(s) => {
                return LivenessView::Acquired {
                    last_check_instant: s.last_check_instant,
                    update_interval_secs: watchdog_interval_secs(&s),
                    oldest_pending_periodic_tick_age_secs: oldest_pending_periodic_tick_age_secs(
                        &s,
                    ),
                };
            }
            Err(TryLockError::Poisoned(poisoned)) => {
                warn!(target: "sys", "liveness-watchdog: state lock poisoned");
                let s = poisoned.into_inner();
                return LivenessView::Acquired {
                    last_check_instant: s.last_check_instant,
                    update_interval_secs: watchdog_interval_secs(&s),
                    oldest_pending_periodic_tick_age_secs: oldest_pending_periodic_tick_age_secs(
                        &s,
                    ),
                };
            }
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return LivenessView::LockUnavailable;
                }
                thread::sleep(poll);
            }
        }
    }
}

fn evaluate_liveness(
    view: LivenessView,
    consecutive_lock_failures: &mut u64,
    check_interval_secs: u64,
    min_stale_secs: u64,
) -> Option<(u64, u64, &'static str)> {
    match view {
        LivenessView::Acquired {
            last_check_instant,
            update_interval_secs,
            oldest_pending_periodic_tick_age_secs,
        } => {
            *consecutive_lock_failures = 0;
            stale_pending_periodic_tick(oldest_pending_periodic_tick_age_secs, update_interval_secs)
                .map(|(age, threshold)| (age, threshold, "periodic_tick_pending_stale"))
                .or_else(|| {
                    stale_periodic_liveness(last_check_instant, update_interval_secs)
                        .map(|(age, threshold)| (age, threshold, "periodic_stale"))
                })
        }
        LivenessView::LockUnavailable => {
            *consecutive_lock_failures = consecutive_lock_failures.saturating_add(1);
            let blocked_secs = consecutive_lock_failures.saturating_mul(check_interval_secs);
            (blocked_secs >= min_stale_secs).then_some((
                blocked_secs,
                min_stale_secs,
                "state_lock_blocked",
            ))
        }
    }
}

/// The pacing math can stretch the actual periodic cadence well past the
/// configured interval (e.g. 60s configured, 180s effective on a loaded
/// appliance). Staleness thresholds must scale to the cadence the loop
/// actually runs at, or the watchdog reboots healthy devices.
fn watchdog_interval_secs(s: &rhythm_os::state::AppState) -> u64 {
    s.runtime_config
        .update_interval_secs
        .max(s.effective_periodic_interval_secs.unwrap_or(0))
}

fn oldest_pending_periodic_tick_age_secs(s: &rhythm_os::state::AppState) -> Option<u64> {
    s.pending_periodic_ticks
        .values()
        .map(|pending| pending.enqueued_at.elapsed().as_secs())
        .max()
}

fn stale_pending_periodic_tick(
    oldest_pending_periodic_tick_age_secs: Option<u64>,
    update_interval_secs: u64,
) -> Option<(u64, u64)> {
    let age_secs = oldest_pending_periodic_tick_age_secs?;
    let threshold_secs = periodic_liveness_threshold_secs(update_interval_secs);
    (age_secs > threshold_secs).then_some((age_secs, threshold_secs))
}

fn stale_periodic_liveness(
    last_check_instant: Option<Instant>,
    update_interval_secs: u64,
) -> Option<(u64, u64)> {
    let age_secs = last_check_instant?.elapsed().as_secs();
    let threshold_secs = periodic_liveness_threshold_secs(update_interval_secs);
    (age_secs > threshold_secs).then_some((age_secs, threshold_secs))
}

fn periodic_liveness_threshold_secs(update_interval_secs: u64) -> u64 {
    update_interval_secs
        .saturating_mul(PERIODIC_WATCHDOG_INTERVAL_MULTIPLIER)
        .max(PERIODIC_WATCHDOG_MIN_STALE.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_os::state::{AppState, SharedState};
    use std::sync::{Arc, Mutex};

    #[test]
    fn periodic_liveness_threshold_uses_minimum_for_default_interval() {
        assert_eq!(periodic_liveness_threshold_secs(60), 300);
    }

    #[test]
    fn watchdog_interval_scales_to_effective_periodic_cycle() {
        // The pacing math can stretch the real cadence (e.g. 180s) past the
        // configured interval (60s). The watchdog must scale with the real
        // cadence or it reboots healthy appliances: 60s configured gave a
        // 300s threshold while healthy cycles stamped only every ~180s.
        let mut state = AppState::default();
        state.runtime_config.update_interval_secs = 60;
        assert_eq!(watchdog_interval_secs(&state), 60);

        state.effective_periodic_interval_secs = Some(180);
        assert_eq!(watchdog_interval_secs(&state), 180);
        assert_eq!(periodic_liveness_threshold_secs(180), 900);

        // Effective never lowers the threshold below the configured interval.
        state.effective_periodic_interval_secs = Some(10);
        assert_eq!(watchdog_interval_secs(&state), 60);
    }

    #[test]
    fn stale_periodic_liveness_trips_after_threshold() {
        let threshold_secs = periodic_liveness_threshold_secs(60);

        assert!(stale_periodic_liveness(
            Some(Instant::now() - Duration::from_secs(threshold_secs)),
            60
        )
        .is_none());
        assert_eq!(
            stale_periodic_liveness(
                Some(Instant::now() - Duration::from_secs(threshold_secs + 1)),
                60
            ),
            Some((301, 300))
        );
    }

    #[test]
    fn stale_periodic_liveness_ignores_unseeded_checks() {
        assert!(stale_periodic_liveness(None, 60).is_none());
    }

    #[test]
    fn try_acquire_liveness_view_returns_lock_unavailable_when_state_mutex_is_held_forever() {
        // Reproduces the issue #81 freeze: a thread holds the AppState mutex
        // forever and never releases it, so `state.lock()` in the watchdog used
        // to block indefinitely. With the new try_lock path we must observe
        // `LockUnavailable` after the bounded acquisition timeout.
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let held = state.lock().expect("acquire state guard for test");

        let started = Instant::now();
        let view = try_acquire_liveness_view(
            &state,
            Duration::from_millis(200),
            Duration::from_millis(20),
        );
        let elapsed = started.elapsed();

        assert_eq!(view, LivenessView::LockUnavailable);
        // We must have waited at least one poll cycle but not significantly
        // longer than the configured timeout.
        assert!(elapsed >= Duration::from_millis(200));
        assert!(elapsed < Duration::from_secs(2));
        drop(held);
    }

    #[test]
    fn evaluate_liveness_trips_after_sustained_lock_unavailability() {
        // The original watchdog blocked on `state.lock()` and never tripped
        // when the mutex was held by a hung thread. Now we track consecutive
        // lock-acquisition failures and synthesize a stale signal once they
        // accumulate to the same 5 minute threshold the periodic-stale path
        // uses.
        let mut consecutive = 0u64;
        let check_interval = 30u64;
        let min_stale = 300u64;

        // First 9 failures (= 4m30s of unavailability) should not trip yet.
        for expected in 1..=9u64 {
            let outcome = evaluate_liveness(
                LivenessView::LockUnavailable,
                &mut consecutive,
                check_interval,
                min_stale,
            );
            assert!(
                outcome.is_none(),
                "should not trip after {} failures",
                expected
            );
            assert_eq!(consecutive, expected);
        }

        // The 10th consecutive failure reaches 300s of unavailability and trips.
        let tripped = evaluate_liveness(
            LivenessView::LockUnavailable,
            &mut consecutive,
            check_interval,
            min_stale,
        );
        assert_eq!(tripped, Some((300, 300, "state_lock_blocked")));
    }

    #[test]
    fn evaluate_liveness_resets_lock_failures_after_a_successful_acquire() {
        // A transient burst of contention must not poison the consecutive
        // counter; once we acquire the lock again we go back to assessing
        // periodic liveness directly.
        let mut consecutive = 7u64;

        let outcome = evaluate_liveness(
            LivenessView::Acquired {
                last_check_instant: Some(Instant::now()),
                update_interval_secs: 60,
                oldest_pending_periodic_tick_age_secs: None,
            },
            &mut consecutive,
            30,
            300,
        );

        assert!(outcome.is_none());
        assert_eq!(consecutive, 0);
    }

    #[test]
    fn evaluate_liveness_reports_periodic_stale_when_lock_is_acquired() {
        let mut consecutive = 0u64;
        let stale_instant = Instant::now() - Duration::from_secs(301);

        let outcome = evaluate_liveness(
            LivenessView::Acquired {
                last_check_instant: Some(stale_instant),
                update_interval_secs: 60,
                oldest_pending_periodic_tick_age_secs: None,
            },
            &mut consecutive,
            30,
            300,
        );

        assert_eq!(outcome, Some((301, 300, "periodic_stale")));
        assert_eq!(consecutive, 0);
    }

    #[test]
    fn stale_pending_periodic_tick_trips_after_threshold() {
        let threshold_secs = periodic_liveness_threshold_secs(60);

        assert!(stale_pending_periodic_tick(Some(threshold_secs), 60).is_none());
        assert_eq!(
            stale_pending_periodic_tick(Some(threshold_secs + 1), 60),
            Some((301, 300))
        );
        assert!(stale_pending_periodic_tick(None, 60).is_none());
    }

    #[test]
    fn evaluate_liveness_reports_pending_tick_before_scheduler_staleness() {
        let mut consecutive = 0u64;

        let outcome = evaluate_liveness(
            LivenessView::Acquired {
                last_check_instant: Some(Instant::now()),
                update_interval_secs: 60,
                oldest_pending_periodic_tick_age_secs: Some(301),
            },
            &mut consecutive,
            30,
            300,
        );

        assert_eq!(outcome, Some((301, 300, "periodic_tick_pending_stale")));
        assert_eq!(consecutive, 0);
    }
}
