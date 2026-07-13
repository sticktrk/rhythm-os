//! Durable, bounded evidence that survives an appliance reboot.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rhythm_os::state::SharedState;
use serde::{Deserialize, Serialize};

pub const DIAGNOSTICS_DIR: &str = "boot-diagnostics";
pub const CURRENT_BOOT_FILE: &str = "boot-diagnostics/current-boot.json";
pub const PREVIOUS_BOOT_FILE: &str = "boot-diagnostics/previous-boot.json";
pub const RESTART_INTENT_FILE: &str = "boot-diagnostics/restart-intent.json";
pub const PREVIOUS_RESTART_INTENT_FILE: &str = "boot-diagnostics/previous-restart-intent.json";
pub const LAST_GASP_FILE: &str = "boot-diagnostics/last-gasp.json";
pub const PREVIOUS_LAST_GASP_FILE: &str = "boot-diagnostics/previous-last-gasp.json";
pub const KERNEL_CURRENT_FILE: &str = "boot-diagnostics/kernel-current.log";
pub const KERNEL_PREVIOUS_FILE: &str = "boot-diagnostics/kernel-previous.log";
pub const HARDWARE_RESET_FILE: &str = "boot-diagnostics/hardware-reset.json";
pub const PSTORE_DIR: &str = "boot-diagnostics/pstore";

const DATA_DIR_ENV: &str = "RHYTHM_DATA_DIR";
const DEFAULT_APPLIANCE_DATA_DIR: &str = "/data";
const LAST_GASP_INTERVAL: Duration = Duration::from_secs(60);
const KERNEL_CAPTURE_INTERVALS: u64 = 10;
#[cfg(target_os = "linux")]
const KERNEL_LOG_BYTES_LIMIT: usize = 256 * 1024;
const PSTORE_FILE_LIMIT: usize = 8;
const PSTORE_TOTAL_BYTES_LIMIT: usize = 256 * 1024;

