use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;

use crate::model::{
    ArchiveResult, BlockedTaskSummary, CpuSnapshot, DetailSample, DiskSnapshot, EarlyBootSnapshot,
    EscalationSample, FilesystemSnapshot, HeartbeatSnapshot, LoadSnapshot, Observation,
    PowerSnapshot, ProcessDetail, PsiLine, PsiSnapshot, PstoreEntry, RecorderHealth, SourceStatus,
    SummarySample, TargetProcessSummary, TaskCounts, ThermalSnapshot, ThreadDetail,
    WatchdogSnapshot, SCHEMA_VERSION,
};
use crate::ring::{
    archive_current_boot_for_boot, atomic_write, atomic_write_json, current_ring_boot_id,
    read_json, recorder_root, RingConfig, RingWriter, EARLY_BOOT_FILE, PREVIOUS_EARLY_BOOT_FILE,
    PSTORE_CURRENT_DIR, PSTORE_PREVIOUS_DIR,
};

const TEXT_BYTES_LIMIT: usize = 16 * 1024;
const PROCESS_STATUS_BYTES_LIMIT: usize = 8 * 1024;
const THREAD_TEXT_BYTES_LIMIT: usize = 4 * 1024;
const THREAD_STACK_BYTES_LIMIT: usize = 2 * 1024;
const DMESG_TAIL_BYTES_LIMIT: usize = 16 * 1024;
const DMESG_SCAN_BYTES_LIMIT: usize = 256 * 1024;
const PSTORE_TOTAL_BYTES_LIMIT: usize = 256 * 1024;
const PSTORE_FILE_LIMIT: usize = 8;
const PROCESS_SCAN_LIMIT: usize = 4096;
const BLOCKED_TASK_LIMIT: usize = 8;
const THREAD_SCAN_LIMIT: usize = 64;
const ESCALATION_STACK_CAPTURE_LIMIT: usize = 4;
const OPTIONAL_SOURCE_TIMEOUT: Duration = Duration::from_millis(100);
const DETAIL_SOURCE_TIMEOUT: Duration = Duration::from_millis(150);
const DMESG_TIMEOUT: Duration = Duration::from_millis(500);

pub const SERVER_HEARTBEAT_FILE: &str = "rhythm-server-heartbeat.json";
pub const HARDWARE_WATCHDOG_HEARTBEAT_FILE: &str = "rhythm-hardware-watchdog-heartbeat.json";
pub const SERVER_HEARTBEAT_STALE_MS: u64 = 90_000;
pub const ESCALATION_REPEAT_INTERVAL_MS: u64 = 15 * 60_000;
pub const D_STATE_MIN_CONSECUTIVE_SAMPLES: u64 = 2;
pub const STARTUP_GRACE_MS: u64 = 60_000;
pub const CADENCE_SLIP_TOLERANCE_MS: u64 = 2_000;
pub const PSI_FULL_AVG10_THRESHOLD: f64 = 1.0;

#[derive(Clone, Debug)]
pub struct CollectorPaths {
    pub data_dir: PathBuf,
    pub proc_root: PathBuf,
    pub sys_root: PathBuf,
    pub run_root: PathBuf,
    pub dmesg_command: PathBuf,
}

