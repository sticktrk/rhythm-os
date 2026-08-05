//! Shared lifecycle and canonical-device orchestration for Hue BLE.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use log::warn;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::api_types::{LightCapabilitiesDto, LightColorTemperatureCapabilitiesDto};
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::pairing::{
    PairedDeviceInfo, PairingSession, PairingStage, PairingStatus, UnpairingCompletionScope,
    UnpairingResult,
};
use rhythm_os::state::SharedState;

use super::discovery::HueBleDiscovery;
use super::store::HueBleDeviceStore;
use super::transport::{HueBleAdapterAvailability, HueBleTransport};
use super::types::{HueBleDevice, HueBlePairingOutcome, HueBlePairingRequest};
use super::{BLE_CONNECTION_POOL_CAPACITY, HUB_ADDRESS, HUB_TYPE};

const OBSERVATION_INTERVAL: Duration = Duration::from_secs(15);
const SHUTDOWN_POLL: Duration = Duration::from_millis(250);
const PAIRING_HANDOFF_FRESHNESS_BUDGET: Duration = Duration::from_secs(10);
/// Serializes recovery, pairing, and unpairing across duplicate transports so
/// a stale recovery worker cannot remove a freshly recreated bond.
static BOND_LIFECYCLE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdapterConnectionTransition {
    None,
    Connected,
    Disconnected,
}

fn adapter_connection_transition(
    connected: bool,
    observation: HueBleAdapterAvailability,
) -> (bool, AdapterConnectionTransition) {
    match observation {
        HueBleAdapterAvailability::Available if !connected => {
            (true, AdapterConnectionTransition::Connected)
        }
        HueBleAdapterAvailability::Unavailable if connected => {
            (false, AdapterConnectionTransition::Disconnected)
        }
        // Foreground pairing or control work owning admission says nothing
        // about adapter health. Preserve the last observed connection state.
        HueBleAdapterAvailability::Busy => (connected, AdapterConnectionTransition::None),
        _ => (connected, AdapterConnectionTransition::None),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HueBleFactoryResetHandoffBatch {
    pub refreshed: usize,
    /// The final adapter scrub must begin before this instant. `None` means
    /// every planned key was already absent under durable release evidence.
    pub valid_until: Option<Instant>,
}

pub struct HueBleHubData {
    pub transport: Arc<dyn HueBleTransport>,
    pub store: Arc<HueBleDeviceStore>,
    pub registry: Arc<Mutex<rhythm_os::registry::HubDeviceRegistry>>,
    pub event_tx: Sender<HubEvent>,
    observer_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

/// A retained handle to the live integration resources that shared factory
/// reset removes from `AppState` before platform key cleanup begins.
pub struct HueBleFactoryResetQuiescence {
    shutdown: Arc<AtomicBool>,
    transport: Arc<dyn HueBleTransport>,
    observer_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl HueBleFactoryResetQuiescence {
    /// Permanently close the old transport to new work, wait for current BlueZ
    /// I/O, and join the observer before a separate final-handoff transport is
    /// opened.
    pub fn quiesce(self) -> Result<()> {
        self.shutdown.store(true, Ordering::SeqCst);
        let transport_result = self
            .transport
            .quiesce()
            .context("quiescing the live Hue BLE transport");
        let observer_result = (|| {
            let observer = self
                .observer_thread
                .lock()
                .map_err(|_| anyhow::anyhow!("Hue BLE observer thread lock poisoned"))?
                .take();
            if let Some(observer) = observer {
                observer
                    .join()
                    .map_err(|_| anyhow::anyhow!("Hue BLE observer panicked during shutdown"))?;
            }
            Ok(())
        })();
        transport_result?;
        observer_result
    }
}

pub fn capture_factory_reset_quiescence(
    state: &SharedState,
) -> Result<Option<HueBleFactoryResetQuiescence>> {
    let key = hub_key();
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let Some(hub) = state.hubs.get(&key) else {
        return Ok(None);
    };
    let data = hub
        .data::<Arc<HueBleHubData>>()
        .ok_or_else(|| anyhow::anyhow!("Active Hue BLE hub has incompatible lifecycle data"))?;
    Ok(Some(HueBleFactoryResetQuiescence {
        shutdown: hub.shutdown.clone(),
        transport: data.transport.clone(),
        observer_thread: data.observer_thread.clone(),
    }))
}

pub fn hub_key() -> HubKey {
    HubKey::new(HubType::new(HUB_TYPE), HUB_ADDRESS)
}

pub fn data_dir(state: &SharedState) -> Result<String> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if state.data_dir.trim().is_empty() {
        anyhow::bail!("data_dir not configured on AppState");
    }
    Ok(state.data_dir.clone())
}

pub fn get_hub_data(state: &SharedState) -> Result<Arc<HueBleHubData>> {
    let key = hub_key();
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state
        .hubs
        .get(&key)
        .and_then(|hub| hub.data::<Arc<HueBleHubData>>())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Hue Bluetooth integration is not connected"))
}

pub fn connect(
    state: &SharedState,
    transport: Arc<dyn HueBleTransport>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let available = transport
        .is_available()
        .context("Hue Bluetooth adapter is unavailable")?;
    if !available {
        anyhow::bail!("Hue Bluetooth adapter is unavailable");
    }
    let store = {
        let _bond_guard = BOND_LIFECYCLE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE bond lifecycle lock poisoned"))?;
        // Load only after acquiring the global lock. A duplicate connector
        // must not retain a stale tombstone snapshot while a live pairing
        // clears that tombstone and creates a fresh bond.
        let store = Arc::new(HueBleDeviceStore::load(data_dir(state)?)?);
        recover_tombstoned_removals(state, transport.as_ref(), store.as_ref());
        store
    };
    let key = hub_key();
    let removed_ids = store
        .tombstones()
        .into_iter()
        .map(|device| device.id)
        .chain(store.blocked_native_ids())
        .chain(store.removed_native_ids())
        .collect::<HashSet<_>>();
    let snapshot = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        state
            .storage
            .as_ref()
            .and_then(|storage| storage.load_hub_registry_for(&key).ok().flatten())
            .and_then(|value| {
                serde_json::from_value::<rhythm_os::registry::RegistrySnapshot>(value).ok()
            })
            .map(|mut snapshot| {
                filter_removed_from_snapshot(&mut snapshot, &removed_ids);
                snapshot
            })
    };

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let transport_for_data = transport.clone();
    let store_for_data = store.clone();
    let event_tx_for_data = event_tx.clone();
    let observer_transport = transport.clone();
    let observer_store = store.clone();
    let observer_state = state.clone();
    let observer_thread = Arc::new(Mutex::new(None));
    let observer_thread_for_data = observer_thread.clone();
    let observer_thread_for_stream = observer_thread.clone();

    let (mut hub, event_rx) = rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new(HUB_TYPE),
        key,
        true,
        snapshot,
        move |registry| {
            Box::new(Arc::new(HueBleHubData {
                transport: transport_for_data,
                store: store_for_data,
                registry,
                event_tx: event_tx_for_data,
                observer_thread: observer_thread_for_data,
            }))
        },
        move |_registry, shutdown| {
            let observer_shutdown = shutdown.clone();
            match start_observer(
                observer_state,
                observer_transport,
                observer_store,
                event_tx,
                shutdown,
            ) {
                Ok(observer) => match observer_thread_for_stream.lock() {
                    Ok(mut slot) => *slot = Some(observer),
                    Err(_) => {
                        observer_shutdown.store(true, Ordering::SeqCst);
                        let _ = observer.join();
                        warn!(
                            target: "evt",
                            "Hue BLE observer thread lock was poisoned during startup"
                        );
                    }
                },
                Err(error) => {
                    warn!(target: "evt", "Failed to start Hue BLE state observer: {error:#}");
                }
            }
            event_rx
        },
    )?;
    hub.discovery = Some(Arc::new(HueBleDiscovery::new(store)));
    Ok((hub, event_rx))
}

fn filter_removed_from_snapshot(
    snapshot: &mut rhythm_os::registry::RegistrySnapshot,
    removed_ids: &HashSet<String>,
) {
    snapshot
        .rooms
        .retain(|room| !removed_ids.contains(&room.id));
    snapshot
        .devices
        .retain(|device| !removed_ids.contains(&device.id));
    snapshot
        .buttons
        .retain(|_, (device_id, _)| !removed_ids.contains(device_id));
    for light_ids in snapshot.area_lights.values_mut() {
        light_ids.retain(|id| !removed_ids.contains(id));
    }
    snapshot
        .area_lights
        .retain(|room_id, _| !removed_ids.contains(room_id));
}

fn start_observer(
    state: SharedState,
    transport: Arc<dyn HueBleTransport>,
    store: Arc<HueBleDeviceStore>,
    event_tx: Sender<HubEvent>,
    shutdown: Arc<AtomicBool>,
) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("hue-ble-state".to_string())
        .spawn(move || {
            let _ = event_tx.send(HubEvent::Connected { hub_key: None });
            prewarm_known_devices(transport.as_ref(), store.as_ref(), shutdown.as_ref());
            let mut previous_power = HashMap::<String, bool>::new();
            let mut adapter_connected = true;

            while !shutdown.load(Ordering::Relaxed) {
                let availability = transport.probe_availability().unwrap_or_else(|error| {
                    warn!(target: "evt", "Hue BLE adapter health probe failed: {error:#}");
                    HueBleAdapterAvailability::Unavailable
                });
                let (next_connected, transition) =
                    adapter_connection_transition(adapter_connected, availability);
                match transition {
                    AdapterConnectionTransition::Connected => {
                        if let Ok(_bond_guard) = BOND_LIFECYCLE_LOCK.lock() {
                            recover_tombstoned_removals(&state, transport.as_ref(), store.as_ref());
                        }
                        let _ = event_tx.send(HubEvent::Connected { hub_key: None });
                    }
                    AdapterConnectionTransition::Disconnected => {
                        let _ = event_tx.send(HubEvent::Disconnected {
                            hub_key: None,
                            reason: "Bluetooth adapter is unavailable".to_string(),
                        });
                    }
                    AdapterConnectionTransition::None => {}
                }
                adapter_connected = next_connected;

                let devices = store.all();
                for listed_device in devices {
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    let (device, observation_token) =
                        match store.begin_state_observation(&listed_device.id) {
                            Ok(observation) => observation,
                            Err(error) => {
                                warn!(
                                    target: "cmd",
                                    "Hue BLE could not begin passive state observation for {}: {error:#}",
                                    listed_device.display_name()
                                );
                                continue;
                            }
                        };
                    match transport.read_state_passive(&device) {
                        Ok(Some(state)) => {
                            let accepted = store
                                .record_state_observation(&device.id, observation_token, state)
                                .unwrap_or(false);
                            if accepted {
                                if let Some(on) = state.on {
                                    let changed =
                                        previous_power.insert(device.id.clone(), on) != Some(on);
                                    if changed {
                                        let _ = event_tx.send(HubEvent::LightPower {
                                            hub_key: None,
                                            device_id: device.id,
                                            lights_on: on,
                                        });
                                    }
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            warn!(
                                target: "evt",
                                "Hue BLE state read failed for {}: {error:#}",
                                device.id
                            );
                        }
                    }
                }

                if availability == HueBleAdapterAvailability::Available {
                    let _ = event_tx.send(HubEvent::Heartbeat { hub_key: None });
                }

                if wait_for_shutdown(&shutdown, OBSERVATION_INTERVAL) {
                    break;
                }
            }
        })
        .context("spawning Hue BLE state observer")
}

fn prewarm_known_devices(
    transport: &dyn HueBleTransport,
    store: &HueBleDeviceStore,
    shutdown: &AtomicBool,
) {
    if shutdown.load(Ordering::Relaxed) {
        return;
    }
    let devices = store.all();
    if let Err(error) = transport.initialize_connection_pool(&devices) {
        tracing::warn!(
            target: "cmd",
            event = "hue_ble_pool_initialize",
            outcome = "error",
            error = %error,
            "Hue BLE startup could not reconcile inherited connections"
        );
        return;
    }
    // Prewarming beyond the warm-link capacity would churn through the whole
    // durable catalog at every boot. Cold devices connect on first command.
    for device in devices.into_iter().take(BLE_CONNECTION_POOL_CAPACITY) {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        match transport.prewarm(&device) {
            Ok(true) => {}
            Ok(false) => tracing::debug!(
                target: "cmd",
                event = "hue_ble_prewarm",
                device_id = %device.id,
                outcome = "skipped",
                "Hue BLE startup prewarm skipped busy work"
            ),
            Err(error) => tracing::warn!(
                target: "cmd",
                event = "hue_ble_prewarm",
                device_id = %device.id,
                outcome = "error",
                error = %error,
                "Hue BLE startup prewarm could not prepare bulb"
            ),
        }
    }
}

fn wait_for_shutdown(shutdown: &AtomicBool, duration: Duration) -> bool {
    let deadline = std::time::Instant::now() + duration;
    while !shutdown.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(SHUTDOWN_POLL));
    }
    true
}

