//! Runtime liveness watchdogs shared by native server-class binaries.

use std::time::{Duration, Instant};

use log::warn;
use rhythm_os::state::SharedState;

const PERIODIC_WATCHDOG_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const PERIODIC_WATCHDOG_MIN_STALE: Duration = Duration::from_secs(5 * 60);
const PERIODIC_WATCHDOG_INTERVAL_MULTIPLIER: u64 = 5;

pub fn spawn_periodic_watchdog(state: SharedState) {
    std::thread::Builder::new()
        .name("liveness-watchdog".to_string())
        .spawn(move || loop {
            std::thread::sleep(PERIODIC_WATCHDOG_CHECK_INTERVAL);

            let stale = {
                let Ok(s) = state.lock() else {
                    warn!(target: "sys", "liveness-watchdog: state lock poisoned");
                    continue;
                };
                stale_periodic_liveness(s.last_check_instant, s.runtime_config.update_interval_secs)
            };

            if let Some((age_secs, threshold_secs)) = stale {
                log::error!(
                    target: "sys",
                    "Periodic liveness stale for {}s (threshold {}s); restarting appliance",
                    age_secs,
                    threshold_secs
                );
                crate::self_update::schedule_liveness_restart();
                return;
            }
        })
        .expect("Failed to spawn liveness watchdog thread");
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

    #[test]
    fn periodic_liveness_threshold_uses_minimum_for_default_interval() {
        assert_eq!(periodic_liveness_threshold_secs(60), 300);
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
}
