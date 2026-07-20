use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;

use crate::model::{
    ArchiveResult, RecordEnvelope, RecorderHealth, RecorderManifest, RingReadReport, SourceStatus,
    SCHEMA_VERSION,
};

pub const ROOT_RELATIVE_PATH: &str = "boot-diagnostics/host-flight-recorder";
pub const CURRENT_DIR: &str = "current";
pub const PREVIOUS_DIR: &str = "previous";
pub const EARLY_BOOT_FILE: &str = "early-boot.json";
pub const PREVIOUS_EARLY_BOOT_FILE: &str = "previous-early-boot.json";
pub const PSTORE_CURRENT_DIR: &str = "pstore-current";
pub const PSTORE_PREVIOUS_DIR: &str = "pstore-previous";

pub const DEFAULT_SEGMENT_COUNT: usize = 8;
pub const DEFAULT_SEGMENT_BYTES_LIMIT: u64 = 248 * 1024;
pub const DEFAULT_TOTAL_RING_BYTES_LIMIT: u64 = 4 * 1024 * 1024;
pub const MAX_RECORD_BYTES: usize = 64 * 1024;
pub const SUMMARY_RECORD_BYTES_LIMIT: usize = 8 * 1024;
pub const DETAIL_RECORD_BYTES_LIMIT: usize = 16 * 1024;
pub const ESCALATION_RECORD_BYTES_LIMIT: usize = MAX_RECORD_BYTES;

#[derive(Clone, Debug)]
pub struct RingConfig {
    pub segment_count: usize,
    pub segment_bytes_limit: u64,
    pub summary_interval_secs: u64,
    pub detail_interval_secs: u64,
    pub sync_interval_secs: u64,
}

impl Default for RingConfig {
    fn default() -> Self {
        Self {
            segment_count: DEFAULT_SEGMENT_COUNT,
            segment_bytes_limit: DEFAULT_SEGMENT_BYTES_LIMIT,
            summary_interval_secs: 10,
            detail_interval_secs: 60,
            sync_interval_secs: 60,
        }
    }
}

pub fn recorder_root(data_dir: &Path) -> PathBuf {
    data_dir.join(ROOT_RELATIVE_PATH)
}

pub fn current_ring_boot_id(data_dir: &Path) -> Option<String> {
    let current = recorder_root(data_dir).join(CURRENT_DIR);
    read_ring(&current)
        .ok()?
        .records
        .first()
        .map(|record| record.boot_id.clone())
        .or_else(|| {
            read_json::<RecorderManifest>(&current.join("manifest.json"))
                .map(|manifest| manifest.boot_id)
        })
}

pub struct RingWriter {
    dir: PathBuf,
    manifest: RecorderManifest,
}