fn recover_tombstoned_removals(
    state: &SharedState,
    transport: &dyn HueBleTransport,
    store: &HueBleDeviceStore,
) {
    // A re-association writes this marker before devices.json and clears it
    // only after the active record is durable. Finish that transaction before
    // interpreting a simultaneous active record + tombstone as an interrupted
    // force-removal.
    for native_id in store.reassociation_in_progress_ids() {
        let Some(device) = store.get(&native_id) else {
            log::debug!(
                target: "pair",
                "Hue BLE re-association for {} has no active metadata yet; waiting for an explicit nearby scan",
                native_id
            );
            continue;
        };
        if let Err(error) = store.finish_reassociation(&device) {
            warn!(
                target: "pair",
                "Could not finish durable Hue BLE re-association for {}: {error:#}",
                native_id
            );
        }
    }
    for native_id in store
        .blocked_native_ids()
        .into_iter()
        .chain(store.removed_native_ids())
    {
        if let Err(error) =
            rhythm_os::commands::do_device_endpoint_remove(state, &native_id, &hub_key())
        {
            warn!(
                target: "pair",
                "Could not finish canonical cleanup for metadata-missing Hue BLE device {}: {error:#}",
                native_id
            );
        }
    }
    for device in store.tombstones() {
        if store.reassociation_was_started(&device.id) {
            // The active write either has not happened yet or its commit could
            // not be finalized above. In both cases this is association work,
            // never removal work.
            continue;
        }
        let active_metadata_present = store.get(&device.id).is_some();
        let can_finish_cleanup = if store.handoff_was_prepared(&device.id) {
            match transport.has_local_bond(&device) {
                Ok(false) => true,
                Ok(true) => {
                    if active_metadata_present {
                        log::debug!(
                            target: "pair",
                            "Hue BLE pairing handoff for {} still has a local bond; waiting for an explicit removal retry",
                            device.id
                        );
                        false
                    } else {
                        // Forced removal already committed its active-store
                        // deletion but intentionally retained the exact bond
                        // quarantine. Finish only a possibly interrupted
                        // canonical cleanup; store.remove below is a no-op and
                        // the prepared tombstone remains durable.
                        true
                    }
                }
                Err(error) => {
                    warn!(
                        target: "pair",
                        "Could not verify the pending Hue BLE bond removal for {}: {error:#}",
                        device.id
                    );
                    false
                }
            }
        } else {
            // An ordinary tombstone is a completed or explicitly forced local
            // forget. Never open a vendor pairing window from background
            // recovery; the exact retained bond may be re-adopted only by a
            // later user-triggered nearby scan.
            true
        };
        if !can_finish_cleanup {
            continue;
        }
        match store.remove(&device.id) {
            Ok(_) => {
                if let Err(error) =
                    rhythm_os::commands::do_device_endpoint_remove(state, &device.id, &hub_key())
                {
                    warn!(
                        target: "pair",
                        "Could not finish canonical cleanup for tombstoned Hue BLE device {}: {error:#}",
                        device.id
                    );
                }
            }
            Err(error) => warn!(
                target: "pair",
                "Could not finish metadata cleanup for tombstoned Hue BLE device {}: {error:#}",
                device.id
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BondReleaseOutcome {
    HandoffOpenedNow,
    PreviouslyReleased,
}

fn release_bond_with_journal(
    transport: &dyn HueBleTransport,
    store: &HueBleDeviceStore,
    device: &HueBleDevice,
) -> Result<BondReleaseOutcome> {
    let has_local_bond = transport.has_local_bond(device)?;
    if !has_local_bond {
        if store.handoff_was_prepared(&device.id) {
            return Ok(BondReleaseOutcome::PreviouslyReleased);
        }
        anyhow::bail!(
            "The local Hue Bluetooth key is already absent, but no successful bulb-side pairing handoff was recorded"
        );
    }

    let handoff_at = transport.prepare_pairing_handoff(device)?;
    // The write-only Hue handoff command cannot be read back. Persist the
    // successful authenticated write before deleting BlueZ's key so restart
    // recovery can distinguish a completed release from a one-sided forget.
    store.record_prepared_handoff(device.clone())?;
    let handoff_valid_until = handoff_at
        .checked_add(PAIRING_HANDOFF_FRESHNESS_BUDGET)
        .ok_or_else(|| anyhow::anyhow!("Hue Bluetooth handoff deadline overflowed"))?;
    if Instant::now() >= handoff_valid_until {
        anyhow::bail!(
            "Hue Bluetooth replacement-pairing window expired before the local key deletion boundary; the bond was retained so removal can be retried safely"
        );
    }
    transport.remove_local_bond(device, Some(handoff_valid_until))?;
    if transport.has_local_bond(device)? {
        anyhow::bail!("BlueZ still owns the Hue Bluetooth bond after removal");
    }
    Ok(BondReleaseOutcome::HandoffOpenedNow)
}

/// Prepare every known Hue Bluetooth bond before the appliance erases its
/// BlueZ key database.
///
/// Validate every authenticated handoff path without opening any replacement
/// pairing window. If a bonded bulb cannot be reached, fail before shared
/// reset state or BlueZ storage is cleared. The actual vendor writes are
/// reserved for the final bounded phase so a later platform failure cannot
/// expose a bulb to another controller during an aborted reset.
pub fn prepare_for_factory_reset(state: &SharedState) -> Result<usize> {
    let _bond_guard = BOND_LIFECYCLE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Hue BLE bond lifecycle lock poisoned"))?;
    let data_dir = data_dir(state)?;
    let _store_on_disk = HueBleDeviceStore::load(&data_dir)?;
    let pending_plan = HueBleDeviceStore::load_factory_reset_plan(&data_dir)
        .context("loading the pending Hue Bluetooth factory-reset plan")?;
    let (transport, store): (Arc<dyn HueBleTransport>, Arc<HueBleDeviceStore>) = match get_hub_data(
        state,
    ) {
        Ok(data) => (data.transport.clone(), data.store.clone()),
        Err(connection_error) => {
            #[cfg(all(target_os = "linux", feature = "bluez"))]
            {
                let transport = Arc::new(
                        super::bluez::BluezHueBleTransport::new().with_context(|| {
                            format!(
                                "The live Hue Bluetooth integration is unavailable ({connection_error:#}); opening a factory-reset transport to audit local bonds"
                            )
                        })?,
                    );
                (transport, Arc::new(_store_on_disk))
            }
            #[cfg(not(all(target_os = "linux", feature = "bluez")))]
            {
                return Err(connection_error).context(
                        "The Hue Bluetooth integration is unavailable; keep every bulb powered on nearby and try the factory reset again",
                    );
            }
        }
    };
    let mut devices = HashMap::new();
    let mut released_device_ids = pending_plan
        .released_device_ids
        .into_iter()
        .collect::<HashSet<_>>();
    let mut prepared_device_ids = pending_plan
        .prepared_device_ids
        .into_iter()
        .collect::<HashSet<_>>();
    for device in pending_plan.devices {
        devices.insert(device.id.clone(), device);
    }
    for device in store.tombstones() {
        devices.insert(device.id.clone(), device);
    }
    for device in store.all() {
        devices.insert(device.id.clone(), device);
    }
    let known_addresses = devices
        .values()
        .map(|device| device.address.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let local_bonds = transport
        .local_hue_bond_addresses()
        .context("enumerating local Hue Bluetooth bonds before factory reset")?;
    for address in local_bonds
        .iter()
        .filter(|address| !known_addresses.contains(&address.to_ascii_lowercase()))
    {
        // A paired Hue key is already owned by this adapter. Reconstruct only
        // enough vendor metadata for the explicit reset handoff; do not
        // silently adopt it into the normal Rhythm installation.
        let recovered = transport.inspect_local_bond(address).with_context(|| {
            format!("recovering reset-only metadata for untracked bonded Hue bulb {address}")
        })?;
        if !recovered.address.eq_ignore_ascii_case(address) {
            anyhow::bail!(
                "Recovered Hue Bluetooth metadata address {} did not match bonded address {address}",
                recovered.address
            );
        }
        if let Some(existing) = devices.get(&recovered.id) {
            if !existing.address.eq_ignore_ascii_case(&recovered.address) {
                anyhow::bail!(
                    "Hue Bluetooth identity {} maps to conflicting bonded addresses {} and {}",
                    recovered.id,
                    existing.address,
                    recovered.address
                );
            }
        }
        devices.insert(recovered.id.clone(), recovered);
    }

    let mut devices = devices.into_values().collect::<Vec<_>>();
    devices.sort_by(|left, right| left.id.cmp(&right.id));
    let mut planned_devices = Vec::new();
    let mut validated = 0;
    for device in devices {
        let has_local_bond = transport
            .has_local_bond(&device)
            .with_context(|| format!("checking the local bond for {}", device.display_name()))?;
        if has_local_bond {
            transport
                .validate_pairing_handoff(&device)
                .with_context(|| {
                format!(
                    "could not validate the transfer path for {}; keep it powered on nearby and try the factory reset again",
                    device.display_name()
                )
                })?;
            released_device_ids.remove(&device.id);
            prepared_device_ids.remove(&device.id);
            planned_devices.push(device);
            validated += 1;
        } else if released_device_ids.contains(&device.id) {
            // A prior post-reset attempt already proved the vendor handoff and
            // exact local deletion. Keep full metadata until the raw adapter
            // database scrub commits.
            planned_devices.push(device);
        } else if prepared_device_ids.remove(&device.id) {
            // Power may have failed after exact RemoveDevice but before the
            // released progress write. The durable pre-delete handoff marker
            // plus an absent key completes that transaction.
            released_device_ids.insert(device.id.clone());
            planned_devices.push(device);
        } else if store.handoff_was_prepared(&device.id) {
            log::debug!(
                target: "pair",
                "Hue BLE device {} was already released before this factory reset",
                device.id
            );
        } else {
            anyhow::bail!(
                "Rhythm has metadata for {}, but its local Bluetooth bond is absent and no successful Hue handoff was recorded. Factory-reset that bulb or explicitly remove it before retrying the Rhythm Box reset",
                device.display_name()
            );
        }
    }
    released_device_ids.retain(|id| planned_devices.iter().any(|device| &device.id == id));
    prepared_device_ids.retain(|id| planned_devices.iter().any(|device| &device.id == id));
    let plan = super::store::HueBleFactoryResetPlan {
        devices: planned_devices,
        prepared_device_ids: prepared_device_ids.into_iter().collect(),
        released_device_ids: released_device_ids.into_iter().collect(),
        ..Default::default()
    };
    HueBleDeviceStore::persist_factory_reset_plan(&data_dir, &plan)
        .context("persisting the Hue Bluetooth factory-reset bond plan")?;
    Ok(validated)
}

#[cfg_attr(not(all(target_os = "linux", feature = "bluez")), allow(dead_code))]
fn complete_factory_reset_bond_release_with_transport(
    data_dir: &str,
    transport: &dyn HueBleTransport,
) -> Result<HueBleFactoryResetHandoffBatch> {
    let _bond_guard = BOND_LIFECYCLE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Hue BLE bond lifecycle lock poisoned"))?;
    let mut plan = HueBleDeviceStore::load_factory_reset_plan(data_dir)
        .context("loading the Hue Bluetooth factory-reset bond plan")?;
    plan.devices.sort_by(|left, right| left.id.cmp(&right.id));
    let planned_addresses = plan
        .devices
        .iter()
        .map(|device| device.address.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let unknown_bonds = transport
        .local_hue_bond_addresses()
        .context("auditing Hue Bluetooth bonds before final factory-reset cleanup")?
        .into_iter()
        .filter(|address| !planned_addresses.contains(&address.to_ascii_lowercase()))
        .collect::<Vec<_>>();
    if !unknown_bonds.is_empty() {
        anyhow::bail!(
            "Rhythm found Hue Bluetooth bond(s) outside the durable factory-reset plan at {}; retrying without exact bulb metadata could strand them",
            unknown_bonds.join(", ")
        );
    }

    let mut released_device_ids = plan
        .released_device_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let mut prepared_device_ids = plan
        .prepared_device_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let mut prepared = 0;
    let mut first_handoff_at = None;
    for device in plan.devices.clone() {
        let has_local_bond = transport
            .has_local_bond(&device)
            .with_context(|| format!("checking the final bond for {}", device.display_name()))?;
        if !has_local_bond && released_device_ids.contains(&device.id) {
            continue;
        }
        if !has_local_bond && prepared_device_ids.contains(&device.id) {
            continue;
        }
        if !has_local_bond {
            anyhow::bail!(
                "The local bond for {} disappeared after factory-reset preflight without a durable exact-release record",
                device.display_name()
            );
        }
        // Refresh every bulb while retaining every BlueZ key. A later bulb
        // failure therefore leaves the entire key set usable and retryable.
        // Only the platform's all-at-once daemon stop + raw database scrub may
        // cross the deletion boundary.
        let handoff_at = transport
            .prepare_pairing_handoff(&device)
            .with_context(|| {
                format!(
                    "refreshing the transfer window for {} during final factory-reset cleanup",
                    device.display_name()
                )
            })?;
        first_handoff_at.get_or_insert(handoff_at);
        prepared_device_ids.insert(device.id.clone());
        released_device_ids.remove(&device.id);
        plan.prepared_device_ids = prepared_device_ids.iter().cloned().collect();
        plan.released_device_ids = released_device_ids.iter().cloned().collect();
        HueBleDeviceStore::persist_factory_reset_plan(data_dir, &plan)
            .context("journaling final Hue Bluetooth handoff before adapter scrub")?;
        prepared += 1;
    }

    if first_handoff_at.is_some_and(|started| started.elapsed() > PAIRING_HANDOFF_FRESHNESS_BUDGET)
    {
        anyhow::bail!(
            "Refreshing every Hue Bluetooth handoff took longer than {} seconds; all local keys were retained so the factory reset can be retried safely",
            PAIRING_HANDOFF_FRESHNESS_BUDGET.as_secs()
        );
    }
    Ok(HueBleFactoryResetHandoffBatch {
        refreshed: prepared,
        valid_until: first_handoff_at.map(|started| started + PAIRING_HANDOFF_FRESHNESS_BUDGET),
    })
}

/// Refresh every planned vendor handoff while retaining all BlueZ keys. The
/// appliance then stops bluetoothd and commits one raw adapter-database scrub.
pub fn complete_factory_reset_bond_release(
    data_dir: &str,
) -> Result<HueBleFactoryResetHandoffBatch> {
    #[cfg(all(target_os = "linux", feature = "bluez"))]
    {
        let transport = super::bluez::BluezHueBleTransport::new()
            .context("opening the final Hue Bluetooth factory-reset transport")?;
        complete_factory_reset_bond_release_with_transport(data_dir, &transport)
    }
    #[cfg(not(all(target_os = "linux", feature = "bluez")))]
    {
        let _ = data_dir;
        anyhow::bail!("Final Hue Bluetooth bond release requires Linux BlueZ support")
    }
}

pub fn create_controller(state: &SharedState) -> Result<Arc<dyn rhythm_core::HubLightController>> {
    let data = get_hub_data(state)?;
    Ok(Arc::new(super::controller::HueBleLightController::new(
        data.transport.clone(),
        data.store.clone(),
        data.registry.clone(),
    )))
}

fn select_stale_bond_replacements(
    request: &HueBlePairingRequest,
    explicit_reassociation_addresses: &[String],
) -> Result<Vec<String>> {
    if !request.replace_stale_bonds {
        return Ok(Vec::new());
    }

    if let Some(candidate_address) = request.candidate_address.as_deref() {
        let Some(quarantined_address) = explicit_reassociation_addresses
            .iter()
            .find(|address| address.eq_ignore_ascii_case(candidate_address))
        else {
            anyhow::bail!(
                "Selected Hue Bluetooth bulb {candidate_address} is not a quarantined stale bond; no Bluetooth bond was removed"
            );
        };
        return Ok(vec![quarantined_address.clone()]);
    }

    match explicit_reassociation_addresses {
        [only_address] => Ok(vec![only_address.clone()]),
        [] => anyhow::bail!(
            "No quarantined Hue Bluetooth bond is available for stale-bond recovery; no Bluetooth bond was removed"
        ),
        _ => anyhow::bail!(
            "More than one Hue Bluetooth bond is quarantined. Select a specific bulb before stale-bond recovery so no other bulb's bond is removed"
        ),
    }
}

fn stale_bond_recovery_session(store: &HueBleDeviceStore) -> PairingSession {
    let active_addresses = store
        .all()
        .into_iter()
        .map(|device| device.address.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut candidates = HashMap::<String, serde_json::Value>::new();
    for device in store.tombstones() {
        let normalized = device.address.to_ascii_lowercase();
        if active_addresses.contains(&normalized) {
            continue;
        }
        let display_name = device.display_name();
        candidates.insert(
            normalized,
            serde_json::json!({
                "candidate_address": device.address,
                "device_id": device.id,
                "name": display_name,
                "model": device.model,
            }),
        );
    }
    for address in store.pairing_retry_addresses() {
        let normalized = address.to_ascii_lowercase();
        if active_addresses.contains(&normalized) {
            continue;
        }
        candidates.entry(normalized).or_insert_with(|| {
            serde_json::json!({
                "candidate_address": address,
                "name": "Hue Bluetooth bulb",
            })
        });
    }
    let mut candidates = candidates.into_values().collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left["candidate_address"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .cmp(
                &right["candidate_address"]
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
            )
    });
    PairingSession {
        hub_type: HUB_TYPE.to_string(),
        status: PairingStatus::Complete,
        device: None,
        devices: Vec::new(),
        error: None,
        failure_stage: None,
        warnings: Vec::new(),
        details: Some(serde_json::json!({
            "recovery_candidates": candidates,
        })),
    }
}

pub fn pair(state: &SharedState, request: &HueBlePairingRequest) -> Result<PairingSession> {
    let _bond_guard = BOND_LIFECYCLE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Hue BLE bond lifecycle lock poisoned"))?;
    let data = get_hub_data(state)?;
    recover_tombstoned_removals(state, data.transport.as_ref(), data.store.as_ref());
    if request.list_stale_bond_candidates {
        return Ok(stale_bond_recovery_session(data.store.as_ref()));
    }
    rhythm_os::pairing::emit_pairing_progress(
        state,
        HUB_TYPE,
        request.session_id.as_deref(),
        PairingStatus::Searching,
        PairingStage::Searching,
        "Looking for nearby factory-reset Hue Bluetooth bulbs",
        None,
        None,
    );
    let mut transport_request = request.clone();
    transport_request.known_addresses = data
        .store
        .all()
        .into_iter()
        .map(|device| device.address)
        .collect();
    // Pairing is always an explicit user action. Exact retained tombstones are
    // therefore eligible for re-adoption, while unknown paired orphans remain
    // fail-closed behind the global corruption/factory-reset block below.
    transport_request.explicit_reassociation_addresses = data
        .store
        .tombstones()
        .into_iter()
        .map(|device| device.address)
        .chain(data.store.pairing_retry_addresses())
        .collect();
    transport_request
        .explicit_reassociation_addresses
        .sort_by_key(|address| address.to_ascii_uppercase());
    transport_request
        .explicit_reassociation_addresses
        .dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    // A physical bulb reset invalidates its old link key but BlueZ may retain
    // a stale Paired=true object. Recovery must resolve to exactly one
    // already-quarantined address before the transport can delete any key.
    transport_request.replace_stale_bond_addresses = select_stale_bond_replacements(
        request,
        &transport_request.explicit_reassociation_addresses,
    )?;
    let stale_recovery_address = transport_request
        .replace_stale_bond_addresses
        .first()
        .cloned();
    let stale_recovery_expected_device_id =
        stale_recovery_address.as_deref().and_then(|selected| {
            data.store
                .tombstones()
                .into_iter()
                .find(|device| device.address.eq_ignore_ascii_case(selected))
                .map(|device| device.id)
        });
    if request.replace_stale_bonds {
        // candidate_address selected the old quarantined key above. A
        // factory-reset Hue bulb may advertise under a new random address, so
        // that old address must never become the post-reset discovery filter.
        transport_request.candidate_address = None;
    }
    transport_request.blocked_paired_addresses = Vec::new();
    transport_request.allow_paired_orphan_adoption = !data.store.blocks_paired_orphan_adoption();
    let store_for_intent = data.store.clone();
    let record_bond_intent = move |address: &str| {
        if let Err(error) = store_for_intent.record_pairing_retry_address(address) {
            let block_error = store_for_intent.block_paired_orphan_adoption().err();
            anyhow::bail!(
                "persisting exact Hue bond intent failed: {error:#}{}",
                block_error
                    .map(|error| format!("; global bond quarantine also failed: {error:#}"))
                    .unwrap_or_default()
            );
        }
        Ok(())
    };
    let HueBlePairingOutcome {
        devices: discovered,
        warnings,
        retained_bond_addresses,
    } = data
        .transport
        .pair_lights(&transport_request, &record_bond_intent)?;
    let mut paired = Vec::new();
    let mut failures = warnings;
    if let Some(address) = stale_recovery_address {
        if let Err(error) = data.store.clear_pairing_retry_address(&address) {
            failures.push(format!(
                "{address}: stale BlueZ key was removed, but its retry marker could not be cleared: {error:#}"
            ));
        }
    }
    if let Some(expected_device_id) = stale_recovery_expected_device_id {
        if !discovered
            .iter()
            .any(|device| device.id == expected_device_id)
        {
            failures.push(
                "The selected factory-reset Hue bulb was not rediscovered under its stable identity. Its stale local key was removed, but its prior record remains quarantined; keep it powered nearby and scan again."
                    .to_string(),
            );
        }
    }
    for address in retained_bond_addresses {
        if let Err(error) = data.store.record_pairing_retry_address(&address) {
            // If the exact-address journal is unavailable, close the broader
            // orphan-adoption gate so the possible bond cannot be silently
            // claimed as a new device on a later scan.
            let block_error = data.store.block_paired_orphan_adoption().err();
            failures.push(format!(
                "{address}: could not persist the retained Hue Bluetooth bond for retry: {error:#}{}",
                block_error
                    .map(|error| format!("; global bond quarantine also failed: {error:#}"))
                    .unwrap_or_default()
            ));
        }
    }

    for device in discovered {
        rhythm_os::pairing::emit_pairing_progress(
            state,
            HUB_TYPE,
            request.session_id.as_deref(),
            PairingStatus::Found,
            PairingStage::Connecting,
            format!("Connected to {}", device.display_name()),
            None,
            None,
        );
        rhythm_os::pairing::emit_pairing_progress(
            state,
            HUB_TYPE,
            request.session_id.as_deref(),
            PairingStatus::Commissioning,
            PairingStage::Finalizing,
            format!("Saving {}", device.display_name()),
            None,
            None,
        );

        let previous = data.store.get(&device.id);
        let had_previous = previous.is_some();
        let rotated_prior = previous
            .clone()
            .or_else(|| data.store.tombstone(&device.id))
            .filter(|prior| !prior.address.eq_ignore_ascii_case(&device.address));
        let replaced_stale_previous_bond = if let Some(previous) = rotated_prior.as_ref() {
            // A factory reset may preserve the stable Hue EUI while rotating
            // the random BLE address. Before B replaces active record A,
            // durably journal B and remove A's now-stale exact BlueZ key.
            // A crash at any point then leaves either active A + retry B or
            // active B, never an unknown paired address.
            if let Err(error) = data.store.record_pairing_retry_address(&device.address) {
                let block_error = data.store.block_paired_orphan_adoption().err();
                failures.push(format!(
                    "{}: could not journal replacement address {} before removing stale bond {}: {error:#}{}",
                    device.display_name(),
                    device.address,
                    previous.address,
                    block_error
                        .map(|error| format!("; global bond quarantine also failed: {error:#}"))
                        .unwrap_or_default()
                ));
                continue;
            }
            if let Err(error) = data.transport.remove_local_bond(previous, None) {
                failures.push(format!(
                    "{}: paired at {}, but the stale prior BlueZ bond at {} could not be removed: {error:#}",
                    device.display_name(),
                    device.address,
                    previous.address
                ));
                continue;
            }
            true
        } else {
            false
        };
        if let Err(error) = data.store.upsert(device.clone()) {
            // Do not delete only the Pi-side key: Hue would retain its copy and
            // reject a future bond. Quarantine a newly created exact bond so a
            // later explicit scan can safely re-adopt it.
            if !had_previous {
                if let Err(tombstone_error) = data.store.record_tombstone(device.clone()) {
                    warn!(
                        target: "pair",
                        "Could not quarantine Hue BLE bond after metadata persistence failed for {}: {tombstone_error:#}",
                        device.id
                    );
                }
            }
            failures.push(format!(
                "{}: {:#}",
                device.display_name(),
                error.context("persisting paired Hue Bluetooth bulb")
            ));
            continue;
        }
        if let Err(error) = register_canonical_identity(state, &device)
            .and_then(|()| materialize_unassigned(state, &device.id))
        {
            if replaced_stale_previous_bond {
                if had_previous {
                    // A's key is already gone and B is the only valid local
                    // bond. The existing canonical projection still identifies
                    // this stable bulb, so keep B's durable metadata rather than
                    // restoring A and orphaning B.
                    warn!(
                        target: "pair",
                        "Retaining replacement Hue BLE metadata {} at {} after canonical projection failed because stale address cleanup already committed",
                        device.id,
                        device.address
                    );
                } else if let Err(quarantine_error) = data.store.tombstone_and_remove(&device) {
                    // Here A came only from a tombstone, so no prior canonical
                    // projection can keep B usable. Convert B back to an exact
                    // retained-bond quarantine; a later explicit scan can
                    // re-adopt it without treating it as an unknown orphan.
                    warn!(
                        target: "pair",
                        "Could not quarantine replacement Hue BLE bond {} after canonical projection failed: {quarantine_error:#}",
                        device.address
                    );
                }
            } else if let Some(previous) = previous {
                // The same stable EUI may have moved from random address A to
                // B after a physical reset. Quarantine the new exact bond
                // before restoring A so a failed canonical commit can never
                // leave B as an untracked paired orphan.
                let rollback_is_safe = match data
                    .store
                    .record_pairing_retry_address(&device.address)
                {
                    Ok(()) => true,
                    Err(quarantine_error) => match data.store.block_paired_orphan_adoption() {
                        Ok(()) => {
                            warn!(
                                target: "pair",
                                "Could not quarantine replacement Hue BLE bond {} exactly before rollback ({quarantine_error:#}); globally blocked paired-orphan adoption instead",
                                device.address
                            );
                            true
                        }
                        Err(block_error) => {
                            warn!(
                                target: "pair",
                                "Could not quarantine replacement Hue BLE bond {} before rollback: {quarantine_error:#}; global bond quarantine also failed: {block_error:#}. Retaining the replacement's durable active metadata",
                                device.address
                            );
                            false
                        }
                    },
                };
                if rollback_is_safe {
                    if let Err(rollback_error) = data.store.upsert(previous) {
                        warn!(
                            target: "pair",
                            "Could not restore prior Hue BLE metadata after canonical projection failed for {}: {rollback_error:#}",
                            device.id
                        );
                    }
                }
            } else {
                let _ = data.store.tombstone_and_remove(&device);
            }
            if !had_previous {
                if let Err(rollback_error) =
                    rhythm_os::commands::do_device_endpoint_remove(state, &device.id, &hub_key())
                {
                    warn!(
                        target: "pair",
                        "Failed to roll back Hue BLE canonical projection {}: {rollback_error:#}",
                        device.id
                    );
                }
            }
            failures.push(format!("{}: {error:#}", device.display_name()));
            continue;
        }

        let _ = data.event_tx.send(HubEvent::DevicePaired {
            hub_key: None,
            device_id: device.id.clone(),
            name: device.display_name(),
            device_type: DeviceType::Light,
        });
        paired.push(device);
    }

    if paired.is_empty() {
        anyhow::bail!(
            "No discovered Hue Bluetooth bulbs could be saved{}",
            if failures.is_empty() {
                String::new()
            } else {
                format!(": {}", failures.join("; "))
            }
        );
    }
    for failure in &failures {
        warn!(target: "pair", "Hue BLE batch pairing partial failure: {failure}");
    }
    Ok(complete_session(paired, failures))
}

fn complete_session(devices: Vec<HueBleDevice>, warnings: Vec<String>) -> PairingSession {
    let paired_devices = devices
        .into_iter()
        .map(|device| PairedDeviceInfo {
            device_id: device.id.clone(),
            name: device.display_name(),
            device_type: DeviceType::Light,
            manufacturer: Some(device.manufacturer),
            model: Some(device.model),
        })
        .collect::<Vec<_>>();
    PairingSession {
        hub_type: HUB_TYPE.to_string(),
        status: PairingStatus::Complete,
        device: paired_devices.first().cloned(),
        devices: paired_devices,
        error: None,
        failure_stage: None,
        warnings,
        details: None,
    }
}

fn register_canonical_identity(state: &SharedState, device: &HueBleDevice) -> Result<()> {
    let identity = DiscoveredIdentity {
        native_id: device.id.clone(),
        room_id: None,
        room_name: None,
        name: device.display_name(),
        device_type: DeviceType::Light,
        hardware_ids: vec![HardwareId::mac(&device.eui64)],
        manufacturer: Some(device.manufacturer.clone()),
        model: Some(device.model.clone()),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let endpoint_capabilities = normalized_endpoint_capabilities(device)?;
    let key = hub_key();
    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state.canonical_registry.resolve(&identity, &key, now);
    let canonical_id = state
        .canonical_registry
        .find_by_native_id(&key, &device.id)
        .map(|device| device.id.clone())
        .ok_or_else(|| anyhow::anyhow!("Canonical Hue BLE endpoint was not registered"))?;
    let endpoint = state
        .canonical_registry
        .get_mut(&canonical_id)
        .and_then(|device| {
            device.endpoints.iter_mut().find(|endpoint| {
                endpoint.hub_key == key && endpoint.native_id == identity.native_id
            })
        })
        .ok_or_else(|| anyhow::anyhow!("Canonical Hue BLE endpoint disappeared"))?;
    endpoint.capabilities = Some(endpoint_capabilities);

    rhythm_os::commands::save_authority_state(&state)
        .context("persisting Hue BLE endpoint capabilities")?;
    Ok(())
}

fn normalized_endpoint_capabilities(device: &HueBleDevice) -> Result<serde_json::Value> {
    let color_temperature = if device.capabilities.color_temperature {
        let min_kelvin = device
            .capabilities
            .min_kelvin()
            .ok_or_else(|| anyhow::anyhow!("Hue bulb CT minimum is unavailable"))?;
        let max_kelvin = device
            .capabilities
            .max_kelvin()
            .ok_or_else(|| anyhow::anyhow!("Hue bulb CT maximum is unavailable"))?;
        if min_kelvin == 0 || min_kelvin > max_kelvin {
            anyhow::bail!("Hue bulb reported an invalid CT range");
        }
        Some(LightColorTemperatureCapabilitiesDto {
            min_kelvin,
            max_kelvin,
        })
    } else {
        None
    };
    Ok(serde_json::json!({
        "light_capabilities": LightCapabilitiesDto {
            color_temperature,
            individual_profile_overrides: None,
        }
    }))
}

fn materialize_unassigned(state: &SharedState, native_id: &str) -> Result<()> {
    let state_guard = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let (canonical_id, assigned) = state_guard
        .canonical_registry
        .find_by_native_id(&hub_key(), native_id)
        .map(|device| (device.id.clone(), device.room_id.is_some()))
        .ok_or_else(|| anyhow::anyhow!("Canonical Hue BLE device was not registered"))?;
    drop(state_guard);
    if assigned {
        rhythm_os::commands::reconcile_runtime_from_state(state)
    } else {
        rhythm_os::commands::do_canonical_assign_room(state, &canonical_id, None)
    }
}

pub fn resolve_native_id(state: &SharedState, requested_id: &str) -> Result<String> {
    if requested_id.starts_with("hue-ble-") {
        return Ok(requested_id.to_string());
    }
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state
        .canonical_registry
        .get(requested_id)
        .and_then(|device| {
            device
                .endpoints
                .iter()
                .find(|endpoint| endpoint.hub_key == hub_key())
                .map(|endpoint| endpoint.native_id.clone())
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Canonical device '{}' has no Hue Bluetooth endpoint",
                requested_id
            )
        })
}

fn canonical_endpoint_exists(state: &SharedState, native_id: &str) -> bool {
    state
        .lock()
        .ok()
        .and_then(|state| {
            state
                .canonical_registry
                .find_by_native_id(&hub_key(), native_id)
                .map(|_| ())
        })
        .is_some()
}

pub fn unpair(state: &SharedState, requested_id: &str, force: bool) -> Result<UnpairingResult> {
    let _bond_guard = BOND_LIFECYCLE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Hue BLE bond lifecycle lock poisoned"))?;
    let native_id = resolve_native_id(state, requested_id)?;
    let had_canonical_endpoint = canonical_endpoint_exists(state, &native_id);
    let data = get_hub_data(state)?;
    let had_verified_absent_removal = data
        .store
        .removed_native_ids()
        .into_iter()
        .any(|removed| removed == native_id);
    let had_unresolved_metadata = data
        .store
        .blocked_native_ids()
        .into_iter()
        .any(|blocked| blocked == native_id);
    let device = data
        .store
        .get(&native_id)
        .or_else(|| data.store.tombstone(&native_id));
    let Some(device) = device else {
        if had_verified_absent_removal {
            return Ok(UnpairingResult {
                hub_type: HUB_TYPE.to_string(),
                hub_address: Some(HUB_ADDRESS.to_string()),
                status: PairingStatus::Complete,
                device_id: Some(native_id),
                error: None,
                completion_scope: Some(UnpairingCompletionScope::LocalStateOnly),
                warning: Some(
                    "Rhythm already verified that this adapter has no local Bluetooth bond for the bulb. Bulb-side trust could not be verified; factory-reset the bulb before pairing it to another controller if needed."
                        .to_string(),
                ),
            });
        }
        if had_canonical_endpoint {
            if !force {
                data.store.block_paired_orphan_adoption()?;
                warn!(
                    target: "pair",
                    "Hue BLE endpoint {} had no durable address metadata; blocking adoption of all paired BlueZ orphans",
                    native_id
                );
                return Ok(UnpairingResult {
                    hub_type: HUB_TYPE.to_string(),
                    hub_address: Some(HUB_ADDRESS.to_string()),
                    status: PairingStatus::Failed,
                    device_id: Some(native_id),
                    error: Some(
                        "Hue Bluetooth address metadata is missing, so the BlueZ bond could not be removed; retry with force to forget the endpoint while keeping paired-orphan adoption blocked"
                            .to_string(),
                    ),
                    completion_scope: None,
                    warning: None,
                });
            }
            data.store.record_missing_metadata_removal(&native_id)?;
            warn!(
                target: "pair",
                "Force-removing metadata-missing Hue BLE endpoint {}; paired orphan adoption remains blocked",
                native_id
            );
        }
        return Ok(UnpairingResult {
            hub_type: HUB_TYPE.to_string(),
            hub_address: Some(HUB_ADDRESS.to_string()),
            status: PairingStatus::Complete,
            device_id: Some(native_id),
            error: None,
            completion_scope: Some(if had_canonical_endpoint || had_unresolved_metadata {
                UnpairingCompletionScope::LocalStateOnly
            } else {
                UnpairingCompletionScope::AlreadyAbsent
            }),
            warning: (had_canonical_endpoint || had_unresolved_metadata).then(|| {
                "Rhythm removed its endpoint record, but the bulb-side Bluetooth trust could not be verified. A factory reset may be needed before pairing it to another controller."
                    .to_string()
            }),
        });
    };

    let (completion_scope, warning) = match release_bond_with_journal(
        data.transport.as_ref(),
        data.store.as_ref(),
        &device,
    ) {
        Ok(release_outcome) => {
            // record_prepared_handoff durably installed the tombstone
            // before the BlueZ key was removed. Keep it until an explicit
            // fresh pairing, and now remove only the active metadata.
            data.store.remove(&device.id)?;
            (
                UnpairingCompletionScope::LocalBondRemoved,
                Some(match release_outcome {
                    BondReleaseOutcome::HandoffOpenedNow =>
                        "Rhythm removed its local Bluetooth bond after opening the bulb's short replacement-pairing window. Pair it to the next controller now; if it no longer appears, use the bulb's normal factory-reset process."
                            .to_string(),
                    BondReleaseOutcome::PreviouslyReleased =>
                        "Rhythm's local Bluetooth bond was already absent after a recorded Hue handoff. That replacement-pairing window may have expired; if the bulb is not available, use its normal factory-reset process."
                            .to_string(),
                }),
            )
        }
        Err(error) if !force => {
            return Ok(UnpairingResult {
                hub_type: HUB_TYPE.to_string(),
                hub_address: Some(HUB_ADDRESS.to_string()),
                status: PairingStatus::Failed,
                device_id: Some(native_id),
                error: Some(format!("{error:#}")),
                completion_scope: None,
                warning: None,
            });
        }
        Err(error) => {
            warn!(
                target: "pair",
                "Force-removing Hue BLE device {} while quarantining its unresolved BlueZ bond: {error:#}",
                native_id
            );
            let handoff_was_prepared = data.store.handoff_was_prepared(&device.id);
            let bond_state = data.transport.has_local_bond(&device);
            if handoff_was_prepared {
                // An authenticated handoff may have been followed by a
                // successful RemoveDevice whose verification failed. Never
                // downgrade that durable proof during forced cleanup.
                data.store.remove(&device.id)?;
            } else {
                data.store.tombstone_and_remove(&device)?;
                if matches!(&bond_state, Ok(false)) {
                    // This second, live adapter check plus explicit force is
                    // the user's acknowledgement that no local key remains.
                    // Clear the exact tombstone so a discarded/reset bulb
                    // cannot permanently block a later appliance reset.
                    data.store.acknowledge_absent_bond_removal(&device)?;
                }
            }
            match (bond_state, handoff_was_prepared) {
                (Ok(false), true) => (
                    UnpairingCompletionScope::LocalBondRemoved,
                    Some(
                        "Rhythm verified that its local Bluetooth bond is gone after an authenticated Hue handoff, although the original removal request ended ambiguously. Pair the bulb to the next controller now; factory-reset it if it no longer appears."
                            .to_string(),
                    ),
                ),
                (Ok(true), _) => (
                    UnpairingCompletionScope::LocalBondRetained,
                    Some(
                        "Rhythm forgot the endpoint but retained its Bluetooth bond. A nearby Hue Bluetooth scan can restore it on this Rhythm Box; release or factory-reset the bulb before moving it to another controller."
                            .to_string(),
                    ),
                ),
                (Ok(false), false) => (
                    UnpairingCompletionScope::LocalStateOnly,
                    Some(
                        "Rhythm removed its endpoint record and verified that this adapter no longer has the bulb's Bluetooth bond. Bulb-side trust cannot be verified; factory-reset the bulb before pairing it to another controller if needed."
                            .to_string(),
                    ),
                ),
                (Err(_), _) => (
                    UnpairingCompletionScope::LocalStateOnly,
                    Some(
                        "Rhythm removed its endpoint record, but bulb-side Bluetooth trust could not be verified. A factory reset may be needed before pairing it to another controller."
                            .to_string(),
                    ),
                ),
            }
        }
    };
    Ok(UnpairingResult {
        hub_type: HUB_TYPE.to_string(),
        hub_address: Some(HUB_ADDRESS.to_string()),
        status: PairingStatus::Complete,
        device_id: Some(native_id),
        error: None,
        completion_scope: Some(completion_scope),
        warning,
    })
}

/// Remove only Rhythm's durable Hue BLE metadata when the adapter cannot be
/// reached and the user explicitly requested force removal.
///
/// The BlueZ bond may remain on disk, so its durable tombstone prevents orphan
/// adoption. The canonical endpoint is removed by the shared unpairing handler
/// after this function reports completion.
pub fn force_forget_offline(state: &SharedState, requested_id: &str) -> Result<UnpairingResult> {
    let _bond_guard = BOND_LIFECYCLE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Hue BLE bond lifecycle lock poisoned"))?;
    let native_id = resolve_native_id(state, requested_id)?;
    let had_canonical_endpoint = canonical_endpoint_exists(state, &native_id);
    let data_dir = data_dir(state)?;
    let (store, quarantined) = HueBleDeviceStore::load_recovering_corrupt(data_dir)?;
    let had_verified_absent_removal = store
        .removed_native_ids()
        .into_iter()
        .any(|removed| removed == native_id);
    let had_unresolved_metadata = store
        .blocked_native_ids()
        .into_iter()
        .any(|blocked| blocked == native_id);
    if let Some(path) = quarantined {
        warn!(
            target: "pair",
            "Quarantined corrupt Hue BLE metadata at {}; paired BlueZ orphans are blocked from adoption until the bond database is reset",
            path.display()
        );
    }
    if had_verified_absent_removal {
        return Ok(UnpairingResult {
            hub_type: HUB_TYPE.to_string(),
            hub_address: Some(HUB_ADDRESS.to_string()),
            status: PairingStatus::Complete,
            device_id: Some(native_id),
            error: None,
            completion_scope: Some(UnpairingCompletionScope::LocalStateOnly),
            warning: Some(
                "Rhythm already verified that this adapter has no local Bluetooth bond for the bulb. Bulb-side trust could not be verified; factory-reset the bulb before pairing it to another controller if needed."
                    .to_string(),
            ),
        });
    }
    let had_durable_metadata = if store.handoff_was_prepared(&native_id) {
        // A repeated offline force-forget must not erase proof that a prior
        // authenticated handoff preceded local key deletion. Remove any
        // leftover active projection while preserving the prepared tombstone
        // for restart/factory-reset recovery.
        store.remove(&native_id)?;
        true
    } else if let Some(device) = store
        .get(&native_id)
        .or_else(|| store.tombstone(&native_id))
    {
        store.tombstone_and_remove(&device)?;
        true
    } else if had_canonical_endpoint {
        store.record_missing_metadata_removal(&native_id)?;
        warn!(
            target: "pair",
            "Offline Hue BLE endpoint {} had no durable address metadata; blocking adoption of all paired BlueZ orphans",
            native_id
        );
        true
    } else {
        had_unresolved_metadata
    };
    if !had_durable_metadata {
        return Ok(UnpairingResult {
            hub_type: HUB_TYPE.to_string(),
            hub_address: Some(HUB_ADDRESS.to_string()),
            status: PairingStatus::Complete,
            device_id: Some(native_id),
            error: None,
            completion_scope: Some(UnpairingCompletionScope::AlreadyAbsent),
            warning: None,
        });
    }
    Ok(UnpairingResult {
        hub_type: HUB_TYPE.to_string(),
        hub_address: Some(HUB_ADDRESS.to_string()),
        status: PairingStatus::Complete,
        device_id: Some(native_id),
        error: None,
        completion_scope: Some(UnpairingCompletionScope::LocalStateOnly),
        warning: Some(
            "Rhythm removed its endpoint record while the Bluetooth adapter was unavailable. The bulb may retain old trust and may need a factory reset before pairing to another controller."
                .to_string(),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::types::{HueBleCapabilities, HueBleCommand, HueBleState};

    #[test]
    fn foreground_adapter_contention_preserves_observed_connection_state() {
        assert_eq!(
            adapter_connection_transition(true, HueBleAdapterAvailability::Busy),
            (true, AdapterConnectionTransition::None)
        );
        assert_eq!(
            adapter_connection_transition(false, HueBleAdapterAvailability::Busy),
            (false, AdapterConnectionTransition::None)
        );
        assert_eq!(
            adapter_connection_transition(true, HueBleAdapterAvailability::Unavailable),
            (false, AdapterConnectionTransition::Disconnected)
        );
        assert_eq!(
            adapter_connection_transition(false, HueBleAdapterAvailability::Available),
            (true, AdapterConnectionTransition::Connected)
        );
    }

    #[derive(Default)]
    struct CapturingTransport {
        request: Mutex<Option<HueBlePairingRequest>>,
        pairing_outcome: Mutex<Option<HueBlePairingOutcome>>,
        bond_intent_address: Option<String>,
        available: bool,
        prewarmed: Mutex<Vec<String>>,
        pool_initializations: Mutex<Vec<Vec<String>>>,
        prewarm_error_device: Option<String>,
        lifecycle_events: Mutex<Vec<String>>,
        validated: Mutex<Vec<String>>,
        prepared: Mutex<Vec<String>>,
        unpaired: Mutex<Vec<String>>,
        removed_bond_addresses: Mutex<Vec<String>>,
        bond_removal_deadlines: Mutex<Vec<Option<Instant>>>,
        local_hue_bonds: Vec<String>,
        local_bond_devices: Vec<HueBleDevice>,
        unpair_error: Option<String>,
        handoff_error_device: Option<String>,
        handoff_age: Option<Duration>,
        bond_absent: bool,
        bond_remove_error: Option<String>,
        bond_remove_error_device: Option<String>,
        bond_remove_error_after_removal_device: Option<String>,
        quiesced: AtomicBool,
    }

    impl HueBleTransport for CapturingTransport {
        fn is_available(&self) -> Result<bool> {
            Ok(self.available)
        }

        fn quiesce(&self) -> Result<()> {
            self.quiesced.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn pair_lights(
            &self,
            request: &HueBlePairingRequest,
            record_bond_intent: &(dyn Fn(&str) -> Result<()> + Send + Sync),
        ) -> Result<HueBlePairingOutcome> {
            *self.request.lock().unwrap() = Some(request.clone());
            if let Some(address) = self.bond_intent_address.as_deref() {
                record_bond_intent(address)?;
            }
            self.pairing_outcome
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| anyhow::anyhow!("captured"))
        }

        fn apply_command(&self, _device: &HueBleDevice, _command: &HueBleCommand) -> Result<()> {
            Ok(())
        }

        fn apply_commands_until(
            &self,
            commands: &[(HueBleDevice, HueBleCommand)],
            deadline: Instant,
        ) -> Result<Vec<(String, Result<()>)>> {
            if deadline <= Instant::now() {
                anyhow::bail!("capturing transport command deadline expired");
            }
            Ok(commands
                .iter()
                .map(|(device, command)| (device.id.clone(), self.apply_command(device, command)))
                .collect())
        }

        fn prewarm(&self, device: &HueBleDevice) -> Result<bool> {
            self.prewarmed.lock().unwrap().push(device.id.clone());
            if self.prewarm_error_device.as_deref() == Some(device.id.as_str()) {
                anyhow::bail!("selected bulb prewarm failed");
            }
            Ok(true)
        }

        fn initialize_connection_pool(&self, devices: &[HueBleDevice]) -> Result<()> {
            self.pool_initializations
                .lock()
                .unwrap()
                .push(devices.iter().map(|device| device.id.clone()).collect());
            Ok(())
        }

        fn read_state(&self, _device: &HueBleDevice) -> Result<HueBleState> {
            Ok(HueBleState::default())
        }

        fn read_state_passive(&self, _device: &HueBleDevice) -> Result<Option<HueBleState>> {
            Ok(None)
        }

        fn has_local_bond(&self, _device: &HueBleDevice) -> Result<bool> {
            Ok(!self.bond_absent
                && (self.local_hue_bonds.is_empty()
                    || self
                        .local_hue_bonds
                        .iter()
                        .any(|address| address.eq_ignore_ascii_case(&_device.address)))
                && !self
                    .unpaired
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|removed| removed == &_device.id))
        }

        fn local_hue_bond_addresses(&self) -> Result<Vec<String>> {
            let removed = self.removed_bond_addresses.lock().unwrap();
            Ok(self
                .local_hue_bonds
                .iter()
                .filter(|address| {
                    !removed
                        .iter()
                        .any(|deleted| deleted.eq_ignore_ascii_case(address))
                })
                .cloned()
                .collect())
        }

        fn inspect_local_bond(&self, address: &str) -> Result<HueBleDevice> {
            self.local_bond_devices
                .iter()
                .find(|device| device.address.eq_ignore_ascii_case(address))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("bonded Hue metadata unavailable for {address}"))
        }

        fn validate_pairing_handoff(&self, device: &HueBleDevice) -> Result<()> {
            if self.handoff_error_device.as_deref() == Some(device.id.as_str()) {
                anyhow::bail!("selected bulb is offline");
            }
            self.lifecycle_events
                .lock()
                .unwrap()
                .push(format!("validate:{}", device.id));
            self.validated.lock().unwrap().push(device.id.clone());
            Ok(())
        }

        fn prepare_pairing_handoff(&self, device: &HueBleDevice) -> Result<Instant> {
            if self.handoff_error_device.as_deref() == Some(device.id.as_str()) {
                anyhow::bail!("selected bulb is offline");
            }
            if let Some(error) = &self.unpair_error {
                anyhow::bail!("{error}");
            }
            self.lifecycle_events
                .lock()
                .unwrap()
                .push(format!("prepare:{}", device.id));
            self.prepared.lock().unwrap().push(device.id.clone());
            Ok(Instant::now() - self.handoff_age.unwrap_or_default())
        }

        fn remove_local_bond(
            &self,
            device: &HueBleDevice,
            handoff_valid_until: Option<Instant>,
        ) -> Result<()> {
            self.bond_removal_deadlines
                .lock()
                .unwrap()
                .push(handoff_valid_until);
            if self.bond_remove_error_device.as_deref() == Some(device.id.as_str()) {
                anyhow::bail!("selected BlueZ removal failed");
            }
            if let Some(error) = &self.bond_remove_error {
                anyhow::bail!("{error}");
            }
            self.lifecycle_events
                .lock()
                .unwrap()
                .push(format!("remove:{}", device.id));
            self.unpaired.lock().unwrap().push(device.id.clone());
            self.removed_bond_addresses
                .lock()
                .unwrap()
                .push(device.address.clone());
            if self.bond_remove_error_after_removal_device.as_deref() == Some(device.id.as_str()) {
                anyhow::bail!("BlueZ verification failed after RemoveDevice");
            }
            Ok(())
        }
    }

    fn stored_device(id_suffix: &str, address: &str) -> HueBleDevice {
        let eui64 = format!("0017880100{id_suffix}");
        HueBleDevice {
            id: HueBleDevice::stable_id(&eui64),
            address: address.to_string(),
            address_type: "random".to_string(),
            eui64,
            name: "Hue lamp".to_string(),
            manufacturer: "Signify Netherlands B.V.".to_string(),
            model: "LWA003".to_string(),
            firmware: "1.0".to_string(),
            capabilities: HueBleCapabilities {
                dimming: true,
                ..Default::default()
            },
            paired_at_epoch_secs: 1,
            last_state: None,
        }
    }

    #[test]
    fn startup_prewarm_is_best_effort_within_the_connection_pool_capacity() {
        let unique = format!(
            "rhythm-hue-ble-prewarm-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let data_dir = std::env::temp_dir().join(unique);
        let store = HueBleDeviceStore::load(&data_dir).unwrap();
        let first = stored_device("000001", "AA:00:00:00:00:01");
        let second = stored_device("000002", "AA:00:00:00:00:02");
        store.upsert(first.clone()).unwrap();
        store.upsert(second.clone()).unwrap();
        for suffix in 3..=BLE_CONNECTION_POOL_CAPACITY + 1 {
            store
                .upsert(stored_device(
                    &format!("{suffix:06}"),
                    &format!("AA:00:00:00:00:{suffix:02X}"),
                ))
                .unwrap();
        }
        let transport = CapturingTransport {
            prewarm_error_device: Some(first.id.clone()),
            ..Default::default()
        };

        prewarm_known_devices(&transport, &store, &AtomicBool::new(false));

        let prewarmed = transport.prewarmed.lock().unwrap();
        assert_eq!(prewarmed.len(), BLE_CONNECTION_POOL_CAPACITY);
        assert_eq!(&prewarmed[..2], &[first.id, second.id]);
        let initializations = transport.pool_initializations.lock().unwrap();
        assert_eq!(initializations.len(), 1);
        assert_eq!(initializations[0].len(), BLE_CONNECTION_POOL_CAPACITY + 1);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn startup_prewarm_honors_shutdown_before_touching_a_bulb() {
        let unique = format!(
            "rhythm-hue-ble-prewarm-shutdown-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let data_dir = std::env::temp_dir().join(unique);
        let store = HueBleDeviceStore::load(&data_dir).unwrap();
        store
            .upsert(stored_device("000001", "AA:00:00:00:00:01"))
            .unwrap();
        let transport = CapturingTransport::default();

        prewarm_known_devices(&transport, &store, &AtomicBool::new(true));

        assert!(transport.pool_initializations.lock().unwrap().is_empty());
        assert!(transport.prewarmed.lock().unwrap().is_empty());
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    fn factory_reset_plan(
        devices: Vec<HueBleDevice>,
    ) -> super::super::store::HueBleFactoryResetPlan {
        super::super::store::HueBleFactoryResetPlan {
            devices,
            ..Default::default()
        }
    }

    #[test]
    fn pairing_excludes_every_tracked_bond_but_can_adopt_an_orphan() {
        let unique = format!(
            "rhythm-hue-ble-lifecycle-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let data_dir = std::env::temp_dir().join(unique);
        let mut app_state = rhythm_os::state::AppState::default();
        app_state.data_dir = data_dir.to_string_lossy().into_owned();
        let state = Arc::new(Mutex::new(app_state));
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (hub, _events) = connect(&state, transport.clone()).unwrap();
        state.lock().unwrap().hubs.insert(hub_key(), hub);
        let data = get_hub_data(&state).unwrap();
        data.store
            .upsert(stored_device("000001", "AA:00:00:00:00:01"))
            .unwrap();
        data.store
            .upsert(stored_device("000002", "AA:00:00:00:00:02"))
            .unwrap();

        let request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        assert!(pair(&state, &request).is_err());
        let captured = transport.request.lock().unwrap().clone().unwrap();
        assert_eq!(
            captured.known_addresses,
            vec![
                "AA:00:00:00:00:01".to_string(),
                "AA:00:00:00:00:02".to_string()
            ]
        );
        assert!(captured.explicit_reassociation_addresses.is_empty());
        assert!(captured.replace_stale_bond_addresses.is_empty());
        assert!(captured.blocked_paired_addresses.is_empty());
        assert!(captured.allow_paired_orphan_adoption);

        state.lock().unwrap().hubs.remove(&hub_key());
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn stale_bond_candidate_listing_is_non_mutating_exact_and_excludes_active_bonds() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("pairing-list-stale-bonds", transport.clone());
        let store = get_hub_data(&state).unwrap().store.clone();
        let tombstoned = stored_device("000031", "AA:00:00:00:00:31");
        let active = stored_device("000033", "AA:00:00:00:00:33");
        store.record_tombstone(tombstoned.clone()).unwrap();
        store
            .record_pairing_retry_address("AA:00:00:00:00:32")
            .unwrap();
        store.upsert(active.clone()).unwrap();
        store.record_pairing_retry_address(&active.address).unwrap();

        let request = HueBlePairingRequest::from_value(&serde_json::json!({
            "list_stale_bond_candidates": true
        }))
        .unwrap();
        let session = pair(&state, &request).unwrap();

        assert_eq!(session.status, PairingStatus::Complete);
        assert!(session.devices.is_empty());
        let candidates = session
            .details
            .as_ref()
            .and_then(|details| details["recovery_candidates"].as_array())
            .unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates[0]["candidate_address"],
            tombstoned.address.as_str()
        );
        assert_eq!(candidates[0]["device_id"], tombstoned.id.as_str());
        assert_eq!(candidates[0]["model"], "LWA003");
        assert_eq!(candidates[1]["candidate_address"], "AA:00:00:00:00:32");
        assert!(
            transport.request.lock().unwrap().is_none(),
            "candidate listing must not start discovery or touch BlueZ pairing"
        );
        assert_eq!(store.tombstone(&tombstoned.id), Some(tombstoned));
        assert_eq!(store.get(&active.id), Some(active));
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn pairing_failure_persists_exact_retained_bond_for_explicit_retry() {
        let retained = "AA:00:00:00:00:22";
        let transport = Arc::new(CapturingTransport {
            available: true,
            bond_intent_address: Some(retained.to_ascii_lowercase()),
            pairing_outcome: Mutex::new(Some(HueBlePairingOutcome {
                devices: Vec::new(),
                warnings: vec!["validation failed".to_string()],
                retained_bond_addresses: Vec::new(),
            })),
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("pairing-retained-bond", transport.clone());

        let request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        let error = pair(&state, &request).unwrap_err();

        assert!(format!("{error:#}").contains("validation failed"));
        assert_eq!(
            get_hub_data(&state)
                .unwrap()
                .store
                .pairing_retry_addresses(),
            vec![retained.to_string()]
        );

        let transport_for_retry = transport.request.lock().unwrap().clone().unwrap();
        assert!(transport_for_retry
            .explicit_reassociation_addresses
            .is_empty());
        *transport.pairing_outcome.lock().unwrap() = None;
        assert!(pair(&state, &request).is_err());
        let retry_request = transport.request.lock().unwrap().clone().unwrap();
        assert_eq!(
            retry_request.explicit_reassociation_addresses,
            vec![retained.to_string()]
        );
        assert!(retry_request.replace_stale_bond_addresses.is_empty());

        let recovery_request = HueBlePairingRequest::from_value(&serde_json::json!({
            "replace_stale_bonds": true
        }))
        .unwrap();
        assert!(pair(&state, &recovery_request).is_err());
        let recovery_request = transport.request.lock().unwrap().clone().unwrap();
        assert_eq!(
            recovery_request.replace_stale_bond_addresses,
            vec![retained.to_string()]
        );
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn stale_bond_recovery_candidate_selects_only_one_of_multiple_quarantined_bonds() {
        let selected = "AA:00:00:00:00:31";
        let other = "AA:00:00:00:00:32";
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("pairing-select-stale-bond", transport.clone());
        let store = get_hub_data(&state).unwrap().store.clone();
        store.record_pairing_retry_address(selected).unwrap();
        store.record_pairing_retry_address(other).unwrap();

        let request = HueBlePairingRequest::from_value(&serde_json::json!({
            "candidate_address": selected.to_ascii_lowercase(),
            "replace_stale_bonds": true
        }))
        .unwrap();
        assert!(pair(&state, &request).is_err());
        let captured = transport.request.lock().unwrap().clone().unwrap();
        assert_eq!(
            captured.explicit_reassociation_addresses,
            vec![selected.to_string(), other.to_string()]
        );
        assert_eq!(
            captured.replace_stale_bond_addresses,
            vec![selected.to_string()]
        );
        assert!(
            captured.candidate_address.is_none(),
            "the old quarantined address must not filter post-reset discovery"
        );

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn stale_bond_recovery_requires_selection_when_multiple_bonds_are_quarantined() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("pairing-ambiguous-stale-bond", transport.clone());
        let store = get_hub_data(&state).unwrap().store.clone();
        store
            .record_pairing_retry_address("AA:00:00:00:00:41")
            .unwrap();
        store
            .record_pairing_retry_address("AA:00:00:00:00:42")
            .unwrap();

        let request = HueBlePairingRequest::from_value(&serde_json::json!({
            "replace_stale_bonds": true
        }))
        .unwrap();
        let error = pair(&state, &request).unwrap_err();
        assert!(format!("{error:#}").contains("More than one Hue Bluetooth bond"));
        assert!(
            transport.request.lock().unwrap().is_none(),
            "the transport must not run before stale-bond recovery is unambiguous"
        );

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn stale_bond_recovery_rejects_an_unquarantined_candidate_before_transport() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("pairing-unknown-stale-bond", transport.clone());
        get_hub_data(&state)
            .unwrap()
            .store
            .record_pairing_retry_address("AA:00:00:00:00:51")
            .unwrap();

        let request = HueBlePairingRequest::from_value(&serde_json::json!({
            "candidate_address": "AA:00:00:00:00:99",
            "replace_stale_bonds": true
        }))
        .unwrap();
        let error = pair(&state, &request).unwrap_err();
        assert!(format!("{error:#}").contains("is not a quarantined stale bond"));
        assert!(
            transport.request.lock().unwrap().is_none(),
            "the transport must not run for an arbitrary replacement address"
        );

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn failed_address_rotation_projection_retains_new_metadata_after_stale_key_cleanup() {
        let old_device = stored_device("000052", "AA:00:00:00:00:52");
        let mut replacement = old_device.clone();
        replacement.address = "AA:00:00:00:01:52".to_string();
        let transport = Arc::new(CapturingTransport {
            available: true,
            pairing_outcome: Mutex::new(Some(HueBlePairingOutcome {
                devices: vec![replacement.clone()],
                ..Default::default()
            })),
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("pairing-address-rotation-projection", transport.clone());
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(old_device.clone()).unwrap();
        register_canonical_identity(&state, &old_device).unwrap();
        state.lock().unwrap().prepare_hub_device_room_assignment_fn = Some(Arc::new(|_, _| {
            Err(anyhow::anyhow!(
                "injected canonical materialization failure"
            ))
        }));

        let request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        let error = pair(&state, &request).unwrap_err();

        assert!(format!("{error:#}").contains("injected canonical materialization failure"));
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert_eq!(reloaded.get(&replacement.id), Some(replacement));
        assert!(reloaded.pairing_retry_addresses().is_empty());
        assert_eq!(
            transport.unpaired.lock().unwrap().as_slice(),
            &[old_device.id],
            "the exact stale address must be removed before replacement metadata commits"
        );

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn rotated_tombstone_projection_failure_quarantines_replacement_for_retry() {
        let old_device = stored_device("000054", "AA:00:00:00:00:54");
        let mut replacement = old_device.clone();
        replacement.address = "AA:00:00:00:01:54".to_string();
        let transport = Arc::new(CapturingTransport {
            available: true,
            pairing_outcome: Mutex::new(Some(HueBlePairingOutcome {
                devices: vec![replacement.clone()],
                ..Default::default()
            })),
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("pairing-rotated-tombstone-cleanup", transport.clone());
        let data = get_hub_data(&state).unwrap();
        data.store.record_tombstone(old_device.clone()).unwrap();
        state.lock().unwrap().prepare_hub_device_room_assignment_fn = Some(Arc::new(|_, _| {
            Err(anyhow::anyhow!(
                "injected canonical materialization failure"
            ))
        }));

        let request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        let error = pair(&state, &request).unwrap_err();

        assert!(format!("{error:#}").contains("injected canonical materialization failure"));
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert!(reloaded.get(&replacement.id).is_none());
        assert_eq!(
            reloaded.tombstone(&replacement.id),
            Some(replacement.clone()),
            "the surviving replacement bond must remain an exact explicit-scan retry target"
        );
        assert_eq!(
            transport.unpaired.lock().unwrap().as_slice(),
            &[old_device.id],
            "ordinary nearby scan must not erase the old exact cleanup record before its key is removed"
        );
        assert_eq!(
            transport.bond_removal_deadlines.lock().unwrap().as_slice(),
            &[None],
            "stale address-rotation cleanup must not invent a Hue handoff deadline"
        );

        state.lock().unwrap().prepare_hub_device_room_assignment_fn = None;
        *transport.pairing_outcome.lock().unwrap() = Some(HueBlePairingOutcome {
            devices: vec![replacement.clone()],
            ..Default::default()
        });
        let retry = pair(&state, &request).unwrap();
        assert_eq!(retry.status, PairingStatus::Complete);
        let retry_request = transport.request.lock().unwrap().clone().unwrap();
        assert!(
            retry_request.known_addresses.is_empty(),
            "quarantined B must not be filtered as an already-active bulb"
        );
        assert_eq!(
            retry_request.explicit_reassociation_addresses,
            vec![replacement.address.clone()],
            "the next explicit scan must be allowed to re-adopt B's exact retained bond"
        );
        let committed = HueBleDeviceStore::load(&data_dir).unwrap();
        assert_eq!(committed.get(&replacement.id), Some(replacement.clone()));
        assert!(committed.tombstone(&replacement.id).is_none());

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn address_rotation_never_removes_old_key_when_new_bond_journal_cannot_persist() {
        let old_device = stored_device("000053", "AA:00:00:00:00:53");
        let mut replacement = old_device.clone();
        replacement.address = "AA:00:00:00:01:53".to_string();
        let transport = Arc::new(CapturingTransport {
            available: true,
            pairing_outcome: Mutex::new(Some(HueBlePairingOutcome {
                devices: vec![replacement.clone()],
                ..Default::default()
            })),
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state(
            "pairing-address-rotation-journal-failure",
            transport.clone(),
        );
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(old_device.clone()).unwrap();
        register_canonical_identity(&state, &old_device).unwrap();
        state.lock().unwrap().prepare_hub_device_room_assignment_fn = Some(Arc::new(|_, _| {
            Err(anyhow::anyhow!(
                "injected canonical materialization failure"
            ))
        }));
        // Device metadata remains writable, but both the exact retry journal
        // and global orphan block now fail. The stale key boundary must not be
        // crossed and active A must remain intact.
        std::fs::create_dir_all(data_dir.join("hue_ble").join("unpaired_bonds.json")).unwrap();

        let request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        let error = pair(&state, &request).unwrap_err();

        assert!(format!("{error:#}").contains("could not journal replacement address"));
        std::fs::remove_dir(data_dir.join("hue_ble").join("unpaired_bonds.json")).unwrap();
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert_eq!(
            reloaded.get(&old_device.id),
            Some(old_device),
            "active metadata must not advance past the stale-key boundary"
        );
        assert!(transport.unpaired.lock().unwrap().is_empty());

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn connect_rejects_an_unavailable_bluetooth_adapter() {
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        let transport = Arc::new(CapturingTransport::default());

        let error = match connect(&state, transport) {
            Ok(_) => panic!("unavailable adapter unexpectedly connected"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("adapter is unavailable"));
    }

    #[test]
    fn canonical_endpoint_carries_live_vendor_temperature_capabilities() {
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        let mut device = stored_device("000003", "AA:00:00:00:00:03");
        device.model = "FUTURE-HUE".to_string();
        device.capabilities = HueBleCapabilities {
            dimming: true,
            color_temperature: true,
            xy_color: true,
            combined_control: true,
            effects: true,
            min_mired: Some(50),
            max_mired: Some(1000),
        };

        register_canonical_identity(&state, &device).unwrap();

        let state = state.lock().unwrap();
        let canonical = state
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .unwrap();
        let endpoint = canonical.endpoint_for_hub(&hub_key()).unwrap();
        assert_eq!(
            endpoint.capabilities,
            Some(serde_json::json!({
                "light_capabilities": {
                    "color_temperature": {
                        "min_kelvin": 1000,
                        "max_kelvin": 20000
                    }
                }
            }))
        );
    }

    #[test]
    fn batch_pairing_session_keeps_first_device_compatibility_field() {
        let first = stored_device("000004", "AA:00:00:00:00:04");
        let second = stored_device("000005", "AA:00:00:00:00:05");

        let session = complete_session(
            vec![first.clone(), second.clone()],
            vec!["One farther bulb could not be added".to_string()],
        );

        assert_eq!(session.device.as_ref().unwrap().device_id, first.id);
        assert_eq!(
            session
                .devices
                .iter()
                .map(|device| device.device_id.as_str())
                .collect::<Vec<_>>(),
            vec![first.id.as_str(), second.id.as_str()]
        );
        assert_eq!(
            session.warnings,
            vec!["One farther bulb could not be added"]
        );
    }

    #[test]
    fn pending_missing_metadata_removal_filters_every_registry_projection() {
        use rhythm_os::registry::{RegistrySnapshot, SnapshotDevice, SnapshotRoom};

        let removed_id = "hue-ble-removed".to_string();
        let kept_id = "hue-ble-kept".to_string();
        let mut snapshot = RegistrySnapshot {
            rooms: vec![
                SnapshotRoom {
                    id: removed_id.clone(),
                    name: "Removed".to_string(),
                    grouped_light_id: removed_id.clone(),
                },
                SnapshotRoom {
                    id: kept_id.clone(),
                    name: "Kept".to_string(),
                    grouped_light_id: kept_id.clone(),
                },
            ],
            devices: vec![
                SnapshotDevice {
                    id: removed_id.clone(),
                    room_id: Some(removed_id.clone()),
                    device_type: DeviceType::Motion,
                },
                SnapshotDevice {
                    id: kept_id.clone(),
                    room_id: Some(kept_id.clone()),
                    device_type: DeviceType::Motion,
                },
            ],
            buttons: HashMap::from([
                ("removed-button".to_string(), (removed_id.clone(), 1)),
                ("kept-button".to_string(), (kept_id.clone(), 1)),
            ]),
            area_lights: HashMap::from([
                (
                    removed_id.clone(),
                    vec![removed_id.clone(), kept_id.clone()],
                ),
                (kept_id.clone(), vec![removed_id.clone(), kept_id.clone()]),
            ]),
        };

        filter_removed_from_snapshot(&mut snapshot, &HashSet::from([removed_id.clone()]));

        assert_eq!(
            snapshot
                .rooms
                .iter()
                .map(|room| room.id.as_str())
                .collect::<Vec<_>>(),
            vec![kept_id.as_str()]
        );
        assert_eq!(
            snapshot
                .devices
                .iter()
                .map(|device| device.id.as_str())
                .collect::<Vec<_>>(),
            vec![kept_id.as_str()]
        );
        assert_eq!(snapshot.buttons.len(), 1);
        assert!(snapshot.buttons.contains_key("kept-button"));
        assert!(!snapshot.area_lights.contains_key(&removed_id));
        assert_eq!(
            snapshot.area_lights.get(&kept_id),
            Some(&vec![kept_id.clone()])
        );
    }

    fn connected_test_state(
        label: &str,
        transport: Arc<CapturingTransport>,
    ) -> (SharedState, std::path::PathBuf) {
        let unique = format!(
            "rhythm-hue-ble-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let data_dir = std::env::temp_dir().join(unique);
        let mut app_state = rhythm_os::state::AppState::default();
        app_state.data_dir = data_dir.to_string_lossy().into_owned();
        let state = Arc::new(Mutex::new(app_state));
        let (hub, _events) = connect(&state, transport).unwrap();
        state.lock().unwrap().hubs.insert(hub_key(), hub);
        (state, data_dir)
    }

    fn clean_test_state(state: &SharedState, data_dir: std::path::PathBuf) {
        state.lock().unwrap().hubs.remove(&hub_key());
        if let Err(error) = std::fs::remove_dir_all(data_dir) {
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        }
    }

    #[test]
    fn factory_reset_quiescence_closes_transport_and_joins_observer() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("factory-reset-quiescence", transport.clone());
        let shutdown = state
            .lock()
            .unwrap()
            .hubs
            .get(&hub_key())
            .unwrap()
            .shutdown
            .clone();
        let data = get_hub_data(&state).unwrap();
        let quiescence = capture_factory_reset_quiescence(&state)
            .unwrap()
            .expect("the live Hue BLE hub should expose a reset barrier");

        quiescence.quiesce().unwrap();

        assert!(shutdown.load(Ordering::SeqCst));
        assert!(transport.quiesced.load(Ordering::SeqCst));
        assert!(
            data.observer_thread.lock().unwrap().is_none(),
            "the Hue BLE observer must be joined before BlueZ shutdown"
        );
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn graceful_unpair_removes_bond_then_metadata_using_canonical_id() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("unpair-success", transport.clone());
        let device = stored_device("000006", "AA:00:00:00:00:06");
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();
        register_canonical_identity(&state, &device).unwrap();
        let canonical_id = state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .unwrap()
            .id
            .clone();

        let result = unpair(&state, &canonical_id, false).unwrap();

        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(result.hub_address.as_deref(), Some(HUB_ADDRESS));
        assert_eq!(result.device_id.as_deref(), Some(device.id.as_str()));
        assert_eq!(
            result.completion_scope,
            Some(UnpairingCompletionScope::LocalBondRemoved)
        );
        assert!(result
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("replacement-pairing window")));
        assert_eq!(
            transport.unpaired.lock().unwrap().as_slice(),
            &[device.id.clone()]
        );
        assert_eq!(
            transport.lifecycle_events.lock().unwrap().as_slice(),
            &[
                format!("prepare:{}", device.id),
                format!("remove:{}", device.id)
            ]
        );
        let bond_removal_deadlines = transport.bond_removal_deadlines.lock().unwrap();
        assert_eq!(bond_removal_deadlines.len(), 1);
        assert!(
            bond_removal_deadlines[0].is_some(),
            "single-bulb unpair must propagate its absolute Hue handoff deadline"
        );
        drop(bond_removal_deadlines);
        assert!(data.store.get(&device.id).is_none());
        assert_eq!(data.store.tombstone(&device.id), Some(device.clone()));
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert!(reloaded.get(&device.id).is_none());
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));
        assert!(
            state
                .lock()
                .unwrap()
                .canonical_registry
                .find_by_native_id(&hub_key(), &device.id)
                .is_some(),
            "the shared HTTP handler has not yet committed endpoint cleanup"
        );

        // Simulate a restart in the gap between lifecycle completion and the
        // shared handler's canonical endpoint commit.
        recover_tombstoned_removals(&state, transport.as_ref(), &reloaded);
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_none());
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));

        let repeated = unpair(&state, &device.id, false).unwrap();
        assert_eq!(
            repeated.completion_scope,
            Some(UnpairingCompletionScope::LocalBondRemoved)
        );
        assert!(repeated
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("may have expired")));
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn repeated_offline_force_preserves_prior_handoff_evidence() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("offline-repeat-preserves-handoff", transport);
        let device = stored_device("000024", "AA:00:00:00:00:24");
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();
        register_canonical_identity(&state, &device).unwrap();

        let released = unpair(&state, &device.id, false).unwrap();
        assert_eq!(
            released.completion_scope,
            Some(UnpairingCompletionScope::LocalBondRemoved)
        );
        assert!(HueBleDeviceStore::load(&data_dir)
            .unwrap()
            .handoff_was_prepared(&device.id));

        let repeated = force_forget_offline(&state, &device.id).unwrap();
        assert_eq!(
            repeated.completion_scope,
            Some(UnpairingCompletionScope::LocalStateOnly)
        );
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));
        assert!(
            reloaded.handoff_was_prepared(&device.id),
            "offline cleanup must not downgrade a proven authenticated release"
        );

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn graceful_unpair_failure_preserves_metadata() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            unpair_error: Some("adapter rejected removal".to_string()),
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("unpair-failure", transport);
        let device = stored_device("000007", "AA:00:00:00:00:07");
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();

        let result = unpair(&state, &device.id, false).unwrap();

        assert_eq!(result.status, PairingStatus::Failed);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("adapter rejected removal")));
        assert!(data.store.get(&device.id).is_some());
        assert!(HueBleDeviceStore::load(&data_dir)
            .unwrap()
            .get(&device.id)
            .is_some());
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn bond_deletion_failure_preserves_active_metadata_and_prepared_journal() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            bond_remove_error: Some("BlueZ refused RemoveDevice".to_string()),
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("unpair-bond-delete-failure", transport.clone());
        let device = stored_device("000014", "AA:00:00:00:00:14");
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();

        let result = unpair(&state, &device.id, false).unwrap();

        assert_eq!(result.status, PairingStatus::Failed);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("BlueZ refused RemoveDevice")));
        assert_eq!(
            transport.prepared.lock().unwrap().as_slice(),
            &[device.id.clone()]
        );
        assert!(transport.unpaired.lock().unwrap().is_empty());
        assert_eq!(data.store.get(&device.id), Some(device.clone()));
        assert_eq!(data.store.tombstone(&device.id), Some(device.clone()));
        assert!(data.store.handoff_was_prepared(&device.id));
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert_eq!(reloaded.get(&device.id), Some(device.clone()));
        assert!(reloaded.handoff_was_prepared(&device.id));
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn expired_single_bulb_handoff_never_crosses_local_key_deletion_boundary() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            handoff_age: Some(PAIRING_HANDOFF_FRESHNESS_BUDGET + Duration::from_secs(1)),
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("unpair-expired-handoff", transport.clone());
        let device = stored_device("000057", "AA:00:00:00:00:57");
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();

        let result = unpair(&state, &device.id, false).unwrap();

        assert_eq!(result.status, PairingStatus::Failed);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("window expired")));
        assert!(transport.unpaired.lock().unwrap().is_empty());
        assert!(
            transport.bond_removal_deadlines.lock().unwrap().is_empty(),
            "an already-expired handoff must stop before entering bond removal"
        );
        assert_eq!(data.store.get(&device.id), Some(device.clone()));
        assert!(data.store.handoff_was_prepared(&device.id));

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn forced_unpair_preserves_handoff_proof_after_ambiguous_remove_success() {
        let device = stored_device("000034", "AA:00:00:00:00:34");
        let transport = Arc::new(CapturingTransport {
            available: true,
            bond_remove_error_after_removal_device: Some(device.id.clone()),
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("force-unpair-ambiguous-remove", transport.clone());
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();

        let result = unpair(&state, &device.id, true).unwrap();

        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(
            result.completion_scope,
            Some(UnpairingCompletionScope::LocalBondRemoved)
        );
        assert!(result
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("authenticated Hue handoff")));
        assert!(data.store.get(&device.id).is_none());
        assert_eq!(data.store.tombstone(&device.id), Some(device.clone()));
        assert!(data.store.handoff_was_prepared(&device.id));
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));
        assert!(
            reloaded.handoff_was_prepared(&device.id),
            "forced cleanup must not erase proof that preceded an ambiguous RemoveDevice result"
        );
        assert_eq!(
            transport.unpaired.lock().unwrap().as_slice(),
            &[device.id.clone()]
        );

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn restart_finishes_forced_canonical_cleanup_while_retaining_prepared_bond_quarantine() {
        let device = stored_device("000056", "AA:00:00:00:00:56");
        let transport = Arc::new(CapturingTransport {
            available: true,
            bond_remove_error: Some("BlueZ refused RemoveDevice".to_string()),
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("force-unpair-prepared-restart", transport.clone());
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();
        register_canonical_identity(&state, &device).unwrap();

        let result = unpair(&state, &device.id, true).unwrap();
        assert_eq!(
            result.completion_scope,
            Some(UnpairingCompletionScope::LocalBondRetained)
        );
        assert!(data.store.get(&device.id).is_none());
        assert!(data.store.handoff_was_prepared(&device.id));
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_some());

        recover_tombstoned_removals(&state, transport.as_ref(), data.store.as_ref());

        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_none());
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));
        assert!(reloaded.handoff_was_prepared(&device.id));

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn handoff_journal_failure_never_deletes_the_local_bond() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("unpair-journal-failure", transport.clone());
        let device = stored_device("000019", "AA:00:00:00:00:19");
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();
        std::fs::create_dir_all(data_dir.join("hue_ble").join("unpaired_bonds.json")).unwrap();

        let result = unpair(&state, &device.id, false).unwrap();

        assert_eq!(result.status, PairingStatus::Failed);
        assert_eq!(
            transport.prepared.lock().unwrap().as_slice(),
            &[device.id.clone()]
        );
        assert!(
            transport.unpaired.lock().unwrap().is_empty(),
            "BlueZ deletion must follow a durable journal commit"
        );
        assert_eq!(data.store.get(&device.id), Some(device));
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn restart_finishes_prepared_handoff_only_after_local_bond_is_absent() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("prepared-recovery", transport.clone());
        let device = stored_device("000015", "AA:00:00:00:00:15");
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();
        data.store.record_prepared_handoff(device.clone()).unwrap();
        register_canonical_identity(&state, &device).unwrap();

        recover_tombstoned_removals(&state, transport.as_ref(), data.store.as_ref());
        assert_eq!(data.store.get(&device.id), Some(device.clone()));
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_some());
        assert!(transport.prepared.lock().unwrap().is_empty());
        assert!(transport.unpaired.lock().unwrap().is_empty());

        let absent_transport = CapturingTransport {
            available: true,
            bond_absent: true,
            ..Default::default()
        };
        recover_tombstoned_removals(&state, &absent_transport, data.store.as_ref());

        assert!(data.store.get(&device.id).is_none());
        assert!(data.store.handoff_was_prepared(&device.id));
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_none());
        assert!(absent_transport.prepared.lock().unwrap().is_empty());
        assert!(absent_transport.unpaired.lock().unwrap().is_empty());
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn force_unpair_removes_metadata_when_bond_removal_fails() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            unpair_error: Some("adapter unavailable".to_string()),
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("force-unpair", transport.clone());
        let device = stored_device("000008", "AA:00:00:00:00:08");
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();

        let result = unpair(&state, &device.id, true).unwrap();

        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(
            result.completion_scope,
            Some(UnpairingCompletionScope::LocalBondRetained)
        );
        assert!(result
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("nearby Hue Bluetooth scan")));
        assert!(data.store.get(&device.id).is_none());
        assert_eq!(data.store.tombstone(&device.id), Some(device.clone()));
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert!(reloaded.get(&device.id).is_none());
        assert_eq!(reloaded.tombstone(&device.id), Some(device.clone()));

        let request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        assert!(pair(&state, &request).is_err());
        let captured = transport.request.lock().unwrap().clone().unwrap();
        assert_eq!(
            captured.explicit_reassociation_addresses,
            vec![device.address]
        );
        assert!(captured.blocked_paired_addresses.is_empty());
        assert!(captured.allow_paired_orphan_adoption);
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn repeated_force_with_verified_absent_bond_clears_tombstone_and_unblocks_box_reset() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            bond_absent: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("force-unpair-verified-absent", transport);
        let device = stored_device("000055", "AA:00:00:00:00:55");
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();
        register_canonical_identity(&state, &device).unwrap();

        let result = unpair(&state, &device.id, true).unwrap();

        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(
            result.completion_scope,
            Some(UnpairingCompletionScope::LocalStateOnly)
        );
        assert!(result
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("no longer has")));
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert!(reloaded.get(&device.id).is_none());
        assert!(
            reloaded.tombstone(&device.id).is_none(),
            "explicit force plus live absent-bond verification resolves the conservative tombstone"
        );
        assert_eq!(reloaded.removed_native_ids(), vec![device.id.clone()]);
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_some());
        let repeated_online = unpair(&state, &device.id, false).unwrap();
        assert_eq!(
            repeated_online.completion_scope,
            Some(UnpairingCompletionScope::LocalStateOnly)
        );
        let repeated_offline = force_forget_offline(&state, &device.id).unwrap();
        assert_eq!(
            repeated_offline.completion_scope,
            Some(UnpairingCompletionScope::LocalStateOnly)
        );
        let after_retries = HueBleDeviceStore::load(&data_dir).unwrap();
        assert_eq!(
            after_retries.removed_native_ids(),
            vec![device.id.clone()],
            "idempotent retries must retain verified-absent cleanup proof"
        );
        assert!(after_retries.blocked_native_ids().is_empty());
        assert!(
            !after_retries.blocks_paired_orphan_adoption(),
            "verified absence must not be downgraded into a global unknown-bond block"
        );
        recover_tombstoned_removals(&state, data.transport.as_ref(), &reloaded);
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_none());
        assert_eq!(
            prepare_for_factory_reset(&state).unwrap(),
            0,
            "a discarded or physically reset absent bulb must not permanently block appliance reset"
        );

        clean_test_state(&state, data_dir);
    }

    #[test]
    fn factory_reset_validates_every_known_bond_without_opening_transfer_windows() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("factory-reset-release", transport.clone());
        let first = stored_device("000016", "AA:00:00:00:00:16");
        let second = stored_device("000017", "AA:00:00:00:00:17");
        let data = get_hub_data(&state).unwrap();
        for device in [&first, &second] {
            data.store.upsert(device.clone()).unwrap();
            register_canonical_identity(&state, device).unwrap();
        }

        let validated = prepare_for_factory_reset(&state).unwrap();

        assert_eq!(validated, 2);
        assert_eq!(
            transport.validated.lock().unwrap().as_slice(),
            &[first.id.clone(), second.id.clone()]
        );
        assert!(
            transport.prepared.lock().unwrap().is_empty(),
            "preflight must not mutate a bulb's replacement-pairing state"
        );
        assert!(transport.unpaired.lock().unwrap().is_empty());
        let plan = HueBleDeviceStore::load_factory_reset_plan(&data_dir).unwrap();
        assert_eq!(plan.devices, vec![first.clone(), second.clone()]);
        assert!(plan.prepared_device_ids.is_empty());
        assert!(plan.released_device_ids.is_empty());
        for device in [&first, &second] {
            assert_eq!(data.store.get(&device.id), Some(device.clone()));
            assert!(data.store.tombstone(&device.id).is_none());
            assert!(state
                .lock()
                .unwrap()
                .canonical_registry
                .find_by_native_id(&hub_key(), &device.id)
                .is_some());
        }
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn final_factory_reset_refreshes_every_window_before_any_bond_deletion() {
        let first = stored_device("000026", "AA:00:00:00:00:26");
        let second = stored_device("000027", "AA:00:00:00:00:27");
        let transport = CapturingTransport {
            available: true,
            local_hue_bonds: vec![first.address.clone(), second.address.clone()],
            ..Default::default()
        };
        let data_dir = std::env::temp_dir().join(format!(
            "rhythm-hue-ble-final-reset-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        HueBleDeviceStore::persist_factory_reset_plan(
            &data_dir,
            &factory_reset_plan(vec![first.clone(), second.clone()]),
        )
        .unwrap();

        let released = complete_factory_reset_bond_release_with_transport(
            data_dir.to_str().unwrap(),
            &transport,
        )
        .unwrap();

        assert_eq!(released.refreshed, 2);
        assert!(released.valid_until.is_some());
        assert_eq!(
            transport.lifecycle_events.lock().unwrap().as_slice(),
            &[
                format!("prepare:{}", first.id),
                format!("prepare:{}", second.id),
            ]
        );
        assert!(transport.unpaired.lock().unwrap().is_empty());
        let plan = HueBleDeviceStore::load_factory_reset_plan(&data_dir).unwrap();
        assert_eq!(plan.devices, vec![first.clone(), second.clone()]);
        assert_eq!(
            plan.prepared_device_ids,
            vec![first.id.clone(), second.id.clone()]
        );
        assert!(plan.released_device_ids.is_empty());
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn final_factory_reset_handoff_failure_retains_every_local_bond_for_retry() {
        let first = stored_device("000028", "AA:00:00:00:00:28");
        let second = stored_device("000029", "AA:00:00:00:00:29");
        let data_dir = std::env::temp_dir().join(format!(
            "rhythm-hue-ble-final-reset-retry-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        HueBleDeviceStore::persist_factory_reset_plan(
            &data_dir,
            &factory_reset_plan(vec![first.clone(), second.clone()]),
        )
        .unwrap();
        let transport = CapturingTransport {
            available: true,
            local_hue_bonds: vec![first.address.clone(), second.address.clone()],
            handoff_error_device: Some(second.id.clone()),
            ..Default::default()
        };

        let error = complete_factory_reset_bond_release_with_transport(
            data_dir.to_str().unwrap(),
            &transport,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("selected bulb is offline"));
        let partial_plan = HueBleDeviceStore::load_factory_reset_plan(&data_dir).unwrap();
        assert_eq!(partial_plan.devices, vec![first.clone(), second.clone()]);
        assert_eq!(partial_plan.prepared_device_ids, vec![first.id.clone()]);
        assert!(partial_plan.released_device_ids.is_empty());
        assert!(transport.unpaired.lock().unwrap().is_empty());

        let retry_transport = CapturingTransport {
            available: true,
            local_hue_bonds: vec![first.address.clone(), second.address.clone()],
            ..Default::default()
        };
        let released = complete_factory_reset_bond_release_with_transport(
            data_dir.to_str().unwrap(),
            &retry_transport,
        )
        .unwrap();
        assert_eq!(released.refreshed, 2);
        assert!(released.valid_until.is_some());
        let completed_plan = HueBleDeviceStore::load_factory_reset_plan(&data_dir).unwrap();
        assert_eq!(completed_plan.devices, vec![first.clone(), second.clone()]);
        assert_eq!(
            completed_plan.prepared_device_ids,
            vec![first.id.clone(), second.id.clone()]
        );
        assert!(completed_plan.released_device_ids.is_empty());
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn factory_reset_failure_leaves_unreleased_bulb_and_installation_metadata() {
        let device = stored_device("000018", "AA:00:00:00:00:18");
        let transport = Arc::new(CapturingTransport {
            available: true,
            handoff_error_device: Some(device.id.clone()),
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("factory-reset-release-failure", transport.clone());
        let data = get_hub_data(&state).unwrap();
        data.store.upsert(device.clone()).unwrap();
        register_canonical_identity(&state, &device).unwrap();

        let error = prepare_for_factory_reset(&state).unwrap_err();

        assert!(format!("{error:#}").contains("bulb is offline"));
        assert_eq!(data.store.get(&device.id), Some(device.clone()));
        assert!(data.store.tombstone(&device.id).is_none());
        assert!(transport.unpaired.lock().unwrap().is_empty());
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_some());
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn later_factory_reset_validation_failure_never_opens_earlier_bulb_window() {
        let first = stored_device("000023", "AA:00:00:00:00:23");
        let second = stored_device("000024", "AA:00:00:00:00:24");
        let transport = Arc::new(CapturingTransport {
            available: true,
            handoff_error_device: Some(second.id.clone()),
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("factory-reset-partial-preflight", transport.clone());
        let data = get_hub_data(&state).unwrap();
        for device in [&first, &second] {
            data.store.upsert(device.clone()).unwrap();
            register_canonical_identity(&state, device).unwrap();
        }

        let error = prepare_for_factory_reset(&state).unwrap_err();

        assert!(format!("{error:#}").contains("selected bulb is offline"));
        assert_eq!(
            transport.validated.lock().unwrap().as_slice(),
            &[first.id.clone()]
        );
        assert!(transport.prepared.lock().unwrap().is_empty());
        assert!(transport.unpaired.lock().unwrap().is_empty());
        for device in [&first, &second] {
            assert_eq!(data.store.get(&device.id), Some(device.clone()));
            assert!(data.store.tombstone(&device.id).is_none());
            assert!(state
                .lock()
                .unwrap()
                .canonical_registry
                .find_by_native_id(&hub_key(), &device.id)
                .is_some());
        }
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn factory_reset_can_proceed_past_stale_unknown_metadata_when_no_unknown_bond_exists() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("factory-reset-unknown-bond", transport.clone());
        let data = get_hub_data(&state).unwrap();
        data.store
            .record_missing_metadata_removal("hue-ble-unknown")
            .unwrap();

        let prepared = prepare_for_factory_reset(&state).unwrap();

        assert_eq!(prepared, 0);
        assert!(transport.prepared.lock().unwrap().is_empty());
        assert!(transport.unpaired.lock().unwrap().is_empty());
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn factory_reset_fails_closed_for_an_untracked_bluez_hue_bond() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            local_hue_bonds: vec!["EB:01:B4:6B:01:34".to_string()],
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("factory-reset-untracked-bond", transport.clone());

        let error = prepare_for_factory_reset(&state).unwrap_err();

        let message = format!("{error:#}");
        assert!(message.contains("recovering reset-only metadata"));
        assert!(message.contains("EB:01:B4:6B:01:34"));
        assert!(transport.prepared.lock().unwrap().is_empty());
        assert!(transport.unpaired.lock().unwrap().is_empty());
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn factory_reset_recovers_untracked_bond_metadata_without_normal_adoption() {
        let device = stored_device("000030", "EB:01:B4:6B:01:34");
        let transport = Arc::new(CapturingTransport {
            available: true,
            local_hue_bonds: vec![device.address.clone()],
            local_bond_devices: vec![device.clone()],
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("factory-reset-recover-untracked-bond", transport.clone());

        let prepared = prepare_for_factory_reset(&state).unwrap();

        assert_eq!(prepared, 1);
        assert_eq!(
            transport.validated.lock().unwrap().as_slice(),
            &[device.id.clone()]
        );
        assert!(transport.prepared.lock().unwrap().is_empty());
        let plan = HueBleDeviceStore::load_factory_reset_plan(&data_dir).unwrap();
        assert_eq!(plan.devices, vec![device.clone()]);
        assert!(get_hub_data(&state)
            .unwrap()
            .store
            .get(&device.id)
            .is_none());
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn factory_reset_fails_when_known_bulb_key_is_absent_without_handoff_proof() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            bond_absent: true,
            ..Default::default()
        });
        let (state, data_dir) =
            connected_test_state("factory-reset-unproven-absent-bond", transport);
        let device = stored_device("000031", "AA:00:00:00:00:31");
        get_hub_data(&state)
            .unwrap()
            .store
            .upsert(device.clone())
            .unwrap();

        let error = prepare_for_factory_reset(&state).unwrap_err();

        assert!(format!("{error:#}").contains("no successful Hue handoff was recorded"));
        assert!(HueBleDeviceStore::load_factory_reset_plan(&data_dir)
            .unwrap()
            .devices
            .is_empty());
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn restart_finishes_tombstone_first_force_removal_after_crash() {
        let unique = format!(
            "rhythm-hue-ble-force-crash-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let data_dir = std::env::temp_dir().join(unique);
        let mut app_state = rhythm_os::state::AppState::default();
        app_state.data_dir = data_dir.to_string_lossy().into_owned();
        let state = Arc::new(Mutex::new(app_state));
        let device = stored_device("000011", "AA:00:00:00:00:11");
        let store = HueBleDeviceStore::load(&data_dir).unwrap();
        store.upsert(device.clone()).unwrap();
        register_canonical_identity(&state, &device).unwrap();
        // Simulate power loss after tombstone persistence but before active
        // metadata and canonical endpoint cleanup.
        store.record_tombstone(device.clone()).unwrap();
        assert!(store.get(&device.id).is_some());
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_some());
        let transport = CapturingTransport {
            available: true,
            unpair_error: Some("temporary BlueZ failure".to_string()),
            ..Default::default()
        };

        recover_tombstoned_removals(&state, &transport, &store);

        assert!(store.get(&device.id).is_none());
        assert_eq!(store.tombstone(&device.id), Some(device.clone()));
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_none());
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert!(reloaded.get(&device.id).is_none());
        assert_eq!(reloaded.tombstone(&device.id), Some(device));
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn restart_commits_interrupted_reassociation_instead_of_finishing_removal() {
        let unique = format!(
            "rhythm-hue-ble-reassociation-crash-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let data_dir = std::env::temp_dir().join(unique);
        let mut app_state = rhythm_os::state::AppState::default();
        app_state.data_dir = data_dir.to_string_lossy().into_owned();
        let state = Arc::new(Mutex::new(app_state));
        let device = stored_device("000021", "AA:00:00:00:00:21");
        let store = HueBleDeviceStore::load(&data_dir).unwrap();
        store.upsert(device.clone()).unwrap();
        store.record_tombstone(device.clone()).unwrap();
        store.begin_reassociation_if_needed(&device).unwrap();
        register_canonical_identity(&state, &device).unwrap();
        let transport = CapturingTransport {
            available: true,
            ..Default::default()
        };

        recover_tombstoned_removals(&state, &transport, &store);

        assert_eq!(store.get(&device.id), Some(device.clone()));
        assert!(store.tombstone(&device.id).is_none());
        assert!(!store.reassociation_was_started(&device.id));
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_some());
        assert!(transport.prepared.lock().unwrap().is_empty());
        assert!(transport.unpaired.lock().unwrap().is_empty());
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn unpair_is_idempotent_when_metadata_is_already_absent() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("unpair-idempotent", transport.clone());
        let device = stored_device("000009", "AA:00:00:00:00:09");

        let result = unpair(&state, &device.id, false).unwrap();

        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(
            result.completion_scope,
            Some(UnpairingCompletionScope::AlreadyAbsent)
        );
        assert!(result.warning.is_none());
        assert!(transport.unpaired.lock().unwrap().is_empty());
        assert!(!get_hub_data(&state)
            .unwrap()
            .store
            .blocks_paired_orphan_adoption());
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn missing_metadata_for_known_endpoint_blocks_all_paired_orphan_adoption() {
        let transport = Arc::new(CapturingTransport {
            available: true,
            ..Default::default()
        });
        let (state, data_dir) = connected_test_state("unpair-missing-metadata", transport.clone());
        let device = stored_device("000012", "AA:00:00:00:00:12");
        register_canonical_identity(&state, &device).unwrap();

        let graceful = unpair(&state, &device.id, false).unwrap();

        assert_eq!(graceful.status, PairingStatus::Failed);
        assert!(graceful
            .error
            .as_deref()
            .is_some_and(|error| error.contains("retry with force")));
        assert!(transport.unpaired.lock().unwrap().is_empty());
        assert!(get_hub_data(&state)
            .unwrap()
            .store
            .blocks_paired_orphan_adoption());
        assert!(get_hub_data(&state)
            .unwrap()
            .store
            .blocked_native_ids()
            .is_empty());
        assert!(HueBleDeviceStore::load(&data_dir)
            .unwrap()
            .blocks_paired_orphan_adoption());

        let forced = unpair(&state, &device.id, true).unwrap();
        assert_eq!(forced.status, PairingStatus::Complete);
        assert_eq!(
            forced.completion_scope,
            Some(UnpairingCompletionScope::LocalStateOnly)
        );
        assert!(forced.warning.is_some());
        assert_eq!(
            HueBleDeviceStore::load(&data_dir)
                .unwrap()
                .blocked_native_ids(),
            vec![device.id.clone()]
        );
        rhythm_os::commands::do_device_endpoint_remove(&state, &device.id, &hub_key()).unwrap();

        let repeated = unpair(&state, &device.id, true).unwrap();

        assert_eq!(
            repeated.completion_scope,
            Some(UnpairingCompletionScope::LocalStateOnly)
        );
        assert!(repeated.warning.is_some());
        clean_test_state(&state, data_dir);
    }

    #[test]
    fn offline_force_forget_removes_valid_metadata_without_a_live_hub() {
        let unique = format!(
            "rhythm-hue-ble-offline-force-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let data_dir = std::env::temp_dir().join(unique);
        let mut app_state = rhythm_os::state::AppState::default();
        app_state.data_dir = data_dir.to_string_lossy().into_owned();
        let state = Arc::new(Mutex::new(app_state));
        let device = stored_device("000010", "AA:00:00:00:00:10");
        let store = HueBleDeviceStore::load(&data_dir).unwrap();
        store.upsert(device.clone()).unwrap();

        let result = force_forget_offline(&state, &device.id).unwrap();

        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(
            result.completion_scope,
            Some(UnpairingCompletionScope::LocalStateOnly)
        );
        assert!(result.warning.is_some());
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert!(reloaded.get(&device.id).is_none());
        assert_eq!(reloaded.tombstone(&device.id), Some(device));
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn offline_force_forget_is_idempotent_when_every_record_is_already_absent() {
        let unique = format!(
            "rhythm-hue-ble-offline-already-absent-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let data_dir = std::env::temp_dir().join(unique);
        let mut app_state = rhythm_os::state::AppState::default();
        app_state.data_dir = data_dir.to_string_lossy().into_owned();
        let state = Arc::new(Mutex::new(app_state));
        let device = stored_device("000025", "AA:00:00:00:00:25");

        let result = force_forget_offline(&state, &device.id).unwrap();

        assert_eq!(
            result.completion_scope,
            Some(UnpairingCompletionScope::AlreadyAbsent)
        );
        assert!(result.warning.is_none());
        std::fs::remove_dir_all(data_dir).ok();
    }
}