static PROCESS_STARTED: OnceLock<Instant> = OnceLock::new();
static LAST_HTTP_EPOCH_MS: AtomicI64 = AtomicI64::new(0);
static WATCHDOG_ARMED: AtomicBool = AtomicBool::new(false);
static WATCHDOG_LAST_HEARTBEAT_EPOCH_MS: AtomicI64 = AtomicI64::new(0);
static WATCHDOG_TRIGGER_COUNT: AtomicU64 = AtomicU64::new(0);
static WATCHDOG_LAST_TRIGGER_REASON: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BootRecord {
    schema_version: u32,
    boot_id: Option<String>,
    process_started_at: String,
    process_pid: u32,
    platform_type: String,
    system_uptime_secs: Option<f64>,
    inferred_boot_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RestartIntent {
    schema_version: u32,
    recorded_at: String,
    boot_id: Option<String>,
    process_pid: u32,
    reason: String,
    category: String,
}

#[derive(Debug, Serialize)]
struct PreviousBootRecord {
    schema_version: u32,
    detected_at: String,
    classification: String,
    prior_boot_id: Option<String>,
    current_boot_id: Option<String>,
    prior_process_started_at: String,
    planned_restart: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    restart_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restart_category: Option<String>,
    kernel_evidence_available: bool,
    pstore_evidence_available: bool,
}

#[derive(Debug, Serialize)]
struct WatchdogSnapshot {
    owner: &'static str,
    armed: bool,
    last_heartbeat_epoch_ms: Option<i64>,
    trigger_count: u64,
    last_trigger_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct LastGaspSnapshot {
    schema_version: u32,
    generated_at: String,
    generated_at_epoch_ms: i64,
    boot_id: Option<String>,
    system_uptime_secs: Option<f64>,
    process_uptime_secs: f64,
    last_http_request_epoch_ms: Option<i64>,
    open_fd_count: Option<usize>,
    thread_count: Option<usize>,
    loadavg: Option<String>,
    meminfo: Option<String>,
    watchdog: WatchdogSnapshot,
    runtime_health: serde_json::Value,
    remote_access_status: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct HardwareResetSnapshot {
    schema_version: u32,
    generated_at: String,
    values: std::collections::BTreeMap<String, String>,
}

/// Record this process start and classify the prior process/boot using the
/// durable marker written before every planned restart.
pub fn record_startup(data_dir: &Path, platform_type: &str) -> Result<(), String> {
    PROCESS_STARTED.get_or_init(Instant::now);
    capture_pstore(data_dir, Path::new("/sys/fs/pstore"));
    record_startup_with(
        data_dir,
        platform_type,
        read_trimmed(Path::new("/proc/sys/kernel/random/boot_id")),
        system_uptime_secs(),
        Utc::now(),
    )?;

    Ok(())
}

fn record_startup_with(
    data_dir: &Path,
    platform_type: &str,
    boot_id: Option<String>,
    uptime_secs: Option<f64>,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let diagnostics_dir = data_dir.join(DIAGNOSTICS_DIR);
    fs::create_dir_all(&diagnostics_dir).map_err(|error| error.to_string())?;

    archive_if_present(data_dir, LAST_GASP_FILE, PREVIOUS_LAST_GASP_FILE);
    archive_if_present(data_dir, KERNEL_CURRENT_FILE, KERNEL_PREVIOUS_FILE);

    let prior_boot = read_json::<BootRecord>(&data_dir.join(CURRENT_BOOT_FILE));
    let restart_intent = read_json::<RestartIntent>(&data_dir.join(RESTART_INTENT_FILE));

    if let Some(prior) = prior_boot {
        let intent_matches = restart_intent.as_ref().is_some_and(|intent| {
            intent.process_pid == prior.process_pid
                && (intent.boot_id.is_none()
                    || prior.boot_id.is_none()
                    || intent.boot_id == prior.boot_id)
        });
        let same_boot = match (prior.boot_id.as_ref(), boot_id.as_ref()) {
            (Some(prior_boot_id), Some(current_boot_id)) => Some(prior_boot_id == current_boot_id),
            _ => None,
        };
        let classification = match (same_boot, intent_matches) {
            (Some(true), true) => "planned_process_restart",
            (Some(true), false) => "unplanned_process_restart",
            (Some(false), true) => restart_intent
                .as_ref()
                .map(|intent| intent.category.as_str())
                .unwrap_or("planned_host_restart"),
            (Some(false), false) => "unplanned_host_restart_unknown",
            (None, true) => "planned_restart_scope_unknown",
            (None, false) => "unplanned_restart_scope_unknown",
        };
        let previous = PreviousBootRecord {
            schema_version: 1,
            detected_at: now.to_rfc3339(),
            classification: classification.to_string(),
            prior_boot_id: prior.boot_id,
            current_boot_id: boot_id.clone(),
            prior_process_started_at: prior.process_started_at,
            planned_restart: intent_matches,
            restart_reason: intent_matches
                .then(|| restart_intent.as_ref().map(|intent| intent.reason.clone()))
                .flatten(),
            restart_category: intent_matches
                .then(|| {
                    restart_intent
                        .as_ref()
                        .map(|intent| intent.category.clone())
                })
                .flatten(),
            kernel_evidence_available: data_dir.join(KERNEL_PREVIOUS_FILE).is_file(),
            pstore_evidence_available: pstore_has_files(&data_dir.join(PSTORE_DIR)),
        };
        write_json(&data_dir.join(PREVIOUS_BOOT_FILE), &previous)?;
    }

    if let Some(intent) = restart_intent {
        write_json(&data_dir.join(PREVIOUS_RESTART_INTENT_FILE), &intent)?;
        let _ = fs::remove_file(data_dir.join(RESTART_INTENT_FILE));
    }

    let inferred_boot_at = uptime_secs.and_then(|uptime| {
        chrono::Duration::from_std(Duration::from_secs_f64(uptime))
            .ok()
            .map(|duration| (now - duration).to_rfc3339())
    });
    let current = BootRecord {
        schema_version: 1,
        boot_id,
        process_started_at: now.to_rfc3339(),
        process_pid: std::process::id(),
        platform_type: platform_type.to_string(),
        system_uptime_secs: uptime_secs,
        inferred_boot_at,
    };
    write_json(&data_dir.join(CURRENT_BOOT_FILE), &current)
}

/// Persist explicit restart intent before the restart thread sleeps or exits.
pub fn record_restart_intent(reason: &str) -> Result<(), String> {
    let data_dir = std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_APPLIANCE_DATA_DIR));
    let intent = RestartIntent {
        schema_version: 1,
        recorded_at: Utc::now().to_rfc3339(),
        boot_id: read_trimmed(Path::new("/proc/sys/kernel/random/boot_id")),
        process_pid: std::process::id(),
        reason: reason.to_string(),
        category: restart_category(reason).to_string(),
    };
    write_json(&data_dir.join(RESTART_INTENT_FILE), &intent)
}

fn restart_category(reason: &str) -> &'static str {
    match reason {
        "self-update" => "ota_restart",
        "user request" => "manual_restart",
        "factory reset" => "factory_reset",
        "liveness watchdog" => "watchdog_restart",
        _ => "planned_restart",
    }
}