impl RingWriter {
    pub fn open(data_dir: &Path, boot_id: &str, config: RingConfig) -> io::Result<Self> {
        if config.segment_count == 0 || config.segment_bytes_limit < MAX_RECORD_BYTES as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid ring segment configuration",
            ));
        }
        let dir = recorder_root(data_dir).join(CURRENT_DIR);
        fs::create_dir_all(&dir)?;
        let report = read_ring(&dir)?;
        let existing = read_json::<RecorderManifest>(&dir.join("manifest.json"));
        let next_sequence = report
            .records
            .last()
            .map(|record| record.sequence.saturating_add(1))
            .unwrap_or(0);
        let mut health = existing
            .as_ref()
            .filter(|manifest| manifest.boot_id == boot_id)
            .map(|manifest| manifest.health.clone())
            .unwrap_or_default();
        health.recovered_segments = health
            .recovered_segments
            .saturating_add(u64::from(report.valid_records > 0));
        health.torn_records = health.torn_records.saturating_add(report.torn_records);
        health.corrupt_records = health
            .corrupt_records
            .saturating_add(report.corrupt_records);
        let active_segment = existing
            .filter(|manifest| manifest.boot_id == boot_id)
            .map(|manifest| manifest.active_segment % config.segment_count)
            .unwrap_or(0);
        let manifest = RecorderManifest {
            schema_version: SCHEMA_VERSION,
            boot_id: boot_id.to_string(),
            next_sequence,
            active_segment,
            segment_count: config.segment_count,
            segment_bytes_limit: config.segment_bytes_limit,
            summary_interval_secs: config.summary_interval_secs,
            detail_interval_secs: config.detail_interval_secs,
            sync_interval_secs: config.sync_interval_secs,
            health,
            last_sync_at: None,
        };
        let mut writer = Self { dir, manifest };
        writer.write_manifest()?;
        Ok(writer)
    }

    pub fn health(&self) -> &RecorderHealth {
        &self.manifest.health
    }

    pub fn health_mut(&mut self) -> &mut RecorderHealth {
        &mut self.manifest.health
    }

    pub fn next_sequence(&self) -> u64 {
        self.manifest.next_sequence
    }

    pub fn append<T: Serialize>(
        &mut self,
        boot_id: &str,
        monotonic_ms: u64,
        kind: &str,
        payload: &T,
        sync_now: bool,
    ) -> io::Result<Option<u64>> {
        let payload = match serde_json::to_value(payload) {
            Ok(payload) => payload,
            Err(error) => {
                self.record_failure(format!("serialize {kind}: {error}"));
                return Ok(None);
            }
        };
        let sequence = self.manifest.next_sequence;
        let record = RecordEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence,
            boot_id: boot_id.to_string(),
            wall_time: Utc::now().to_rfc3339(),
            monotonic_ms,
            kind: kind.to_string(),
            payload,
        };
        let mut bytes = match serde_json::to_vec(&record) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.record_failure(format!("encode {kind}: {error}"));
                return Ok(None);
            }
        };
        let record_limit = record_bytes_limit(kind);
        if bytes.len().saturating_add(1) > record_limit {
            self.manifest.health.dropped_samples =
                self.manifest.health.dropped_samples.saturating_add(1);
            self.manifest.health.truncated_sources =
                self.manifest.health.truncated_sources.saturating_add(1);
            self.manifest.health.last_error =
                Some(format!("record {kind} exceeded {record_limit} byte limit"));
            self.write_manifest()?;
            return Ok(None);
        }
        bytes.push(b'\n');

        let mut path = self.active_segment_path();
        let current_len = fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if current_len.saturating_add(bytes.len() as u64) > self.manifest.segment_bytes_limit {
            self.rotate()?;
            path = self.active_segment_path();
        }

        let append_result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
            file.write_all(&bytes)?;
            Ok(())
        })();
        if let Err(error) = append_result {
            self.record_failure(format!("append {}: {error}", path.display()));
            let _ = self.write_manifest();
            return Err(error);
        }

        self.manifest.next_sequence = self.manifest.next_sequence.saturating_add(1);
        if sync_now {
            self.sync()?;
        }
        Ok(Some(sequence))
    }

    pub fn sync(&mut self) -> io::Result<()> {
        for index in 0..self.manifest.segment_count {
            let path = self.dir.join(segment_name(index));
            if !path.is_file() {
                continue;
            }
            if let Err(error) = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .and_then(|file| file.sync_data())
            {
                self.manifest.health.sync_failures =
                    self.manifest.health.sync_failures.saturating_add(1);
                self.manifest.health.last_error = Some(format!("sync {}: {error}", path.display()));
                let _ = self.write_manifest();
                return Err(error);
            }
        }
        self.manifest.last_sync_at = Some(Utc::now().to_rfc3339());
        self.write_manifest()
    }

    fn rotate(&mut self) -> io::Result<()> {
        let old_active = self.active_segment_path();
        if old_active.is_file() {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&old_active)?
                .sync_data()?;
        }
        self.manifest.active_segment =
            (self.manifest.active_segment + 1) % self.manifest.segment_count;
        let path = self.active_segment_path();
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        file.sync_data()?;
        self.manifest.health.rotations = self.manifest.health.rotations.saturating_add(1);
        self.write_manifest()
    }

    fn active_segment_path(&self) -> PathBuf {
        self.dir.join(segment_name(self.manifest.active_segment))
    }

    fn write_manifest(&mut self) -> io::Result<()> {
        atomic_write_json(&self.dir.join("manifest.json"), &self.manifest)
    }

    fn record_failure(&mut self, detail: String) {
        self.manifest.health.write_failures = self.manifest.health.write_failures.saturating_add(1);
        self.manifest.health.dropped_samples =
            self.manifest.health.dropped_samples.saturating_add(1);
        self.manifest.health.last_error = Some(detail);
    }
}

fn record_bytes_limit(kind: &str) -> usize {
    match kind {
        "summary" => SUMMARY_RECORD_BYTES_LIMIT,
        "detail" => DETAIL_RECORD_BYTES_LIMIT,
        "escalation" => ESCALATION_RECORD_BYTES_LIMIT,
        _ => MAX_RECORD_BYTES,
    }
}

pub fn segment_name(index: usize) -> String {
    format!("segment-{index:03}.ndjson")
}

pub fn archive_current_boot(data_dir: &Path) -> io::Result<ArchiveResult> {
    archive_current_boot_for_new_boot(data_dir, None)
}

pub fn archive_current_boot_for_boot(
    data_dir: &Path,
    current_boot_id: &str,
) -> io::Result<ArchiveResult> {
    archive_current_boot_for_new_boot(data_dir, Some(current_boot_id))
}

