//! Durable per-device Monster LAN credentials for the appliance hub.
//!
//! One JSON document under `<data_dir>/monster/devices.json`, written
//! atomically with owner-only permissions. Keys leave the store only into a
//! `LightLanClient`; `Debug` output is redacted. A corrupt document is
//! quarantined beside the store rather than silently discarded.
use crate::{types::validate_dsn, LightCredentials, LightResult, LightSecret};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::Write,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub const STORE_DIRECTORY: &str = "monster";
const STORE_FILE: &str = "devices.json";
const SCHEMA_VERSION: u32 = 1;
const MAX_STORED_DEVICES: usize = 64;
const MAX_REMOVED_DEVICES: usize = 64;
const MAX_STORE_BYTES: u64 = 512 * 1024;
const MAX_NAME_CHARS: usize = 64;

/// Stable Rhythm-side native id for a Monster strip.
pub fn native_id_for_dsn(dsn: &str) -> String {
    format!("monster-{}", dsn.to_ascii_lowercase())
}

pub fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Bound and trim a user-facing name; falls back to a DSN-suffixed label.
pub fn display_name_for(name: Option<&str>, dsn: &str) -> String {
    let trimmed = name.map(str::trim).unwrap_or_default();
    if trimmed.is_empty() {
        let suffix: String = dsn
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return format!("Monster Neon Flow {}", suffix.to_ascii_uppercase());
    }
    trimmed.chars().take(MAX_NAME_CHARS).collect()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LightDeviceRecord {
    pub dsn: String,
    pub ip: Ipv4Addr,
    pub local_key: LightSecret,
    pub local_key_id: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub paired_at_epoch_secs: u64,
}

impl LightDeviceRecord {
    pub fn native_id(&self) -> String {
        native_id_for_dsn(&self.dsn)
    }

    pub fn credentials(&self) -> LightCredentials {
        LightCredentials {
            dsn: self.dsn.clone(),
            ip: self.ip,
            local_key: self.local_key.clone(),
            local_key_id: self.local_key_id,
        }
    }

    pub fn validate(&self) -> LightResult<()> {
        validate_dsn(&self.dsn)?;
        self.credentials().validate()
    }
}

impl fmt::Debug for LightDeviceRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LightDeviceRecord")
            .field("native_id", &self.native_id())
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovedDevice {
    pub dsn: String,
    pub name: String,
    pub removed_at_epoch_secs: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Document {
    schema_version: u32,
    #[serde(default)]
    devices: Vec<LightDeviceRecord>,
    #[serde(default)]
    removed: Vec<RemovedDevice>,
}

pub struct LightDeviceStore {
    path: PathBuf,
    inner: Mutex<BTreeMap<String, LightDeviceRecord>>,
    removed: Mutex<Vec<RemovedDevice>>,
    /// True when the on-disk document was unreadable and set aside.
    quarantined: bool,
}

impl LightDeviceStore {
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = data_dir.as_ref().join(STORE_DIRECTORY);
        let path = dir.join(STORE_FILE);
        let mut quarantined = false;
        let document = match fs::metadata(&path) {
            Ok(metadata) => {
                if metadata.len() > MAX_STORE_BYTES {
                    quarantine(&path)?;
                    quarantined = true;
                    Document::default()
                } else {
                    let bytes =
                        fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
                    match serde_json::from_slice::<Document>(&bytes) {
                        Ok(document) if document.schema_version <= SCHEMA_VERSION => document,
                        _ => {
                            quarantine(&path)?;
                            quarantined = true;
                            Document::default()
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Document::default(),
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", path.display()))
            }
        };
        let mut devices = BTreeMap::new();
        for record in document.devices {
            if record.validate().is_ok() {
                devices.insert(record.dsn.clone(), record);
            }
        }
        Ok(Self {
            path,
            inner: Mutex::new(devices),
            removed: Mutex::new(document.removed),
            quarantined,
        })
    }

    pub fn was_quarantined(&self) -> bool {
        self.quarantined
    }

    pub fn all(&self) -> Vec<LightDeviceRecord> {
        self.inner
            .lock()
            .map(|devices| devices.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get(&self, dsn: &str) -> Option<LightDeviceRecord> {
        self.inner.lock().ok()?.get(dsn).cloned()
    }

    pub fn get_by_native_id(&self, native_id: &str) -> Option<LightDeviceRecord> {
        self.inner
            .lock()
            .ok()?
            .values()
            .find(|record| record.native_id() == native_id)
            .cloned()
    }

    pub fn removed(&self) -> Vec<RemovedDevice> {
        self.removed.lock().map(|r| r.clone()).unwrap_or_default()
    }

    pub fn upsert(&self, record: LightDeviceRecord) -> Result<()> {
        record
            .validate()
            .map_err(|error| anyhow::anyhow!("invalid Monster device record: {error}"))?;
        let mut devices = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("Monster store lock poisoned"))?;
        if !devices.contains_key(&record.dsn) && devices.len() >= MAX_STORED_DEVICES {
            anyhow::bail!("Monster device store is full");
        }
        let mut removed = self
            .removed
            .lock()
            .map_err(|_| anyhow::anyhow!("Monster store lock poisoned"))?;
        removed.retain(|entry| entry.dsn != record.dsn);
        devices.insert(record.dsn.clone(), record);
        self.persist(&devices, &removed)
    }

    pub fn remove(&self, dsn: &str) -> Result<Option<LightDeviceRecord>> {
        let mut devices = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("Monster store lock poisoned"))?;
        let removed_record = devices.remove(dsn);
        let mut removed = self
            .removed
            .lock()
            .map_err(|_| anyhow::anyhow!("Monster store lock poisoned"))?;
        if let Some(record) = removed_record.as_ref() {
            removed.retain(|entry| entry.dsn != record.dsn);
            removed.push(RemovedDevice {
                dsn: record.dsn.clone(),
                name: record.name.clone(),
                removed_at_epoch_secs: now_epoch_secs(),
            });
            while removed.len() > MAX_REMOVED_DEVICES {
                removed.remove(0);
            }
        }
        self.persist(&devices, &removed)?;
        Ok(removed_record)
    }

    fn persist(
        &self,
        devices: &BTreeMap<String, LightDeviceRecord>,
        removed: &[RemovedDevice],
    ) -> Result<()> {
        let document = Document {
            schema_version: SCHEMA_VERSION,
            devices: devices.values().cloned().collect(),
            removed: removed.to_vec(),
        };
        let bytes = serde_json::to_vec_pretty(&document)?;
        let dir = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Monster store has no parent directory"))?;
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("securing {}", dir.display()))?;
        }
        let temp = dir.join(format!(".{STORE_FILE}.tmp-{}", std::process::id()));
        {
            let mut options = fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temp)
                .with_context(|| format!("opening {}", temp.display()))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&temp, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))?;
        #[cfg(unix)]
        {
            if let Ok(handle) = fs::File::open(dir) {
                let _ = handle.sync_all();
            }
        }
        Ok(())
    }
}

