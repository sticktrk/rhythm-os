use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const STORE_SCHEMA_VERSION: u32 = 1;
const STORE_DIRECTORY: &str = "aidot_ble";
const STORE_FILE: &str = "devices.json";

fn store_schema_version() -> u32 {
    STORE_SCHEMA_VERSION
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AidotButtonDevice {
    pub id: String,
    pub ble_identity: String,
    pub serial_metadata: String,
    pub model_metadata: String,
    pub observed_address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_counter: Option<u8>,
    pub associated_at_epoch_secs: u64,
}

impl AidotButtonDevice {
    pub fn stable_id(identity: &str) -> String {
        format!("aidot-ble-{}", identity.to_ascii_lowercase())
    }

    pub fn button_id(&self) -> String {
        format!("{}-button-1", self.id)
    }

    pub fn display_name(&self) -> &'static str {
        "Orein/AiDot Button"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CounterDisposition {
    New,
    Duplicate,
    UnknownDevice,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoreDocument {
    #[serde(default = "store_schema_version")]
    schema_version: u32,
    #[serde(default)]
    devices: Vec<AidotButtonDevice>,
}

impl Default for StoreDocument {
    fn default() -> Self {
        Self {
            schema_version: STORE_SCHEMA_VERSION,
            devices: Vec::new(),
        }
    }
}

pub struct AidotButtonStore {
    path: PathBuf,
    document: Mutex<StoreDocument>,
}

impl AidotButtonStore {
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Self> {
        let path = data_dir.as_ref().join(STORE_DIRECTORY).join(STORE_FILE);
        let document = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<StoreDocument>(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreDocument::default(),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        if document.schema_version != STORE_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported AiDot button store schema {}",
                document.schema_version
            );
        }
        Ok(Self {
            path,
            document: Mutex::new(document),
        })
    }

    pub fn all(&self) -> Vec<AidotButtonDevice> {
        self.document
            .lock()
            .map(|document| document.devices.clone())
            .unwrap_or_default()
    }

    pub fn get_by_identity(&self, identity: &str) -> Option<AidotButtonDevice> {
        self.document
            .lock()
            .ok()?
            .devices
            .iter()
            .find_map(|device| {
                device
                    .ble_identity
                    .eq_ignore_ascii_case(identity)
                    .then(|| device.clone())
            })
    }

    pub fn get_by_id(&self, id: &str) -> Option<AidotButtonDevice> {
        self.document
            .lock()
            .ok()?
            .devices
            .iter()
            .find(|device| device.id == id)
            .cloned()
    }

    pub fn get_by_observed_address(&self, address: &str) -> Option<AidotButtonDevice> {
        self.document
            .lock()
            .ok()?
            .devices
            .iter()
            .find_map(|device| {
                device
                    .observed_address
                    .eq_ignore_ascii_case(address)
                    .then(|| device.clone())
            })
    }

    pub fn upsert(&self, mut device: AidotButtonDevice) -> Result<()> {
        let mut document = self
            .document
            .lock()
            .map_err(|_| anyhow::anyhow!("AiDot button store lock poisoned"))?;
        let previous = document.clone();
        if let Some(existing) = document.devices.iter_mut().find(|existing| {
            existing
                .ble_identity
                .eq_ignore_ascii_case(&device.ble_identity)
        }) {
            if device.last_counter.is_none() {
                device.last_counter = existing.last_counter;
            }
            *existing = device;
        } else {
            document.devices.push(device);
        }
        document
            .devices
            .sort_by(|left, right| left.ble_identity.cmp(&right.ble_identity));
        if let Err(error) = self.persist_locked(&document) {
            *document = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<Option<AidotButtonDevice>> {
        let mut document = self
            .document
            .lock()
            .map_err(|_| anyhow::anyhow!("AiDot button store lock poisoned"))?;
        let Some(index) = document.devices.iter().position(|device| device.id == id) else {
            return Ok(None);
        };
        let removed = document.devices.remove(index);
        if let Err(error) = self.persist_locked(&document) {
            document.devices.insert(index, removed);
            return Err(error);
        }
        Ok(Some(removed))
    }

    /// Commit a changed rolling counter before the corresponding hub event.
    pub fn accept_counter(&self, identity: &str, counter: u8) -> Result<CounterDisposition> {
        let mut document = self
            .document
            .lock()
            .map_err(|_| anyhow::anyhow!("AiDot button store lock poisoned"))?;
        let Some(device) = document
            .devices
            .iter_mut()
            .find(|device| device.ble_identity.eq_ignore_ascii_case(identity))
        else {
            return Ok(CounterDisposition::UnknownDevice);
        };
        if device.last_counter == Some(counter) {
            return Ok(CounterDisposition::Duplicate);
        }
        let previous_counter = device.last_counter;
        device.last_counter = Some(counter);
        if let Err(error) = self.persist_locked(&document) {
            if let Some(device) = document
                .devices
                .iter_mut()
                .find(|device| device.ble_identity.eq_ignore_ascii_case(identity))
            {
                device.last_counter = previous_counter;
            }
            return Err(error);
        }
        Ok(CounterDisposition::New)
    }

    fn persist_locked(&self, document: &StoreDocument) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("AiDot store path has no parent"))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let temp_path = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(document)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp_path)
            .with_context(|| format!("opening {}", temp_path.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("writing {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", temp_path.display()))?;
        fs::rename(&temp_path, &self.path).with_context(|| {
            format!(
                "renaming {} to {}",
                temp_path.display(),
                self.path.display()
            )
        })?;
        OpenOptions::new()
            .read(true)
            .open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("syncing {}", parent.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rhythm-aidot-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn device(counter: Option<u8>) -> AidotButtonDevice {
        AidotButtonDevice {
            id: AidotButtonDevice::stable_id("1CD6BD2273F9"),
            ble_identity: "1CD6BD2273F9".to_string(),
            serial_metadata: "L10599FAR002073".to_string(),
            model_metadata: "A001462".to_string(),
            observed_address: "1C:D6:BD:22:73:F9".to_string(),
            last_counter: counter,
            associated_at_epoch_secs: 1,
        }
    }

    #[test]
    fn counter_dedup_accepts_gaps_and_wraparound() {
        let root = temporary_dir("counter");
        let store = AidotButtonStore::load(&root).unwrap();
        store.upsert(device(Some(0x85))).unwrap();

        assert_eq!(
            store.accept_counter("1CD6BD2273F9", 0x85).unwrap(),
            CounterDisposition::Duplicate
        );
        assert_eq!(
            store.accept_counter("1CD6BD2273F9", 0x90).unwrap(),
            CounterDisposition::New
        );
        assert_eq!(
            store.accept_counter("1CD6BD2273F9", 0x00).unwrap(),
            CounterDisposition::New
        );
        assert_eq!(
            store.accept_counter("1CD6BD2273F9", 0x00).unwrap(),
            CounterDisposition::Duplicate
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_restores_identity_metadata_and_last_counter() {
        let root = temporary_dir("restart");
        AidotButtonStore::load(&root)
            .unwrap()
            .upsert(device(Some(0xff)))
            .unwrap();

        let restored = AidotButtonStore::load(&root)
            .unwrap()
            .get_by_identity("1cd6bd2273f9")
            .unwrap();
        assert_eq!(restored.last_counter, Some(0xff));
        assert_eq!(restored.serial_metadata, "L10599FAR002073");
        assert_eq!(restored.model_metadata, "A001462");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_counter_persistence_rolls_back_in_memory_dedup_state() {
        let root = temporary_dir("counter-rollback");
        let mut store = AidotButtonStore::load(&root).unwrap();
        store.upsert(device(Some(0x10))).unwrap();
        store.path = root.join("blocked-target");
        fs::create_dir_all(&store.path).unwrap();

        assert!(store.accept_counter("1CD6BD2273F9", 0x20).is_err());
        assert_eq!(
            store.get_by_identity("1CD6BD2273F9").unwrap().last_counter,
            Some(0x10)
        );
        fs::remove_dir_all(root).unwrap();
    }
}