fn archive_current_boot_for_new_boot(
    data_dir: &Path,
    current_boot_id: Option<&str>,
) -> io::Result<ArchiveResult> {
    let root = recorder_root(data_dir);
    fs::create_dir_all(&root)?;
    let current = root.join(CURRENT_DIR);
    let previous = root.join(PREVIOUS_DIR);
    let pending = root.join("previous.next");

    if pending.is_dir() {
        if previous.is_dir() {
            fs::remove_dir_all(&previous)?;
        }
        fs::rename(&pending, &previous)?;
        sync_directory(&root)?;
    }

    if !current.is_dir() {
        return Ok(ArchiveResult {
            status: SourceStatus::Unavailable,
            archived_boot_id: None,
            valid_records: 0,
            torn_records: 0,
            corrupt_records: 0,
            detail: Some("no current ring existed".to_string()),
        });
    }

    let report = read_ring(&current)?;
    let archived_boot_id = report
        .records
        .first()
        .map(|record| record.boot_id.clone())
        .or_else(|| {
            read_json::<RecorderManifest>(&current.join("manifest.json"))
                .map(|manifest| manifest.boot_id)
        });
    if current_boot_id.is_some_and(|boot_id| archived_boot_id.as_deref() == Some(boot_id)) {
        let previous_report = read_ring(&previous)?;
        let previous_boot_id = previous_report
            .records
            .first()
            .map(|record| record.boot_id.clone())
            .or_else(|| {
                read_json::<RecorderManifest>(&previous.join("manifest.json"))
                    .map(|manifest| manifest.boot_id)
            });
        return Ok(ArchiveResult {
            status: SourceStatus::Ok,
            archived_boot_id: previous_boot_id,
            valid_records: previous_report.valid_records,
            torn_records: previous_report.torn_records,
            corrupt_records: previous_report.corrupt_records,
            detail: Some(
                "current ring already belongs to this boot; preserved previous-boot evidence"
                    .to_string(),
            ),
        });
    }
    fs::rename(&current, &pending)?;
    sync_directory(&root)?;
    if previous.is_dir() {
        fs::remove_dir_all(&previous)?;
    }
    fs::rename(&pending, &previous)?;
    sync_directory(&root)?;

    Ok(ArchiveResult {
        status: SourceStatus::Ok,
        archived_boot_id,
        valid_records: report.valid_records,
        torn_records: report.torn_records,
        corrupt_records: report.corrupt_records,
        detail: None,
    })
}

pub fn read_ring(dir: &Path) -> io::Result<RingReadReport> {
    let mut report = RingReadReport::default();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(error) => return Err(error),
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("segment-") && name.ends_with(".ndjson"))
                && path.is_file()
        })
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        let mut bytes = Vec::new();
        File::open(&path)?
            .take(DEFAULT_SEGMENT_BYTES_LIMIT.saturating_add(1))
            .read_to_end(&mut bytes)?;
        report.bytes_read = report.bytes_read.saturating_add(bytes.len() as u64);
        let ended_with_newline = bytes.ends_with(b"\n");
        let mut lines = bytes.split(|byte| *byte == b'\n').peekable();
        while let Some(line) = lines.next() {
            if line.is_empty() {
                continue;
            }
            let is_last = lines.peek().is_none();
            if is_last && !ended_with_newline {
                report.torn_records = report.torn_records.saturating_add(1);
                continue;
            }
            if line.len() > MAX_RECORD_BYTES {
                report.corrupt_records = report.corrupt_records.saturating_add(1);
                continue;
            }
            match serde_json::from_slice::<RecordEnvelope>(line) {
                Ok(record) if record.schema_version == SCHEMA_VERSION => {
                    report.records.push(record);
                    report.valid_records = report.valid_records.saturating_add(1);
                }
                Ok(_) | Err(_) => report.corrupt_records = report.corrupt_records.saturating_add(1),
            }
        }
    }
    report.records.sort_by_key(|record| record.sequence);
    report.records.dedup_by_key(|record| record.sequence);
    report.valid_records = report.records.len() as u64;
    Ok(report)
}

pub fn ring_data_bytes(data_dir: &Path) -> io::Result<u64> {
    let root = recorder_root(data_dir);
    let mut total = 0_u64;
    for boot in [CURRENT_DIR, PREVIOUS_DIR] {
        let dir = root.join(boot);
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                total = total.saturating_add(fs::metadata(path)?.len());
            }
        }
    }
    Ok(total)
}

