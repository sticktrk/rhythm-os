//! Durable state shared by simple local-BLE profiles.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use anyhow::{Context, Result};
use log::warn;
use rand::{rngs::OsRng, RngCore};
use rhythm_os::hub::is_valid_device_profile_id;
use rhythm_os::registry::HubDeviceRegistry;
use serde::{Deserialize, Serialize};

use crate::profile::{
    canonical_profile_id, profile_by_id, BleReplayToken, ReplayOrder, ValidatedBleSetup,
};

const STORE_SCHEMA_VERSION: u32 = 1;
pub const STORE_DIRECTORY: &str = "local_ble";
const STORE_FILE: &str = "devices.json";
static SHARED_STORES: OnceLock<Mutex<HashMap<PathBuf, Weak<LocalBleDeviceStore>>>> =
    OnceLock::new();

fn store_schema_version() -> u32 {
    STORE_SCHEMA_VERSION
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBleDevice {
    pub id: String,
    pub profile_id: String,
    /// Profile-stable physical identity. This is private appliance state and
    /// must never be copied into logs, diagnostics, pairing history, or IDs.
    pub stable_identity: String,
    /// Bounded opaque fields validated by the originating profile.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Replaceable BlueZ routing hint, never public identity.
    pub transport_hint: String,
    /// Opaque, profile-versioned replay evidence, keyed by logical event
    /// stream. The host bounds and persists these bytes but never interprets
    /// their protocol shape.
    #[serde(default)]
    pub replay_state: BTreeMap<String, Vec<u8>>,
    /// Legacy/staging compatibility flag. New associations commit here only
    /// after canonical projection succeeds; startup removes a projection that
    /// has no corresponding active record.
    #[serde(default)]
    pub blocked: bool,
    pub associated_at_epoch_secs: u64,
}

impl LocalBleDevice {
    /// Generate an appliance-local opaque ID. It is persisted and reused on
    /// re-association, but cannot be enumerated from a vendor/OUI identity or
    /// correlated across appliances and factory resets.
    pub fn new_public_id() -> String {
        let mut random = [0_u8; 16];
        OsRng.fill_bytes(&mut random);
        let token = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("local-ble-{token}")
    }

    pub fn from_setup(
        profile_id: &str,
        setup: &ValidatedBleSetup,
        transport_hint: String,
        replay_state: BTreeMap<String, Vec<u8>>,
        blocked: bool,
        associated_at_epoch_secs: u64,
    ) -> Self {
        Self {
            id: Self::new_public_id(),
            profile_id: canonical_profile_id(profile_id)
                .unwrap_or(profile_id)
                .to_string(),
            stable_identity: setup.stable_identity.clone(),
            metadata: setup.metadata.clone(),
            transport_hint,
            replay_state,
            blocked,
            associated_at_epoch_secs,
        }
    }

    pub fn button_id(&self, endpoint_suffix: &str) -> Option<String> {
        profile_by_id(&self.profile_id)?
            .projection()
            .button_endpoints
            .iter()
            .any(|endpoint| endpoint.suffix == endpoint_suffix)
            .then(|| format!("{}-{endpoint_suffix}", self.id))
    }

    pub fn display_name(&self) -> Option<&'static str> {
        profile_by_id(&self.profile_id).map(|profile| profile.projection().display_name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayDisposition {
    New,
    Duplicate,
    Stale,
    Blocked,
    UnknownDevice,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoreDocument {
    #[serde(default = "store_schema_version")]
    schema_version: u32,
    #[serde(default)]
    devices: Vec<LocalBleDevice>,
}

impl Default for StoreDocument {
    fn default() -> Self {
        Self {
            schema_version: STORE_SCHEMA_VERSION,
            devices: Vec::new(),
        }
    }
}

pub struct LocalBleDeviceStore {
    path: PathBuf,
    document: Mutex<StoreDocument>,
    registries: Mutex<Vec<Weak<Mutex<HubDeviceRegistry>>>>,
    quiescing: AtomicBool,
    durability_degraded: AtomicBool,
    #[cfg(test)]
    persist_fault: Mutex<Option<PersistFault>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PersistFault {
    Write,
    Rename,
    DirectorySync,
}

impl LocalBleDeviceStore {
    /// Return the one live store instance for this process and data path.
    ///
    /// Hub bootstrap and adapter-independent unpairing can overlap before the
    /// hub is published in `AppState`. Sharing the document prevents the
    /// offline path from committing a removal through one snapshot while a
    /// bootstrap path later writes an older snapshot back to disk.
    pub fn load_shared(data_dir: impl AsRef<Path>) -> Result<Arc<Self>> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let key = data_dir.join(STORE_DIRECTORY).join(STORE_FILE);
        let stores = SHARED_STORES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut stores = stores
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth shared-store map poisoned"))?;
        stores.retain(|_, store| store.strong_count() > 0);
        if let Some(store) = stores.get(&key).and_then(Weak::upgrade) {
            return Ok(store);
        }
        let store = Arc::new(Self::load(&data_dir)?);
        stores.insert(key, Arc::downgrade(&store));
        Ok(store)
    }

    pub fn load(data_dir: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_profile_resolver(data_dir, |profile_id| {
            canonical_profile_id(profile_id).map(str::to_string)
        })
    }

    fn load_with_profile_resolver(
        data_dir: impl AsRef<Path>,
        resolve_profile_id: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let path = data_dir.as_ref().join(STORE_DIRECTORY).join(STORE_FILE);
        secure_existing_store_permissions(&path)?;
        let mut document = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<StoreDocument>(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreDocument::default(),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        if document.schema_version != STORE_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported local Bluetooth store schema {}",
                document.schema_version
            );
        }
        if document
            .devices
            .iter()
            .any(|device| !device_is_bounded(device))
        {
            anyhow::bail!("local Bluetooth store contains an unsupported device profile");
        }
        for device in &mut document.devices {
            if let Some(canonical_profile_id) = resolve_profile_id(&device.profile_id) {
                if !is_valid_device_profile_id(&canonical_profile_id) {
                    anyhow::bail!("local Bluetooth profile resolver returned an invalid ID");
                }
                device.profile_id = canonical_profile_id;
            }
        }
        let mut physical_devices = std::collections::BTreeSet::new();
        if document.devices.iter().any(|device| {
            !physical_devices.insert((device.profile_id.clone(), device.stable_identity.clone()))
        }) {
            anyhow::bail!(
                "local Bluetooth store contains duplicate devices after profile migration"
            );
        }
        Ok(Self {
            path,
            document: Mutex::new(document),
            registries: Mutex::new(Vec::new()),
            quiescing: AtomicBool::new(false),
            durability_degraded: AtomicBool::new(false),
            #[cfg(test)]
            persist_fault: Mutex::new(None),
        })
    }

    pub fn quiesce(&self) {
        self.quiescing.store(true, Ordering::SeqCst);
    }

    /// True when the last committed rename could not be acknowledged by a
    /// parent-directory fsync. The file and memory already agree at that
    /// point, so callers must not roll back the logical operation; support can
    /// still see that crash durability needs another successful store write.
    pub fn durability_degraded(&self) -> bool {
        self.durability_degraded.load(Ordering::Acquire)
    }

    pub fn all(&self) -> Vec<LocalBleDevice> {
        self.document
            .lock()
            .map(|document| {
                document
                    .devices
                    .iter()
                    .filter(|device| !device.blocked)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get_by_identity(&self, profile_id: &str, identity: &str) -> Option<LocalBleDevice> {
        let profile_id = canonical_profile_id(profile_id).unwrap_or(profile_id);
        self.document
            .lock()
            .ok()?
            .devices
            .iter()
            .find(|device| device.profile_id == profile_id && device.stable_identity == identity)
            .cloned()
    }

    pub fn get_by_id(&self, id: &str) -> Option<LocalBleDevice> {
        self.document
            .lock()
            .ok()?
            .devices
            .iter()
            .find(|device| device.id == id)
            .cloned()
    }

    /// Register a live or not-yet-published hub registry and reconcile it to
    /// the store while holding the document lock. A concurrent offline remove
    /// therefore either runs first (and this projection sees no device) or
    /// runs second (and removes the just-projected device from this registry).
    pub fn register_registry_and_reconcile(
        &self,
        registry: &Arc<Mutex<HubDeviceRegistry>>,
    ) -> Result<()> {
        let document = self
            .document
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth store lock poisoned"))?;
        let active_devices = document
            .devices
            .iter()
            .filter(|device| !device.blocked)
            .collect::<Vec<_>>();
        let active_ids = active_devices
            .iter()
            .map(|device| device.id.as_str())
            .collect::<HashSet<_>>();
        let mut registry_state = registry
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth hub registry lock poisoned"))?;
        let snapshot = registry_state.snapshot();
        let existing_rooms = snapshot
            .devices
            .iter()
            .map(|device| (device.id.as_str(), device.room_id.as_deref()))
            .collect::<HashMap<_, _>>();
        for device in &snapshot.devices {
            if !active_ids.contains(device.id.as_str()) {
                registry_state.remove_device(&device.id);
            }
        }
        for device in active_devices {
            let Some(profile) = profile_by_id(&device.profile_id) else {
                // Preserve an opaque newer profile's snapshot until a decoder
                // that understands it is installed.
                continue;
            };
            let projection = profile.projection();
            let buttons = projection
                .button_endpoints
                .iter()
                .map(|endpoint| {
                    (
                        format!("{}-{}", device.id, endpoint.suffix),
                        endpoint.control_id,
                    )
                })
                .collect::<Vec<_>>();
            // Remove first so a compatible profile migration cannot retain
            // button endpoints that the current projection no longer owns.
            registry_state.remove_device(&device.id);
            registry_state.upsert_device(
                &device.id,
                existing_rooms.get(device.id.as_str()).copied().flatten(),
                &buttons,
                projection.device_type,
            );
        }
        drop(registry_state);

        let mut registries = self
            .registries
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth registry list poisoned"))?;
        registries.retain(|registered| registered.strong_count() > 0);
        if !registries.iter().any(|registered| {
            registered
                .upgrade()
                .is_some_and(|registered| Arc::ptr_eq(&registered, registry))
        }) {
            registries.push(Arc::downgrade(registry));
        }
        Ok(())
    }

    pub fn upsert(&self, mut device: LocalBleDevice) -> Result<()> {
        self.ensure_writable()?;
        if let Some(profile_id) = canonical_profile_id(&device.profile_id) {
            device.profile_id = profile_id.to_string();
        }
        if !device_is_bounded(&device) {
            anyhow::bail!("local Bluetooth device record is invalid");
        }
        let mut document = self
            .document
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth store lock poisoned"))?;
        self.ensure_writable()?;
        let previous = document.clone();
        if let Some(existing) = document.devices.iter_mut().find(|existing| {
            existing.profile_id == device.profile_id
                && existing.stable_identity == device.stable_identity
        }) {
            // Public identity is allocated once per appliance association and
            // the active record remains authoritative throughout re-pair.
            // Preserve every already-committed stream under this lock: a
            // monitor may have advanced it after association took its earlier
            // snapshot, and letting pairing overwrite that value would rewind
            // deduplication and replay a consumed event. Association may seed
            // only streams the active record has never seen.
            device.id.clone_from(&existing.id);
            for (stream, value) in &device.replay_state {
                if !existing.replay_state.contains_key(stream) {
                    existing.replay_state.insert(stream.clone(), value.clone());
                }
            }
            device.replay_state.clone_from(&existing.replay_state);
            *existing = device;
        } else {
            document.devices.push(device);
        }
        document
            .devices
            .sort_by(|left, right| left.id.cmp(&right.id));
        if let Err(error) = self.persist_locked(&document) {
            *document = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<Option<LocalBleDevice>> {
        self.ensure_writable()?;
        let mut document = self
            .document
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth store lock poisoned"))?;
        self.ensure_writable()?;
        let Some(index) = document.devices.iter().position(|device| device.id == id) else {
            return Ok(None);
        };
        let removed = document.devices.remove(index);
        if let Err(error) = self.persist_locked(&document) {
            document.devices.insert(index, removed);
            return Err(error);
        }
        self.remove_from_registered_registries(id);
        Ok(Some(removed))
    }

    pub fn activate(&self, id: &str) -> Result<bool> {
        self.ensure_writable()?;
        let mut document = self
            .document
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth store lock poisoned"))?;
        let Some(index) = document.devices.iter().position(|device| device.id == id) else {
            return Ok(false);
        };
        if !document.devices[index].blocked {
            return Ok(true);
        }
        document.devices[index].blocked = false;
        if let Err(error) = self.persist_locked(&document) {
            document.devices[index].blocked = true;
            return Err(error);
        }
        Ok(true)
    }

    pub fn update_transport_hint(
        &self,
        profile_id: &str,
        identity: &str,
        transport_hint: &str,
    ) -> Result<bool> {
        self.ensure_writable()?;
        let profile_id = canonical_profile_id(profile_id).unwrap_or(profile_id);
        let mut document = self
            .document
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth store lock poisoned"))?;
        let Some(index) = document.devices.iter().position(|device| {
            device.profile_id == profile_id && device.stable_identity == identity
        }) else {
            return Ok(false);
        };
        if document.devices[index]
            .transport_hint
            .eq_ignore_ascii_case(transport_hint)
        {
            return Ok(false);
        }
        let previous = document.devices[index].transport_hint.clone();
        document.devices[index].transport_hint = transport_hint.to_string();
        if let Err(error) = self.persist_locked(&document) {
            document.devices[index].transport_hint = previous;
            return Err(error);
        }
        Ok(true)
    }

    /// Commit profile-owned replay evidence before emitting its normalized
    /// event. A profile without replay evidence is explicitly at-least-once;
    /// no Orein-specific counter shape leaks into the shared store contract.
    pub fn accept_replay(
        &self,
        profile_id: &str,
        identity: &str,
        replay: Option<&BleReplayToken>,
    ) -> Result<ReplayDisposition> {
        self.ensure_writable()?;
        let profile_id = canonical_profile_id(profile_id).unwrap_or(profile_id);
        let mut document = self
            .document
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth store lock poisoned"))?;
        let Some(index) = document.devices.iter().position(|device| {
            device.profile_id == profile_id && device.stable_identity == identity
        }) else {
            return Ok(ReplayDisposition::UnknownDevice);
        };
        if document.devices[index].blocked {
            return Ok(ReplayDisposition::Blocked);
        }
        let Some(replay) = replay else {
            return Ok(ReplayDisposition::New);
        };
        if !replay_token_is_bounded(replay) {
            anyhow::bail!("local Bluetooth replay evidence is invalid");
        }
        let previous = document.devices[index]
            .replay_state
            .get(&replay.stream)
            .cloned();
        if let Some(previous) = previous.as_deref() {
            let profile = profile_by_id(profile_id)
                .ok_or_else(|| anyhow::anyhow!("unsupported local Bluetooth profile"))?;
            match profile.replay_order(&replay.stream, previous, &replay.value) {
                ReplayOrder::Forward => {}
                ReplayOrder::Duplicate => return Ok(ReplayDisposition::Duplicate),
                ReplayOrder::Stale => return Ok(ReplayDisposition::Stale),
            }
        }
        document.devices[index]
            .replay_state
            .insert(replay.stream.clone(), replay.value.clone());
        if let Err(error) = self.persist_locked(&document) {
            match previous {
                Some(previous) => {
                    document.devices[index]
                        .replay_state
                        .insert(replay.stream.clone(), previous);
                }
                None => {
                    document.devices[index].replay_state.remove(&replay.stream);
                }
            }
            return Err(error);
        }
        Ok(ReplayDisposition::New)
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.quiescing.load(Ordering::Acquire) {
            anyhow::bail!("local Bluetooth store is quiesced for appliance reset");
        }
        Ok(())
    }

    fn remove_from_registered_registries(&self, id: &str) {
        let Ok(mut registries) = self.registries.lock() else {
            warn!(
                target: "sys",
                "Local Bluetooth removal committed, but the registry list lock was poisoned"
            );
            return;
        };
        registries.retain(|registered| {
            let Some(registry) = registered.upgrade() else {
                return false;
            };
            match registry.lock() {
                Ok(mut registry) => registry.remove_device(id),
                Err(_) => warn!(
                    target: "sys",
                    "Local Bluetooth removal committed, but a hub registry lock was poisoned"
                ),
            }
            true
        });
    }

    fn persist_locked(&self, document: &StoreDocument) -> Result<()> {
        self.ensure_writable()?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("local Bluetooth store path has no parent"))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        secure_private_directory(parent)?;
        let temp_path = self.path.with_extension("json.tmp");
        let precommit = (|| -> Result<()> {
            let bytes = serde_json::to_vec_pretty(document)?;
            self.inject_fault(PersistFault::Write)?;
            let mut options = OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temp_path)
                .with_context(|| format!("opening {}", temp_path.display()))?;
            secure_private_file(&file, &temp_path)?;
            file.write_all(&bytes)
                .with_context(|| format!("writing {}", temp_path.display()))?;
            file.sync_all()
                .with_context(|| format!("syncing {}", temp_path.display()))?;
            self.inject_fault(PersistFault::Rename)?;
            fs::rename(&temp_path, &self.path).with_context(|| {
                format!(
                    "renaming {} to {}",
                    temp_path.display(),
                    self.path.display()
                )
            })?;
            Ok(())
        })();
        if let Err(error) = precommit {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }

        // Rename is the logical commit point: after it succeeds, the file and
        // in-memory document must advance together. A parent-directory fsync
        // failure weakens crash durability but must not be reported as an
        // uncommitted operation (which would resurrect or lose records on the
        // next load). Keep the commit, mark it degraded, and retry the fsync on
        // the next store mutation.
        let directory_sync = self
            .inject_fault(PersistFault::DirectorySync)
            .and_then(|()| {
                OpenOptions::new()
                    .read(true)
                    .open(parent)
                    .and_then(|directory| directory.sync_all())
                    .with_context(|| format!("syncing {}", parent.display()))
            });
        match directory_sync {
            Ok(()) => self.durability_degraded.store(false, Ordering::Release),
            Err(error) => {
                self.durability_degraded.store(true, Ordering::Release);
                warn!(
                    target: "sys",
                    "Local Bluetooth store committed, but directory durability acknowledgement failed: {error}"
                );
            }
        }
        Ok(())
    }

    #[cfg(not(test))]
    fn inject_fault(&self, _fault: PersistFault) -> Result<()> {
        Ok(())
    }

    #[cfg(test)]
    fn inject_fault(&self, fault: PersistFault) -> Result<()> {
        let mut configured = self
            .persist_fault
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth fault lock poisoned"))?;
        if configured.as_ref() == Some(&fault) {
            *configured = None;
            anyhow::bail!("injected local Bluetooth {fault:?} persistence failure");
        }
        Ok(())
    }

    #[cfg(test)]
    fn fail_next_persist_at(&self, fault: PersistFault) {
        *self.persist_fault.lock().unwrap() = Some(fault);
    }
}

fn secure_existing_store_permissions(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("local Bluetooth store path has no parent"))?;
    if parent
        .try_exists()
        .with_context(|| format!("checking {}", parent.display()))?
    {
        secure_private_directory(parent)?;
    }
    for private_file in [path.to_path_buf(), path.with_extension("json.tmp")] {
        if private_file
            .try_exists()
            .with_context(|| format!("checking {}", private_file.display()))?
        {
            let file = OpenOptions::new()
                .read(true)
                .open(&private_file)
                .with_context(|| {
                    format!("opening {} to secure permissions", private_file.display())
                })?;
            secure_private_file(&file, &private_file)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn secure_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("securing private directory {}", path.display()))
}

#[cfg(not(unix))]
fn secure_private_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_private_file(file: &fs::File, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("securing private file {}", path.display()))
}

#[cfg(not(unix))]
fn secure_private_file(_file: &fs::File, _path: &Path) -> Result<()> {
    Ok(())
}

fn device_is_bounded(device: &LocalBleDevice) -> bool {
    is_valid_device_profile_id(&device.profile_id)
        && public_id_is_bounded(&device.id)
        && !device.stable_identity.is_empty()
        && device.stable_identity.len() <= 128
        && device
            .stable_identity
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
        && !device.transport_hint.is_empty()
        && device.transport_hint.len() <= 128
        && device
            .transport_hint
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
        && device.metadata.len() <= 16
        && device.metadata.iter().all(|(key, value)| {
            !key.is_empty()
                && key.len() <= 32
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                && !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(|byte| byte.is_ascii_graphic())
        })
        && device.replay_state.len() <= 16
        && device.replay_state.iter().all(|(stream, value)| {
            replay_token_is_bounded(&BleReplayToken {
                stream: stream.clone(),
                value: value.clone(),
            })
        })
}

fn public_id_is_bounded(id: &str) -> bool {
    id.strip_prefix("local-ble-").is_some_and(|token| {
        token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn replay_token_is_bounded(replay: &BleReplayToken) -> bool {
    !replay.stream.is_empty()
        && replay.stream.len() <= 32
        && replay.stream.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_'
        })
        && !replay.value.is_empty()
        && replay.value.len() <= 64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::OREIN_OC02001_PROFILE_ID;

    fn temporary_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rhythm-local-ble-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn replay_state(counter: Option<u8>) -> BTreeMap<String, Vec<u8>> {
        counter
            .map(|counter| BTreeMap::from([("press".to_string(), vec![counter])]))
            .unwrap_or_default()
    }

    #[cfg(unix)]
    #[test]
    fn private_store_permissions_are_created_and_repaired() {
        use std::os::unix::fs::PermissionsExt;

        let root = temporary_dir("private-permissions");
        let store_path = root.join(STORE_DIRECTORY).join(STORE_FILE);
        let store_directory = store_path.parent().unwrap();
        LocalBleDeviceStore::load(&root)
            .unwrap()
            .upsert(device(None, false))
            .unwrap();

        assert_eq!(
            fs::metadata(store_directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&store_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::set_permissions(store_directory, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&store_path, fs::Permissions::from_mode(0o644)).unwrap();
        let store = LocalBleDeviceStore::load(&root).unwrap();
        assert_eq!(
            fs::metadata(store_directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&store_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let stale_temp = store_path.with_extension("json.tmp");
        fs::write(&stale_temp, b"stale private state").unwrap();
        fs::set_permissions(&stale_temp, fs::Permissions::from_mode(0o644)).unwrap();
        drop(store);
        let store = LocalBleDeviceStore::load(&root).unwrap();
        assert_eq!(
            fs::metadata(&stale_temp).unwrap().permissions().mode() & 0o777,
            0o600
        );
        store.upsert(device(Some(1), false)).unwrap();
        assert!(!stale_temp.exists());
        assert_eq!(
            fs::metadata(&store_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn replay(counter: u8) -> BleReplayToken {
        BleReplayToken {
            stream: "press".to_string(),
            value: vec![counter],
        }
    }

    fn device(counter: Option<u8>, blocked: bool) -> LocalBleDevice {
        LocalBleDevice::from_setup(
            OREIN_OC02001_PROFILE_ID,
            &ValidatedBleSetup {
                stable_identity: "0A0B0C0D0E0F".to_string(),
                metadata: BTreeMap::from([
                    ("serial".to_string(), "SYNTHETIC000001".to_string()),
                    ("model".to_string(), "TESTMODEL001".to_string()),
                ]),
            },
            "0A:0B:0C:0D:0E:0F".to_string(),
            replay_state(counter),
            blocked,
            1,
        )
    }

    #[test]
    fn offline_remove_cannot_be_resurrected_by_a_stale_bootstrap_snapshot() {
        use std::sync::Barrier;

        let root = temporary_dir("bootstrap-unpair-race");
        let bootstrap_store = LocalBleDeviceStore::load_shared(&root).unwrap();
        let record = device(Some(7), false);
        let public_id = record.id.clone();
        bootstrap_store.upsert(record.clone()).unwrap();
        let offline_store = LocalBleDeviceStore::load_shared(&root).unwrap();
        assert!(Arc::ptr_eq(&bootstrap_store, &offline_store));

        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        let snapshot_ready = Arc::new(Barrier::new(2));
        let removal_committed = Arc::new(Barrier::new(2));
        let bootstrap = {
            let bootstrap_store = bootstrap_store.clone();
            let registry = registry.clone();
            let snapshot_ready = snapshot_ready.clone();
            let removal_committed = removal_committed.clone();
            std::thread::spawn(move || {
                // Model bootstrap restoring a registry snapshot from the same
                // record before its ActiveHub is visible in AppState.
                registry.lock().unwrap().upsert_device(
                    &record.id,
                    None,
                    &[(record.button_id("button-1").unwrap(), 1)],
                    rhythm_core::runtime::hub_registry::DeviceType::Button,
                );
                snapshot_ready.wait();
                removal_committed.wait();
                bootstrap_store
                    .register_registry_and_reconcile(&registry)
                    .unwrap();
            })
        };

        snapshot_ready.wait();
        assert_eq!(
            offline_store
                .remove(&public_id)
                .unwrap()
                .map(|device| device.id),
            Some(public_id.clone())
        );
        removal_committed.wait();
        bootstrap.join().unwrap();

        assert!(bootstrap_store.all().is_empty());
        assert!(!registry.lock().unwrap().has_device(&public_id));
        assert!(!bootstrap_store
            .update_transport_hint(
                OREIN_OC02001_PROFILE_ID,
                "0A0B0C0D0E0F",
                "0A:0B:0C:0D:0E:10",
            )
            .unwrap());
        assert_eq!(
            bootstrap_store
                .accept_replay(OREIN_OC02001_PROFILE_ID, "0A0B0C0D0E0F", Some(&replay(8)),)
                .unwrap(),
            ReplayDisposition::UnknownDevice
        );

        drop(offline_store);
        drop(bootstrap_store);
        assert!(LocalBleDeviceStore::load(&root).unwrap().all().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_id_is_random_private_and_reused_by_the_store() {
        let root = temporary_dir("public-id");
        let store = LocalBleDeviceStore::load(&root).unwrap();
        let first = device(None, false);
        let first_id = first.id.clone();
        let second = device(None, false);
        assert_ne!(first.id, second.id);
        assert!(first.id.starts_with("local-ble-"));
        assert_eq!(first.id.len(), "local-ble-".len() + 32);
        assert!(!first.id.contains("orein"));
        assert!(!first.id.contains("button"));
        assert!(!first.id.to_ascii_lowercase().contains("0a0b0c0d0e0f"));
        let public_pairing_result = serde_json::json!({ "device_id": first.id });
        assert!(!public_pairing_result.to_string().contains("0A0B0C0D0E0F"));

        store.upsert(first).unwrap();
        store.upsert(second).unwrap();
        assert_eq!(
            store
                .get_by_identity(OREIN_OC02001_PROFILE_ID, "0A0B0C0D0E0F")
                .unwrap()
                .id,
            first_id
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opaque_replay_dedup_accepts_every_changed_orein_token_including_gaps_and_wraparound() {
        let root = temporary_dir("counter");
        let store = LocalBleDeviceStore::load(&root).unwrap();
        store.upsert(device(Some(0x85), false)).unwrap();

        assert_eq!(
            store
                .accept_replay(
                    OREIN_OC02001_PROFILE_ID,
                    "0A0B0C0D0E0F",
                    Some(&replay(0x85)),
                )
                .unwrap(),
            ReplayDisposition::Duplicate
        );
        assert_eq!(
            store
                .accept_replay(
                    OREIN_OC02001_PROFILE_ID,
                    "0A0B0C0D0E0F",
                    Some(&replay(0x90)),
                )
                .unwrap(),
            ReplayDisposition::New
        );
        assert_eq!(
            store
                .accept_replay(
                    OREIN_OC02001_PROFILE_ID,
                    "0A0B0C0D0E0F",
                    Some(&replay(0x00)),
                )
                .unwrap(),
            ReplayDisposition::New
        );
        assert_eq!(
            store
                .accept_replay(
                    OREIN_OC02001_PROFILE_ID,
                    "0A0B0C0D0E0F",
                    Some(&replay(0xff)),
                )
                .unwrap(),
            ReplayDisposition::New
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_repair_cannot_rewind_monitor_committed_replay_state() {
        let root = temporary_dir("repair-replay-race");
        let store = LocalBleDeviceStore::load(&root).unwrap();
        store.upsert(device(Some(0x10), false)).unwrap();

        // Pairing snapshots the old advertisement, then the active monitor
        // commits a newer press before pairing reaches its activation upsert.
        let stale_pairing_candidate = device(Some(0x10), false);
        assert_eq!(
            store
                .accept_replay(
                    OREIN_OC02001_PROFILE_ID,
                    "0A0B0C0D0E0F",
                    Some(&replay(0x11)),
                )
                .unwrap(),
            ReplayDisposition::New
        );
        store.upsert(stale_pairing_candidate).unwrap();

        let restored = store
            .get_by_identity(OREIN_OC02001_PROFILE_ID, "0A0B0C0D0E0F")
            .unwrap();
        assert_eq!(restored.replay_state, replay_state(Some(0x11)));
        assert_eq!(
            store
                .accept_replay(
                    OREIN_OC02001_PROFILE_ID,
                    "0A0B0C0D0E0F",
                    Some(&replay(0x11)),
                )
                .unwrap(),
            ReplayDisposition::Duplicate
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn precommit_write_and_rename_failures_leave_memory_and_disk_unchanged() {
        for (label, fault) in [
            ("write-fault", PersistFault::Write),
            ("rename-fault", PersistFault::Rename),
        ] {
            let root = temporary_dir(label);
            let store = LocalBleDeviceStore::load(&root).unwrap();
            store.fail_next_persist_at(fault);

            assert!(store.upsert(device(Some(0x10), false)).is_err());
            assert!(store.all().is_empty());
            assert!(LocalBleDeviceStore::load(&root).unwrap().all().is_empty());
            assert!(!root.join(STORE_DIRECTORY).join("devices.json.tmp").exists());
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn directory_sync_failure_keeps_the_committed_file_and_memory_aligned() {
        let root = temporary_dir("directory-sync-fault");
        let store = LocalBleDeviceStore::load(&root).unwrap();
        store.fail_next_persist_at(PersistFault::DirectorySync);

        store.upsert(device(Some(0x10), false)).unwrap();

        assert!(store.durability_degraded());
        assert_eq!(store.all().len(), 1);
        assert_eq!(LocalBleDeviceStore::load(&root).unwrap().all().len(), 1);

        // A later successful mutation retries the parent-directory fsync and
        // clears the support-visible degraded state.
        store.upsert(device(Some(0x20), false)).unwrap();
        assert!(!store.durability_degraded());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn truncated_and_invalid_records_fail_closed_without_partial_restore() {
        let root = temporary_dir("corrupt");
        let directory = root.join(STORE_DIRECTORY);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(STORE_FILE);

        fs::write(&path, br#"{"schema_version":1,"devices":["#).unwrap();
        assert!(LocalBleDeviceStore::load(&root).is_err());

        let mut invalid = device(None, false);
        invalid.id = "identity-derived-and-invalid".to_string();
        fs::write(
            &path,
            serde_json::to_vec(&StoreDocument {
                schema_version: STORE_SCHEMA_VERSION,
                devices: vec![invalid],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(LocalBleDeviceStore::load(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_restores_exact_profile_identity_hint_and_opaque_replay_state() {
        let root = temporary_dir("restart");
        LocalBleDeviceStore::load(&root)
            .unwrap()
            .upsert(device(Some(0xff), false))
            .unwrap();

        let store = LocalBleDeviceStore::load(&root).unwrap();
        assert!(store
            .get_by_identity(OREIN_OC02001_PROFILE_ID, "0a0b0c0d0e0f")
            .is_none());
        let restored = store
            .get_by_identity(OREIN_OC02001_PROFILE_ID, "0A0B0C0D0E0F")
            .unwrap();
        assert_eq!(restored.replay_state, replay_state(Some(0xff)));
        assert_eq!(restored.profile_id, OREIN_OC02001_PROFILE_ID);
        assert_eq!(restored.transport_hint, "0A:0B:0C:0D:0E:0F");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_owned_identity_is_exact_and_never_host_case_folded() {
        let root = temporary_dir("case-sensitive-identity");
        let store = LocalBleDeviceStore::load(&root).unwrap();
        let mut upper = device(None, false);
        upper.stable_identity = "CaseSensitive".to_string();
        let mut lower = device(None, false);
        lower.stable_identity = "casesensitive".to_string();

        store.upsert(upper.clone()).unwrap();
        store.upsert(lower.clone()).unwrap();

        assert_eq!(store.all().len(), 2);
        assert_eq!(
            store
                .get_by_identity(OREIN_OC02001_PROFILE_ID, "CaseSensitive")
                .unwrap()
                .id,
            upper.id
        );
        assert_eq!(
            store
                .get_by_identity(OREIN_OC02001_PROFILE_ID, "casesensitive")
                .unwrap()
                .id,
            lower.id
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn downgrade_preserves_a_bounded_record_for_a_newer_unknown_profile() {
        let root = temporary_dir("unknown-profile");
        let setup = ValidatedBleSetup {
            stable_identity: "SYNTHETIC-01".to_string(),
            metadata: BTreeMap::from([("model".to_string(), "future-sensor".to_string())]),
        };
        let device = LocalBleDevice::from_setup(
            "example.future.sensor.v1",
            &setup,
            "02:00:00:00:00:09".to_string(),
            BTreeMap::from([("opaque.v2".to_string(), vec![0xde, 0xad, 0xbe, 0xef])]),
            false,
            1,
        );
        LocalBleDeviceStore::load(&root)
            .unwrap()
            .upsert(device.clone())
            .unwrap();

        let restored = LocalBleDeviceStore::load(&root).unwrap().all();
        assert_eq!(restored, vec![device]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_canonicalizes_an_explicit_compatible_profile_without_changing_identity_or_replay() {
        let root = temporary_dir("compatible-profile-migration");
        let mut legacy = device(Some(0xfe), false);
        legacy.profile_id = "example.motion.v1".to_string();
        let public_id = legacy.id.clone();
        LocalBleDeviceStore::load(&root)
            .unwrap()
            .upsert(legacy)
            .unwrap();

        let migrated = LocalBleDeviceStore::load_with_profile_resolver(&root, |profile_id| {
            (profile_id == "example.motion.v1").then(|| "example.motion.v2".to_string())
        })
        .unwrap();
        let restored = migrated
            .get_by_identity("example.motion.v2", "0A0B0C0D0E0F")
            .unwrap();
        assert_eq!(restored.id, public_id);
        assert_eq!(restored.profile_id, "example.motion.v2");
        assert_eq!(restored.replay_state, replay_state(Some(0xfe)));
        assert_eq!(
            restored.metadata.get("serial").map(String::as_str),
            Some("SYNTHETIC000001")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_rejects_alias_migration_that_would_merge_two_public_devices() {
        let root = temporary_dir("compatible-profile-collision");
        let store = LocalBleDeviceStore::load(&root).unwrap();
        let mut legacy = device(None, false);
        legacy.profile_id = "example.motion.v1".to_string();
        let mut current = device(None, false);
        current.profile_id = "example.motion.v2".to_string();
        store.upsert(legacy).unwrap();
        store.upsert(current).unwrap();
        drop(store);

        assert!(
            LocalBleDeviceStore::load_with_profile_resolver(&root, |profile_id| {
                (profile_id == "example.motion.v1").then(|| "example.motion.v2".to_string())
            })
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn blocked_projection_record_is_inert_until_durably_activated() {
        let root = temporary_dir("blocked");
        let store = LocalBleDeviceStore::load(&root).unwrap();
        let device = device(Some(0x10), true);
        let id = device.id.clone();
        store.upsert(device).unwrap();

        assert!(store.all().is_empty());
        assert_eq!(
            store
                .accept_replay(
                    OREIN_OC02001_PROFILE_ID,
                    "0A0B0C0D0E0F",
                    Some(&replay(0x11)),
                )
                .unwrap(),
            ReplayDisposition::Blocked
        );
        assert!(store.activate(&id).unwrap());
        assert_eq!(store.all().len(), 1);
        assert_eq!(
            store
                .accept_replay(
                    OREIN_OC02001_PROFILE_ID,
                    "0A0B0C0D0E0F",
                    Some(&replay(0x11)),
                )
                .unwrap(),
            ReplayDisposition::New
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quiesce_prevents_a_deleted_store_from_being_recreated() {
        let root = temporary_dir("quiesce");
        let store = LocalBleDeviceStore::load(&root).unwrap();
        store.upsert(device(Some(0x10), false)).unwrap();
        store.quiesce();
        fs::remove_dir_all(root.join(STORE_DIRECTORY)).unwrap();

        assert!(store
            .accept_replay(
                OREIN_OC02001_PROFILE_ID,
                "0A0B0C0D0E0F",
                Some(&replay(0x11)),
            )
            .is_err());
        assert!(!root.join(STORE_DIRECTORY).exists());
        let _ = fs::remove_dir_all(root);
    }
}