impl CollectorPaths {
    pub fn appliance(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            proc_root: PathBuf::from("/proc"),
            sys_root: PathBuf::from("/sys"),
            run_root: PathBuf::from("/run"),
            dmesg_command: PathBuf::from("/bin/dmesg"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TriggerState {
    started_monotonic_ms: Option<u64>,
    last_sample_monotonic_ms: Option<u64>,
    d_state_streak: u64,
    last_escalation_by_key: BTreeMap<&'static str, u64>,
}

impl TriggerState {
    pub fn evaluate(
        &mut self,
        sample: &SummarySample,
        monotonic_ms: u64,
        expected_interval_ms: u64,
    ) -> Vec<String> {
        let started = *self.started_monotonic_ms.get_or_insert(monotonic_ms);
        let mut candidates = Vec::<(&'static str, String)>::new();

        if let Some(previous) = self.last_sample_monotonic_ms {
            let actual = monotonic_ms.saturating_sub(previous);
            if actual > expected_interval_ms.saturating_add(CADENCE_SLIP_TOLERANCE_MS) {
                candidates.push((
                    "recorder_cadence_slip",
                    format!("recorder_cadence_slip_ms:{actual}"),
                ));
            }
        }
        self.last_sample_monotonic_ms = Some(monotonic_ms);

        if let Some(tasks) = sample.tasks.value.as_ref() {
            if tasks.uninterruptible > 0 {
                self.d_state_streak = self.d_state_streak.saturating_add(1);
            } else {
                self.d_state_streak = 0;
            }
            if self.d_state_streak >= D_STATE_MIN_CONSECUTIVE_SAMPLES {
                candidates.push((
                    "persistent_d_state",
                    format!(
                        "persistent_d_state_count:{}_samples:{}",
                        tasks.uninterruptible, self.d_state_streak
                    ),
                ));
            }
        } else {
            self.d_state_streak = 0;
        }

        for (name, psi) in [("memory", &sample.psi_memory), ("io", &sample.psi_io)] {
            if psi
                .value
                .as_ref()
                .and_then(|value| value.full.as_ref())
                .is_some_and(|full| full.avg10 >= PSI_FULL_AVG10_THRESHOLD)
            {
                let key = match name {
                    "memory" => "memory_psi_full",
                    _ => "io_psi_full",
                };
                candidates.push((key, format!("{name}_psi_full_avg10")));
            }
        }

        let outside_startup_grace = monotonic_ms.saturating_sub(started) >= STARTUP_GRACE_MS;
        if outside_startup_grace {
            if sample
                .target_processes
                .value
                .as_ref()
                .is_some_and(|processes| {
                    !processes
                        .iter()
                        .any(|process| process.target == "rhythm-server")
                })
            {
                candidates.push((
                    "rhythm_server_process_missing",
                    "rhythm_server_process_missing".to_string(),
                ));
            }
            if sample
                .server_heartbeat
                .value
                .as_ref()
                .and_then(|heartbeat| heartbeat.age_ms)
                .is_some_and(|age| age >= SERVER_HEARTBEAT_STALE_MS)
            {
                candidates.push((
                    "rhythm_server_heartbeat_stale",
                    "rhythm_server_heartbeat_stale".to_string(),
                ));
            }
        }

        let mut reasons = Vec::new();
        for (key, reason) in candidates {
            let due = self.last_escalation_by_key.get(key).is_none_or(|last| {
                monotonic_ms.saturating_sub(*last) >= ESCALATION_REPEAT_INTERVAL_MS
            });
            if due {
                self.last_escalation_by_key.insert(key, monotonic_ms);
                reasons.push(reason);
            }
        }
        reasons
    }
}

pub fn collect_summary(paths: &CollectorPaths, health: &RecorderHealth) -> (u64, SummarySample) {
    let uptime = collect_uptime(paths);
    let monotonic_ms = uptime
        .value
        .as_ref()
        .map(|value| (*value * 1000.0).max(0.0) as u64)
        .unwrap_or(0);
    let (tasks, targets, blocked_tasks) = collect_tasks_and_targets(paths);
    let mut sample = SummarySample {
        load: collect_load(paths),
        cpu: collect_cpu(paths),
        memory_kib: collect_key_values(
            &paths.proc_root.join("meminfo"),
            &[
                "MemTotal",
                "MemAvailable",
                "Buffers",
                "Cached",
                "SwapTotal",
                "SwapFree",
                "Dirty",
                "Writeback",
                "Slab",
            ],
            false,
        ),
        vmstat: collect_key_values(
            &paths.proc_root.join("vmstat"),
            &[
                "pgpgin",
                "pgpgout",
                "pswpin",
                "pswpout",
                "pgfault",
                "pgmajfault",
                "oom_kill",
            ],
            false,
        ),
        psi_cpu: collect_psi(&paths.proc_root.join("pressure/cpu")),
        psi_memory: collect_psi(&paths.proc_root.join("pressure/memory")),
        psi_io: collect_psi(&paths.proc_root.join("pressure/io")),
        disks: collect_disks(paths),
        filesystem: collect_filesystem(&paths.data_dir),
        tasks,
        blocked_tasks,
        target_processes: targets,
        thermal: collect_thermal(paths),
        cpu_frequency_khz: collect_u64_file(
            &paths
                .sys_root
                .join("devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
            true,
        ),
        firmware_power: collect_power(paths),
        watchdog: collect_watchdog(paths),
        server_heartbeat: collect_heartbeat(
            &paths.run_root.join(SERVER_HEARTBEAT_FILE),
            monotonic_ms,
        ),
        hardware_watchdog_heartbeat: collect_heartbeat(
            &paths.run_root.join(HARDWARE_WATCHDOG_HEARTBEAT_FILE),
            monotonic_ms,
        ),
        recorder_health: health.clone(),
    };
    let (truncated, timed_out) = sample_status_counts(&sample);
    sample.recorder_health.truncated_sources = sample
        .recorder_health
        .truncated_sources
        .saturating_add(truncated);
    sample.recorder_health.timed_out_sources = sample
        .recorder_health
        .timed_out_sources
        .saturating_add(timed_out);
    (monotonic_ms, sample)
}

pub fn collect_boot_id(paths: &CollectorPaths) -> Observation<String> {
    read_bounded_text(
        &paths.proc_root.join("sys/kernel/random/boot_id"),
        4096,
        OPTIONAL_SOURCE_TIMEOUT,
        false,
    )
    .map_value(|value| value.trim().to_string())
}

pub fn collect_detail(paths: &CollectorPaths, with_stacks: bool) -> DetailSample {
    DetailSample {
        processes: collect_process_details(paths, with_stacks),
    }
}

pub fn collect_escalation(
    paths: &CollectorPaths,
    reasons: Vec<String>,
    blocked_tasks: &[BlockedTaskSummary],
) -> EscalationSample {
    EscalationSample {
        reasons,
        blocked_tasks: blocked_tasks.to_vec(),
        processes: collect_process_details(paths, true),
        dmesg_tail: run_bounded_command(
            &paths.dmesg_command,
            DMESG_SCAN_BYTES_LIMIT,
            DMESG_TAIL_BYTES_LIMIT,
            DMESG_TIMEOUT,
        ),
    }
}

fn collect_load(paths: &CollectorPaths) -> Observation<LoadSnapshot> {
    parse_observation(
        read_bounded_text(
            &paths.proc_root.join("loadavg"),
            1024,
            OPTIONAL_SOURCE_TIMEOUT,
            false,
        ),
        |text| {
            let fields = text.split_whitespace().collect::<Vec<_>>();
            let (runnable, total) = fields
                .get(3)
                .and_then(|value| value.split_once('/'))
                .ok_or("missing runnable/total")?;
            Ok(LoadSnapshot {
                one: parse_field(&fields, 0)?,
                five: parse_field(&fields, 1)?,
                fifteen: parse_field(&fields, 2)?,
                runnable_tasks: runnable.parse().map_err(|_| "invalid runnable")?,
                total_tasks: total.parse().map_err(|_| "invalid total")?,
            })
        },
    )
}

fn collect_cpu(paths: &CollectorPaths) -> Observation<CpuSnapshot> {
    parse_observation(
        read_bounded_text(
            &paths.proc_root.join("stat"),
            TEXT_BYTES_LIMIT,
            OPTIONAL_SOURCE_TIMEOUT,
            false,
        ),
        |text| {
            let line = text
                .lines()
                .find(|line| line.starts_with("cpu "))
                .ok_or("missing cpu")?;
            let counters = line
                .split_whitespace()
                .skip(1)
                .map(|value| value.parse::<u64>().map_err(|_| "invalid cpu counter"))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CpuSnapshot { counters })
        },
    )
}

fn collect_key_values(
    path: &Path,
    allowed: &[&str],
    optional: bool,
) -> Observation<BTreeMap<String, u64>> {
    let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
    parse_observation(
        read_bounded_text(path, TEXT_BYTES_LIMIT, OPTIONAL_SOURCE_TIMEOUT, optional),
        |text| {
            let mut values = BTreeMap::new();
            for line in text.lines() {
                let mut fields = line.split_whitespace();
                let key = fields.next().unwrap_or_default().trim_end_matches(':');
                if !allowed.contains(key) {
                    continue;
                }
                let Some(raw) = fields.next() else {
                    continue;
                };
                if let Ok(value) = raw.parse::<u64>() {
                    values.insert(key.to_string(), value);
                }
            }
            Ok(values)
        },
    )
}

fn collect_psi(path: &Path) -> Observation<PsiSnapshot> {
    parse_observation(
        read_bounded_text(path, 4096, OPTIONAL_SOURCE_TIMEOUT, true),
        |text| {
            let mut snapshot = PsiSnapshot::default();
            for line in text.lines() {
                let mut fields = line.split_whitespace();
                let kind = fields.next().unwrap_or_default();
                let mut values = BTreeMap::<&str, &str>::new();
                for field in fields {
                    if let Some((key, value)) = field.split_once('=') {
                        values.insert(key, value);
                    }
                }
                let parsed = PsiLine {
                    avg10: parse_map_float(&values, "avg10")?,
                    avg60: parse_map_float(&values, "avg60")?,
                    avg300: parse_map_float(&values, "avg300")?,
                    total_usec: values
                        .get("total")
                        .ok_or("missing PSI total")?
                        .parse()
                        .map_err(|_| "invalid PSI total")?,
                };
                match kind {
                    "some" => snapshot.some = Some(parsed),
                    "full" => snapshot.full = Some(parsed),
                    _ => {}
                }
            }
            Ok(snapshot)
        },
    )
}

fn collect_disks(paths: &CollectorPaths) -> Observation<Vec<DiskSnapshot>> {
    parse_observation(
        read_bounded_text(
            &paths.proc_root.join("diskstats"),
            TEXT_BYTES_LIMIT,
            OPTIONAL_SOURCE_TIMEOUT,
            false,
        ),
        |text| {
            let mut disks = Vec::new();
            for line in text.lines().take(16) {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                if fields.len() < 13 {
                    continue;
                }
                disks.push(DiskSnapshot {
                    major: parse_field(&fields, 0)?,
                    minor: parse_field(&fields, 1)?,
                    name: fields[2].to_string(),
                    reads_completed: parse_field(&fields, 3)?,
                    sectors_read: parse_field(&fields, 5)?,
                    writes_completed: parse_field(&fields, 7)?,
                    sectors_written: parse_field(&fields, 9)?,
                    io_in_progress: parse_field(&fields, 11)?,
                    io_time_ms: parse_field(&fields, 12)?,
                });
            }
            Ok(disks)
        },
    )
}

#[cfg(unix)]
fn collect_filesystem(path: &Path) -> Observation<FilesystemSnapshot> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(path_c) = CString::new(path.as_os_str().as_bytes()) else {
        return Observation::error(SourceStatus::Unavailable, "filesystem path contains NUL");
    };
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path_c is NUL terminated and stats points to writable storage.
    if unsafe { libc::statvfs(path_c.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return observation_from_io_error(io::Error::last_os_error(), false);
    }
    // SAFETY: statvfs returned success and initialized stats.
    let stats = unsafe { stats.assume_init() };
    #[cfg(target_os = "macos")]
    let (block_size, blocks, blocks_available) = (
        stats.f_frsize,
        u64::from(stats.f_blocks),
        u64::from(stats.f_bavail),
    );
    #[cfg(all(not(target_os = "macos"), target_pointer_width = "64"))]
    let (block_size, blocks, blocks_available) = (stats.f_frsize, stats.f_blocks, stats.f_bavail);
    #[cfg(all(not(target_os = "macos"), target_pointer_width = "32"))]
    let (block_size, blocks, blocks_available) = (
        u64::from(stats.f_frsize),
        u64::from(stats.f_blocks),
        u64::from(stats.f_bavail),
    );
    Observation::ok(FilesystemSnapshot {
        path: path.display().to_string(),
        block_size,
        blocks,
        blocks_available,
        bytes_total: blocks.saturating_mul(block_size),
        bytes_available: blocks_available.saturating_mul(block_size),
    })
}

#[cfg(not(unix))]
fn collect_filesystem(_path: &Path) -> Observation<FilesystemSnapshot> {
    Observation::unsupported("statvfs is unavailable on this platform")
}

fn collect_tasks_and_targets(
    paths: &CollectorPaths,
) -> (
    Observation<TaskCounts>,
    Observation<Vec<TargetProcessSummary>>,
    Vec<BlockedTaskSummary>,
) {
    let entries = match fs::read_dir(&paths.proc_root) {
        Ok(entries) => entries,
        Err(error) => {
            let status = status_from_io_error(&error, false);
            return (
                Observation::error(status, error.to_string()),
                Observation::error(status, error.to_string()),
                Vec::new(),
            );
        }
    };
    let started = Instant::now();
    let mut counts = TaskCounts::default();
    let mut targets = Vec::new();
    let mut blocked_tasks = Vec::new();
    for entry in entries.flatten().take(PROCESS_SCAN_LIMIT) {
        if started.elapsed() >= DETAIL_SOURCE_TIMEOUT {
            counts.truncated = true;
            break;
        }
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let stat = read_bounded_text(
            &entry.path().join("stat"),
            4096,
            OPTIONAL_SOURCE_TIMEOUT,
            false,
        );
        let Some(stat) = stat.value else {
            continue;
        };
        let state = parse_proc_stat_state(&stat).unwrap_or("?");
        counts.scanned = counts.scanned.saturating_add(1);
        match state {
            "R" => counts.running = counts.running.saturating_add(1),
            "D" => counts.uninterruptible = counts.uninterruptible.saturating_add(1),
            _ => {}
        }
        let comm = read_bounded_text(
            &entry.path().join("comm"),
            128,
            OPTIONAL_SOURCE_TIMEOUT,
            false,
        )
        .value
        .unwrap_or_default()
        .trim()
        .to_string();
        if state == "D" && blocked_tasks.len() < BLOCKED_TASK_LIMIT {
            blocked_tasks.push(BlockedTaskSummary {
                pid,
                comm: comm.clone(),
                wait_channel: read_bounded_text(
                    &entry.path().join("wchan"),
                    256,
                    OPTIONAL_SOURCE_TIMEOUT,
                    false,
                )
                .map_value(|value| value.trim().to_string()),
            });
        }
        let Some(target) = target_name(&comm) else {
            continue;
        };
        let (thread_count, threads_truncated) =
            bounded_entry_count(&entry.path().join("task"), 256);
        targets.push(TargetProcessSummary {
            target: target.to_string(),
            pid,
            comm,
            state: state.to_string(),
            thread_count: thread_count as u64,
        });
        counts.truncated |= threads_truncated;
    }
    let task_observation = if counts.truncated {
        Observation::truncated(counts, "process scan entry/time limit reached")
    } else {
        Observation::ok(counts)
    };
    (task_observation, Observation::ok(targets), blocked_tasks)
}

fn collect_thermal(paths: &CollectorPaths) -> Observation<Vec<ThermalSnapshot>> {
    let root = paths.sys_root.join("class/thermal");
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) => return observation_from_io_error(error, true),
    };
    let mut values = Vec::new();
    for entry in entries.flatten().take(8) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("thermal_zone") {
            continue;
        }
        if let Some(value) = collect_u64_file(&entry.path().join("temp"), false).value {
            values.push(ThermalSnapshot {
                zone: name,
                millidegrees_c: value.min(i64::MAX as u64) as i64,
            });
        }
    }
    Observation::ok(values)
}

fn collect_power(paths: &CollectorPaths) -> Observation<PowerSnapshot> {
    let candidates = [
        "class/hwmon/hwmon0/in0_lcrit_alarm",
        "class/hwmon/hwmon1/in0_lcrit_alarm",
        "devices/platform/soc/soc:firmware/get_throttled",
        "firmware/devicetree/base/chosen/bootloader/reset_reason",
    ];
    let mut values = BTreeMap::new();
    let mut supported = false;
    for relative in candidates {
        let path = paths.sys_root.join(relative);
        let value = read_bounded_text(&path, 4096, OPTIONAL_SOURCE_TIMEOUT, true);
        supported |= value.status != SourceStatus::Unsupported;
        values.insert(relative.to_string(), value);
    }
    let snapshot = PowerSnapshot { values };
    if supported {
        Observation::ok(snapshot)
    } else {
        Observation {
            status: SourceStatus::Unsupported,
            value: Some(snapshot),
            detail: Some("firmware power/reset sysfs is not exposed".to_string()),
        }
    }
}

fn collect_watchdog(paths: &CollectorPaths) -> Observation<WatchdogSnapshot> {
    let names = [
        "identity",
        "state",
        "status",
        "bootstatus",
        "timeout",
        "timeleft",
    ];
    let mut values = BTreeMap::new();
    let mut supported = false;
    for name in names {
        let value = read_bounded_text(
            &paths.sys_root.join("class/watchdog/watchdog0").join(name),
            4096,
            OPTIONAL_SOURCE_TIMEOUT,
            true,
        );
        supported |= value.status != SourceStatus::Unsupported;
        values.insert(name.to_string(), value);
    }
    let snapshot = WatchdogSnapshot { values };
    if supported {
        Observation::ok(snapshot)
    } else {
        Observation {
            status: SourceStatus::Unsupported,
            value: Some(snapshot),
            detail: Some("watchdog sysfs is not exposed".to_string()),
        }
    }
}

fn collect_process_details(paths: &CollectorPaths, with_stacks: bool) -> Vec<ProcessDetail> {
    let (_, targets, _) = collect_tasks_and_targets(paths);
    let mut details = Vec::new();
    let mut expensive_capture_budget = ESCALATION_STACK_CAPTURE_LIMIT;
    for target in targets.value.unwrap_or_default() {
        let process_dir = paths.proc_root.join(target.pid.to_string());
        let mut threads = Vec::new();
        let mut threads_truncated = false;
        if let Ok(entries) = fs::read_dir(process_dir.join("task")) {
            for entry in entries.flatten().take(THREAD_SCAN_LIMIT + 1) {
                if threads.len() >= THREAD_SCAN_LIMIT {
                    threads_truncated = true;
                    break;
                }
                let Some(tid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                else {
                    continue;
                };
                let task_dir = entry.path();
                let capture_expensive = with_stacks && expensive_capture_budget > 0;
                if capture_expensive {
                    expensive_capture_budget -= 1;
                }
                let status = read_bounded_text(
                    &task_dir.join("status"),
                    THREAD_TEXT_BYTES_LIMIT,
                    DETAIL_SOURCE_TIMEOUT,
                    false,
                );
                let state = status
                    .value
                    .as_deref()
                    .and_then(|text| status_value(text, "State"))
                    .map(|value| Observation::ok(value.to_string()))
                    .unwrap_or_else(|| {
                        Observation::error(SourceStatus::Unavailable, "State missing")
                    });
                threads.push(ThreadDetail {
                    tid,
                    name: read_bounded_text(
                        &task_dir.join("comm"),
                        128,
                        DETAIL_SOURCE_TIMEOUT,
                        false,
                    )
                    .map_value(|value| value.trim().to_string()),
                    state,
                    wait_channel: read_bounded_text(
                        &task_dir.join("wchan"),
                        256,
                        DETAIL_SOURCE_TIMEOUT,
                        false,
                    )
                    .map_value(|value| value.trim().to_string()),
                    kernel_stack: capture_expensive.then(|| {
                        read_bounded_text(
                            &task_dir.join("stack"),
                            THREAD_STACK_BYTES_LIMIT,
                            DETAIL_SOURCE_TIMEOUT,
                            false,
                        )
                    }),
                    scheduler: capture_expensive.then(|| {
                        read_bounded_text(
                            &task_dir.join("sched"),
                            THREAD_TEXT_BYTES_LIMIT,
                            DETAIL_SOURCE_TIMEOUT,
                            false,
                        )
                    }),
                });
            }
        }
        details.push(ProcessDetail {
            target: target.target,
            pid: target.pid,
            status: read_bounded_text(
                &process_dir.join("status"),
                PROCESS_STATUS_BYTES_LIMIT,
                DETAIL_SOURCE_TIMEOUT,
                false,
            ),
            io: read_bounded_text(
                &process_dir.join("io"),
                THREAD_TEXT_BYTES_LIMIT,
                DETAIL_SOURCE_TIMEOUT,
                false,
            ),
            threads,
            threads_truncated,
        });
    }
    details
}

pub fn boot_capture(
    paths: &CollectorPaths,
    late_fallback_capture: bool,
) -> io::Result<EarlyBootSnapshot> {
    let now = Utc::now();
    let boot_id = collect_boot_id(paths);
    let current_boot_id = boot_id
        .value
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let repeated_same_boot_capture =
        current_ring_boot_id(&paths.data_dir).as_deref() == Some(current_boot_id);
    let uptime = collect_uptime(paths);
    let inferred_boot_at = match uptime.value {
        Some(seconds) => chrono::Duration::from_std(Duration::from_secs_f64(seconds.max(0.0)))
            .map(|duration| Observation::ok((now - duration).to_rfc3339()))
            .unwrap_or_else(|error| {
                Observation::error(SourceStatus::Unavailable, error.to_string())
            }),
        None => Observation::error(
            uptime.status,
            uptime
                .detail
                .clone()
                .unwrap_or_else(|| "uptime unavailable".to_string()),
        ),
    };
    let restart_intent = parse_json_observation(read_bounded_text(
        &paths.data_dir.join("boot-diagnostics/restart-intent.json"),
        16 * 1024,
        OPTIONAL_SOURCE_TIMEOUT,
        true,
    ));
    let pstore = rotate_and_capture_pstore(paths, !repeated_same_boot_capture)?;
    let watchdog = collect_watchdog(paths);
    let firmware_reset_power = collect_power(paths);
    let archive =
        archive_current_boot_for_boot(&paths.data_dir, current_boot_id).unwrap_or_else(|error| {
            ArchiveResult {
                status: SourceStatus::Unavailable,
                archived_boot_id: None,
                valid_records: 0,
                torn_records: 0,
                corrupt_records: 0,
                detail: Some(error.to_string()),
            }
        });

    let root = recorder_root(&paths.data_dir);
    fs::create_dir_all(&root)?;
    let early_boot = root.join(EARLY_BOOT_FILE);
    let previous_early_boot = root.join(PREVIOUS_EARLY_BOOT_FILE);
    if !repeated_same_boot_capture {
        if let Some(previous_snapshot) = read_json::<EarlyBootSnapshot>(&early_boot) {
            let _ = atomic_write_json(&previous_early_boot, &previous_snapshot);
        }
    }

    let snapshot = EarlyBootSnapshot {
        schema_version: SCHEMA_VERSION,
        captured_at: now.to_rfc3339(),
        capture_order: vec![
            "boot_identity".to_string(),
            "prior_restart_intent".to_string(),
            "pstore".to_string(),
            "watchdog".to_string(),
            "firmware_reset_power".to_string(),
            "archive_result".to_string(),
        ],
        boot_id: boot_id.clone(),
        uptime_secs: uptime,
        inferred_boot_at,
        prior_restart_intent: restart_intent,
        pstore,
        watchdog,
        firmware_reset_power,
        archive,
        late_fallback_capture,
    };
    atomic_write_json(&early_boot, &snapshot)?;
    let mut writer = RingWriter::open(&paths.data_dir, current_boot_id, RingConfig::default())?;
    writer.sync()?;
    Ok(snapshot)
}

fn rotate_and_capture_pstore(
    paths: &CollectorPaths,
    rotate_previous: bool,
) -> io::Result<Observation<Vec<PstoreEntry>>> {
    let root = recorder_root(&paths.data_dir);
    fs::create_dir_all(&root)?;
    let current = root.join(PSTORE_CURRENT_DIR);
    let previous = root.join(PSTORE_PREVIOUS_DIR);
    if rotate_previous {
        if previous.is_dir() {
            fs::remove_dir_all(&previous)?;
        }
        if current.is_dir() {
            fs::rename(&current, &previous)?;
        }
    }

    let source = paths.sys_root.join("fs/pstore");
    let entries = match fs::read_dir(&source) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Observation::unsupported("pstore is not mounted/exposed"));
        }
        Err(error) => return Ok(observation_from_io_error(error, false)),
    };
    fs::create_dir_all(&current)?;
    let mut captured = Vec::new();
    let mut remaining = PSTORE_TOTAL_BYTES_LIMIT;
    for entry in entries.flatten().take(PSTORE_FILE_LIMIT) {
        if remaining == 0 || !entry.path().is_file() {
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.contains('/') || name.contains('\\') {
            continue;
        }
        let source_bytes = fs::metadata(entry.path())
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let read = read_bounded_bytes(&entry.path(), remaining, DETAIL_SOURCE_TIMEOUT, false);
        let BytesObservation { status, bytes, .. } = read;
        let bytes = bytes.unwrap_or_default();
        if !bytes.is_empty() || status == SourceStatus::Ok || status == SourceStatus::Truncated {
            atomic_write(&current.join(&name), &bytes)?;
            remaining = remaining.saturating_sub(bytes.len());
        }
        captured.push(PstoreEntry {
            name,
            status,
            source_bytes,
            captured_bytes: bytes.len() as u64,
        });
    }
    Ok(Observation::ok(captured))
}