pub fn record_http_request() {
    LAST_HTTP_EPOCH_MS.store(Utc::now().timestamp_millis(), Ordering::Relaxed);
}

pub fn watchdog_armed() {
    WATCHDOG_ARMED.store(true, Ordering::Relaxed);
    watchdog_heartbeat();
}

pub fn watchdog_heartbeat() {
    WATCHDOG_LAST_HEARTBEAT_EPOCH_MS.store(Utc::now().timestamp_millis(), Ordering::Relaxed);
}

pub fn watchdog_triggered(reason: &str) {
    WATCHDOG_TRIGGER_COUNT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut last) = WATCHDOG_LAST_TRIGGER_REASON
        .get_or_init(|| Mutex::new(None))
        .lock()
    {
        *last = Some(reason.to_string());
    }
}

pub fn spawn_last_gasp_recorder(state: SharedState, data_dir: PathBuf) {
    std::thread::Builder::new()
        .name("last-gasp-recorder".to_string())
        .spawn(move || {
            let mut intervals = 0_u64;
            loop {
                if let Err(error) = write_last_gasp(&state, &data_dir) {
                    log::warn!(target: "sys", "Failed to persist last-gasp diagnostics: {}", error);
                }
                if intervals.is_multiple_of(KERNEL_CAPTURE_INTERVALS) {
                    capture_kernel_evidence(&data_dir);
                }
                intervals = intervals.saturating_add(1);
                std::thread::sleep(LAST_GASP_INTERVAL);
            }
        })
        .expect("Failed to spawn last-gasp diagnostics thread");
}

fn write_last_gasp(state: &SharedState, data_dir: &Path) -> Result<(), String> {
    let now = Utc::now();
    let runtime_health = crate::debug_bundle::build_runtime_health_json(state, now)
        .map_err(|error| error.to_string())
        .and_then(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))?;
    let remote_access_status = rhythm_os::remote_access::status_snapshot_json(state);
    let remote_access_status = serde_json::from_str(&remote_access_status)
        .unwrap_or_else(|_| serde_json::json!({"status": "snapshot_unavailable"}));
    let last_http = LAST_HTTP_EPOCH_MS.load(Ordering::Relaxed);
    let last_watchdog_heartbeat = WATCHDOG_LAST_HEARTBEAT_EPOCH_MS.load(Ordering::Relaxed);
    let snapshot = LastGaspSnapshot {
        schema_version: 1,
        generated_at: now.to_rfc3339(),
        generated_at_epoch_ms: now.timestamp_millis(),
        boot_id: read_trimmed(Path::new("/proc/sys/kernel/random/boot_id")),
        system_uptime_secs: system_uptime_secs(),
        process_uptime_secs: PROCESS_STARTED
            .get_or_init(Instant::now)
            .elapsed()
            .as_secs_f64(),
        last_http_request_epoch_ms: (last_http > 0).then_some(last_http),
        open_fd_count: directory_entry_count(Path::new("/proc/self/fd")),
        thread_count: directory_entry_count(Path::new("/proc/self/task")),
        loadavg: read_bounded(Path::new("/proc/loadavg"), 1024),
        meminfo: read_bounded(Path::new("/proc/meminfo"), 16 * 1024),
        watchdog: WatchdogSnapshot {
            owner: "periodic_liveness",
            armed: WATCHDOG_ARMED.load(Ordering::Relaxed),
            last_heartbeat_epoch_ms: (last_watchdog_heartbeat > 0)
                .then_some(last_watchdog_heartbeat),
            trigger_count: WATCHDOG_TRIGGER_COUNT.load(Ordering::Relaxed),
            last_trigger_reason: WATCHDOG_LAST_TRIGGER_REASON
                .get_or_init(|| Mutex::new(None))
                .lock()
                .ok()
                .and_then(|last| last.clone()),
        },
        runtime_health,
        remote_access_status,
    };
    write_json(&data_dir.join(LAST_GASP_FILE), &snapshot)?;
    write_hardware_reset_snapshot(data_dir, now)
}

