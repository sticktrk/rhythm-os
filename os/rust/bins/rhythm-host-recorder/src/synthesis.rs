use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use chrono::Utc;
use serde_json::Value;

use crate::model::{
    BootSynthesis, EarlyBootSnapshot, FlightRecorderSynthesis, RingReadReport, SourceStatus,
    SCHEMA_VERSION,
};
use crate::ring::{
    read_json, read_ring, recorder_root, CURRENT_DIR, EARLY_BOOT_FILE, PREVIOUS_DIR,
};

const EXPECTED_SUMMARY_INTERVAL_MS: u64 = 10_000;
const CADENCE_GAP_TOLERANCE_MS: u64 = 2_000;

pub fn build_synthesis(data_dir: &Path) -> io::Result<String> {
    let root = recorder_root(data_dir);
    let current_report = read_ring(&root.join(CURRENT_DIR))?;
    let previous_report = read_ring(&root.join(PREVIOUS_DIR))?;
    let early_boot = read_json::<EarlyBootSnapshot>(&root.join(EARLY_BOOT_FILE));
    let (restart_classification, classification_basis) = classify_restart(early_boot.as_ref());
    let current = summarize_boot("current", &current_report);
    let previous = summarize_boot("previous", &previous_report);
    let mut source_status_counts = BTreeMap::new();
    for record in current_report
        .records
        .iter()
        .chain(previous_report.records.iter())
    {
        count_statuses(&record.payload, &mut source_status_counts);
    }
    if let Some(early_boot) = early_boot {
        if let Ok(value) = serde_json::to_value(early_boot) {
            count_statuses(&value, &mut source_status_counts);
        }
    }
    let synthesis = FlightRecorderSynthesis {
        schema_version: SCHEMA_VERSION,
        generated_at: Utc::now().to_rfc3339(),
        restart_classification,
        classification_basis,
        current,
        previous,
        source_status_counts,
        safety_note: "On-device evidence can show observed undervoltage/throttling before a failure, but missing durable evidence cannot prove instantaneous external power removal; external power telemetry remains authoritative.".to_string(),
    };
    serde_json::to_string_pretty(&synthesis)
        .map(|mut body| {
            body.push('\n');
            body
        })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn summarize_boot(name: &str, report: &RingReadReport) -> BootSynthesis {
    let mut summary_times = report
        .records
        .iter()
        .filter(|record| record.kind == "summary")
        .map(|record| record.monotonic_ms)
        .collect::<Vec<_>>();
    summary_times.sort_unstable();
    let mut cadence_gap_count = 0_u64;
    let mut max_cadence_gap_ms = 0_u64;
    for pair in summary_times.windows(2) {
        let gap = pair[1].saturating_sub(pair[0]);
        max_cadence_gap_ms = max_cadence_gap_ms.max(gap);
        if gap > EXPECTED_SUMMARY_INTERVAL_MS.saturating_add(CADENCE_GAP_TOLERANCE_MS) {
            cadence_gap_count = cadence_gap_count.saturating_add(1);
        }
    }
    let mut escalation_triggers = report
        .records
        .iter()
        .filter(|record| record.kind == "escalation")
        .flat_map(|record| {
            record
                .payload
                .get("reasons")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    escalation_triggers.sort();
    escalation_triggers.dedup();
    let final_summary = report
        .records
        .iter()
        .rev()
        .find(|record| record.kind == "summary")
        .map(|record| record.payload.clone());
    BootSynthesis {
        boot: name.to_string(),
        boot_id: report.records.first().map(|record| record.boot_id.clone()),
        valid_records: report.valid_records,
        torn_records: report.torn_records,
        corrupt_records: report.corrupt_records,
        first_sample_at: report
            .records
            .first()
            .map(|record| record.wall_time.clone()),
        last_sample_at: report.records.last().map(|record| record.wall_time.clone()),
        cadence_gap_count,
        max_cadence_gap_ms,
        escalation_triggers,
        final_summary,
    }
}

fn classify_restart(early_boot: Option<&EarlyBootSnapshot>) -> (String, Vec<String>) {
    let Some(early_boot) = early_boot else {
        return (
            "unknown_no_early_boot_snapshot".to_string(),
            vec!["early boot snapshot unavailable".to_string()],
        );
    };
    if let Some(intent) = early_boot.prior_restart_intent.value.as_ref() {
        let category = intent
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("planned_restart");
        let reason = intent
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("unspecified");
        return (
            category.to_string(),
            vec![format!("persisted restart intent: {reason}")],
        );
    }
    let pstore_bytes = early_boot
        .pstore
        .value
        .as_ref()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.captured_bytes)
                .sum::<u64>()
        })
        .unwrap_or(0);
    if pstore_bytes > 0 {
        return (
            "kernel_crash_evidence".to_string(),
            vec![format!("captured {pstore_bytes} pstore bytes")],
        );
    }
    let bootstatus = early_boot
        .watchdog
        .value
        .as_ref()
        .and_then(|watchdog| watchdog.values.get("bootstatus"))
        .and_then(|value| value.value.as_deref())
        .and_then(parse_nonzero_integer);
    if bootstatus.is_some_and(|value| value != 0) {
        return (
            "hardware_watchdog_reset".to_string(),
            vec![format!(
                "watchdog bootstatus={}",
                bootstatus.unwrap_or_default()
            )],
        );
    }
    (
        "unplanned_restart_unknown".to_string(),
        vec!["no planned intent, pstore record, or non-zero watchdog bootstatus".to_string()],
    )
}