pub fn write_heartbeat(path: &Path, kind: &str) -> io::Result<()> {
    let boot_id = read_bounded_text(
        Path::new("/proc/sys/kernel/random/boot_id"),
        4096,
        OPTIONAL_SOURCE_TIMEOUT,
        true,
    )
    .value
    .map(|value| value.trim().to_string());
    let monotonic_ms = read_bounded_text(
        Path::new("/proc/uptime"),
        1024,
        OPTIONAL_SOURCE_TIMEOUT,
        true,
    )
    .value
    .and_then(|value| value.split_whitespace().next()?.parse::<f64>().ok())
    .map(|seconds| (seconds.max(0.0) * 1000.0) as u64);
    let heartbeat = HeartbeatSnapshot {
        kind: kind.to_string(),
        pid: std::process::id(),
        boot_id,
        monotonic_ms,
        wall_time: Some(Utc::now().to_rfc3339()),
        age_ms: None,
    };
    atomic_write_json(path, &heartbeat)
}

fn collect_heartbeat(path: &Path, monotonic_ms: u64) -> Observation<HeartbeatSnapshot> {
    parse_observation(
        read_bounded_text(path, 4096, OPTIONAL_SOURCE_TIMEOUT, true),
        |text| {
            let mut heartbeat = serde_json::from_str::<HeartbeatSnapshot>(text)
                .map_err(|_| "malformed heartbeat JSON")?;
            heartbeat.age_ms = heartbeat
                .monotonic_ms
                .map(|written| monotonic_ms.saturating_sub(written));
            Ok(heartbeat)
        },
    )
}

