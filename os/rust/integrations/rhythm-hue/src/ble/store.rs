//! Durable metadata for bonded Hue BLE bulbs.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::types::{HueBleDevice, HueBleState};

const SCHEMA_VERSION: u32 = 1;
const TOMBSTONE_SCHEMA_VERSION: u32 = 4;
const FACTORY_RESET_BLOCK_FILE: &str = ".hue_ble_factory_reset_pending";
const FACTORY_RESET_PLAN_FILE: &str = ".hue_ble_factory_reset_plan.json";
const FACTORY_RESET_PLAN_SCHEMA_VERSION: u32 = 1;
const STATE_OBSERVATION_MAX_AGE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug)]
struct TimedStateObservation {
    state: HueBleState,
    observed_at: Instant,
}

#[derive(Default)]
struct StateObservationCache {
    states: BTreeMap<String, TimedStateObservation>,
    generations: BTreeMap<String, u64>,
    commands_in_flight: BTreeMap<String, usize>,
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)
            .with_context(|| format!("opening {} for directory sync", path.display()))?
            .sync_all()
            .with_context(|| format!("syncing directory {}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sync_parent_directory_entry(path: &Path) -> Result<()> {
    match path.parent() {
        Some(parent) => sync_directory(parent),
        None => Ok(()),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreDocument {
    schema_version: u32,
    #[serde(default)]
    devices: Vec<HueBleDevice>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct TombstoneDocument {
    schema_version: u32,
    #[serde(default)]
    devices: Vec<HueBleDevice>,
    /// Device IDs whose authenticated Hue pairing-handoff command was
    /// durably recorded before BlueZ bond deletion. If power fails after the
    /// local key disappears, this journal proves the peripheral was first
    /// placed in its short-lived replacement-pairing window.
    #[serde(default)]
    prepared_handoff_ids: Vec<String>,
    /// Native IDs whose exact BlueZ address metadata was missing when the user
    /// explicitly force-removed the endpoint. These still filter stale
    /// registry snapshots even though no per-address bond cleanup is possible.
    #[serde(default)]
    blocked_native_ids: Vec<String>,
    /// Native IDs whose exact local bond was verified absent during an
    /// explicit forced removal. No bond quarantine is needed, but the marker
    /// survives until canonical endpoint cleanup (or a fresh re-association)
    /// so a crash cannot resurrect the endpoint from a registry snapshot.
    #[serde(default)]
    removed_native_ids: Vec<String>,
    /// Exact BlueZ addresses where pairing may have created a bond but device
    /// inspection did not produce complete metadata. Only an explicit nearby
    /// scan may retry these addresses.
    #[serde(default)]
    pairing_retry_addresses: Vec<String>,
    /// Device IDs being moved from a tombstone/retry quarantine back to the
    /// active store. This marker is persisted before devices.json so recovery
    /// can distinguish an interrupted re-association from an interrupted
    /// force-removal.
    #[serde(default)]
    reassociation_in_progress_ids: Vec<String>,
    /// Set when devices.json was corrupt and its former bonded addresses could
    /// not be recovered. This prevents silent adoption of unknown paired
    /// BlueZ objects after recovery.
    #[serde(default)]
    block_paired_orphan_adoption: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HueBleFactoryResetPlan {
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub devices: Vec<HueBleDevice>,
    /// Device IDs whose authenticated handoff write was persisted during the
    /// final all-bulbs refresh. The appliance retains every key until all
    /// entries are prepared, then stops BlueZ for one raw database scrub.
    #[serde(default)]
    pub prepared_device_ids: Vec<String>,
    /// Device IDs already absent after a previously journaled cleanup attempt
    /// (for example, recovery from a partially committed raw database scrub).
    /// Keep these alongside the full device list until the scrub commits.
    #[serde(default)]
    pub released_device_ids: Vec<String>,
}

/// Thread-safe device store backed by `<data_dir>/hue_ble/devices.json`.
pub struct HueBleDeviceStore {
    path: PathBuf,
    devices: Mutex<BTreeMap<String, HueBleDevice>>,
    state_observations: Mutex<StateObservationCache>,
    tombstone_path: PathBuf,
    tombstones: Mutex<TombstoneDocument>,
    factory_reset_blocked: bool,
}

/// Invalidates every read that began before or during a foreground command.
/// Dropping the guard makes later physical observations eligible again.
pub(crate) struct HueBleCommandObservationGuard<'a> {
    store: &'a HueBleDeviceStore,
    id: String,
}

impl Drop for HueBleCommandObservationGuard<'_> {
    fn drop(&mut self) {
        self.store.finish_command_observation(&self.id);
    }
}

impl HueBleDeviceStore {
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        let path = data_dir.join("hue_ble").join("devices.json");
        let tombstone_path = data_dir.join("hue_ble").join("unpaired_bonds.json");
        let tombstones = Self::load_tombstones(&tombstone_path)?;
        let factory_reset_blocked = match fs::metadata(data_dir.join(FACTORY_RESET_BLOCK_FILE)) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "checking Hue BLE factory-reset marker in {}",
                        data_dir.display()
                    )
                });
            }
        };
        let document = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<StoreDocument>(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreDocument {
                schema_version: SCHEMA_VERSION,
                devices: Vec::new(),
            },
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        if document.schema_version > SCHEMA_VERSION {
            anyhow::bail!(
                "Hue BLE device store schema {} is newer than supported {}",
                document.schema_version,
                SCHEMA_VERSION
            );
        }
        let devices = document
            .devices
            .into_iter()
            .map(|device| (device.id.clone(), device))
            .collect();
        Ok(Self {
            path,
            devices: Mutex::new(devices),
            // Durable `last_state` is pairing/debug metadata, not proof of a
            // fresh physical observation after this process started.
            state_observations: Mutex::new(StateObservationCache::default()),
            tombstone_path,
            tombstones: Mutex::new(tombstones),
            factory_reset_blocked,
        })
    }

    /// Persist a global fail-closed marker before platform code removes
    /// adapter-bound BlueZ link keys.
    ///
    /// Factory reset clears ordinary Hue BLE metadata before the appliance
    /// callback can scrub `/data/bluetooth`. If that scrub is interrupted or
    /// fails, this marker prevents remaining paired BlueZ objects from being
    /// silently adopted as freshly paired bulbs after restart.
    pub fn install_paired_orphan_adoption_block(data_dir: impl AsRef<Path>) -> Result<()> {
        let data_dir = data_dir.as_ref();
        fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
        sync_parent_directory_entry(data_dir)?;
        let path = data_dir.join(FACTORY_RESET_BLOCK_FILE);
        let temporary = data_dir.join(format!("{FACTORY_RESET_BLOCK_FILE}.tmp"));
        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("opening {}", temporary.display()))?;
        file.write_all(b"pending\n")
            .with_context(|| format!("writing {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, &path).with_context(|| {
            format!("installing Hue BLE factory-reset marker {}", path.display())
        })?;
        sync_directory(data_dir)?;
        Ok(())
    }

    /// Clear the global factory-reset block only after platform code confirms
    /// that every persistent BlueZ bond has been removed.
    pub fn clear_paired_orphan_adoption_block(data_dir: impl AsRef<Path>) -> Result<()> {
        let data_dir = data_dir.as_ref();
        let path = data_dir.join(FACTORY_RESET_BLOCK_FILE);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| format!("removing {}", path.display()));
            }
        }
        sync_directory(data_dir)?;
        Ok(())
    }

    /// Load the durable list of Hue bonds that a factory-reset retry must
    /// release. The plan lives outside `hue_ble/` because shared reset storage
    /// intentionally removes that integration directory before the appliance
    /// performs its final BlueZ scrub.
    pub fn load_factory_reset_plan(data_dir: impl AsRef<Path>) -> Result<HueBleFactoryResetPlan> {
        let path = data_dir.as_ref().join(FACTORY_RESET_PLAN_FILE);
        let document = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<HueBleFactoryResetPlan>(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HueBleFactoryResetPlan {
                    schema_version: FACTORY_RESET_PLAN_SCHEMA_VERSION,
                    ..Default::default()
                });
            }
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        if document.schema_version > FACTORY_RESET_PLAN_SCHEMA_VERSION {
            anyhow::bail!(
                "Hue BLE factory-reset plan schema {} is newer than supported {}",
                document.schema_version,
                FACTORY_RESET_PLAN_SCHEMA_VERSION
            );
        }
        Ok(document)
    }

    /// Atomically replace the durable factory-reset bond plan.
    pub fn persist_factory_reset_plan(
        data_dir: impl AsRef<Path>,
        plan: &HueBleFactoryResetPlan,
    ) -> Result<()> {
        let data_dir = data_dir.as_ref();
        fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
        sync_parent_directory_entry(data_dir)?;
        let path = data_dir.join(FACTORY_RESET_PLAN_FILE);
        let temporary = data_dir.join(format!("{FACTORY_RESET_PLAN_FILE}.tmp"));
        let mut document = plan.clone();
        document.schema_version = FACTORY_RESET_PLAN_SCHEMA_VERSION;
        document
            .devices
            .sort_by(|left, right| left.id.cmp(&right.id));
        document.devices.dedup_by(|left, right| left.id == right.id);
        document.prepared_device_ids.sort();
        document.prepared_device_ids.dedup();
        document.released_device_ids.sort();
        document.released_device_ids.dedup();
        let document = HueBleFactoryResetPlan {
            schema_version: FACTORY_RESET_PLAN_SCHEMA_VERSION,
            ..document
        };
        let bytes = serde_json::to_vec_pretty(&document)?;
        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("opening {}", temporary.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("writing {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, &path)
            .with_context(|| format!("installing Hue BLE factory-reset plan {}", path.display()))?;
        sync_directory(data_dir)?;
        Ok(())
    }

    pub fn clear_factory_reset_plan(data_dir: impl AsRef<Path>) -> Result<()> {
        let data_dir = data_dir.as_ref();
        let path = data_dir.join(FACTORY_RESET_PLAN_FILE);
        match fs::remove_file(&path) {
            Ok(()) => sync_directory(data_dir),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
        }
    }

    /// Load the store for an explicit force-forget operation.
    ///
    /// A malformed JSON file would otherwise make it impossible to remove a
    /// dead Hue BLE endpoint from Rhythm. Preserve that file for diagnosis,
    /// then continue with an empty store. Valid stores from a newer schema and
    /// ordinary I/O failures remain fail-closed so we never discard readable
    /// data that this version merely cannot use.
    pub fn load_recovering_corrupt(data_dir: impl AsRef<Path>) -> Result<(Self, Option<PathBuf>)> {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(store) => Ok((store, None)),
            Err(load_error) => {
                let path = data_dir.join("hue_ble").join("devices.json");
                let tombstone_path = data_dir.join("hue_ble").join("unpaired_bonds.json");
                // Tombstones are safety-critical. If that file is corrupt,
                // fail closed rather than recovering devices.json and losing
                // the set of bonds that must not be adopted.
                Self::load_tombstones(&tombstone_path)?;
                let Ok(bytes) = fs::read(&path) else {
                    return Err(load_error);
                };
                if let Ok(document) = serde_json::from_slice::<StoreDocument>(&bytes) {
                    if document.schema_version > SCHEMA_VERSION {
                        return Err(load_error);
                    }
                    // A valid, supported document should have loaded above.
                    // Preserve it rather than guessing at an unexpected error.
                    return Err(load_error);
                }

                let mut tombstones = Self::load_tombstones(&tombstone_path)?;
                tombstones.block_paired_orphan_adoption = true;
                Self::persist_tombstones_at(&tombstone_path, &tombstones)?;

                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let quarantine = path.with_file_name(format!(
                    "devices.json.corrupt.{}.{}",
                    std::process::id(),
                    timestamp
                ));
                fs::rename(&path, &quarantine).with_context(|| {
                    format!(
                        "quarantining corrupt Hue BLE store {} as {}",
                        path.display(),
                        quarantine.display()
                    )
                })?;
                if let Some(parent) = path.parent() {
                    sync_directory(parent)?;
                }
                Ok((Self::load(data_dir)?, Some(quarantine)))
            }
        }
    }

    fn load_tombstones(path: &Path) -> Result<TombstoneDocument> {
        let mut document = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<TombstoneDocument>(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => TombstoneDocument {
                schema_version: TOMBSTONE_SCHEMA_VERSION,
                ..Default::default()
            },
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        if document.schema_version > TOMBSTONE_SCHEMA_VERSION {
            anyhow::bail!(
                "Hue BLE unpaired-bond schema {} is newer than supported {}",
                document.schema_version,
                TOMBSTONE_SCHEMA_VERSION
            );
        }
        if document.schema_version < 2 {
            // Schema v1 predates the authenticated handoff journal. Even if a
            // prerelease writer emitted the additive field without bumping the
            // version, it is not trustworthy proof that the bulb-side command
            // completed. Preserve exact tombstones but treat every one as an
            // ordinary retained/forced removal.
            document.prepared_handoff_ids.clear();
        }
        if document.schema_version < 3 {
            // Schema v3 defines the ordering guarantees for re-association.
            // Earlier additive fields, if any prerelease happened to emit
            // them, are not trustworthy transaction markers.
            document.pairing_retry_addresses.clear();
            document.reassociation_in_progress_ids.clear();
        }
        if document.schema_version < 4 {
            // Schema v4 defines the crash ordering for an explicitly
            // acknowledged absent local bond.
            document.removed_native_ids.clear();
        }
        document.schema_version = TOMBSTONE_SCHEMA_VERSION;
        Ok(document)
    }

    pub fn all(&self) -> Vec<HueBleDevice> {
        self.devices
            .lock()
            .map(|devices| devices.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<HueBleDevice> {
        self.devices
            .lock()
            .ok()
            .and_then(|devices| devices.get(id).cloned())
    }

    pub fn tombstones(&self) -> Vec<HueBleDevice> {
        self.tombstones
            .lock()
            .map(|document| document.devices.clone())
            .unwrap_or_default()
    }

    pub fn tombstone(&self, id: &str) -> Option<HueBleDevice> {
        self.tombstones.lock().ok().and_then(|document| {
            document
                .devices
                .iter()
                .find(|device| device.id == id)
                .cloned()
        })
    }

    pub fn handoff_was_prepared(&self, id: &str) -> bool {
        self.tombstones
            .lock()
            .map(|document| {
                document
                    .prepared_handoff_ids
                    .iter()
                    .any(|prepared| prepared == id)
            })
            .unwrap_or(false)
    }

    pub fn blocked_native_ids(&self) -> Vec<String> {
        self.tombstones
            .lock()
            .map(|document| document.blocked_native_ids.clone())
            .unwrap_or_default()
    }

    pub fn removed_native_ids(&self) -> Vec<String> {
        self.tombstones
            .lock()
            .map(|document| document.removed_native_ids.clone())
            .unwrap_or_default()
    }

    pub fn pairing_retry_addresses(&self) -> Vec<String> {
        self.tombstones
            .lock()
            .map(|document| document.pairing_retry_addresses.clone())
            .unwrap_or_default()
    }

    pub fn reassociation_in_progress_ids(&self) -> Vec<String> {
        self.tombstones
            .lock()
            .map(|document| document.reassociation_in_progress_ids.clone())
            .unwrap_or_default()
    }

    pub fn reassociation_was_started(&self, id: &str) -> bool {
        self.tombstones
            .lock()
            .map(|document| {
                document
                    .reassociation_in_progress_ids
                    .iter()
                    .any(|pending| pending == id)
            })
            .unwrap_or(false)
    }

    pub fn blocks_paired_orphan_adoption(&self) -> bool {
        self.factory_reset_blocked
            || self
                .tombstones
                .lock()
                .map(|document| document.block_paired_orphan_adoption)
                .unwrap_or(true)
    }

    /// True when Rhythm knows an endpoint or store existed but no longer has
    /// the exact Bluetooth address needed to release its bond safely.
    ///
    /// This deliberately excludes the external in-progress factory-reset
    /// marker: that marker may survive a reboot and must not make a later
    /// retry impossible.
    pub fn has_unknown_bond_quarantine(&self) -> bool {
        self.tombstones
            .lock()
            .map(|document| document.block_paired_orphan_adoption)
            .unwrap_or(true)
    }

    /// Fail closed when Rhythm knows a Hue BLE endpoint existed but no longer
    /// has enough metadata to identify its BlueZ address for exact removal.
    pub fn block_paired_orphan_adoption(&self) -> Result<()> {
        let mut document = self
            .tombstones
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE tombstone store lock poisoned"))?;
        if document.block_paired_orphan_adoption {
            return Ok(());
        }
        let mut next = document.clone();
        next.block_paired_orphan_adoption = true;
        self.persist_tombstones(&next)?;
        *document = next;
        Ok(())
    }

    /// Record an explicit force-removal when only the canonical native ID
    /// survives. The global flag protects unknown residual bonds; the ID lets
    /// boot recovery filter stale registry/topology projections.
    pub fn record_missing_metadata_removal(&self, id: &str) -> Result<()> {
        let mut document = self
            .tombstones
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE tombstone store lock poisoned"))?;
        let mut next = document.clone();
        next.block_paired_orphan_adoption = true;
        next.prepared_handoff_ids.retain(|prepared| prepared != id);
        next.removed_native_ids.retain(|removed| removed != id);
        next.reassociation_in_progress_ids
            .retain(|pending| pending != id);
        if !next
            .blocked_native_ids
            .iter()
            .any(|existing| existing == id)
        {
            next.blocked_native_ids.push(id.to_string());
            next.blocked_native_ids.sort();
        }
        if next.block_paired_orphan_adoption == document.block_paired_orphan_adoption
            && next.blocked_native_ids == document.blocked_native_ids
            && next.removed_native_ids == document.removed_native_ids
            && next.prepared_handoff_ids == document.prepared_handoff_ids
            && next.reassociation_in_progress_ids == document.reassociation_in_progress_ids
        {
            return Ok(());
        }
        self.persist_tombstones(&next)?;
        *document = next;
        Ok(())
    }

    /// Quarantine an exact address when pairing may have left a BlueZ key but
    /// validation could not produce a HueBleDevice record.
    pub fn record_pairing_retry_address(&self, address: &str) -> Result<()> {
        let address = address.trim();
        if address.is_empty() {
            anyhow::bail!("Cannot quarantine an empty Hue Bluetooth address");
        }
        let normalized = address.to_ascii_uppercase();
        let mut document = self
            .tombstones
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE tombstone store lock poisoned"))?;
        if document
            .pairing_retry_addresses
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&normalized))
        {
            return Ok(());
        }
        let mut next = document.clone();
        next.pairing_retry_addresses.push(normalized);
        next.pairing_retry_addresses.sort();
        next.pairing_retry_addresses.dedup();
        self.persist_tombstones(&next)?;
        *document = next;
        Ok(())
    }

    /// Clear one exact retry allowance after its stale local key was
    /// successfully removed. Other quarantined bulbs remain untouched.
    pub fn clear_pairing_retry_address(&self, address: &str) -> Result<()> {
        let mut document = self
            .tombstones
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE tombstone store lock poisoned"))?;
        let mut next = document.clone();
        next.pairing_retry_addresses
            .retain(|existing| !existing.eq_ignore_ascii_case(address));
        if next.pairing_retry_addresses.len() == document.pairing_retry_addresses.len() {
            return Ok(());
        }
        self.persist_tombstones(&next)?;
        *document = next;
        Ok(())
    }

    pub fn upsert(&self, device: HueBleDevice) -> Result<()> {
        // Re-pairing or replacing metadata starts a new observation epoch.
        // Conservatively forget any process-local state even if persistence
        // later fails.
        self.invalidate_state_observation(&device.id)?;
        // If this is an explicit recovery, journal that intent before active
        // metadata. A crash can therefore leave either the old quarantine or
        // active metadata plus the transaction marker, but never an
        // unallowlisted orphan bond.
        self.begin_reassociation_if_needed(&device)?;
        let mut devices = self
            .devices
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE device store lock poisoned"))?;
        let mut next = devices.clone();
        next.insert(device.id.clone(), device.clone());
        let mut observations = self
            .state_observations
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE observation cache lock poisoned"))?;
        self.persist_locked(&next)?;
        *devices = next;
        // A physical read may have begun against the replaced metadata after
        // the first invalidation. Advance the epoch again so it cannot land as
        // a fresh observation after this upsert commits. Keep the device lock
        // through this final cache bump so readers cannot observe a window
        // between the metadata replacement and its new observation epoch.
        Self::invalidate_state_observation_locked(&mut observations, &device.id);
        drop(observations);
        drop(devices);
        // Commit the association only after devices.json is durable. Recovery
        // finishes this exact step when power fails between the two renames.
        self.clear_matching_tombstone(Some(&device))?;
        Ok(())
    }

    pub(crate) fn begin_reassociation_if_needed(&self, device: &HueBleDevice) -> Result<bool> {
        let mut document = self
            .tombstones
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE tombstone store lock poisoned"))?;
        let quarantined = document.devices.iter().any(|existing| {
            existing.id == device.id || existing.address.eq_ignore_ascii_case(&device.address)
        }) || document
            .pairing_retry_addresses
            .iter()
            .any(|address| address.eq_ignore_ascii_case(&device.address))
            || document
                .blocked_native_ids
                .iter()
                .any(|id| id == &device.id)
            || document
                .removed_native_ids
                .iter()
                .any(|id| id == &device.id);
        if !quarantined {
            return Ok(false);
        }
        if document
            .reassociation_in_progress_ids
            .iter()
            .any(|pending| pending == &device.id)
        {
            return Ok(true);
        }
        let mut next = document.clone();
        next.reassociation_in_progress_ids.push(device.id.clone());
        next.reassociation_in_progress_ids.sort();
        next.reassociation_in_progress_ids.dedup();
        self.persist_tombstones(&next)?;
        *document = next;
        Ok(true)
    }

    pub(crate) fn finish_reassociation(&self, device: &HueBleDevice) -> Result<()> {
        self.clear_matching_tombstone(Some(device))
    }

    /// Persist a bond quarantine before removing live metadata. The ordering
    /// is deliberate: a crash may temporarily leave both records, but can
    /// never leave neither while BlueZ still has the bond.
    pub fn tombstone_and_remove(&self, device: &HueBleDevice) -> Result<()> {
        self.record_tombstone(device.clone())?;
        self.remove(&device.id)?;
        Ok(())
    }

    /// Finish an explicit forced removal after the live adapter verified that
    /// it no longer owns this exact bond. The caller must first remove active
    /// metadata through [`Self::tombstone_and_remove`] so a crash can only
    /// leave the conservative tombstone, never an unjournaled transition.
    pub fn acknowledge_absent_bond_removal(&self, device: &HueBleDevice) -> Result<()> {
        if self.get(&device.id).is_some() {
            anyhow::bail!(
                "Cannot acknowledge absent Hue bond {} while active metadata remains",
                device.id
            );
        }
        let mut document = self
            .tombstones
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE tombstone store lock poisoned"))?;
        let mut next = document.clone();
        next.devices.retain(|existing| {
            existing.id != device.id && !existing.address.eq_ignore_ascii_case(&device.address)
        });
        next.prepared_handoff_ids
            .retain(|prepared| prepared != &device.id);
        next.blocked_native_ids.retain(|id| id != &device.id);
        next.pairing_retry_addresses
            .retain(|address| !address.eq_ignore_ascii_case(&device.address));
        next.reassociation_in_progress_ids
            .retain(|pending| pending != &device.id);
        if !next
            .removed_native_ids
            .iter()
            .any(|removed| removed == &device.id)
        {
            next.removed_native_ids.push(device.id.clone());
            next.removed_native_ids.sort();
            next.removed_native_ids.dedup();
        }
        self.persist_tombstones(&next)?;
        *document = next;
        Ok(())
    }

    /// Journal a successful authenticated pairing-handoff write before the
    /// Pi-side BlueZ key is deleted.
    pub fn record_prepared_handoff(&self, device: HueBleDevice) -> Result<()> {
        let mut document = self
            .tombstones
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE tombstone store lock poisoned"))?;
        let mut next = document.clone();
        next.devices.retain(|existing| {
            existing.id != device.id && !existing.address.eq_ignore_ascii_case(&device.address)
        });
        next.blocked_native_ids.retain(|id| id != &device.id);
        next.removed_native_ids.retain(|id| id != &device.id);
        next.pairing_retry_addresses
            .retain(|address| !address.eq_ignore_ascii_case(&device.address));
        next.reassociation_in_progress_ids
            .retain(|pending| pending != &device.id);
        next.devices.push(device.clone());
        next.devices.sort_by(|left, right| left.id.cmp(&right.id));
        if !next
            .prepared_handoff_ids
            .iter()
            .any(|prepared| prepared == &device.id)
        {
            next.prepared_handoff_ids.push(device.id);
            next.prepared_handoff_ids.sort();
        }
        self.persist_tombstones(&next)?;
        *document = next;
        Ok(())
    }

    pub fn record_tombstone(&self, device: HueBleDevice) -> Result<()> {
        let mut document = self
            .tombstones
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE tombstone store lock poisoned"))?;
        let mut next = document.clone();
        next.devices.retain(|existing| {
            existing.id != device.id && !existing.address.eq_ignore_ascii_case(&device.address)
        });
        next.blocked_native_ids.retain(|id| id != &device.id);
        next.removed_native_ids.retain(|id| id != &device.id);
        next.pairing_retry_addresses
            .retain(|address| !address.eq_ignore_ascii_case(&device.address));
        next.reassociation_in_progress_ids
            .retain(|pending| pending != &device.id);
        next.prepared_handoff_ids
            .retain(|prepared| prepared != &device.id);
        next.devices.push(device);
        next.devices.sort_by(|left, right| left.id.cmp(&right.id));
        self.persist_tombstones(&next)?;
        *document = next;
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<Option<HueBleDevice>> {
        self.invalidate_state_observation(id)?;
        let mut devices = self
            .devices
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE device store lock poisoned"))?;
        let mut next = devices.clone();
        let removed = next.remove(id);
        let mut observations = self
            .state_observations
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE observation cache lock poisoned"))?;
        if removed.is_some() {
            self.persist_locked(&next)?;
            *devices = next;
        }
        // Close the race with a physical read that began after the first
        // invalidation but before durable removal completed. Device-to-cache
        // is the same lock order used by observation commits.
        Self::invalidate_state_observation_locked(&mut observations, id);
        drop(observations);
        drop(devices);
        Ok(removed)
    }

    /// Capture one device incarnation and its observation epoch atomically
    /// immediately before beginning a physical GATT read. A command or
    /// metadata replacement advances the epoch, causing overlapping reads to
    /// be discarded rather than renewed as authoritative state.
    pub(crate) fn begin_state_observation(&self, id: &str) -> Result<(HueBleDevice, u64)> {
        let devices = self
            .devices
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE device store lock poisoned"))?;
        let device = devices
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown Hue BLE device: {id}"))?;
        let cache = self
            .state_observations
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE observation cache lock poisoned"))?;
        let token = cache.generations.get(id).copied().unwrap_or_default();
        Ok((device, token))
    }

    /// Record a physical read without touching durable storage. Returns false
    /// when a command overlapped the read and the observation was discarded.
    ///
    /// Rhythm can change a bulb every few seconds; persisting each observed
    /// brightness/CT value would needlessly wear appliance flash. Bond and
    /// capability metadata remain transactional through `upsert`.
    pub(crate) fn record_state_observation(
        &self,
        id: &str,
        token: u64,
        state: HueBleState,
    ) -> Result<bool> {
        let mut devices = self
            .devices
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE device store lock poisoned"))?;
        let Some(device) = devices.get_mut(id) else {
            anyhow::bail!("Unknown Hue BLE device: {id}");
        };
        let mut cache = self
            .state_observations
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE observation cache lock poisoned"))?;
        let generation = cache.generations.get(id).copied().unwrap_or_default();
        let command_in_flight = cache
            .commands_in_flight
            .get(id)
            .copied()
            .unwrap_or_default()
            > 0;
        if generation != token || command_in_flight {
            return Ok(false);
        }
        device.last_state = Some(state);
        cache.states.insert(
            id.to_string(),
            TimedStateObservation {
                state,
                observed_at: Instant::now(),
            },
        );
        Ok(true)
    }

    pub(crate) fn fresh_state_observation(&self, id: &str) -> Result<Option<HueBleState>> {
        let cache = self
            .state_observations
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE observation cache lock poisoned"))?;
        if cache
            .commands_in_flight
            .get(id)
            .copied()
            .unwrap_or_default()
            > 0
        {
            return Ok(None);
        }
        Ok(cache.states.get(id).and_then(|observation| {
            (Instant::now().saturating_duration_since(observation.observed_at)
                <= STATE_OBSERVATION_MAX_AGE)
                .then_some(observation.state)
        }))
    }

    pub(crate) fn begin_command_observation(
        &self,
        id: &str,
    ) -> Result<HueBleCommandObservationGuard<'_>> {
        if !self
            .devices
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE device store lock poisoned"))?
            .contains_key(id)
        {
            anyhow::bail!("Unknown Hue BLE device: {id}");
        }
        let mut cache = self
            .state_observations
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE observation cache lock poisoned"))?;
        *cache.generations.entry(id.to_string()).or_default() += 1;
        *cache.commands_in_flight.entry(id.to_string()).or_default() += 1;
        cache.states.remove(id);
        drop(cache);
        Ok(HueBleCommandObservationGuard {
            store: self,
            id: id.to_string(),
        })
    }

    fn finish_command_observation(&self, id: &str) {
        let Ok(mut cache) = self.state_observations.lock() else {
            return;
        };
        *cache.generations.entry(id.to_string()).or_default() += 1;
        if let Some(in_flight) = cache.commands_in_flight.get_mut(id) {
            *in_flight = in_flight.saturating_sub(1);
            if *in_flight == 0 {
                cache.commands_in_flight.remove(id);
            }
        }
        cache.states.remove(id);
    }

    fn invalidate_state_observation(&self, id: &str) -> Result<()> {
        let mut cache = self
            .state_observations
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE observation cache lock poisoned"))?;
        Self::invalidate_state_observation_locked(&mut cache, id);
        Ok(())
    }

    fn invalidate_state_observation_locked(cache: &mut StateObservationCache, id: &str) {
        *cache.generations.entry(id.to_string()).or_default() += 1;
        cache.states.remove(id);
    }

    fn persist_locked(&self, devices: &BTreeMap<String, HueBleDevice>) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Hue BLE store path has no parent"))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        sync_parent_directory_entry(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("securing {}", parent.display()))?;
        }
        let document = StoreDocument {
            schema_version: SCHEMA_VERSION,
            devices: devices.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&document)?;
        let temporary = self.path.with_extension("json.tmp");
        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("opening {}", temporary.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("writing {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, &self.path).with_context(|| {
            format!(
                "replacing Hue BLE store {} with {}",
                self.path.display(),
                temporary.display()
            )
        })?;
        sync_directory(parent)?;
        Ok(())
    }

    fn clear_matching_tombstone(&self, device: Option<&HueBleDevice>) -> Result<()> {
        let Some(device) = device else {
            return Ok(());
        };
        let mut document = self
            .tombstones
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE tombstone store lock poisoned"))?;
        let mut next = document.clone();
        // A factory reset can rotate a bulb's random BLE address. Preserve
        // the old tombstone addresses long enough to clear their exact retry
        // allowances when the same stable EUI is committed at a new address.
        let matched_tombstone_addresses = document
            .devices
            .iter()
            .filter(|existing| {
                existing.id == device.id || existing.address.eq_ignore_ascii_case(&device.address)
            })
            .map(|existing| existing.address.clone())
            .collect::<Vec<_>>();
        next.devices.retain(|existing| {
            existing.id != device.id && !existing.address.eq_ignore_ascii_case(&device.address)
        });
        next.blocked_native_ids.retain(|id| id != &device.id);
        next.removed_native_ids.retain(|id| id != &device.id);
        next.pairing_retry_addresses.retain(|address| {
            !address.eq_ignore_ascii_case(&device.address)
                && !matched_tombstone_addresses
                    .iter()
                    .any(|old_address| old_address.eq_ignore_ascii_case(address))
        });
        next.reassociation_in_progress_ids
            .retain(|pending| pending != &device.id);
        next.prepared_handoff_ids
            .retain(|prepared| prepared != &device.id);
        if next.devices.len() == document.devices.len()
            && next.blocked_native_ids.len() == document.blocked_native_ids.len()
            && next.removed_native_ids.len() == document.removed_native_ids.len()
            && next.pairing_retry_addresses.len() == document.pairing_retry_addresses.len()
            && next.reassociation_in_progress_ids.len()
                == document.reassociation_in_progress_ids.len()
            && next.prepared_handoff_ids.len() == document.prepared_handoff_ids.len()
        {
            return Ok(());
        }
        self.persist_tombstones(&next)?;
        *document = next;
        Ok(())
    }

    fn persist_tombstones(&self, document: &TombstoneDocument) -> Result<()> {
        Self::persist_tombstones_at(&self.tombstone_path, document)
    }

    fn persist_tombstones_at(path: &Path, document: &TombstoneDocument) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Hue BLE tombstone path has no parent"))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        sync_parent_directory_entry(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("securing {}", parent.display()))?;
        }
        let mut current_document = document.clone();
        current_document.schema_version = TOMBSTONE_SCHEMA_VERSION;
        let bytes = serde_json::to_vec_pretty(&current_document)?;
        let temporary = path.with_extension("json.tmp");
        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("opening {}", temporary.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("writing {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, path).with_context(|| {
            format!(
                "replacing Hue BLE tombstones {} with {}",
                path.display(),
                temporary.display()
            )
        })?;
        sync_directory(parent)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::types::HueBleCapabilities;

    fn test_device() -> HueBleDevice {
        HueBleDevice {
            id: "hue-ble-001788010c765ba7".to_string(),
            address: "EB:01:B4:6B:01:34".to_string(),
            address_type: "random".to_string(),
            eui64: "001788010c765ba7".to_string(),
            name: "Hue white lamp".to_string(),
            manufacturer: "Signify Netherlands B.V.".to_string(),
            model: "LWA003".to_string(),
            firmware: "1.76.8".to_string(),
            capabilities: HueBleCapabilities {
                dimming: true,
                ..Default::default()
            },
            paired_at_epoch_secs: 123,
            last_state: None,
        }
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        let unique = format!(
            "rhythm-hue-ble-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn paired_device_metadata_survives_reload() {
        let unique = format!(
            "rhythm-hue-ble-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.upsert(test_device()).unwrap();

        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(reloaded.all().len(), 1);
        assert_eq!(reloaded.get(&test_device().id).unwrap().model, "LWA003");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_persistence_does_not_commit_in_memory_mutations() {
        let unique = format!(
            "rhythm-hue-ble-store-failure-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        let blocked_parent = dir.join("not-a-directory");
        fs::write(&blocked_parent, "blocked").unwrap();
        let store = HueBleDeviceStore {
            path: blocked_parent.join("devices.json"),
            devices: Mutex::new(BTreeMap::new()),
            state_observations: Mutex::new(StateObservationCache::default()),
            tombstone_path: blocked_parent.join("unpaired_bonds.json"),
            tombstones: Mutex::new(TombstoneDocument {
                schema_version: TOMBSTONE_SCHEMA_VERSION,
                ..Default::default()
            }),
            factory_reset_blocked: false,
        };

        assert!(store.upsert(test_device()).is_err());
        assert!(store.all().is_empty());

        store
            .devices
            .lock()
            .unwrap()
            .insert(test_device().id.clone(), test_device());
        assert!(store.remove(&test_device().id).is_err());
        assert!(store.get(&test_device().id).is_some());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn live_state_cache_does_not_rewrite_device_metadata() {
        let unique = format!(
            "rhythm-hue-ble-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let store = HueBleDeviceStore::load(&dir).unwrap();
        let device = test_device();
        store.upsert(device.clone()).unwrap();
        let before = fs::read(&store.path).unwrap();

        let state = HueBleState {
            on: Some(true),
            brightness: Some(127),
            color: None,
            effect: None,
        };
        let (_, token) = store.begin_state_observation(&device.id).unwrap();
        assert!(store
            .record_state_observation(&device.id, token, state)
            .unwrap());

        assert_eq!(store.get(&device.id).unwrap().last_state, Some(state));
        assert_eq!(
            store.fresh_state_observation(&device.id).unwrap(),
            Some(state)
        );
        assert_eq!(fs::read(&store.path).unwrap(), before);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reloaded_last_state_is_not_a_fresh_observation() {
        let dir = unique_test_dir("observation-reload");
        let store = HueBleDeviceStore::load(&dir).unwrap();
        let mut device = test_device();
        device.last_state = Some(HueBleState {
            on: Some(true),
            ..Default::default()
        });
        store.upsert(device.clone()).unwrap();

        let reloaded = HueBleDeviceStore::load(&dir).unwrap();

        assert_eq!(
            reloaded.get(&device.id).unwrap().last_state,
            device.last_state
        );
        assert_eq!(reloaded.fresh_state_observation(&device.id).unwrap(), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn command_epoch_rejects_reads_started_before_or_during_dispatch() {
        let dir = unique_test_dir("observation-command-epoch");
        let store = HueBleDeviceStore::load(&dir).unwrap();
        let device = test_device();
        store.upsert(device.clone()).unwrap();
        let (_, before_command) = store.begin_state_observation(&device.id).unwrap();

        let guard = store.begin_command_observation(&device.id).unwrap();
        let (_, during_command) = store.begin_state_observation(&device.id).unwrap();
        assert!(!store
            .record_state_observation(
                &device.id,
                before_command,
                HueBleState {
                    on: Some(false),
                    ..Default::default()
                },
            )
            .unwrap());
        assert!(!store
            .record_state_observation(
                &device.id,
                during_command,
                HueBleState {
                    on: Some(true),
                    ..Default::default()
                },
            )
            .unwrap());
        drop(guard);
        assert_eq!(store.fresh_state_observation(&device.id).unwrap(), None);

        let (_, after_command) = store.begin_state_observation(&device.id).unwrap();
        let observed = HueBleState {
            on: Some(true),
            ..Default::default()
        };
        assert!(store
            .record_state_observation(&device.id, after_command, observed)
            .unwrap());
        assert_eq!(
            store.fresh_state_observation(&device.id).unwrap(),
            Some(observed)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn metadata_replacement_rejects_prior_incarnation_observation() {
        let dir = unique_test_dir("observation-replacement");
        let store = HueBleDeviceStore::load(&dir).unwrap();
        let device = test_device();
        store.upsert(device.clone()).unwrap();
        let (stale_device, stale_token) = store.begin_state_observation(&device.id).unwrap();
        let mut replacement = device.clone();
        replacement.address = "AA:BB:CC:DD:EE:FF".to_string();
        store.upsert(replacement.clone()).unwrap();

        assert!(!store
            .record_state_observation(
                &stale_device.id,
                stale_token,
                HueBleState {
                    on: Some(true),
                    ..Default::default()
                },
            )
            .unwrap());
        assert_eq!(store.get(&device.id).unwrap().address, replacement.address);
        assert_eq!(store.fresh_state_observation(&device.id).unwrap(), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_observation_is_not_fresh() {
        let dir = unique_test_dir("observation-stale");
        let store = HueBleDeviceStore::load(&dir).unwrap();
        let device = test_device();
        store.upsert(device.clone()).unwrap();
        let (_, token) = store.begin_state_observation(&device.id).unwrap();
        store
            .record_state_observation(
                &device.id,
                token,
                HueBleState {
                    on: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .state_observations
            .lock()
            .unwrap()
            .states
            .get_mut(&device.id)
            .unwrap()
            .observed_at = Instant::now() - STATE_OBSERVATION_MAX_AGE - Duration::from_secs(1);

        assert_eq!(store.fresh_state_observation(&device.id).unwrap(), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn force_load_quarantines_malformed_json_but_keeps_it_recoverable() {
        let unique = format!(
            "rhythm-hue-ble-corrupt-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let store_dir = dir.join("hue_ble");
        fs::create_dir_all(&store_dir).unwrap();
        let path = store_dir.join("devices.json");
        fs::write(&path, b"{ definitely not valid json").unwrap();

        let (store, quarantined) = HueBleDeviceStore::load_recovering_corrupt(&dir).unwrap();

        let quarantined = quarantined.expect("malformed store should be quarantined");
        assert!(store.all().is_empty());
        assert!(store.blocks_paired_orphan_adoption());
        assert!(!path.exists());
        assert_eq!(
            fs::read(&quarantined).unwrap(),
            b"{ definitely not valid json"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn factory_reset_orphan_block_is_durable_without_device_metadata() {
        let unique = format!(
            "rhythm-hue-ble-factory-reset-block-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.upsert(test_device()).unwrap();

        HueBleDeviceStore::install_paired_orphan_adoption_block(&dir).unwrap();
        fs::remove_dir_all(dir.join("hue_ble")).unwrap();

        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert!(reloaded.all().is_empty());
        assert!(reloaded.blocks_paired_orphan_adoption());
        assert!(
            !reloaded.has_unknown_bond_quarantine(),
            "an interrupted-reset marker must block scanning without making a reset retry impossible"
        );

        HueBleDeviceStore::clear_paired_orphan_adoption_block(&dir).unwrap();
        let cleared = HueBleDeviceStore::load(&dir).unwrap();
        assert!(!cleared.blocks_paired_orphan_adoption());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn factory_reset_plan_survives_hue_store_removal_until_platform_cleanup() {
        let dir = unique_test_dir("factory-reset-plan");
        let device = test_device();
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.upsert(device.clone()).unwrap();

        let plan = HueBleFactoryResetPlan {
            devices: vec![device.clone()],
            prepared_device_ids: vec![device.id.clone()],
            ..Default::default()
        };
        HueBleDeviceStore::persist_factory_reset_plan(&dir, &plan).unwrap();
        fs::remove_dir_all(dir.join("hue_ble")).unwrap();

        let reloaded = HueBleDeviceStore::load_factory_reset_plan(&dir).unwrap();
        assert_eq!(reloaded.devices, plan.devices);
        assert_eq!(reloaded.prepared_device_ids, plan.prepared_device_ids);
        assert!(reloaded.released_device_ids.is_empty());
        HueBleDeviceStore::clear_factory_reset_plan(&dir).unwrap();
        assert!(HueBleDeviceStore::load_factory_reset_plan(&dir)
            .unwrap()
            .devices
            .is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fresh_pair_clears_matching_missing_metadata_id_but_keeps_global_block() {
        let unique = format!(
            "rhythm-hue-ble-missing-id-block-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let device = test_device();
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.record_missing_metadata_removal(&device.id).unwrap();
        assert_eq!(store.blocked_native_ids(), vec![device.id.clone()]);

        store.upsert(device).unwrap();

        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert!(reloaded.blocked_native_ids().is_empty());
        assert!(reloaded.blocks_paired_orphan_adoption());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forced_removal_tombstone_survives_restart_until_fresh_upsert() {
        let unique = format!(
            "rhythm-hue-ble-tombstone-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let device = test_device();
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.upsert(device.clone()).unwrap();

        store.tombstone_and_remove(&device).unwrap();

        assert!(store.get(&device.id).is_none());
        assert_eq!(store.tombstone(&device.id), Some(device.clone()));
        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert!(reloaded.get(&device.id).is_none());
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));

        reloaded.upsert(device.clone()).unwrap();
        assert_eq!(reloaded.get(&device.id), Some(device));
        assert!(reloaded.tombstones().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pairing_retry_address_survives_reload_and_clears_after_validated_upsert() {
        let dir = unique_test_dir("pairing-retry-address");
        let device = test_device();
        let store = HueBleDeviceStore::load(&dir).unwrap();

        store
            .record_pairing_retry_address(&device.address.to_ascii_lowercase())
            .unwrap();
        store.record_pairing_retry_address(&device.address).unwrap();

        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(
            reloaded.pairing_retry_addresses(),
            vec![device.address.clone()]
        );

        reloaded.upsert(device.clone()).unwrap();

        let committed = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(committed.get(&device.id), Some(device));
        assert!(committed.pairing_retry_addresses().is_empty());
        assert!(committed.reassociation_in_progress_ids().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stable_identity_reassociation_clears_old_rotated_address_retry_only() {
        let dir = unique_test_dir("pairing-rotated-address");
        let old_device = test_device();
        let mut reassociated = old_device.clone();
        reassociated.address = "EA:84:C2:50:A8:65".to_string();
        let unrelated_retry = "AA:00:00:00:00:77";
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.record_tombstone(old_device.clone()).unwrap();
        store
            .record_pairing_retry_address(&old_device.address)
            .unwrap();
        store.record_pairing_retry_address(unrelated_retry).unwrap();

        store.upsert(reassociated.clone()).unwrap();

        let committed = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(committed.get(&reassociated.id), Some(reassociated));
        assert!(committed.tombstone(&old_device.id).is_none());
        assert_eq!(
            committed.pairing_retry_addresses(),
            vec![unrelated_retry.to_string()],
            "a stable-EUI match clears the old random address without touching another bulb"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn absent_bond_acknowledgement_keeps_cleanup_marker_until_fresh_upsert() {
        let dir = unique_test_dir("absent-bond-acknowledgement");
        let device = test_device();
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.upsert(device.clone()).unwrap();
        store.tombstone_and_remove(&device).unwrap();

        store.acknowledge_absent_bond_removal(&device).unwrap();

        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert!(reloaded.get(&device.id).is_none());
        assert!(reloaded.tombstone(&device.id).is_none());
        assert_eq!(reloaded.removed_native_ids(), vec![device.id.clone()]);
        assert!(!reloaded.blocks_paired_orphan_adoption());

        reloaded.upsert(device.clone()).unwrap();
        let committed = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(committed.get(&device.id), Some(device));
        assert!(committed.removed_native_ids().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interrupted_reassociation_keeps_quarantine_until_active_commit_is_finished() {
        let dir = unique_test_dir("reassociation-transaction");
        let device = test_device();
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.record_tombstone(device.clone()).unwrap();
        assert!(store.begin_reassociation_if_needed(&device).unwrap());

        // Simulate power loss after devices.json was renamed but before the
        // tombstone transaction could be committed.
        {
            let mut devices = store.devices.lock().unwrap();
            let mut next = devices.clone();
            next.insert(device.id.clone(), device.clone());
            store.persist_locked(&next).unwrap();
            *devices = next;
        }

        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(reloaded.get(&device.id), Some(device.clone()));
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));
        assert!(reloaded.reassociation_was_started(&device.id));

        reloaded.finish_reassociation(&device).unwrap();

        let committed = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(committed.get(&device.id), Some(device.clone()));
        assert!(committed.tombstone(&device.id).is_none());
        assert!(!committed.reassociation_was_started(&device.id));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prepared_handoff_journal_survives_reload_as_current_schema() {
        let dir = unique_test_dir("prepared-handoff-reload");
        let device = test_device();
        let store = HueBleDeviceStore::load(&dir).unwrap();

        store.record_prepared_handoff(device.clone()).unwrap();

        assert_eq!(store.tombstone(&device.id), Some(device.clone()));
        assert!(store.handoff_was_prepared(&device.id));
        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));
        assert!(reloaded.handoff_was_prepared(&device.id));
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&reloaded.tombstone_path).unwrap()).unwrap();
        assert_eq!(
            persisted["schema_version"].as_u64(),
            Some(u64::from(TOMBSTONE_SCHEMA_VERSION))
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fresh_upsert_clears_prepared_handoff_marker_and_tombstone() {
        let dir = unique_test_dir("prepared-handoff-upsert");
        let device = test_device();
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.record_prepared_handoff(device.clone()).unwrap();

        store.upsert(device.clone()).unwrap();

        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(reloaded.get(&device.id), Some(device.clone()));
        assert!(reloaded.tombstone(&device.id).is_none());
        assert!(!reloaded.handoff_was_prepared(&device.id));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ordinary_tombstone_converts_prepared_handoff_to_retained_removal() {
        let dir = unique_test_dir("prepared-handoff-force");
        let device = test_device();
        let store = HueBleDeviceStore::load(&dir).unwrap();
        store.record_prepared_handoff(device.clone()).unwrap();

        store.record_tombstone(device.clone()).unwrap();

        let reloaded = HueBleDeviceStore::load(&dir).unwrap();
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));
        assert!(!reloaded.handoff_was_prepared(&device.id));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn v1_tombstones_load_as_ordinary_and_upgrade_on_next_write() {
        let dir = unique_test_dir("v1-tombstone-migration");
        let store_dir = dir.join("hue_ble");
        fs::create_dir_all(&store_dir).unwrap();
        let path = store_dir.join("unpaired_bonds.json");
        let device = test_device();
        let v1 = serde_json::json!({
            "schema_version": 1,
            "devices": [device.clone()],
            // A schema-v1 marker is deliberately ignored: v1 never defined a
            // trustworthy crash journal, even if a prerelease emitted it.
            "prepared_handoff_ids": [device.id.clone()],
            "blocked_native_ids": [],
            "block_paired_orphan_adoption": false,
        });
        fs::write(&path, serde_json::to_vec_pretty(&v1).unwrap()).unwrap();

        let store = HueBleDeviceStore::load(&dir).unwrap();

        assert_eq!(store.tombstone(&device.id), Some(device.clone()));
        assert!(!store.handoff_was_prepared(&device.id));

        store.record_tombstone(device.clone()).unwrap();
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            persisted["schema_version"].as_u64(),
            Some(u64::from(TOMBSTONE_SCHEMA_VERSION))
        );
        assert_eq!(persisted["prepared_handoff_ids"], serde_json::json!([]));
        assert_eq!(persisted["devices"], serde_json::json!([device]));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn force_load_does_not_quarantine_a_newer_schema() {
        let unique = format!(
            "rhythm-hue-ble-newer-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let store_dir = dir.join("hue_ble");
        fs::create_dir_all(&store_dir).unwrap();
        let path = store_dir.join("devices.json");
        let contents = format!(
            r#"{{"schema_version":{},"devices":[]}}"#,
            SCHEMA_VERSION + 1
        );
        fs::write(&path, &contents).unwrap();

        let error = HueBleDeviceStore::load_recovering_corrupt(&dir)
            .err()
            .expect("newer schema should fail closed");

        assert!(format!("{error:#}").contains("newer than supported"));
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        assert_eq!(fs::read_dir(&store_dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
