//! Persisted OTA history journal (`ota_history.json` in the data dir).
//!
//! Bundles previously carried no record of firmware transitions, so a
//! mid-incident self-update (issue #123: the appliance updated 90 seconds
//! before the reported failure) had to be reconstructed from log archaeology
//! and git tags. Each apply/verify/rollback appends one small entry here;
//! the debug bundle picks the file up via `OPTIONAL_PERSISTED_FILES`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const OTA_HISTORY_LIMIT: usize = 50;
pub const OTA_HISTORY_SCHEMA_VERSION: u32 = 1;
const OTA_HISTORY_FILE: &str = "ota_history.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OtaHistoryEntry {
    /// RFC3339 UTC timestamp.
    pub at: String,
    pub epoch_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<String>,
    /// "manual" (app-initiated apply), "auto" (auto-update), or "startup"
    /// (verification / rollback decided during boot).
    pub trigger: String,
    /// "applied", "verified", or "rolled_back".
    pub result: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OtaHistory {
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<OtaHistoryEntry>,
}

impl OtaHistory {
    fn normalized(mut self) -> Self {
        self.entries.sort_by_key(|entry| entry.epoch_ms);
        if self.entries.len() > OTA_HISTORY_LIMIT {
            let excess = self.entries.len() - OTA_HISTORY_LIMIT;
            self.entries.drain(..excess);
        }
        self
    }
}

pub fn entry(
    from_version: Option<&str>,
    to_version: Option<&str>,
    trigger: &str,
    result: &str,
) -> OtaHistoryEntry {
    let now = chrono::Utc::now();
    OtaHistoryEntry {
        at: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        epoch_ms: u64::try_from(now.timestamp_millis()).unwrap_or(0),
        from_version: from_version.map(str::to_string),
        to_version: to_version.map(str::to_string),
        trigger: trigger.to_string(),
        result: result.to_string(),
    }
}

fn history_path(data_dir: &Path) -> PathBuf {
    data_dir.join(OTA_HISTORY_FILE)
}

/// Append an entry (load-modify-save). OTA events are rare, and journal
/// failures must never fail the update itself — errors only log.
pub fn record(data_dir: &Path, entry: OtaHistoryEntry) {
    if data_dir.as_os_str().is_empty() {
        return;
    }
    let path = history_path(data_dir);
    let mut history = match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<OtaHistory>(&raw) {
            Ok(history) => history,
            Err(error) => {
                log::warn!(
                    target: "sys",
                    "OTA history {} is corrupt ({}); starting fresh",
                    path.display(),
                    error
                );
                OtaHistory {
                    schema_version: OTA_HISTORY_SCHEMA_VERSION,
                    entries: Vec::new(),
                }
            }
        },
        Err(_) => OtaHistory {
            schema_version: OTA_HISTORY_SCHEMA_VERSION,
            entries: Vec::new(),
        },
    };
    history.entries.push(entry);
    let history = history.normalized();

    let serialized = match serde_json::to_vec_pretty(&history) {
        Ok(serialized) => serialized,
        Err(error) => {
            log::warn!(target: "sys", "Failed to serialize OTA history: {error}");
            return;
        }
    };
    if let Err(error) = crate::bootstate::atomic_write_with_sync(&path, &serialized) {
        log::warn!(
            target: "sys",
            "Failed to write OTA history {}: {}",
            path.display(),
            error
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_appends_caps_and_survives_corruption() {
        let dir = std::env::temp_dir().join(format!(
            "rhythm-ota-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        record(&dir, entry(Some("1.0.0"), Some("1.0.1"), "manual", "applied"));
        record(&dir, entry(Some("1.0.0"), Some("1.0.1"), "startup", "verified"));
        let raw = std::fs::read_to_string(history_path(&dir)).unwrap();
        let history: OtaHistory = serde_json::from_str(&raw).unwrap();
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].result, "applied");
        assert_eq!(history.entries[1].result, "verified");
        assert_eq!(history.entries[1].trigger, "startup");

        std::fs::write(history_path(&dir), b"{corrupt").unwrap();
        record(&dir, entry(None, Some("1.0.2"), "auto", "applied"));
        let raw = std::fs::read_to_string(history_path(&dir)).unwrap();
        let history: OtaHistory = serde_json::from_str(&raw).unwrap();
        assert_eq!(history.entries.len(), 1, "corrupt journal restarts fresh");

        for i in 0..(OTA_HISTORY_LIMIT + 5) {
            record(&dir, entry(None, Some(&format!("v{i}")), "auto", "applied"));
        }
        let raw = std::fs::read_to_string(history_path(&dir)).unwrap();
        let history: OtaHistory = serde_json::from_str(&raw).unwrap();
        assert_eq!(history.entries.len(), OTA_HISTORY_LIMIT);

        let _ = std::fs::remove_dir_all(dir);
    }
}