fn collect_uptime(paths: &CollectorPaths) -> Observation<f64> {
    parse_observation(
        read_bounded_text(
            &paths.proc_root.join("uptime"),
            1024,
            OPTIONAL_SOURCE_TIMEOUT,
            false,
        ),
        |text| {
            text.split_whitespace()
                .next()
                .ok_or("missing uptime")?
                .parse()
                .map_err(|_| "invalid uptime")
        },
    )
}

fn collect_u64_file(path: &Path, optional: bool) -> Observation<u64> {
    parse_observation(
        read_bounded_text(path, 1024, OPTIONAL_SOURCE_TIMEOUT, optional),
        |text| text.trim().parse().map_err(|_| "invalid unsigned integer"),
    )
}

#[derive(Debug)]
struct BytesObservation {
    status: SourceStatus,
    bytes: Option<Vec<u8>>,
    detail: Option<String>,
}

fn read_bounded_text(
    path: &Path,
    limit: usize,
    timeout: Duration,
    optional: bool,
) -> Observation<String> {
    let bytes = read_bounded_bytes(path, limit, timeout, optional);
    let value = bytes
        .bytes
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    Observation {
        status: bytes.status,
        value,
        detail: bytes.detail,
    }
}

#[cfg(unix)]
fn read_bounded_bytes(
    path: &Path,
    limit: usize,
    timeout: Duration,
    optional: bool,
) -> BytesObservation {
    use std::os::unix::fs::OpenOptionsExt;

    let started = Instant::now();
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => return bytes_from_io_error(error, optional),
    };
    let read_limit = limit.saturating_add(1);
    let mut bytes = Vec::with_capacity(read_limit.min(4096));
    let mut chunk = [0_u8; 4096];
    loop {
        if started.elapsed() >= timeout {
            return BytesObservation {
                status: SourceStatus::TimedOut,
                bytes: (!bytes.is_empty()).then_some(bytes),
                detail: Some(format!("read exceeded {} ms", timeout.as_millis())),
            };
        }
        match file.read(&mut chunk) {
            Ok(0) => {
                return BytesObservation {
                    status: SourceStatus::Ok,
                    bytes: Some(bytes),
                    detail: None,
                }
            }
            Ok(count) => {
                let remaining = read_limit.saturating_sub(bytes.len());
                let take = count.min(remaining);
                bytes.extend_from_slice(&chunk[..take]);
                if take < count || bytes.len() > limit {
                    bytes.truncate(limit);
                    return BytesObservation {
                        status: SourceStatus::Truncated,
                        bytes: Some(bytes),
                        detail: Some(format!("source exceeded {limit} bytes")),
                    };
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return bytes_from_io_error(error, optional),
        }
    }
}

#[cfg(not(unix))]
fn read_bounded_bytes(
    path: &Path,
    limit: usize,
    _timeout: Duration,
    optional: bool,
) -> BytesObservation {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => return bytes_from_io_error(error, optional),
    };
    let mut bytes = Vec::new();
    match file
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
    {
        Ok(_) if bytes.len() > limit => {
            bytes.truncate(limit);
            BytesObservation {
                status: SourceStatus::Truncated,
                bytes: Some(bytes),
                detail: Some(format!("source exceeded {limit} bytes")),
            }
        }
        Ok(_) => BytesObservation {
            status: SourceStatus::Ok,
            bytes: Some(bytes),
            detail: None,
        },
        Err(error) => bytes_from_io_error(error, optional),
    }
}