fn parse_nonzero_integer(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}

fn count_statuses(value: &Value, counts: &mut BTreeMap<SourceStatus, u64>) {
    match value {
        Value::Object(map) => {
            if let Some(status) = map
                .get("status")
                .and_then(Value::as_str)
                .and_then(parse_status)
            {
                *counts.entry(status).or_default() += 1;
            }
            for child in map.values() {
                count_statuses(child, counts);
            }
        }
        Value::Array(values) => {
            for child in values {
                count_statuses(child, counts);
            }
        }
        _ => {}
    }
}

fn parse_status(value: &str) -> Option<SourceStatus> {
    match value {
        "ok" => Some(SourceStatus::Ok),
        "unsupported" => Some(SourceStatus::Unsupported),
        "unavailable" => Some(SourceStatus::Unavailable),
        "permission_denied" => Some(SourceStatus::PermissionDenied),
        "timed_out" => Some(SourceStatus::TimedOut),
        "truncated" => Some(SourceStatus::Truncated),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArchiveResult, Observation, PowerSnapshot, PstoreEntry, WatchdogSnapshot};
    use crate::ring::{atomic_write_json, RingConfig, RingWriter};
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rhythm-host-recorder-synthesis-{name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn synthesis_reports_gaps_torn_records_triggers_and_unknown_power_loss() {
        let dir = temp_dir("summary");
        let mut writer = RingWriter::open(&dir, "boot-a", RingConfig::default()).unwrap();
        writer
            .append(
                "boot-a",
                1_000,
                "summary",
                &serde_json::json!({"watchdog":{"status":"unsupported"}}),
                false,
            )
            .unwrap();
        writer
            .append(
                "boot-a",
                30_000,
                "escalation",
                &serde_json::json!({"reasons":["io_psi_full_avg10"]}),
                true,
            )
            .unwrap();
        let ring = recorder_root(&dir).join(CURRENT_DIR);
        fs::OpenOptions::new()
            .append(true)
            .open(ring.join(crate::ring::segment_name(0)))
            .unwrap()
            .write_all(b"{\"torn\":")
            .unwrap();
        let early = EarlyBootSnapshot {
            schema_version: SCHEMA_VERSION,
            captured_at: Utc::now().to_rfc3339(),
            capture_order: Vec::new(),
            boot_id: Observation::ok("boot-a".to_string()),
            uptime_secs: Observation::ok(30.0),
            inferred_boot_at: Observation::ok(Utc::now().to_rfc3339()),
            prior_restart_intent: Observation::unsupported("missing"),
            pstore: Observation::ok(Vec::<PstoreEntry>::new()),
            watchdog: Observation::ok(WatchdogSnapshot {
                values: BTreeMap::new(),
            }),
            firmware_reset_power: Observation::ok(PowerSnapshot {
                values: BTreeMap::new(),
            }),
            archive: ArchiveResult {
                status: SourceStatus::Unavailable,
                archived_boot_id: None,
                valid_records: 0,
                torn_records: 0,
                corrupt_records: 0,
                detail: None,
            },
            late_fallback_capture: false,
        };
        atomic_write_json(&recorder_root(&dir).join(EARLY_BOOT_FILE), &early).unwrap();

        let synthesis: Value = serde_json::from_str(&build_synthesis(&dir).unwrap()).unwrap();
        assert_eq!(
            synthesis["restart_classification"],
            "unplanned_restart_unknown"
        );
        assert_eq!(synthesis["current"]["torn_records"], 1);
        assert_eq!(
            synthesis["current"]["escalation_triggers"][0],
            "io_psi_full_avg10"
        );
        assert!(synthesis["safety_note"]
            .as_str()
            .unwrap()
            .contains("cannot prove"));
        fs::remove_dir_all(dir).unwrap();
    }

    use std::io::Write;
}
