//! Background loop that auto-applies stable OTA updates.
//!
//! Runs on the rpiz appliance only. Wakes every 30 minutes, checks whether the
//! local clock is inside the configured update window (default 14:00-16:00
//! local), and — at most once per 20 hours — pulls the stable manifest,
//! downloads any newer release, persists runtime state, and triggers the rpiz
//! A/B reboot path. The
//! 30-min wake / 20h dedupe combination means at most one update attempt per
//! day even if the device clock skews mid-window.
//!
//! The loop is a no-op when `auto_update == false` in `StoredSettings` (the
//! user opted into beta + manual updates) and on non-appliance builds.

use std::thread;
use std::time::{Duration, Instant};

use chrono::{Datelike, FixedOffset, Timelike, Utc};
use log::{info, warn};
use rhythm_os::state::SharedState;

use crate::self_update::{self, UpdateChannel};

const POLL_INTERVAL: Duration = Duration::from_secs(30 * 60);
const MIN_BETWEEN_CHECKS: Duration = Duration::from_secs(20 * 60 * 60);
const WINDOW_START_HOUR: u32 = 14;
const WINDOW_END_HOUR: u32 = 16;
const MIN_SANE_YEAR: i32 = 2024;

pub fn spawn(state: SharedState) {
    if !should_spawn_for_state(&state) {
        info!(target: "sys", "auto-update: skipping loop (not appliance)");
        return;
    }

    thread::Builder::new()
        .name("auto-update".to_string())
        .spawn(move || run(state))
        .expect("Failed to spawn auto-update thread");
}

fn run(state: SharedState) {
    info!(
        target: "sys",
        "auto-update: loop started (poll={}s, window={}-{} local, min_between_checks={}h)",
        POLL_INTERVAL.as_secs(),
        WINDOW_START_HOUR,
        WINDOW_END_HOUR,
        MIN_BETWEEN_CHECKS.as_secs() / 3600,
    );

    let mut last_attempt: Option<Instant> = None;

    loop {
        let snapshot = match snapshot_settings(&state) {
            Some(s) => s,
            None => {
                thread::sleep(POLL_INTERVAL);
                continue;
            }
        };

        let now = Utc::now();
        if snapshot.auto_update && !clock_is_sane(now) {
            warn!(
                target: "sys",
                "auto-update: skipping check until wall clock is sane (utc={})",
                now
            );
        }

        if should_attempt_update(snapshot, now, last_attempt) {
            last_attempt = Some(Instant::now());
            attempt_update(&state);
        }

        thread::sleep(POLL_INTERVAL);
    }
}

#[derive(Clone, Copy, Debug)]
struct LoopSettings {
    auto_update: bool,
    utc_offset_hours: f32,
}

fn snapshot_settings(state: &SharedState) -> Option<LoopSettings> {
    match state.lock() {
        Ok(s) => Some(LoopSettings {
            auto_update: s.auto_update,
            utc_offset_hours: s.utc_offset_hours,
        }),
        Err(_) => {
            warn!(target: "sys", "auto-update: state lock poisoned");
            None
        }
    }
}

fn should_spawn_for_state(state: &SharedState) -> bool {
    self_update::is_appliance_runtime_default()
        || state
            .lock()
            .map(|s| s.platform_type == "appliance")
            .unwrap_or(false)
}

fn clock_is_sane(now_utc: chrono::DateTime<Utc>) -> bool {
    now_utc.year() >= MIN_SANE_YEAR
}

fn in_auto_update_window(now_utc: chrono::DateTime<Utc>, utc_offset_hours: f32) -> bool {
    let local_hour = local_hour_from_utc(now_utc, utc_offset_hours);
    (WINDOW_START_HOUR..WINDOW_END_HOUR).contains(&local_hour)
}

fn should_attempt_update(
    snapshot: LoopSettings,
    now_utc: chrono::DateTime<Utc>,
    last_attempt: Option<Instant>,
) -> bool {
    if !snapshot.auto_update || !clock_is_sane(now_utc) {
        return false;
    }

    if !in_auto_update_window(now_utc, snapshot.utc_offset_hours) {
        return false;
    }

    last_attempt
        .map(|attempted_at| attempted_at.elapsed() >= MIN_BETWEEN_CHECKS)
        .unwrap_or(true)
}

fn local_hour_from_utc(now_utc: chrono::DateTime<Utc>, utc_offset_hours: f32) -> u32 {
    // FixedOffset takes seconds east of UTC; negative values are west of UTC.
    // The location loader already adjusted for DST when populating
    // utc_offset_hours, so this is correct year-round.
    let offset_secs = (utc_offset_hours * 3600.0) as i32;
    let offset =
        FixedOffset::east_opt(offset_secs).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    now_utc.with_timezone(&offset).hour()
}