fn write_hardware_reset_snapshot(data_dir: &Path, now: DateTime<Utc>) -> Result<(), String> {
    let mut values = std::collections::BTreeMap::new();
    for path in [
        "/sys/class/watchdog/watchdog0/identity",
        "/sys/class/watchdog/watchdog0/state",
        "/sys/class/watchdog/watchdog0/status",
        "/sys/class/watchdog/watchdog0/bootstatus",
        "/sys/class/watchdog/watchdog0/timeout",
        "/sys/class/watchdog/watchdog0/timeleft",
    ] {
        if let Some(value) = read_bounded(Path::new(path), 4096) {
            values.insert(path.to_string(), value.trim().to_string());
        }
    }
    let snapshot = HardwareResetSnapshot {
        schema_version: 1,
        generated_at: now.to_rfc3339(),
        values,
    };
    write_json(&data_dir.join(HARDWARE_RESET_FILE), &snapshot)
}

fn capture_kernel_evidence(data_dir: &Path) {
    #[cfg(target_os = "linux")]
    {
        let output = match Command::new("dmesg").output() {
            Ok(output) => output,
            Err(error) => {
                log::debug!(target: "sys", "Unable to capture dmesg: {}", error);
                return;
            }
        };
        let bytes = if output.status.success() {
            output.stdout
        } else {
            output.stderr
        };
        if bytes.is_empty() {
            return;
        }
        let tail = tail_bytes(&bytes, KERNEL_LOG_BYTES_LIMIT);
        if let Err(error) =
            crate::bootstate::atomic_write_with_sync(&data_dir.join(KERNEL_CURRENT_FILE), tail)
        {
            log::debug!(target: "sys", "Unable to persist dmesg snapshot: {}", error);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = data_dir;
}

fn capture_pstore(data_dir: &Path, source_dir: &Path) {
    let target_dir = data_dir.join(PSTORE_DIR);
    let entries = match fs::read_dir(source_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let _ = fs::remove_dir_all(&target_dir);
    if fs::create_dir_all(&target_dir).is_err() {
        return;
    }
    let mut remaining = PSTORE_TOTAL_BYTES_LIMIT;
    for entry in entries.flatten().take(PSTORE_FILE_LIMIT) {
        if remaining == 0 || !entry.path().is_file() {
            break;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        let captured = tail_bytes(&bytes, remaining);
        if fs::write(target_dir.join(name), captured).is_ok() {
            remaining = remaining.saturating_sub(captured.len());
        }
    }
}

fn pstore_has_files(path: &Path) -> bool {
    fs::read_dir(path).ok().is_some_and(|mut entries| {
        entries.any(|entry| entry.ok().is_some_and(|e| e.path().is_file()))
    })
}

fn archive_if_present(data_dir: &Path, current: &str, previous: &str) {
    let current = data_dir.join(current);
    if current.is_file() {
        let _ = fs::copy(current, data_dir.join(previous));
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut body = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    body.push(b'\n');
    crate::bootstate::atomic_write_with_sync(path, &body)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn read_trimmed(path: &Path) -> Option<String> {
    read_bounded(path, 4096).map(|value| value.trim().to_string())
}

fn read_bounded(path: &Path, limit: usize) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let bytes = tail_bytes(&bytes, limit);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn tail_bytes(bytes: &[u8], limit: usize) -> &[u8] {
    &bytes[bytes.len().saturating_sub(limit)..]
}

fn system_uptime_secs() -> Option<f64> {
    read_trimmed(Path::new("/proc/uptime"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn directory_entry_count(path: &Path) -> Option<usize> {
    Some(fs::read_dir(path).ok()?.flatten().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rhythm-boot-diagnostics-{}-{}-{}",
            name,
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn planned_restart_is_classified_across_boot_ids() {
        let dir = temp_dir("planned");
        let now = Utc::now();
        record_startup_with(&dir, "appliance", Some("boot-a".into()), Some(10.0), now).unwrap();
        write_json(
            &dir.join(RESTART_INTENT_FILE),
            &RestartIntent {
                schema_version: 1,
                recorded_at: now.to_rfc3339(),
                boot_id: Some("boot-a".into()),
                process_pid: std::process::id(),
                reason: "self-update".into(),
                category: restart_category("self-update").into(),
            },
        )
        .unwrap();

        record_startup_with(&dir, "appliance", Some("boot-b".into()), Some(5.0), now).unwrap();

        let previous: serde_json::Value = read_json(&dir.join(PREVIOUS_BOOT_FILE)).unwrap();
        assert_eq!(previous["classification"], "ota_restart");
        assert_eq!(previous["restart_reason"], "self-update");
        assert!(!dir.join(RESTART_INTENT_FILE).exists());
        assert!(dir.join(PREVIOUS_RESTART_INTENT_FILE).is_file());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn changed_boot_without_intent_is_unplanned_unknown() {
        let dir = temp_dir("unknown");
        let now = Utc::now();
        record_startup_with(&dir, "appliance", Some("boot-a".into()), Some(10.0), now).unwrap();
        record_startup_with(&dir, "appliance", Some("boot-b".into()), Some(5.0), now).unwrap();

        let previous: serde_json::Value = read_json(&dir.join(PREVIOUS_BOOT_FILE)).unwrap();
        assert_eq!(previous["classification"], "unplanned_host_restart_unknown");
        assert_eq!(previous["planned_restart"], false);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_boot_ids_do_not_claim_a_host_restart() {
        let dir = temp_dir("missing-boot-id");
        let now = Utc::now();
        record_startup_with(&dir, "desktop", None, None, now).unwrap();
        record_startup_with(&dir, "desktop", None, None, now).unwrap();

        let previous: serde_json::Value = read_json(&dir.join(PREVIOUS_BOOT_FILE)).unwrap();
        assert_eq!(
            previous["classification"],
            "unplanned_restart_scope_unknown"
        );
        assert_eq!(previous["planned_restart"], false);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn planned_restart_without_boot_ids_reports_unknown_scope() {
        let dir = temp_dir("planned-missing-boot-id");
        let now = Utc::now();
        record_startup_with(&dir, "desktop", None, None, now).unwrap();
        write_json(
            &dir.join(RESTART_INTENT_FILE),
            &RestartIntent {
                schema_version: 1,
                recorded_at: now.to_rfc3339(),
                boot_id: None,
                process_pid: std::process::id(),
                reason: "user request".into(),
                category: restart_category("user request").into(),
            },
        )
        .unwrap();

        record_startup_with(&dir, "desktop", None, None, now).unwrap();

        let previous: serde_json::Value = read_json(&dir.join(PREVIOUS_BOOT_FILE)).unwrap();
        assert_eq!(previous["classification"], "planned_restart_scope_unknown");
        assert_eq!(previous["planned_restart"], true);
        assert_eq!(previous["restart_reason"], "user request");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_restart_intent_from_another_process_is_not_matched() {
        let dir = temp_dir("stale-intent");
        let now = Utc::now();
        record_startup_with(&dir, "appliance", Some("boot-a".into()), Some(10.0), now).unwrap();
        write_json(
            &dir.join(RESTART_INTENT_FILE),
            &RestartIntent {
                schema_version: 1,
                recorded_at: now.to_rfc3339(),
                boot_id: Some("boot-a".into()),
                process_pid: std::process::id().saturating_add(1),
                reason: "self-update".into(),
                category: restart_category("self-update").into(),
            },
        )
        .unwrap();

        record_startup_with(&dir, "appliance", Some("boot-a".into()), Some(20.0), now).unwrap();

        let previous: serde_json::Value = read_json(&dir.join(PREVIOUS_BOOT_FILE)).unwrap();
        assert_eq!(previous["classification"], "unplanned_process_restart");
        assert_eq!(previous["planned_restart"], false);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pstore_capture_is_bounded() {
        let dir = temp_dir("pstore");
        let source = temp_dir("pstore-source");
        fs::write(
            source.join("dmesg-ramoops-0"),
            vec![b'x'; PSTORE_TOTAL_BYTES_LIMIT * 2],
        )
        .unwrap();

        capture_pstore(&dir, &source);

        assert_eq!(
            fs::metadata(dir.join(PSTORE_DIR).join("dmesg-ramoops-0"))
                .unwrap()
                .len(),
            PSTORE_TOTAL_BYTES_LIMIT as u64
        );
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(source);
    }
}