fn bytes_from_io_error(error: io::Error, optional: bool) -> BytesObservation {
    BytesObservation {
        status: status_from_io_error(&error, optional),
        bytes: None,
        detail: Some(error.to_string()),
    }
}

fn status_from_io_error(error: &io::Error, optional: bool) -> SourceStatus {
    match error.kind() {
        io::ErrorKind::NotFound if optional => SourceStatus::Unsupported,
        io::ErrorKind::PermissionDenied => SourceStatus::PermissionDenied,
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => SourceStatus::TimedOut,
        _ => SourceStatus::Unavailable,
    }
}

fn observation_from_io_error<T>(error: io::Error, optional: bool) -> Observation<T> {
    Observation::error(status_from_io_error(&error, optional), error.to_string())
}

trait ObservationMap<T> {
    fn map_value<U>(self, mapper: impl FnOnce(T) -> U) -> Observation<U>;
}

impl<T> ObservationMap<T> for Observation<T> {
    fn map_value<U>(self, mapper: impl FnOnce(T) -> U) -> Observation<U> {
        Observation {
            status: self.status,
            value: self.value.map(mapper),
            detail: self.detail,
        }
    }
}

fn parse_observation<T>(
    source: Observation<String>,
    parser: impl FnOnce(&str) -> Result<T, &'static str>,
) -> Observation<T> {
    let Some(text) = source.value.as_deref() else {
        return Observation {
            status: source.status,
            value: None,
            detail: source.detail,
        };
    };
    match parser(text) {
        Ok(value) => Observation {
            status: source.status,
            value: Some(value),
            detail: source.detail,
        },
        Err(detail) => Observation::error(SourceStatus::Unavailable, detail),
    }
}