fn attempt_update(state: &SharedState) {
    let version = crate::BUILD_VERSION;
    info!(target: "sys", "auto-update: checking stable feed (v{} -> ?)", version);

    let info = match self_update::check_blocking(version, UpdateChannel::Stable) {
        Ok(info) => info,
        Err(e) => {
            // Network blips should not dominate the journal. A missing stable
            // manifest is treated as "no update" in self_update.
            info!(target: "sys", "auto-update: stable check failed: {}", e);
            return;
        }
    };

    if !info.update_available {
        info!(target: "sys", "auto-update: already current (v{})", info.current_version);
        return;
    }

    if self_update::appliance_rollback_version_matches(&info.latest_version) {
        warn!(
            target: "sys",
            "auto-update: skipping v{} because this appliance just rolled it back",
            info.latest_version
        );
        return;
    }

    info!(
        target: "sys",
        "auto-update: applying v{} -> v{}",
        info.current_version,
        info.latest_version,
    );

    match info.apply_blocking() {
        Ok(result) => {
            info!(
                target: "sys",
                "auto-update: apply complete (installed={:?}, checksum_verified={:?})",
                result.installed_targets,
                result.checksum_verified,
            );
            if let Err(error) =
                self_update::schedule_post_update_restart_with_best_effort_persist(state.clone())
            {
                warn!(
                    target: "sys",
                    "auto-update: restart scheduled, but failed to spawn persistence worker: {}",
                    error
                );
            }
        }
        Err(e) => {
            warn!(target: "sys", "auto-update: apply failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rhythm_os::state::AppState;
    use std::sync::{Arc, Mutex};

    fn utc_at(hour: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 18, hour, 0, 0).unwrap()
    }

    #[test]
    fn window_open_at_local_1400() {
        // UTC 21:00 with -7h offset -> local 14:00, window opens.
        assert_eq!(local_hour_from_utc(utc_at(21), -7.0), 14);
        assert!(in_auto_update_window(utc_at(21), -7.0));
    }

    #[test]
    fn window_closed_at_local_1600() {
        // UTC 23:00 with -7h offset -> local 16:00, boundary closed.
        assert_eq!(local_hour_from_utc(utc_at(23), -7.0), 16);
        assert!(!in_auto_update_window(utc_at(23), -7.0));
    }

    #[test]
    fn window_closed_at_local_noon() {
        // UTC 19:00 with -7h offset -> local 12:00.
        assert!(!in_auto_update_window(utc_at(19), -7.0));
    }

    #[test]
    fn window_open_with_positive_offset() {
        // UTC 11:00 with +3h offset -> local 14:00.
        assert_eq!(local_hour_from_utc(utc_at(11), 3.0), 14);
        assert!(in_auto_update_window(utc_at(11), 3.0));
    }

    #[test]
    fn clock_sanity_rejects_epoch_boot_time() {
        let boot_epoch = Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 30).unwrap();
        assert!(!clock_is_sane(boot_epoch));
        assert!(clock_is_sane(utc_at(1)));
    }

    #[test]
    fn snapshot_settings_captures_auto_update_and_utc_offset() {
        let state = Arc::new(Mutex::new(AppState::default()));
        {
            let mut guard = state.lock().unwrap();
            guard.auto_update = false;
            guard.utc_offset_hours = -4.5;
        }

        let snapshot = snapshot_settings(&state).unwrap();

        assert!(!snapshot.auto_update);
        assert_eq!(snapshot.utc_offset_hours, -4.5);
    }

    #[test]
    fn first_iteration_inside_window_attempts_immediately() {
        let snapshot = LoopSettings {
            auto_update: true,
            utc_offset_hours: -4.0,
        };

        // UTC 18:00 with -4h offset -> local 14:00, inside the daily window.
        assert!(should_attempt_update(snapshot, utc_at(18), None));
    }

    #[test]
    fn recent_attempt_suppresses_duplicate_check() {
        let snapshot = LoopSettings {
            auto_update: true,
            utc_offset_hours: -4.0,
        };

        assert!(!should_attempt_update(
            snapshot,
            utc_at(18),
            Some(Instant::now())
        ));
    }

    #[test]
    fn should_spawn_when_state_declares_appliance_platform() {
        let state = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().platform_type = "appliance";

        assert!(should_spawn_for_state(&state));
    }

    #[test]
    fn invalid_utc_offset_falls_back_to_utc_hour() {
        let now = utc_at(5);

        assert_eq!(local_hour_from_utc(now, 100.0), 5);
        assert!(!in_auto_update_window(now, 100.0));
    }
}