fn quarantine(path: &Path) -> Result<()> {
    let quarantined = path.with_file_name(format!("{STORE_FILE}.corrupt.{}", now_epoch_secs()));
    fs::rename(path, &quarantined).with_context(|| format!("quarantining {}", path.display()))?;
    log::warn!(
        target: "sys",
        "Monster device store was unreadable and set aside; enrolled lights must be adopted again"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rhythm-monster-store-{label}-{}-{}",
            std::process::id(),
            now_epoch_secs()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn record(dsn: &str) -> LightDeviceRecord {
        LightDeviceRecord {
            dsn: dsn.to_string(),
            ip: "192.168.4.20".parse().unwrap(),
            local_key: LightSecret::new("synthetic-fixture-key".into()),
            local_key_id: 7,
            name: "Desk strip".into(),
            model: Some("xt-16ft-hw-neon-led-rgbic".into()),
            paired_at_epoch_secs: 1,
        }
    }

    #[test]
    fn round_trips_with_owner_only_permissions_and_redacted_debug() {
        let dir = temp_dir("roundtrip");
        let store = LightDeviceStore::load(&dir).unwrap();
        store.upsert(record("ACFIXTURE123456")).unwrap();
        let reloaded = LightDeviceStore::load(&dir).unwrap();
        let loaded = reloaded.get("ACFIXTURE123456").unwrap();
        assert_eq!(loaded.native_id(), "monster-acfixture123456");
        assert_eq!(loaded.local_key.expose(), "synthetic-fixture-key");
        assert!(!format!("{loaded:?}").contains("synthetic-fixture-key"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = dir.join(STORE_DIRECTORY).join(STORE_FILE);
            assert_eq!(
                fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(dir.join(STORE_DIRECTORY))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn remove_keeps_a_tombstone_and_upsert_clears_it() {
        let dir = temp_dir("remove");
        let store = LightDeviceStore::load(&dir).unwrap();
        store.upsert(record("ACFIXTURE123456")).unwrap();
        assert!(store.remove("ACFIXTURE123456").unwrap().is_some());
        assert!(store.get("ACFIXTURE123456").is_none());
        assert_eq!(store.removed().len(), 1);
        assert!(store.remove("ACFIXTURE123456").unwrap().is_none());
        store.upsert(record("ACFIXTURE123456")).unwrap();
        assert!(LightDeviceStore::load(&dir).unwrap().removed().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_document_is_quarantined_not_deleted() {
        let dir = temp_dir("corrupt");
        let store_dir = dir.join(STORE_DIRECTORY);
        fs::create_dir_all(&store_dir).unwrap();
        fs::write(store_dir.join(STORE_FILE), b"{bad").unwrap();
        let store = LightDeviceStore::load(&dir).unwrap();
        assert!(store.was_quarantined());
        assert!(store.all().is_empty());
        let quarantined = fs::read_dir(&store_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt."))
            .count();
        assert_eq!(quarantined, 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn display_name_falls_back_to_dsn_suffix() {
        assert_eq!(
            display_name_for(None, "ACFIXTURE123456"),
            "Monster Neon Flow 3456"
        );
        assert_eq!(display_name_for(Some("  Bar strip "), "AC1"), "Bar strip");
    }
}