pub fn atomic_write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "atomic path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("recorder"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_data()?;
    fs::rename(&temp, path)?;
    sync_directory(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_RECORD_BYTES as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rhythm-host-recorder-ring-{name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn rotation_stays_bounded_and_retains_recent_records() {
        let dir = temp_dir("rotate");
        let config = RingConfig {
            segment_count: 3,
            segment_bytes_limit: MAX_RECORD_BYTES as u64,
            ..RingConfig::default()
        };
        let mut writer = RingWriter::open(&dir, "boot-a", config.clone()).unwrap();
        let payload = json!({"padding": "x".repeat(20_000)});
        for index in 0..20 {
            writer
                .append("boot-a", index, "escalation", &payload, false)
                .unwrap();
        }
        writer.sync().unwrap();

        let report = read_ring(&recorder_root(&dir).join(CURRENT_DIR)).unwrap();
        assert!(report.valid_records > 0);
        assert!(report.valid_records < 20);
        assert!(ring_data_bytes(&dir).unwrap() <= 3 * config.segment_bytes_limit + 64 * 1024);
        assert_eq!(report.records.last().unwrap().sequence, 19);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn torn_final_record_keeps_earlier_records_readable() {
        let dir = temp_dir("torn");
        let ring_dir = recorder_root(&dir).join(CURRENT_DIR);
        fs::create_dir_all(&ring_dir).unwrap();
        let valid = serde_json::to_string(&RecordEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: 7,
            boot_id: "boot-a".to_string(),
            wall_time: Utc::now().to_rfc3339(),
            monotonic_ms: 1,
            kind: "summary".to_string(),
            payload: json!({}),
        })
        .unwrap();
        fs::write(
            ring_dir.join(segment_name(0)),
            format!("{valid}\n{{\"schema_version\":1"),
        )
        .unwrap();

        let report = read_ring(&ring_dir).unwrap();
        assert_eq!(report.valid_records, 1);
        assert_eq!(report.torn_records, 1);
        assert_eq!(report.records[0].sequence, 7);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn boot_archive_separates_current_and_previous() {
        let dir = temp_dir("archive");
        let mut writer = RingWriter::open(&dir, "boot-a", RingConfig::default()).unwrap();
        writer
            .append("boot-a", 1, "summary", &json!({"ok": true}), true)
            .unwrap();

        let archived = archive_current_boot(&dir).unwrap();
        assert_eq!(archived.status, SourceStatus::Ok);
        assert_eq!(archived.archived_boot_id.as_deref(), Some("boot-a"));
        assert!(recorder_root(&dir).join(PREVIOUS_DIR).is_dir());
        assert!(!recorder_root(&dir).join(CURRENT_DIR).exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn repeated_capture_for_same_boot_preserves_previous_boot_ring() {
        let dir = temp_dir("same-boot-capture");
        let mut previous = RingWriter::open(&dir, "boot-a", RingConfig::default()).unwrap();
        previous
            .append("boot-a", 1, "summary", &json!({"boot": "a"}), true)
            .unwrap();
        archive_current_boot_for_boot(&dir, "boot-b").unwrap();

        let mut current = RingWriter::open(&dir, "boot-b", RingConfig::default()).unwrap();
        current
            .append("boot-b", 2, "summary", &json!({"boot": "b"}), true)
            .unwrap();

        let repeated = archive_current_boot_for_boot(&dir, "boot-b").unwrap();
        assert_eq!(repeated.archived_boot_id.as_deref(), Some("boot-a"));
        assert!(repeated
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("preserved previous-boot")));
        let previous_report = read_ring(&recorder_root(&dir).join(PREVIOUS_DIR)).unwrap();
        let current_report = read_ring(&recorder_root(&dir).join(CURRENT_DIR)).unwrap();
        assert_eq!(previous_report.records[0].boot_id, "boot-a");
        assert_eq!(current_report.records[0].boot_id, "boot-b");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn default_ring_retains_thirty_minutes_at_schema_size_envelopes() {
        let dir = temp_dir("retention");
        let mut writer = RingWriter::open(&dir, "boot-a", RingConfig::default()).unwrap();
        let summary = json!({"sample": "s".repeat(7 * 1024)});
        let detail = json!({"processes": "d".repeat(14 * 1024)});
        let mut monotonic_ms = 0_u64;
        for minute in 0..30 {
            for _ in 0..6 {
                writer
                    .append("boot-a", monotonic_ms, "summary", &summary, false)
                    .unwrap();
                monotonic_ms = monotonic_ms.saturating_add(10_000);
            }
            writer
                .append("boot-a", minute * 60_000, "detail", &detail, false)
                .unwrap();
        }
        writer.sync().unwrap();

        let report = read_ring(&recorder_root(&dir).join(CURRENT_DIR)).unwrap();
        assert_eq!(report.valid_records, 210);
        assert_eq!(report.records.first().unwrap().sequence, 0);
        assert!(ring_data_bytes(&dir).unwrap() <= DEFAULT_TOTAL_RING_BYTES_LIMIT);
        fs::remove_dir_all(dir).unwrap();
    }
}
