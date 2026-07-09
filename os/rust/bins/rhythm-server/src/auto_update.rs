//! Background loop that auto-applies OTA updates.
//!
//! Runs on the rpiz appliance only. Wakes every 30 minutes, checks whether the
//! local clock is inside the configured update window (default 14:00-16:00
//! local), and — at most once per 20 hours — pulls the manifest for the
//! device's resolved release channel (stable by default, beta when explicitly
//! selected), downloads any newer release, persists runtime state, and
//! triggers the rpiz A/B reboot path. The 30-min wake / 20h dedupe combination
//! means at most one update attempt per day even if the device clock skews
//! mid-window.
//!
//! The loop is a no-op when `auto_update == false` in `StoredSettings` (the
//! user wants manual updates) and on non-appliance builds.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{Datelike, FixedOffset, Timelike, Utc};
use log::{info, warn};
use rhythm_os::state::SharedState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::self_update::{self, UpdateChannel};

const POLL_INTERVAL: Duration = Duration::from_secs(30 * 60);
const MIN_BETWEEN_CHECKS: Duration = Duration::from_secs(20 * 60 * 60);
const CHECK_FAILURE_RETRY_INTERVAL: Duration = Duration::from_secs(30 * 60);
const WINDOW_START_HOUR: u32 = 14;
const WINDOW_END_HOUR: u32 = 16;
const MIN_SANE_YEAR: i32 = 2024;
const AUTO_UPDATE_STATE_SCHEMA_VERSION: u32 = 1;
pub const AUTO_UPDATE_STATE_RELATIVE_PATH: &str = "ota/auto-update-state.json";

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

    let mut last_attempt: Option<RecentAttempt> = None;

    loop {
        maybe_attempt_update(&state, &mut last_attempt);
        thread::sleep(POLL_INTERVAL);
    }
}

#[derive(Clone, Debug)]
struct RecentAttempt {
    at: Instant,
    decision: AutoUpdateDecision,
}

fn maybe_attempt_update(state: &SharedState, last_attempt: &mut Option<RecentAttempt>) {
    let snapshot = match snapshot_settings(state) {
        Some(s) => s,
        None => return,
    };

    if !snapshot.auto_update {
        return;
    }

    let now = Utc::now();
    if !clock_is_sane(now) {
        warn!(
            target: "sys",
            "auto-update: skipping check until wall clock is sane (utc={})",
            now
        );
        return;
    }

    if !in_auto_update_window(now, snapshot.utc_offset_hours) {
        return;
    }

    if recent_attempt_is_within_cooldown(last_attempt.as_ref()) {
        return;
    }
    if last_persisted_check_within_cooldown(&snapshot, now) {
        return;
    }

    let decision = attempt_update(state, &snapshot);
    *last_attempt = Some(RecentAttempt {
        at: Instant::now(),
        decision,
    });
}

#[derive(Clone, Debug)]
struct LoopSettings {
    auto_update: bool,
    channel: UpdateChannel,
    utc_offset_hours: f32,
    data_dir: String,
}