fn parse_json_observation(source: Observation<String>) -> Observation<Value> {
    parse_observation(source, |text| {
        serde_json::from_str(text).map_err(|_| "malformed JSON")
    })
}

fn parse_field<T: std::str::FromStr>(fields: &[&str], index: usize) -> Result<T, &'static str> {
    fields
        .get(index)
        .ok_or("missing field")?
        .parse()
        .map_err(|_| "invalid field")
}

fn parse_map_float(values: &BTreeMap<&str, &str>, key: &str) -> Result<f64, &'static str> {
    values
        .get(key)
        .ok_or("missing PSI average")?
        .parse()
        .map_err(|_| "invalid PSI average")
}

fn parse_proc_stat_state(stat: &str) -> Option<&str> {
    let end = stat.rfind(')')?;
    stat.get(end + 1..)?.split_whitespace().next()
}

fn target_name(comm: &str) -> Option<&'static str> {
    match comm {
        "rhythm-server" | "rhythm-linux-appliance" => Some("rhythm-server"),
        "rhythm-chipd" => Some("rhythm-chipd"),
        value if value.starts_with("rhythm-hardwar") => Some("rhythm-hardware-watchdog"),
        value if value.starts_with("rhythm-host-rec") => Some("rhythm-host-recorder"),
        _ => None,
    }
}

fn bounded_entry_count(path: &Path, limit: usize) -> (usize, bool) {
    let Ok(entries) = fs::read_dir(path) else {
        return (0, false);
    };
    let count = entries.flatten().take(limit + 1).count();
    (count.min(limit), count > limit)
}

fn status_value<'a>(status: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    status
        .lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
}

