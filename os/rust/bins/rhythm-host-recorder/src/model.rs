use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Ok,
    Unsupported,
    Unavailable,
    PermissionDenied,
    TimedOut,
    Truncated,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Observation<T> {
    pub status: SourceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl<T> Observation<T> {
    pub fn ok(value: T) -> Self {
        Self {
            status: SourceStatus::Ok,
            value: Some(value),
            detail: None,
        }
    }

    pub fn truncated(value: T, detail: impl Into<String>) -> Self {
        Self {
            status: SourceStatus::Truncated,
            value: Some(value),
            detail: Some(detail.into()),
        }
    }

    pub fn error(status: SourceStatus, detail: impl Into<String>) -> Self {
        Self {
            status,
            value: None,
            detail: Some(detail.into()),
        }
    }

    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self::error(SourceStatus::Unsupported, detail)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RecorderHealth {
    pub dropped_samples: u64,
    pub late_cycles: u64,
    pub write_failures: u64,
    pub sync_failures: u64,
    pub rotations: u64,
    pub recovered_segments: u64,
    pub torn_records: u64,
    pub corrupt_records: u64,
    pub truncated_sources: u64,
    pub timed_out_sources: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LoadSnapshot {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
    pub runnable_tasks: u64,
    pub total_tasks: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CpuSnapshot {
    pub counters: Vec<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PsiLine {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total_usec: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PsiSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub some: Option<PsiLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<PsiLine>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DiskSnapshot {
    pub major: u64,
    pub minor: u64,
    pub name: String,
    pub reads_completed: u64,
    pub sectors_read: u64,
    pub writes_completed: u64,
    pub sectors_written: u64,
    pub io_in_progress: u64,
    pub io_time_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FilesystemSnapshot {
    pub path: String,
    pub block_size: u64,
    pub blocks: u64,
    pub blocks_available: u64,
    pub bytes_total: u64,
    pub bytes_available: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TaskCounts {
    pub scanned: u64,
    pub running: u64,
    pub uninterruptible: u64,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TargetProcessSummary {
    pub target: String,
    pub pid: u32,
    pub comm: String,
    pub state: String,
    pub thread_count: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ThermalSnapshot {
    pub zone: String,
    pub millidegrees_c: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WatchdogSnapshot {
    pub values: BTreeMap<String, Observation<String>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PowerSnapshot {
    pub values: BTreeMap<String, Observation<String>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HeartbeatSnapshot {
    pub kind: String,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monotonic_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SummarySample {
    pub load: Observation<LoadSnapshot>,
    pub cpu: Observation<CpuSnapshot>,
    pub memory_kib: Observation<BTreeMap<String, u64>>,
    pub vmstat: Observation<BTreeMap<String, u64>>,
    pub psi_cpu: Observation<PsiSnapshot>,
    pub psi_memory: Observation<PsiSnapshot>,
    pub psi_io: Observation<PsiSnapshot>,
    pub disks: Observation<Vec<DiskSnapshot>>,
    pub filesystem: Observation<FilesystemSnapshot>,
    pub tasks: Observation<TaskCounts>,
    pub target_processes: Observation<Vec<TargetProcessSummary>>,
    pub thermal: Observation<Vec<ThermalSnapshot>>,
    pub cpu_frequency_khz: Observation<u64>,
    pub firmware_power: Observation<PowerSnapshot>,
    pub watchdog: Observation<WatchdogSnapshot>,
    pub server_heartbeat: Observation<HeartbeatSnapshot>,
    pub hardware_watchdog_heartbeat: Observation<HeartbeatSnapshot>,
    pub recorder_health: RecorderHealth,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ThreadDetail {
    pub tid: u32,
    pub name: Observation<String>,
    pub state: Observation<String>,
    pub wait_channel: Observation<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_stack: Option<Observation<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<Observation<String>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProcessDetail {
    pub target: String,
    pub pid: u32,
    pub status: Observation<String>,
    pub io: Observation<String>,
    pub threads: Vec<ThreadDetail>,
    pub threads_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DetailSample {
    pub processes: Vec<ProcessDetail>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EscalationSample {
    pub reasons: Vec<String>,
    pub processes: Vec<ProcessDetail>,
    pub dmesg_tail: Observation<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecordEnvelope {
    pub schema_version: u32,
    pub sequence: u64,
    pub boot_id: String,
    pub wall_time: String,
    pub monotonic_ms: u64,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecorderManifest {
    pub schema_version: u32,
    pub boot_id: String,
    pub next_sequence: u64,
    pub active_segment: usize,
    pub segment_count: usize,
    pub segment_bytes_limit: u64,
    pub summary_interval_secs: u64,
    pub detail_interval_secs: u64,
    pub sync_interval_secs: u64,
    pub health: RecorderHealth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RingReadReport {
    pub records: Vec<RecordEnvelope>,
    pub valid_records: u64,
    pub torn_records: u64,
    pub corrupt_records: u64,
    pub bytes_read: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ArchiveResult {
    pub status: SourceStatus,
    pub archived_boot_id: Option<String>,
    pub valid_records: u64,
    pub torn_records: u64,
    pub corrupt_records: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PstoreEntry {
    pub name: String,
    pub status: SourceStatus,
    pub source_bytes: u64,
    pub captured_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EarlyBootSnapshot {
    pub schema_version: u32,
    pub captured_at: String,
    pub capture_order: Vec<String>,
    pub boot_id: Observation<String>,
    pub uptime_secs: Observation<f64>,
    pub inferred_boot_at: Observation<String>,
    pub prior_restart_intent: Observation<serde_json::Value>,
    pub pstore: Observation<Vec<PstoreEntry>>,
    pub watchdog: Observation<WatchdogSnapshot>,
    pub firmware_reset_power: Observation<PowerSnapshot>,
    pub archive: ArchiveResult,
    pub late_fallback_capture: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BootSynthesis {
    pub boot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    pub valid_records: u64,
    pub torn_records: u64,
    pub corrupt_records: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_sample_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sample_at: Option<String>,
    pub cadence_gap_count: u64,
    pub max_cadence_gap_ms: u64,
    pub escalation_triggers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_summary: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FlightRecorderSynthesis {
    pub schema_version: u32,
    pub generated_at: String,
    pub restart_classification: String,
    pub classification_basis: Vec<String>,
    pub current: BootSynthesis,
    pub previous: BootSynthesis,
    pub source_status_counts: BTreeMap<SourceStatus, u64>,
    pub safety_note: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_serialization_distinguishes_empty_zero_and_failures() {
        let empty = Observation::ok(String::new());
        let zero = Observation::ok(0_u64);
        let unavailable = Observation::<String>::error(SourceStatus::Unavailable, "missing");
        let truncated = Observation::truncated("partial".to_string(), "8 byte limit");

        assert_eq!(serde_json::to_value(empty).unwrap()["value"], "");
        assert_eq!(serde_json::to_value(zero).unwrap()["value"], 0);
        assert_eq!(
            serde_json::to_value(unavailable).unwrap()["status"],
            "unavailable"
        );
        assert_eq!(
            serde_json::to_value(truncated).unwrap()["status"],
            "truncated"
        );
    }
}