fn snapshot_settings(state: &SharedState) -> Option<LoopSettings> {
    match state.lock() {
        Ok(s) => Some(LoopSettings {
            auto_update: s.auto_update,
            channel: s.resolved_update_channel(),
            utc_offset_hours: s.utc_offset_hours,
            data_dir: s.data_dir.clone(),
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

fn local_hour_from_utc(now_utc: chrono::DateTime<Utc>, utc_offset_hours: f32) -> u32 {
    // FixedOffset takes seconds east of UTC; negative values are west of UTC.
    // The location loader already adjusted for DST when populating
    // utc_offset_hours, so this is correct year-round.
    let offset_secs = (utc_offset_hours * 3600.0) as i32;
    let offset =
        FixedOffset::east_opt(offset_secs).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    now_utc.with_timezone(&offset).hour()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AutoUpdateState {
    pub schema_version: u32,
    pub updated_at_epoch_ms: i64,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<AutoUpdateCheckSnapshot>,
    #[serde(default)]
    pub update_results: Vec<AutoUpdateResultSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AutoUpdateCheckSnapshot {
    pub checked_at_epoch_ms: i64,
    pub checked_at: String,
    pub decision_at_epoch_ms: i64,
    pub decision_at: String,
    pub channel: String,
    pub current_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_package_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_package_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_image_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_image_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_available: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_reason: Option<String>,
    pub decision: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_persist_error: Option<String>,
    #[serde(default)]
    pub install_targets: Value,
    #[serde(default)]
    pub image_assets: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AutoUpdateResultSnapshot {
    pub attempt_id: String,
    pub started_at_epoch_ms: i64,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_epoch_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub channel: String,
    pub current_version: String,
    pub latest_version: String,
    pub current_package_version: String,
    pub latest_package_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_image_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_image_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_reason: Option<String>,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum_verified: Option<bool>,
    #[serde(default)]
    pub installed_targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_persist_error: Option<String>,
}

pub fn load_status_json(state: &SharedState) -> Option<Value> {
    let path = auto_update_state_path_for_state(state)?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutoUpdateDecision {
    CheckFailed,
    UpToDate,
    SkippedRecentRollback,
    Applying,
    AppliedRestartScheduled,
    AppliedRestartPersistFailed,
    ApplyFailed,
}

impl AutoUpdateDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::CheckFailed => "check_failed",
            Self::UpToDate => "up_to_date",
            Self::SkippedRecentRollback => "skipped_recent_rollback",
            Self::Applying => "applying",
            Self::AppliedRestartScheduled => "applied_restart_scheduled",
            Self::AppliedRestartPersistFailed => "applied_restart_persist_failed",
            Self::ApplyFailed => "apply_failed",
        }
    }

    fn cooldown(self) -> Duration {
        match self {
            // A transient Wi-Fi/feed blip should not burn the whole daily
            // update window. Retry on the next poll while preserving the
            // durable "last check failed" state for postmortems.
            Self::CheckFailed => CHECK_FAILURE_RETRY_INTERVAL,
            _ => MIN_BETWEEN_CHECKS,
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "check_failed" => Self::CheckFailed,
            "up_to_date" => Self::UpToDate,
            "skipped_recent_rollback" => Self::SkippedRecentRollback,
            "applying" => Self::Applying,
            "applied_restart_scheduled" => Self::AppliedRestartScheduled,
            "applied_restart_persist_failed" => Self::AppliedRestartPersistFailed,
            "apply_failed" => Self::ApplyFailed,
            _ => Self::UpToDate,
        }
    }
}

fn attempt_update(state: &SharedState, settings: &LoopSettings) -> AutoUpdateDecision {
    let version = crate::BUILD_VERSION;
    let channel = settings.channel;
    let state_path = auto_update_state_path(settings);
    let checked_at = Utc::now();
    info!(
        target: "sys",
        "auto-update: checking {} feed (v{} -> ?)",
        channel.as_str(),
        version
    );

    let info = match self_update::check_blocking(version, channel) {
        Ok(info) => info,
        Err(e) => {
            // Network blips should not dominate the journal. A missing stable
            // manifest is treated as "no update" in self_update.
            info!(
                target: "sys",
                "auto-update: {} check failed: {}",
                channel.as_str(),
                e
            );
            persist_last_check(
                state_path.as_deref(),
                check_snapshot_error(
                    checked_at,
                    channel,
                    version,
                    AutoUpdateDecision::CheckFailed,
                    e,
                ),
            );
            return AutoUpdateDecision::CheckFailed;
        }
    };

    if !info.update_available {
        info!(target: "sys", "auto-update: already current (v{})", info.current_version);
        persist_last_check(
            state_path.as_deref(),
            check_snapshot_from_info(
                checked_at,
                channel,
                &info,
                AutoUpdateDecision::UpToDate,
                "Already up to date".to_string(),
                None,
                None,
                None,
            ),
        );
        return AutoUpdateDecision::UpToDate;
    }

    if self_update::appliance_rollback_version_matches(&info.latest_version) {
        warn!(
            target: "sys",
            "auto-update: skipping v{} because this appliance just rolled it back",
            info.latest_version
        );
        persist_last_check(
            state_path.as_deref(),
            check_snapshot_from_info(
                checked_at,
                channel,
                &info,
                AutoUpdateDecision::SkippedRecentRollback,
                format!(
                    "Skipped v{} because this appliance just rolled it back",
                    info.latest_version
                ),
                None,
                Some(info.latest_version.clone()),
                None,
            ),
        );
        return AutoUpdateDecision::SkippedRecentRollback;
    }

    info!(
        target: "sys",
        "auto-update: applying v{} -> v{}",
        info.current_version,
        info.latest_version,
    );

    let started_result = update_result_started(checked_at, channel, &info);
    let attempt_id = started_result.attempt_id.clone();
    persist_update_result(state_path.as_deref(), started_result);
    persist_last_check(
        state_path.as_deref(),
        check_snapshot_from_info(
            checked_at,
            channel,
            &info,
            AutoUpdateDecision::Applying,
            format!(
                "Applying v{} -> v{}",
                info.current_version, info.latest_version
            ),
            None,
            None,
            None,
        ),
    );

    match info.apply_blocking() {
        Ok(result) => {
            info!(
                target: "sys",
                "auto-update: apply complete (installed={:?}, checksum_verified={:?})",
                result.installed_targets,
                result.checksum_verified,
            );
            {
                let data_dir = state
                    .lock()
                    .map(|s| PathBuf::from(&s.data_dir))
                    .unwrap_or_default();
                crate::ota_history::record(
                    &data_dir,
                    crate::ota_history::entry(
                        Some(&info.current_version),
                        Some(&info.latest_version),
                        "auto",
                        "applied",
                    ),
                );
            }
            let restart_persist_error = if let Err(error) =
                self_update::schedule_post_update_restart_with_best_effort_persist(state.clone())
            {
                warn!(
                    target: "sys",
                    "auto-update: restart scheduled, but failed to spawn persistence worker: {}",
                    error
                );
                Some(error.to_string())
            } else {
                None
            };
            let outcome = if restart_persist_error.is_some() {
                AutoUpdateDecision::AppliedRestartPersistFailed
            } else {
                AutoUpdateDecision::AppliedRestartScheduled
            };
            persist_update_result(
                state_path.as_deref(),
                update_result_completed(
                    attempt_id,
                    checked_at,
                    channel,
                    &info,
                    outcome,
                    result.checksum_verified,
                    result.installed_targets,
                    None,
                    restart_persist_error.clone(),
                ),
            );
            persist_last_check(
                state_path.as_deref(),
                check_snapshot_from_info(
                    checked_at,
                    channel,
                    &info,
                    outcome,
                    format!("Applied v{}; restart scheduled", info.latest_version),
                    None,
                    None,
                    restart_persist_error,
                ),
            );
            outcome
        }
        Err(e) => {
            warn!(target: "sys", "auto-update: apply failed: {}", e);
            persist_update_result(
                state_path.as_deref(),
                update_result_completed(
                    attempt_id,
                    checked_at,
                    channel,
                    &info,
                    AutoUpdateDecision::ApplyFailed,
                    None,
                    Vec::new(),
                    Some(e.clone()),
                    None,
                ),
            );
            persist_last_check(
                state_path.as_deref(),
                check_snapshot_from_info(
                    checked_at,
                    channel,
                    &info,
                    AutoUpdateDecision::ApplyFailed,
                    "Update failed".to_string(),
                    Some(e),
                    None,
                    None,
                ),
            );
            AutoUpdateDecision::ApplyFailed
        }
    }
}

fn auto_update_state_path(settings: &LoopSettings) -> Option<PathBuf> {
    if settings.data_dir.trim().is_empty() {
        return None;
    }
    Some(Path::new(&settings.data_dir).join(AUTO_UPDATE_STATE_RELATIVE_PATH))
}

fn auto_update_state_path_for_state(state: &SharedState) -> Option<PathBuf> {
    let settings = snapshot_settings(state)?;
    auto_update_state_path(&settings)
}

fn recent_attempt_is_within_cooldown(attempt: Option<&RecentAttempt>) -> bool {
    attempt
        .map(|attempt| attempt.at.elapsed() < attempt.decision.cooldown())
        .unwrap_or(false)
}

fn last_persisted_check_within_cooldown(
    settings: &LoopSettings,
    now: chrono::DateTime<Utc>,
) -> bool {
    let Some(path) = auto_update_state_path(settings) else {
        return false;
    };
    let state = load_auto_update_state(&path);
    let Some(last_check) = state.last_check else {
        return false;
    };
    timestamp_within_cooldown(
        last_check.checked_at_epoch_ms,
        now.timestamp_millis(),
        AutoUpdateDecision::from_str(&last_check.decision).cooldown(),
    )
}

fn timestamp_within_cooldown(then_epoch_ms: i64, now_epoch_ms: i64, cooldown: Duration) -> bool {
    let cooldown_ms = cooldown.as_millis();
    if then_epoch_ms > now_epoch_ms {
        return ((then_epoch_ms - now_epoch_ms) as u128) < cooldown_ms;
    }
    ((now_epoch_ms - then_epoch_ms) as u128) < cooldown_ms
}

fn persist_last_check(path: Option<&Path>, check: AutoUpdateCheckSnapshot) {
    persist_auto_update_state(path, |state| {
        state.last_check = Some(check);
    });
}

fn persist_update_result(path: Option<&Path>, result: AutoUpdateResultSnapshot) {
    persist_auto_update_state(path, |state| {
        if let Some(existing) = state
            .update_results
            .iter_mut()
            .find(|entry| entry.attempt_id == result.attempt_id)
        {
            *existing = result;
        } else {
            state.update_results.push(result);
        }
    });
}

fn persist_auto_update_state(path: Option<&Path>, mutate: impl FnOnce(&mut AutoUpdateState)) {
    let Some(path) = path else {
        warn!(target: "sys", "auto-update: no data_dir configured; cannot persist status");
        return;
    };

    let mut state = load_auto_update_state(path);
    mutate(&mut state);
    let updated_at = Utc::now();
    state.schema_version = AUTO_UPDATE_STATE_SCHEMA_VERSION;
    state.updated_at_epoch_ms = updated_at.timestamp_millis();
    state.updated_at = updated_at.to_rfc3339();

    let body = match serde_json::to_vec_pretty(&state) {
        Ok(body) => body,
        Err(error) => {
            warn!(target: "sys", "auto-update: failed to encode status: {}", error);
            return;
        }
    };
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            warn!(
                target: "sys",
                "auto-update: failed to create status directory {}: {}",
                parent.display(),
                error
            );
            return;
        }
    }
    if let Err(error) = crate::bootstate::atomic_write_with_sync(path, &body) {
        warn!(
            target: "sys",
            "auto-update: failed to persist status at {}: {}",
            path.display(),
            error
        );
    }
}

fn load_auto_update_state(path: &Path) -> AutoUpdateState {
    let Ok(raw) = fs::read_to_string(path) else {
        return AutoUpdateState::default();
    };
    serde_json::from_str(&raw).unwrap_or_else(|error| {
        warn!(
            target: "sys",
            "auto-update: failed to parse existing status at {}: {}",
            path.display(),
            error
        );
        AutoUpdateState::default()
    })
}

fn check_snapshot_error(
    checked_at: chrono::DateTime<Utc>,
    channel: UpdateChannel,
    current_version: &str,
    decision: AutoUpdateDecision,
    error: String,
) -> AutoUpdateCheckSnapshot {
    let decision_at = Utc::now();
    AutoUpdateCheckSnapshot {
        checked_at_epoch_ms: checked_at.timestamp_millis(),
        checked_at: checked_at.to_rfc3339(),
        decision_at_epoch_ms: decision_at.timestamp_millis(),
        decision_at: decision_at.to_rfc3339(),
        channel: channel.as_str().to_string(),
        current_version: current_version.to_string(),
        latest_version: None,
        current_package_version: None,
        latest_package_version: None,
        current_image_version: None,
        latest_image_version: None,
        update_available: None,
        update_reason: None,
        decision: decision.as_str().to_string(),
        message: "Update check failed".to_string(),
        error: Some(error),
        rollback_version: None,
        restart_persist_error: None,
        install_targets: Value::Array(Vec::new()),
        image_assets: Value::Array(Vec::new()),
    }
}

#[allow(clippy::too_many_arguments)]
fn check_snapshot_from_info(
    checked_at: chrono::DateTime<Utc>,
    channel: UpdateChannel,
    info: &self_update::UpdateInfo,
    decision: AutoUpdateDecision,
    message: String,
    error: Option<String>,
    rollback_version: Option<String>,
    restart_persist_error: Option<String>,
) -> AutoUpdateCheckSnapshot {
    let decision_at = Utc::now();
    AutoUpdateCheckSnapshot {
        checked_at_epoch_ms: checked_at.timestamp_millis(),
        checked_at: checked_at.to_rfc3339(),
        decision_at_epoch_ms: decision_at.timestamp_millis(),
        decision_at: decision_at.to_rfc3339(),
        channel: channel.as_str().to_string(),
        current_version: info.current_version.clone(),
        latest_version: Some(info.latest_version.clone()),
        current_package_version: Some(info.current_package_version.clone()),
        latest_package_version: Some(info.latest_package_version.clone()),
        current_image_version: info.current_image_version.clone(),
        latest_image_version: info.latest_image_version.clone(),
        update_available: Some(info.update_available),
        update_reason: update_reason_name(info.update_reason),
        decision: decision.as_str().to_string(),
        message,
        error,
        rollback_version,
        restart_persist_error,
        install_targets: serde_json::to_value(&info.install_targets)
            .unwrap_or_else(|_| Value::Array(Vec::new())),
        image_assets: serde_json::to_value(&info.image_assets)
            .unwrap_or_else(|_| Value::Array(Vec::new())),
    }
}

fn update_result_started(
    started_at: chrono::DateTime<Utc>,
    channel: UpdateChannel,
    info: &self_update::UpdateInfo,
) -> AutoUpdateResultSnapshot {
    AutoUpdateResultSnapshot {
        attempt_id: format!(
            "{}-{}-{}",
            started_at.timestamp_millis(),
            info.current_version,
            info.latest_version
        ),
        started_at_epoch_ms: started_at.timestamp_millis(),
        started_at: started_at.to_rfc3339(),
        completed_at_epoch_ms: None,
        completed_at: None,
        channel: channel.as_str().to_string(),
        current_version: info.current_version.clone(),
        latest_version: info.latest_version.clone(),
        current_package_version: info.current_package_version.clone(),
        latest_package_version: info.latest_package_version.clone(),
        current_image_version: info.current_image_version.clone(),
        latest_image_version: info.latest_image_version.clone(),
        update_reason: update_reason_name(info.update_reason),
        outcome: "started".to_string(),
        checksum_verified: None,
        installed_targets: Vec::new(),
        error: None,
        restart_persist_error: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn update_result_completed(
    attempt_id: String,
    started_at: chrono::DateTime<Utc>,
    channel: UpdateChannel,
    info: &self_update::UpdateInfo,
    outcome: AutoUpdateDecision,
    checksum_verified: Option<bool>,
    installed_targets: Vec<String>,
    error: Option<String>,
    restart_persist_error: Option<String>,
) -> AutoUpdateResultSnapshot {
    let completed_at = Utc::now();
    AutoUpdateResultSnapshot {
        attempt_id,
        started_at_epoch_ms: started_at.timestamp_millis(),
        started_at: started_at.to_rfc3339(),
        completed_at_epoch_ms: Some(completed_at.timestamp_millis()),
        completed_at: Some(completed_at.to_rfc3339()),
        channel: channel.as_str().to_string(),
        current_version: info.current_version.clone(),
        latest_version: info.latest_version.clone(),
        current_package_version: info.current_package_version.clone(),
        latest_package_version: info.latest_package_version.clone(),
        current_image_version: info.current_image_version.clone(),
        latest_image_version: info.latest_image_version.clone(),
        update_reason: update_reason_name(info.update_reason),
        outcome: outcome.as_str().to_string(),
        checksum_verified,
        installed_targets,
        error,
        restart_persist_error,
    }
}

fn update_reason_name(reason: Option<self_update::UpdateReason>) -> Option<String> {
    reason.and_then(|reason| {
        serde_json::to_value(reason)
            .ok()?
            .as_str()
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rhythm_os::state::AppState;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn utc_at(hour: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 18, hour, 0, 0).unwrap()
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-auto-update-{}-{}", name, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
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
    fn snapshot_settings_captures_auto_update_channel_and_utc_offset() {
        let state = Arc::new(Mutex::new(AppState::default()));
        {
            let mut guard = state.lock().unwrap();
            guard.auto_update = false;
            guard.platform_type = "appliance";
            guard.utc_offset_hours = -4.5;
            guard.data_dir = "/tmp/rhythm-test".to_string();
        }

        let snapshot = snapshot_settings(&state).unwrap();

        assert!(!snapshot.auto_update);
        assert_eq!(snapshot.channel, UpdateChannel::Stable);
        assert_eq!(snapshot.utc_offset_hours, -4.5);
        assert_eq!(snapshot.data_dir, "/tmp/rhythm-test");

        state.lock().unwrap().update_channel = Some(UpdateChannel::Beta);
        let snapshot = snapshot_settings(&state).unwrap();
        assert_eq!(snapshot.channel, UpdateChannel::Beta);
    }

    #[test]
    fn beta_channel_device_with_auto_update_polls_beta_feed() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let _guard = crate::self_update::ENV_LOCK.lock().unwrap();
        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let captured_path = Arc::new(Mutex::new(None::<String>));
        let capture = captured_path.clone();
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0_u8; 1024];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let read = stream.read(&mut request).unwrap_or(0);
            *capture.lock().unwrap() = String::from_utf8_lossy(&request[..read])
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .map(str::to_string);
            let _ = stream.write_all(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        });
        std::env::set_var(
            "RHYTHM_UPDATE_BASE_URL",
            format!("http://127.0.0.1:{}", port),
        );

        let dir = unique_test_dir("beta-feed");
        let state = Arc::new(Mutex::new(AppState::default()));
        {
            let mut guard = state.lock().unwrap();
            guard.platform_type = "appliance";
            guard.update_channel = Some(UpdateChannel::Beta);
            guard.data_dir = dir.display().to_string();
        }
        let settings = snapshot_settings(&state).unwrap();
        assert!(settings.auto_update);
        assert_eq!(settings.channel, UpdateChannel::Beta);

        let decision = attempt_update(&state, &settings);

        std::env::remove_var("RHYTHM_UPDATE_BASE_URL");

        assert_eq!(decision, AutoUpdateDecision::CheckFailed);
        let requested = captured_path.lock().unwrap().clone().expect("feed request");
        assert!(
            requested.ends_with("/manifest.json") && !requested.contains("-stable"),
            "beta channel must poll the un-suffixed feed, got {requested}"
        );
        let persisted = load_auto_update_state(&dir.join(AUTO_UPDATE_STATE_RELATIVE_PATH));
        assert_eq!(persisted.last_check.as_ref().unwrap().channel, "beta");

        let _ = fs::remove_dir_all(dir);
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

    #[test]
    fn status_path_uses_data_dir_ota_subdirectory() {
        let settings = LoopSettings {
            auto_update: true,
            channel: UpdateChannel::Stable,
            utc_offset_hours: 0.0,
            data_dir: "/tmp/rhythm-data".to_string(),
        };

        assert_eq!(
            auto_update_state_path(&settings).unwrap(),
            PathBuf::from("/tmp/rhythm-data").join(AUTO_UPDATE_STATE_RELATIVE_PATH)
        );

        let missing = LoopSettings {
            data_dir: String::new(),
            ..settings
        };
        assert!(auto_update_state_path(&missing).is_none());
    }

    #[test]
    fn persists_last_check_and_update_results_without_check_history() {
        let dir = unique_test_dir("state");
        let path = dir.join(AUTO_UPDATE_STATE_RELATIVE_PATH);
        let checked_at = utc_at(14);

        persist_last_check(
            Some(&path),
            check_snapshot_error(
                checked_at,
                UpdateChannel::Stable,
                "1.0.0",
                AutoUpdateDecision::CheckFailed,
                "timeout".to_string(),
            ),
        );

        let first = load_auto_update_state(&path);
        assert_eq!(first.schema_version, AUTO_UPDATE_STATE_SCHEMA_VERSION);
        assert_eq!(first.last_check.as_ref().unwrap().decision, "check_failed");
        assert_eq!(
            first.last_check.as_ref().unwrap().error.as_deref(),
            Some("timeout")
        );
        assert!(first.update_results.is_empty());

        let started = AutoUpdateResultSnapshot {
            attempt_id: "attempt-1".to_string(),
            started_at_epoch_ms: checked_at.timestamp_millis(),
            started_at: checked_at.to_rfc3339(),
            completed_at_epoch_ms: None,
            completed_at: None,
            channel: "stable".to_string(),
            current_version: "1.0.0".to_string(),
            latest_version: "1.1.0".to_string(),
            current_package_version: "1.0.0".to_string(),
            latest_package_version: "1.1.0".to_string(),
            current_image_version: Some("1.0.0".to_string()),
            latest_image_version: Some("1.1.0".to_string()),
            update_reason: Some("version_mismatch".to_string()),
            outcome: "started".to_string(),
            checksum_verified: None,
            installed_targets: Vec::new(),
            error: None,
            restart_persist_error: None,
        };
        persist_update_result(Some(&path), started.clone());

        let mut completed = started;
        completed.completed_at_epoch_ms = Some(checked_at.timestamp_millis() + 1);
        completed.completed_at = Some(checked_at.to_rfc3339());
        completed.outcome = "applied_restart_scheduled".to_string();
        completed.checksum_verified = Some(true);
        completed.installed_targets = vec!["rootfs_b".to_string(), "boot_slot_b".to_string()];
        persist_update_result(Some(&path), completed);

        persist_last_check(
            Some(&path),
            check_snapshot_error(
                checked_at,
                UpdateChannel::Stable,
                "1.1.0",
                AutoUpdateDecision::UpToDate,
                "not an error, just overwritten check state".to_string(),
            ),
        );

        let state = load_auto_update_state(&path);
        assert_eq!(state.last_check.as_ref().unwrap().current_version, "1.1.0");
        assert_eq!(state.update_results.len(), 1);
        assert_eq!(state.update_results[0].outcome, "applied_restart_scheduled");
        assert_eq!(
            state.update_results[0].installed_targets,
            vec!["rootfs_b", "boot_slot_b"]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn persisted_last_check_extends_dedupe_across_restart() {
        let dir = unique_test_dir("dedupe");
        let settings = LoopSettings {
            auto_update: true,
            channel: UpdateChannel::Stable,
            utc_offset_hours: 0.0,
            data_dir: dir.display().to_string(),
        };
        let path = auto_update_state_path(&settings).unwrap();
        let checked_at = utc_at(14);

        persist_last_check(
            Some(&path),
            check_snapshot_error(
                checked_at,
                UpdateChannel::Stable,
                "1.0.0",
                AutoUpdateDecision::UpToDate,
                "timeout".to_string(),
            ),
        );

        assert!(last_persisted_check_within_cooldown(
            &settings,
            checked_at + chrono::Duration::hours(19),
        ));
        assert!(!last_persisted_check_within_cooldown(
            &settings,
            checked_at + chrono::Duration::hours(21),
        ));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn transient_check_failure_retries_on_next_poll() {
        let dir = unique_test_dir("failure-retry");
        let settings = LoopSettings {
            auto_update: true,
            channel: UpdateChannel::Stable,
            utc_offset_hours: 0.0,
            data_dir: dir.display().to_string(),
        };
        let path = auto_update_state_path(&settings).unwrap();
        let checked_at = utc_at(14);

        persist_last_check(
            Some(&path),
            check_snapshot_error(
                checked_at,
                UpdateChannel::Stable,
                "1.0.0",
                AutoUpdateDecision::CheckFailed,
                "timeout".to_string(),
            ),
        );

        assert!(last_persisted_check_within_cooldown(
            &settings,
            checked_at + chrono::Duration::minutes(29)
        ));
        assert!(!last_persisted_check_within_cooldown(
            &settings,
            checked_at + chrono::Duration::minutes(31)
        ));

        let recent_failure = Some(RecentAttempt {
            at: Instant::now() - Duration::from_secs(29 * 60),
            decision: AutoUpdateDecision::CheckFailed,
        });
        assert!(recent_attempt_is_within_cooldown(recent_failure.as_ref()));

        let retryable_failure = Some(RecentAttempt {
            at: Instant::now() - Duration::from_secs(31 * 60),
            decision: AutoUpdateDecision::CheckFailed,
        });
        assert!(!recent_attempt_is_within_cooldown(
            retryable_failure.as_ref()
        ));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn wildly_future_persisted_check_does_not_block_forever() {
        let now = utc_at(14).timestamp_millis();

        assert!(
            timestamp_within_cooldown(now + 5_000, now, MIN_BETWEEN_CHECKS),
            "small future clock skew should still count as recent"
        );
        assert!(
            !timestamp_within_cooldown(
                now + chrono::Duration::days(365).num_milliseconds(),
                now,
                MIN_BETWEEN_CHECKS,
            ),
            "large future timestamps should not suppress checks until that date"
        );
    }
}