fn run_bounded_command(
    command: &Path,
    scan_limit: usize,
    tail_limit: usize,
    timeout: Duration,
) -> Observation<String> {
    if !command.is_file() {
        return Observation::unsupported(format!("{} is unavailable", command.display()));
    }
    let mut child = match Command::new(command)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return observation_from_io_error(error, false),
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return Observation::error(SourceStatus::Unavailable, "dmesg stdout unavailable");
    };
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(scan_limit.saturating_add(1) as u64)
            .read_to_end(&mut bytes);
        (result, bytes)
    });
    let started = Instant::now();
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Observation::error(SourceStatus::Unavailable, error.to_string());
            }
        }
    };
    let Ok((read_result, mut bytes)) = reader.join() else {
        return Observation::error(SourceStatus::Unavailable, "dmesg reader panicked");
    };
    if let Err(error) = read_result {
        return observation_from_io_error(error, false);
    }
    if timed_out {
        return Observation::error(
            SourceStatus::TimedOut,
            format!("command exceeded {} ms", timeout.as_millis()),
        );
    }
    let truncated = bytes.len() > scan_limit || bytes.len() > tail_limit;
    if bytes.len() > scan_limit {
        bytes.truncate(scan_limit);
    }
    let start = bytes.len().saturating_sub(tail_limit);
    let text = String::from_utf8_lossy(&bytes[start..]).into_owned();
    if truncated {
        Observation::truncated(text, format!("dmesg tail capped at {tail_limit} bytes"))
    } else {
        Observation::ok(text)
    }
}

