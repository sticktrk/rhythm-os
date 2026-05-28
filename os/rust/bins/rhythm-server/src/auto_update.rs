//! Background loop that auto-applies stable OTA updates overnight.
//!
//! Runs on the rpiz appliance only. Wakes every 30 minutes, checks whether the
//! local clock is inside the configured overnight window (default 00:00–02:00
//! local), and — at most once per 20 hours — pulls the stable manifest,
//! downloads any newer release, persists runtime state, and triggers the rpiz
//! A/B reboot path. The
//! 30-min wake / 20h dedupe combination means at most one update attempt per
//! night even if the device clock skews mid-window.
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
const WINDOW_START_HOUR: u32 = 0;
const WINDOW_END_HOUR: u32 = 2;
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
        thread::sleep(POLL_INTERVAL);

        let snapshot = match snapshot_settings(&state) {
            Some(s) => s,
            None => continue,
        };

        if !snapshot.auto_update {
            continue;
        }

        let now = Utc::now();
        if !clock_is_sane(now) {
            warn!(
                target: "sys",
                "auto-update: skipping check until wall clock is sane (utc={})",
                now
            );
            continue;
        }

        if !in_overnight_window(now, snapshot.utc_offset_hours) {
            continue;
        }

        if let Some(t) = last_attempt {
            if t.elapsed() < MIN_BETWEEN_CHECKS {
                continue;
            }
        }
        last_attempt = Some(Instant::now());

        attempt_update(&state);
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

fn in_overnight_window(now_utc: chrono::DateTime<Utc>, utc_offset_hours: f32) -> bool {
    let local_hour = local_hour_from_utc(now_utc, utc_offset_hours);
    (WINDOW_START_HOUR..WINDOW_END_HOUR).contains(&local_hour)
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

    fn utc_at(hour: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 18, hour, 0, 0).unwrap()
    }

    #[test]
    fn window_open_at_local_midnight() {
        // UTC 07:00 with -7h offset -> local 00:00, window opens.
        assert_eq!(local_hour_from_utc(utc_at(7), -7.0), 0);
        assert!(in_overnight_window(utc_at(7), -7.0));
    }

    #[test]
    fn window_closed_at_local_0200() {
        // UTC 09:00 with -7h offset -> local 02:00, boundary closed.
        assert_eq!(local_hour_from_utc(utc_at(9), -7.0), 2);
        assert!(!in_overnight_window(utc_at(9), -7.0));
    }

    #[test]
    fn window_closed_at_local_noon() {
        // UTC 19:00 with -7h offset -> local 12:00.
        assert!(!in_overnight_window(utc_at(19), -7.0));
    }

    #[test]
    fn window_open_with_positive_offset() {
        // UTC 22:00 with +3h offset -> local 01:00 next day.
        assert_eq!(local_hour_from_utc(utc_at(22), 3.0), 1);
        assert!(in_overnight_window(utc_at(22), 3.0));
    }

    #[test]
    fn clock_sanity_rejects_epoch_boot_time() {
        let boot_epoch = Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 30).unwrap();
        assert!(!clock_is_sane(boot_epoch));
        assert!(clock_is_sane(utc_at(1)));
    }
}