fn sample_status_counts(sample: &SummarySample) -> (u64, u64) {
    let statuses = [
        sample.load.status,
        sample.cpu.status,
        sample.memory_kib.status,
        sample.vmstat.status,
        sample.psi_cpu.status,
        sample.psi_memory.status,
        sample.psi_io.status,
        sample.disks.status,
        sample.filesystem.status,
        sample.tasks.status,
        sample.target_processes.status,
        sample.thermal.status,
        sample.cpu_frequency_khz.status,
        sample.firmware_power.status,
        sample.watchdog.status,
        sample.server_heartbeat.status,
        sample.hardware_watchdog_heartbeat.status,
    ];
    (
        statuses
            .iter()
            .filter(|status| **status == SourceStatus::Truncated)
            .count() as u64,
        statuses
            .iter()
            .filter(|status| **status == SourceStatus::TimedOut)
            .count() as u64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(name: &str) -> CollectorPaths {
        let root = std::env::temp_dir().join(format!(
            "rhythm-host-recorder-collector-{name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let paths = CollectorPaths {
            data_dir: root.join("data"),
            proc_root: root.join("proc"),
            sys_root: root.join("sys"),
            run_root: root.join("run"),
            dmesg_command: root.join("bin/dmesg"),
        };
        for path in [
            &paths.data_dir,
            &paths.proc_root,
            &paths.sys_root,
            &paths.run_root,
            paths.dmesg_command.parent().unwrap(),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        paths
    }

    #[test]
    fn bounded_reader_distinguishes_empty_truncated_missing_and_timeout() {
        let paths = fixture("bounded");
        let empty = paths.data_dir.join("empty");
        fs::write(&empty, b"").unwrap();
        assert_eq!(
            read_bounded_text(&empty, 8, Duration::from_millis(20), false).status,
            SourceStatus::Ok
        );
        let large = paths.data_dir.join("large");
        fs::write(&large, b"0123456789").unwrap();
        assert_eq!(
            read_bounded_text(&large, 8, Duration::from_millis(20), false).status,
            SourceStatus::Truncated
        );
        let exact = paths.data_dir.join("exact");
        fs::write(&exact, b"01234567").unwrap();
        assert_eq!(
            read_bounded_text(&exact, 8, Duration::from_millis(20), false).status,
            SourceStatus::Ok
        );
        assert_eq!(
            read_bounded_text(
                &paths.data_dir.join("missing"),
                8,
                Duration::from_millis(20),
                true
            )
            .status,
            SourceStatus::Unsupported
        );

        #[cfg(unix)]
        {
            let fifo = paths.data_dir.join("blocked");
            let fifo_c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
            // SAFETY: fifo_c is a valid NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
            let _open_writer = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&fifo)
                .unwrap();
            assert_eq!(
                read_bounded_text(&fifo, 8, Duration::from_millis(20), false).status,
                SourceStatus::TimedOut
            );
        }
        fs::remove_dir_all(paths.data_dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn trigger_cools_down_each_signal_without_hiding_a_new_signal() {
        let paths = fixture("trigger");
        fs::write(paths.proc_root.join("uptime"), "100 0\n").unwrap();
        let mut sample = minimal_sample();
        sample.server_heartbeat = Observation::ok(HeartbeatSnapshot {
            kind: "server".to_string(),
            pid: 42,
            boot_id: Some("boot".to_string()),
            monotonic_ms: Some(1),
            wall_time: None,
            age_ms: Some(SERVER_HEARTBEAT_STALE_MS),
        });
        sample.target_processes = Observation::ok(vec![TargetProcessSummary {
            target: "rhythm-server".to_string(),
            pid: 42,
            comm: "rhythm-server".to_string(),
            state: "S".to_string(),
            thread_count: 2,
        }]);
        let mut state = TriggerState::default();
        assert!(state.evaluate(&sample, 0, 10_000).is_empty());
        let reasons = state.evaluate(&sample, STARTUP_GRACE_MS, 10_000);
        assert!(reasons
            .iter()
            .any(|reason| reason.contains("heartbeat_stale")));
        assert!(state
            .evaluate(&sample, STARTUP_GRACE_MS + 10_000, 10_000)
            .is_empty());

        sample.server_heartbeat = Observation::unsupported("healthy for test");
        sample.psi_memory = Observation::ok(PsiSnapshot {
            some: None,
            full: Some(PsiLine {
                avg10: PSI_FULL_AVG10_THRESHOLD,
                avg60: 0.0,
                avg300: 0.0,
                total_usec: 1,
            }),
        });
        let distinct = state.evaluate(&sample, STARTUP_GRACE_MS + 20_000, 10_000);
        assert_eq!(distinct, vec!["memory_psi_full_avg10"]);
        fs::remove_dir_all(paths.data_dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn transient_d_state_is_ignored_and_persistent_d_state_is_cooled_down() {
        let mut sample = minimal_sample();
        sample.target_processes = Observation::ok(vec![TargetProcessSummary {
            target: "rhythm-server".to_string(),
            pid: 42,
            comm: "rhythm-server".to_string(),
            state: "S".to_string(),
            thread_count: 1,
        }]);
        sample.server_heartbeat = Observation::ok(HeartbeatSnapshot {
            kind: "server".to_string(),
            pid: 42,
            boot_id: Some("boot".to_string()),
            monotonic_ms: Some(0),
            wall_time: None,
            age_ms: Some(0),
        });
        sample.tasks = Observation::ok(TaskCounts {
            uninterruptible: 1,
            ..TaskCounts::default()
        });
        let mut state = TriggerState::default();

        assert!(state.evaluate(&sample, 0, 30_000).is_empty());
        assert_eq!(
            state.evaluate(&sample, 30_000, 30_000),
            vec!["persistent_d_state_count:1_samples:2"]
        );
        assert!(state.evaluate(&sample, 60_000, 30_000).is_empty());

        sample.tasks = Observation::ok(TaskCounts::default());
        assert!(state.evaluate(&sample, 90_000, 30_000).is_empty());
        sample.tasks = Observation::ok(TaskCounts {
            uninterruptible: 1,
            ..TaskCounts::default()
        });
        assert!(state.evaluate(&sample, 120_000, 30_000).is_empty());
        assert!(state.evaluate(&sample, 150_000, 30_000).is_empty());
        state.last_sample_monotonic_ms = Some(ESCALATION_REPEAT_INTERVAL_MS);
        assert_eq!(
            state.evaluate(&sample, 30_000 + ESCALATION_REPEAT_INTERVAL_MS, 30_000,),
            vec!["persistent_d_state_count:1_samples:3"]
        );
    }

    #[test]
    fn process_scan_retains_blocked_task_identity() {
        let paths = fixture("blocked-task");
        let process = paths.proc_root.join("123");
        fs::create_dir_all(&process).unwrap();
        fs::write(process.join("stat"), "123 (mmcqd/0) D 1 2 3\n").unwrap();
        fs::write(process.join("comm"), "mmcqd/0\n").unwrap();
        fs::write(process.join("wchan"), "io_schedule\n").unwrap();

        let (tasks, _, blocked) = collect_tasks_and_targets(&paths);

        assert_eq!(tasks.value.unwrap().uninterruptible, 1);
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].pid, 123);
        assert_eq!(blocked[0].comm, "mmcqd/0");
        assert_eq!(
            blocked[0].wait_channel.value.as_deref(),
            Some("io_schedule")
        );
        fs::remove_dir_all(paths.data_dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn boot_capture_archives_ring_and_preserves_planned_intent_and_pstore() {
        let paths = fixture("boot");
        fs::create_dir_all(paths.proc_root.join("sys/kernel/random")).unwrap();
        fs::write(
            paths.proc_root.join("sys/kernel/random/boot_id"),
            "boot-b\n",
        )
        .unwrap();
        fs::write(paths.proc_root.join("uptime"), "10 0\n").unwrap();
        fs::create_dir_all(paths.sys_root.join("fs/pstore")).unwrap();
        fs::write(paths.sys_root.join("fs/pstore/dmesg-ramoops-0"), "panic").unwrap();
        fs::create_dir_all(paths.data_dir.join("boot-diagnostics")).unwrap();
        fs::write(
            paths.data_dir.join("boot-diagnostics/restart-intent.json"),
            r#"{"category":"manual_restart","reason":"user request"}"#,
        )
        .unwrap();
        let mut ring = RingWriter::open(&paths.data_dir, "boot-a", RingConfig::default()).unwrap();
        ring.append("boot-a", 1, "summary", &serde_json::json!({}), true)
            .unwrap();

        let snapshot = boot_capture(&paths, false).unwrap();
        assert_eq!(snapshot.archive.archived_boot_id.as_deref(), Some("boot-a"));
        assert_eq!(snapshot.prior_restart_intent.status, SourceStatus::Ok);
        assert_eq!(snapshot.pstore.value.unwrap()[0].captured_bytes, 5);
        assert!(recorder_root(&paths.data_dir)
            .join(crate::ring::PREVIOUS_DIR)
            .is_dir());

        let root = recorder_root(&paths.data_dir);
        fs::write(
            root.join(PREVIOUS_EARLY_BOOT_FILE),
            b"previous-early-sentinel",
        )
        .unwrap();
        fs::create_dir_all(root.join(PSTORE_PREVIOUS_DIR)).unwrap();
        fs::write(
            root.join(PSTORE_PREVIOUS_DIR)
                .join("previous-pstore-sentinel"),
            b"previous-pstore",
        )
        .unwrap();
        let repeated = boot_capture(&paths, true).unwrap();
        assert_eq!(repeated.archive.archived_boot_id.as_deref(), Some("boot-a"));
        assert_eq!(
            fs::read(root.join(PREVIOUS_EARLY_BOOT_FILE)).unwrap(),
            b"previous-early-sentinel"
        );
        assert_eq!(
            fs::read(
                root.join(PSTORE_PREVIOUS_DIR)
                    .join("previous-pstore-sentinel")
            )
            .unwrap(),
            b"previous-pstore"
        );
        fs::remove_dir_all(paths.data_dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn command_capture_is_time_and_byte_bounded() {
        let paths = fixture("command");
        let mut script = std::fs::File::create(&paths.dmesg_command).unwrap();
        script
            .write_all(b"#!/bin/sh\nprintf '0123456789abcdefghijklmnopqrstuvwxyz'\n")
            .unwrap();
        script.flush().unwrap();
        drop(script);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&paths.dmesg_command, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let captured = run_bounded_command(&paths.dmesg_command, 64, 8, Duration::from_millis(500));
        assert_eq!(captured.status, SourceStatus::Truncated);
        assert_eq!(captured.value.as_deref(), Some("stuvwxyz"));
        fs::remove_dir_all(paths.data_dir.parent().unwrap()).unwrap();
    }

    fn minimal_sample() -> SummarySample {
        SummarySample {
            load: Observation::unsupported("test"),
            cpu: Observation::unsupported("test"),
            memory_kib: Observation::unsupported("test"),
            vmstat: Observation::unsupported("test"),
            psi_cpu: Observation::unsupported("test"),
            psi_memory: Observation::unsupported("test"),
            psi_io: Observation::unsupported("test"),
            disks: Observation::unsupported("test"),
            filesystem: Observation::unsupported("test"),
            tasks: Observation::ok(TaskCounts::default()),
            blocked_tasks: Vec::new(),
            target_processes: Observation::ok(Vec::new()),
            thermal: Observation::unsupported("test"),
            cpu_frequency_khz: Observation::unsupported("test"),
            firmware_power: Observation::unsupported("test"),
            watchdog: Observation::unsupported("test"),
            server_heartbeat: Observation::unsupported("test"),
            hardware_watchdog_heartbeat: Observation::unsupported("test"),
            recorder_health: RecorderHealth::default(),
        }
    }
}
