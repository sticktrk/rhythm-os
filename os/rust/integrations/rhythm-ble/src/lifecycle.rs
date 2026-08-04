//! Shared local-BLE persistence, pairing, and canonical-device lifecycle.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::pairing::{
    complete_pairing_result_with_fingerprint, reconcile_committed_pairing_success_with_fingerprint,
    PairedDeviceInfo, PairingSession, PairingStage, PairingStatus, UnpairingCompletionScope,
    UnpairingResult,
};
use rhythm_os::state::SharedState;

use crate::discovery::LocalBleDiscovery;
use crate::profile::profile_by_id;
use crate::store::{latch_reset_guard, LocalBleDevice, LocalBleDeviceStore};
use crate::transport::{LocalBleAssociationError, LocalBleTransport};
use crate::{HUB_ADDRESS, HUB_TYPE};

const ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(60);
const FINALIZATION_RESERVE: Duration = Duration::from_secs(15);
pub const PAIRING_SERVER_SLA: Duration = Duration::from_secs(120);

pub struct LocalBleHubData {
    pub transport: Arc<dyn LocalBleTransport>,
    pub store: Arc<LocalBleDeviceStore>,
    event_tx: Sender<HubEvent>,
    monitor_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
    monitor_shutdown: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

pub fn hub_key() -> HubKey {
    HubKey::new(HubType::new(HUB_TYPE), HUB_ADDRESS)
}

fn data_dir(state: &SharedState) -> Result<String> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if state.data_dir.trim().is_empty() {
        anyhow::bail!("data_dir not configured on AppState");
    }
    Ok(state.data_dir.clone())
}

fn remaining_pairing_budget(deadline: Instant, stage: &str) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| anyhow::anyhow!("local Bluetooth pairing deadline expired before {stage}"))
}

fn association_deadline(deadline: Instant) -> Result<Instant> {
    association_deadline_at(Instant::now(), deadline)
}

fn association_deadline_at(now: Instant, pairing_deadline: Instant) -> Result<Instant> {
    let latest = pairing_deadline
        .checked_sub(FINALIZATION_RESERVE)
        .filter(|latest| *latest > now)
        .ok_or_else(|| {
            anyhow::anyhow!("local Bluetooth pairing deadline left no finalization budget")
        })?;
    Ok(now
        .checked_add(ASSOCIATION_TIMEOUT)
        .unwrap_or(latest)
        .min(latest))
}

/// The local profile store is activation authority. If a process stopped after
/// projecting a new endpoint but before committing that store, remove the
/// orphan before the hub registry or monitor becomes visible again. Existing
/// active records remain untouched during re-pair and therefore survive the
/// same crash window.
fn reconcile_orphaned_canonical_projections(
    state: &SharedState,
    active_device_ids: &HashSet<String>,
) -> Result<()> {
    let key = hub_key();
    let orphaned = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        state
            .canonical_registry
            .all_devices_including_removed()
            .flat_map(|device| device.endpoints.iter())
            .filter(|endpoint| {
                endpoint.hub_key == key && !active_device_ids.contains(&endpoint.native_id)
            })
            .map(|endpoint| endpoint.native_id.clone())
            .collect::<Vec<_>>()
    };
    for native_id in orphaned {
        rhythm_os::commands::do_device_endpoint_remove(state, &native_id, &key)?;
    }
    Ok(())
}

fn terminal_session_for_device(device: &LocalBleDevice) -> Result<PairingSession> {
    let projection = profile_by_id(&device.profile_id)
        .ok_or_else(|| anyhow::anyhow!("activation receipt names an unsupported BLE profile"))?
        .projection();
    let info = PairedDeviceInfo {
        device_id: device.id.clone(),
        name: projection.display_name.to_string(),
        device_type: projection.device_type,
        manufacturer: projection.manufacturer.map(str::to_string),
        model: projection.model.map(str::to_string),
    };
    Ok(PairingSession {
        hub_type: HUB_TYPE.to_string(),
        status: PairingStatus::Complete,
        device: Some(info.clone()),
        devices: vec![info],
        error: None,
        failure_stage: None,
        warnings: Vec::new(),
        details: None,
    })
}

/// Resolve a repeat QR scan from authoritative local state without reopening
/// adapter association. The private profile identity never crosses this
/// boundary; callers still receive only the appliance-local public device ID.
fn terminal_session_for_existing_device(
    state: &SharedState,
    store: &LocalBleDeviceStore,
    profile_id: &str,
    stable_identity: &str,
) -> Result<Option<PairingSession>> {
    let Some(existing) = store
        .get_by_identity(profile_id, stable_identity)
        .filter(|device| !device.blocked)
    else {
        return Ok(None);
    };
    let canonical_exists = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .canonical_registry
        .find_by_native_id(&hub_key(), &existing.id)
        .is_some();
    if !canonical_exists {
        return Ok(None);
    }

    let mut session = terminal_session_for_device(&existing)?;
    session.details = Some(serde_json::json!({
        "profile_id": profile_id,
        "existing": true,
    }));
    Ok(Some(session))
}

/// Drain the activation outbox before consulting BlueZ. A committed device
/// association is authoritative even when the prior process died before its
/// HTTP response or pairing-history write.
fn reconcile_activation_receipts(state: &SharedState, store: &LocalBleDeviceStore) -> Result<()> {
    store.with_reset_safe_operation(|| {
        for receipt in store.pending_activation_receipts()? {
            let device = store
                .get_by_id(&receipt.device_id)
                .filter(|device| device.profile_id == receipt.profile_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "local Bluetooth activation receipt has no authoritative device record"
                    )
                })?;
            let session = terminal_session_for_device(&device)?;
            reconcile_committed_pairing_success_with_fingerprint(
                state,
                &receipt.session_id,
                HUB_TYPE,
                &receipt.request_fingerprint,
                &session,
            )?;
            store.acknowledge_activation_receipt(
                &receipt.session_id,
                &receipt.request_fingerprint,
            )?;
        }
        Ok(())
    })
}

/// Repair committed activation receipts without consulting BlueZ. Pairing
/// status polling calls this while the process is live, so transient ledger
/// failures do not require an OS restart before the app can recover success.
pub fn reconcile_pending_pairing_results(state: &SharedState) -> Result<()> {
    let store = LocalBleDeviceStore::load_shared(data_dir(state)?)?;
    reconcile_activation_receipts(state, &store)
}

pub fn get_hub_data(state: &SharedState) -> Result<Arc<LocalBleHubData>> {
    let key = hub_key();
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .hubs
        .get(&key)
        .and_then(|hub| hub.data::<Arc<LocalBleHubData>>())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("local Bluetooth integration is not connected"))
}

pub fn connect(
    state: &SharedState,
    transport: Arc<dyn LocalBleTransport>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    connect_with_deadline(state, transport, None)
}

/// Connect while charging adapter admission and health validation to an
/// already-running caller budget. Pairing uses this path so first-boot hub
/// setup cannot consume a fresh transport timeout before association begins.
pub fn connect_until(
    state: &SharedState,
    transport: Arc<dyn LocalBleTransport>,
    deadline: Instant,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    connect_with_deadline(state, transport, Some(deadline))
}

fn connect_with_deadline(
    state: &SharedState,
    transport: Arc<dyn LocalBleTransport>,
    deadline: Option<Instant>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    // Store authority and orphan repair are adapter-independent. Reconcile
    // them even when BlueZ is down so a crash-staged canonical endpoint cannot
    // remain visible until the radio recovers.
    let store = LocalBleDeviceStore::load_shared(data_dir(state)?)?;
    reconcile_activation_receipts(state, &store)?;
    let active_device_ids = store.with_reset_safe_operation(|| {
        let active_device_ids = store
            .all()
            .into_iter()
            .map(|device| device.id.clone())
            .collect::<HashSet<_>>();
        reconcile_orphaned_canonical_projections(state, &active_device_ids)?;
        Ok(active_device_ids)
    })?;

    let available = match deadline {
        Some(deadline) => transport.is_available_until(deadline)?,
        None => transport.is_available()?,
    };
    if !available {
        anyhow::bail!("Bluetooth adapter is unavailable");
    }

    let key = hub_key();
    let mut snapshot = state
        .lock()
        .ok()
        .and_then(|state| {
            state
                .storage
                .as_ref()?
                .load_hub_registry_for(&key)
                .ok()
                .flatten()
        })
        .and_then(|value| {
            serde_json::from_value::<rhythm_os::registry::RegistrySnapshot>(value).ok()
        });
    if let Some(registry) = snapshot.as_mut() {
        registry
            .devices
            .retain(|device| active_device_ids.contains(&device.id));
        registry
            .buttons
            .retain(|_, (device_id, _)| active_device_ids.contains(device_id));
    }
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let monitor_thread = Arc::new(Mutex::new(None));
    let monitor_shutdown = Arc::new(Mutex::new(None));

    let data_transport = transport.clone();
    let data_store = store.clone();
    let data_monitor_thread = monitor_thread.clone();
    let data_monitor_shutdown = monitor_shutdown.clone();
    let data_event_tx = event_tx.clone();
    let monitor_transport = transport;
    let monitor_store = store.clone();
    let monitor_thread_slot = monitor_thread;
    let monitor_shutdown_slot = monitor_shutdown;
    let (mut hub, event_rx) = rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new(HUB_TYPE),
        key,
        true,
        snapshot,
        move |_registry| {
            Box::new(Arc::new(LocalBleHubData {
                transport: data_transport,
                store: data_store,
                event_tx: data_event_tx,
                monitor_thread: data_monitor_thread,
                monitor_shutdown: data_monitor_shutdown,
            }))
        },
        move |registry, shutdown| {
            if let Ok(mut slot) = monitor_shutdown_slot.lock() {
                *slot = Some(shutdown.clone());
            }
            if monitor_store
                .register_registry_and_reconcile(&registry)
                .is_err()
            {
                let _ = event_tx.send(HubEvent::Disconnected {
                    hub_key: None,
                    reason: "Local Bluetooth registry could not be restored".to_string(),
                });
                return event_rx;
            }
            match monitor_transport.start_monitor(
                monitor_store,
                registry,
                event_tx.clone(),
                shutdown,
            ) {
                Ok(thread) => {
                    if let Ok(mut slot) = monitor_thread_slot.lock() {
                        *slot = Some(thread);
                    }
                }
                Err(_) => {
                    let _ = event_tx.send(HubEvent::Disconnected {
                        hub_key: None,
                        reason: "Local Bluetooth observer could not start".to_string(),
                    });
                }
            }
            event_rx
        },
    )?;
    hub.discovery = Some(Arc::new(LocalBleDiscovery::new(store)));
    Ok((hub, event_rx))
}

pub fn pair(
    state: &SharedState,
    profile_id: &str,
    setup_value: &serde_json::Value,
    session_id: &str,
    request_fingerprint: &str,
    deadline: Instant,
) -> Result<PairingSession> {
    remaining_pairing_budget(deadline, "profile validation")?;
    let profile = profile_by_id(profile_id)
        .ok_or_else(|| anyhow::anyhow!("unsupported local Bluetooth profile"))?;
    // Compatible intake IDs select the one current decoder, but never become
    // durable or public device identity.
    let profile_id = profile.descriptor().id;
    let setup = profile.parse_pairing_setup(setup_value)?;
    let data = get_hub_data(state)?;
    if let Some(session) = terminal_session_for_existing_device(
        state,
        &data.store,
        profile_id,
        setup.stable_identity(),
    )? {
        rhythm_os::pairing::emit_pairing_progress(
            state,
            HUB_TYPE,
            Some(session_id),
            PairingStatus::Found,
            PairingStage::Finalizing,
            "Existing Bluetooth device found",
            session.device.clone(),
            None,
        );
        return Ok(session);
    }
    let association_deadline = association_deadline(deadline)?;
    let mut announce_listener_ready = || {
        rhythm_os::pairing::emit_pairing_progress(
            state,
            HUB_TYPE,
            Some(session_id),
            PairingStatus::Searching,
            PairingStage::Searching,
            "Bluetooth listener ready; put the device in pairing mode now",
            None,
            None,
        );
    };
    let candidate = data
        .transport
        .associate_until_with_readiness(
            profile_id,
            &setup,
            association_deadline,
            &mut announce_listener_ready,
        )
        .map_err(|error| match error.downcast::<LocalBleAssociationError>() {
            Ok(error) => anyhow::Error::new(error.into_pairing_failure()),
            Err(_) => anyhow::Error::new(rhythm_os::pairing::PairingFailure::new(
                rhythm_os::pairing::PairingFailureStage::Transport,
                "The Rhythm Box Bluetooth service was unavailable. Restart the Rhythm Box and try again.",
            )),
        })?;
    remaining_pairing_budget(deadline, "device finalization")?;
    rhythm_os::pairing::emit_pairing_progress(
        state,
        HUB_TYPE,
        Some(session_id),
        PairingStatus::Commissioning,
        PairingStage::Finalizing,
        "Saving the Bluetooth device",
        None,
        None,
    );

    // Pairing finalization is one reset-safe transaction. If reset closes the
    // path first, no canonical projection starts. If finalization starts first,
    // reset waits until both the projection and its durable activation either
    // commit together or finish rolling back.
    data.store.with_reset_safe_operation(|| {
        let previous = data
            .store
            .get_by_identity(profile_id, setup.stable_identity());
        let mut initial_replay = candidate.initial_replay;
        if let Some(previous) = previous.as_ref() {
            for (stream, value) in &previous.replay_state {
                initial_replay
                    .entry(stream.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        let mut device = LocalBleDevice::from_setup(
            profile_id,
            &setup,
            candidate.transport_hint,
            initial_replay,
            false,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        if let Some(previous) = previous.as_ref() {
            device.id.clone_from(&previous.id);
        }

        let existing_room = state.lock().ok().and_then(|state| {
            state
                .canonical_registry
                .find_by_native_id(&hub_key(), &device.id)
                .and_then(|device| device.room_id.clone())
        });
        let projection = profile.projection();
        let had_canonical_endpoint = state.lock().ok().is_some_and(|state| {
            state
                .canonical_registry
                .find_by_native_id(&hub_key(), &device.id)
                .is_some()
        });
        let project = register_canonical_identity(state, &device).and_then(|canonical_id| {
            rhythm_os::commands::do_device_set(
                state,
                &device.id,
                existing_room.as_deref(),
                &device_buttons(&device, &projection),
                projection.device_type.clone(),
                &hub_key(),
                true,
            )?;
            rhythm_os::commands::do_canonical_assign_room(
                state,
                &canonical_id,
                existing_room.as_deref(),
            )
        });
        if let Err(project_error) = project {
            let endpoint_rollback = if !had_canonical_endpoint {
                rhythm_os::commands::do_device_endpoint_remove(state, &device.id, &hub_key())
            } else {
                Ok(())
            };
            if endpoint_rollback.is_err() {
                anyhow::bail!(
                    "local Bluetooth device projection failed and its endpoint rollback was incomplete"
                );
            }
            let _ = project_error;
            anyhow::bail!("local Bluetooth device projection failed and was rolled back");
        }

        if let Err(deadline_error) = remaining_pairing_budget(deadline, "activation commit") {
            if !had_canonical_endpoint
                && rhythm_os::commands::do_device_endpoint_remove(state, &device.id, &hub_key())
                    .is_err()
            {
                anyhow::bail!(
                    "local Bluetooth pairing expired and its endpoint rollback was incomplete"
                );
            }
            return Err(deadline_error);
        }
        let committed_device = match data.store.upsert_activation_until(
            device.clone(),
            session_id,
            request_fingerprint,
            deadline,
        ) {
            Ok(device) => device,
            Err(_) => {
            if !had_canonical_endpoint
                && rhythm_os::commands::do_device_endpoint_remove(state, &device.id, &hub_key())
                    .is_err()
            {
                anyhow::bail!(
                    "local Bluetooth activation commit failed and its endpoint rollback was incomplete"
                );
            }
            anyhow::bail!("local Bluetooth association could not be saved and was rolled back");
            }
        };
        let warnings = data
            .store
            .durability_degraded()
            .then(|| {
                "Bluetooth association was saved, but storage durability acknowledgement is degraded"
                    .to_string()
            })
            .into_iter()
            .collect::<Vec<_>>();

        let _ = data.event_tx.send(HubEvent::DevicePaired {
            hub_key: None,
            device_id: committed_device.id.clone(),
            name: projection.display_name.to_string(),
            device_type: projection.device_type.clone(),
        });
        let mut session = terminal_session_for_device(&committed_device)?;
        session.details = Some(serde_json::json!({ "profile_id": profile_id }));
        session.warnings = warnings;

        // The activation receipt is an outbox: acknowledge it only after the
        // core terminal ledger durably accepts the same fingerprint-bound
        // success. Failure here cannot turn an authoritative association into
        // a failed pairing; retain the receipt for startup repair instead.
        match complete_pairing_result_with_fingerprint(
            state,
            session_id,
            HUB_TYPE,
            request_fingerprint,
            &session,
        ) {
            Ok(()) => {
                if let Err(error) = data
                    .store
                    .acknowledge_activation_receipt(session_id, request_fingerprint)
                {
                    log::warn!(
                        target: "pair",
                        "Local Bluetooth activation receipt acknowledgement will retry: {error:#}"
                    );
                }
            }
            Err(error) => {
                log::warn!(
                    target: "pair",
                    "Local Bluetooth terminal receipt will retry from the activation outbox: {error:#}"
                );
                session.warnings.push(
                    "Bluetooth association was saved; confirmation recovery is still pending"
                        .to_string(),
                );
            }
        }
        Ok(session)
    })
}

fn register_canonical_identity(state: &SharedState, device: &LocalBleDevice) -> Result<String> {
    let projection = profile_by_id(&device.profile_id)
        .ok_or_else(|| anyhow::anyhow!("unsupported local Bluetooth profile"))?
        .projection();
    let identity = DiscoveredIdentity {
        native_id: device.id.clone(),
        room_id: None,
        room_name: None,
        name: projection.display_name.to_string(),
        device_type: projection.device_type,
        hardware_ids: vec![HardwareId::serial(&device.id)],
        manufacturer: projection.manufacturer.map(str::to_string),
        model: projection.model.map(str::to_string),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let key = hub_key();
    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state.canonical_registry.resolve(&identity, &key, now);
    let canonical_id = state
        .canonical_registry
        .find_by_native_id(&key, &device.id)
        .map(|device| device.id.clone())
        .ok_or_else(|| anyhow::anyhow!("canonical local Bluetooth endpoint was not registered"))?;
    rhythm_os::commands::save_authority_state(&state)?;
    Ok(canonical_id)
}

fn device_buttons(
    device: &LocalBleDevice,
    projection: &crate::profile::BleProfileProjection,
) -> Vec<(String, u8)> {
    projection
        .button_endpoints
        .iter()
        .map(|endpoint| {
            (
                format!("{}-{}", device.id, endpoint.suffix),
                endpoint.control_id,
            )
        })
        .collect()
}

pub fn resolve_native_id(state: &SharedState, requested_id: &str) -> Result<String> {
    if requested_id.starts_with("local-ble-") {
        return Ok(requested_id.to_string());
    }
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .canonical_registry
        .get(requested_id)
        .and_then(|device| {
            device
                .endpoints
                .iter()
                .find(|endpoint| endpoint.hub_key == hub_key())
                .map(|endpoint| endpoint.native_id.clone())
        })
        .ok_or_else(|| anyhow::anyhow!("device has no local Bluetooth endpoint"))
}

/// Local removal is deliberately adapter-independent. Simple advertisement
/// profiles do not own a BlueZ bond, so an unavailable adapter must not trap
/// appliance-local metadata.
pub fn unpair(state: &SharedState, requested_id: &str) -> Result<UnpairingResult> {
    let native_id = resolve_native_id(state, requested_id)?;
    let store = match get_hub_data(state) {
        Ok(data) => data.store.clone(),
        Err(_) => LocalBleDeviceStore::load_shared(data_dir(state)?)?,
    };
    // Do not let an intentional removal erase the only evidence that a prior
    // activation succeeded. Flush its outbox first; if terminal persistence is
    // unavailable, preserve the device and let the user retry removal.
    reconcile_activation_receipts(state, &store)?;
    let removed = store.remove(&native_id)?;
    let has_active_hub = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .hubs
        .contains_key(&hub_key());
    if has_active_hub {
        rhythm_os::commands::do_device_remove(state, &native_id, &hub_key())?;
    } else {
        // BlueZ and the live hub are not required to remove the canonical
        // endpoint. The persisted hub-registry snapshot is filtered against
        // the authoritative store on the next bootstrap.
        rhythm_os::commands::do_device_endpoint_remove(state, &native_id, &hub_key())?;
    }
    Ok(UnpairingResult {
        hub_type: HUB_TYPE.to_string(),
        hub_address: Some(HUB_ADDRESS.to_string()),
        status: PairingStatus::Complete,
        device_id: Some(native_id),
        error: None,
        completion_scope: Some(if removed.is_some() {
            UnpairingCompletionScope::LocalStateOnly
        } else {
            UnpairingCompletionScope::AlreadyAbsent
        }),
        warning: None,
    })
}

/// Stop and join the simple-profile observer before factory-reset deletes its
/// store. This intentionally does not stop the shared adapter: Hue must still
/// perform its authenticated bond handoff later in the same reset.
pub fn quiesce_for_factory_reset(state: &SharedState) -> Result<()> {
    let data_dir = data_dir(state)?;
    if let Ok(data) = get_hub_data(state) {
        if let Ok(slot) = data.monitor_shutdown.lock() {
            if let Some(shutdown) = slot.as_ref() {
                shutdown.store(true, Ordering::SeqCst);
            }
        }
        data.store.quiesce();
        if let Some(thread) = data
            .monitor_thread
            .lock()
            .map_err(|_| anyhow::anyhow!("local Bluetooth observer lock poisoned"))?
            .take()
        {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("local Bluetooth observer join failed"))?;
        }
        data.transport.quiesce()?;
    }
    // This path-scoped fence exists independently of ActiveHub. It closes new
    // store loads, mutations, registry reconciliation, and late hub publication
    // before waiting for every already-admitted operation to drain.
    latch_reset_guard(data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{
        BleButtonEndpointProjection, BleProfileProjection, ValidatedBleSetup,
        OREIN_OC02001_PROFILE_ID,
    };
    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_os::state::AppState;
    use std::collections::BTreeMap;

    const TEST_PAIRING_HMAC_KEY: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn temporary_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rhythm-local-ble-lifecycle-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn projection_registers_every_profile_declared_button_endpoint() {
        let device = LocalBleDevice::from_setup(
            OREIN_OC02001_PROFILE_ID,
            &ValidatedBleSetup {
                stable_identity: "SYNTHETIC-MULTI".to_string(),
                metadata: BTreeMap::new(),
            },
            "02:00:00:00:00:10".to_string(),
            BTreeMap::new(),
            false,
            1,
        );
        let projection = BleProfileProjection {
            device_type: DeviceType::Button,
            display_name: "Remote",
            manufacturer: None,
            model: None,
            button_endpoints: vec![
                BleButtonEndpointProjection {
                    suffix: "up",
                    control_id: 2,
                },
                BleButtonEndpointProjection {
                    suffix: "down",
                    control_id: 3,
                },
            ],
        };

        assert_eq!(
            device_buttons(&device, &projection),
            vec![
                (format!("{}-up", device.id), 2),
                (format!("{}-down", device.id), 3),
            ]
        );
    }

    #[test]
    fn server_pairing_budget_reserves_finalization_and_rejects_late_work() {
        let start = Instant::now();
        assert_eq!(
            association_deadline_at(start, start + PAIRING_SERVER_SLA).unwrap(),
            start + ASSOCIATION_TIMEOUT
        );
        assert_eq!(
            association_deadline_at(start, start + Duration::from_secs(20)).unwrap(),
            start + Duration::from_secs(5)
        );

        let no_finalization_room = association_deadline_at(start, start + FINALIZATION_RESERVE);
        assert!(no_finalization_room.is_err());
        assert!(remaining_pairing_budget(start, "test").is_err());
    }

    #[test]
    fn repeat_scan_returns_the_existing_public_device_without_store_mutation() {
        let root = temporary_dir("repeat-scan");
        let store = LocalBleDeviceStore::load_shared(&root).unwrap();
        let setup = ValidatedBleSetup {
            stable_identity: "A1B2C3D4E5F6".to_string(),
            metadata: BTreeMap::new(),
        };
        let fingerprint = rhythm_os::pairing::pairing_request_fingerprint(
            TEST_PAIRING_HMAC_KEY,
            HUB_TYPE,
            &serde_json::json!({
                "profile_id": OREIN_OC02001_PROFILE_ID,
                "setup": {"ble_identity": setup.stable_identity()}
            }),
        )
        .unwrap();
        let committed = store
            .upsert_activation_until(
                LocalBleDevice::from_setup(
                    OREIN_OC02001_PROFILE_ID,
                    &setup,
                    "02:00:00:00:00:31".to_string(),
                    BTreeMap::new(),
                    false,
                    1,
                ),
                "repeat-scan-original",
                &fingerprint,
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));

        assert!(terminal_session_for_existing_device(
            &state,
            &store,
            OREIN_OC02001_PROFILE_ID,
            setup.stable_identity(),
        )
        .unwrap()
        .is_none());

        state.lock().unwrap().canonical_registry.resolve(
            &DiscoveredIdentity {
                native_id: committed.id.clone(),
                room_id: Some("room-existing".to_string()),
                room_name: Some("Existing room".to_string()),
                name: "Button".to_string(),
                device_type: DeviceType::Button,
                hardware_ids: vec![HardwareId::serial(&committed.id)],
                manufacturer: Some("Synthetic".to_string()),
                model: Some("TEST".to_string()),
            },
            &hub_key(),
            1,
        );

        let receipt_count = store.pending_activation_receipts().unwrap().len();
        let session = terminal_session_for_existing_device(
            &state,
            &store,
            OREIN_OC02001_PROFILE_ID,
            setup.stable_identity(),
        )
        .unwrap()
        .expect("active store and canonical state should resolve the repeat scan");

        assert_eq!(session.status, PairingStatus::Complete);
        assert_eq!(session.device.unwrap().device_id, committed.id);
        assert_eq!(
            session.details,
            Some(serde_json::json!({
                "profile_id": OREIN_OC02001_PROFILE_ID,
                "existing": true,
            }))
        );
        assert_eq!(
            store.pending_activation_receipts().unwrap().len(),
            receipt_count,
            "repeat scan must not append another activation receipt"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    struct DeadlineAvailabilityProbe {
        deadline_probe_called: AtomicBool,
    }

    impl LocalBleTransport for DeadlineAvailabilityProbe {
        fn is_available(&self) -> Result<bool> {
            panic!("pairing setup must not start a fresh availability timeout")
        }

        fn is_available_until(&self, deadline: Instant) -> Result<bool> {
            assert!(deadline > Instant::now());
            self.deadline_probe_called.store(true, Ordering::SeqCst);
            Ok(false)
        }

        fn associate(
            &self,
            _profile_id: &str,
            _setup: &ValidatedBleSetup,
            _timeout: Duration,
        ) -> Result<crate::transport::LocalBlePairingCandidate> {
            unreachable!("unavailable transports cannot associate")
        }

        fn start_monitor(
            &self,
            _store: Arc<LocalBleDeviceStore>,
            _registry: Arc<Mutex<rhythm_os::registry::HubDeviceRegistry>>,
            _event_tx: Sender<HubEvent>,
            _shutdown: Arc<AtomicBool>,
        ) -> Result<JoinHandle<()>> {
            unreachable!("unavailable transports cannot start a monitor")
        }
    }

    #[test]
    fn pairing_connect_charges_availability_to_the_existing_deadline() {
        let root = temporary_dir("deadline-availability");
        let mut app = AppState::default();
        app.data_dir = root.to_string_lossy().into_owned();
        let orphan_id = "local-ble-00000000000000000000000000000001";
        app.canonical_registry.resolve(
            &DiscoveredIdentity {
                native_id: orphan_id.to_string(),
                room_id: None,
                room_name: None,
                name: "Orphaned button".to_string(),
                device_type: DeviceType::Button,
                hardware_ids: vec![HardwareId::serial(orphan_id)],
                manufacturer: Some("Synthetic".to_string()),
                model: Some("TEST".to_string()),
            },
            &hub_key(),
            1,
        );
        let state = Arc::new(Mutex::new(app));
        let transport = Arc::new(DeadlineAvailabilityProbe {
            deadline_probe_called: AtomicBool::new(false),
        });

        let error = match connect_until(
            &state,
            transport.clone(),
            Instant::now() + Duration::from_secs(1),
        ) {
            Ok(_) => panic!("unavailable transport unexpectedly connected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("adapter is unavailable"));
        assert!(transport.deadline_probe_called.load(Ordering::SeqCst));
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), orphan_id)
            .is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restart_repairs_an_activation_receipt_before_adapter_availability() {
        let root = temporary_dir("activation-outbox");
        let store = LocalBleDeviceStore::load_shared(&root).unwrap();
        let device = LocalBleDevice::from_setup(
            OREIN_OC02001_PROFILE_ID,
            &ValidatedBleSetup {
                stable_identity: "A1B2C3D4E5F6".to_string(),
                metadata: BTreeMap::new(),
            },
            "02:00:00:00:00:20".to_string(),
            BTreeMap::new(),
            false,
            1,
        );
        let session_id = "local-ble-crash-recovery";
        let fingerprint = rhythm_os::pairing::pairing_request_fingerprint(
            TEST_PAIRING_HMAC_KEY,
            HUB_TYPE,
            &serde_json::json!({
                "profile_id": OREIN_OC02001_PROFILE_ID,
                "setup": {"ble_identity": "A1B2C3D4E5F6"}
            }),
        )
        .unwrap();
        store
            .upsert_activation_until(
                device,
                session_id,
                &fingerprint,
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap();

        let mut app = AppState::default();
        app.data_dir = root.to_string_lossy().into_owned();
        app.storage = Some(Arc::new(
            rhythm_os::storage::FileStorage::new(root.to_str().unwrap()).unwrap(),
        ));
        let state = Arc::new(Mutex::new(app));
        let transport = Arc::new(DeadlineAvailabilityProbe {
            deadline_probe_called: AtomicBool::new(false),
        });

        let error = match connect_until(&state, transport, Instant::now() + Duration::from_secs(1))
        {
            Ok(_) => panic!("unavailable transport unexpectedly connected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("adapter is unavailable"));

        let status = rhythm_os::pairing::lookup_pairing_result(&state, session_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            status.state,
            rhythm_os::pairing::PairingResultState::Terminal
        );
        assert_eq!(status.result.unwrap().status, PairingStatus::Complete);
        assert!(store.pending_activation_receipts().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn live_status_lookup_repairs_a_committed_receipt_before_pending_expiry() {
        let root = temporary_dir("activation-outbox-live-status");
        let store = LocalBleDeviceStore::load_shared(&root).unwrap();
        let session_id = "local-ble-live-recovery";
        let fingerprint = rhythm_os::pairing::pairing_request_fingerprint(
            TEST_PAIRING_HMAC_KEY,
            HUB_TYPE,
            &serde_json::json!({}),
        )
        .unwrap();
        store
            .upsert_activation_until(
                LocalBleDevice::from_setup(
                    OREIN_OC02001_PROFILE_ID,
                    &ValidatedBleSetup {
                        stable_identity: "C1B2C3D4E5F6".to_string(),
                        metadata: BTreeMap::new(),
                    },
                    "02:00:00:00:00:22".to_string(),
                    BTreeMap::new(),
                    false,
                    1,
                ),
                session_id,
                &fingerprint,
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap();

        let storage =
            Arc::new(rhythm_os::storage::FileStorage::new(root.to_str().unwrap()).unwrap());
        rhythm_os::storage::Storage::save_pairing_history(
            storage.as_ref(),
            &rhythm_os::pairing::PairingHistory {
                schema_version: rhythm_os::pairing::PAIRING_HISTORY_SCHEMA_VERSION,
                entries: Vec::new(),
                pairing_results: vec![rhythm_os::pairing::PairingResultRecord {
                    session_id: session_id.to_string(),
                    hub_type: HUB_TYPE.to_string(),
                    request_fingerprint: fingerprint.clone(),
                    // Deliberately older than the local pending TTL. GET must
                    // drain the authoritative outbox before it expires this.
                    updated_at_epoch_ms: 1,
                    result: None,
                }],
            },
        )
        .unwrap();
        let mut app = AppState::default();
        app.data_dir = root.to_string_lossy().into_owned();
        app.storage = Some(storage);
        app.reconcile_pairing_results_fn = Some(Arc::new(|state, _| {
            reconcile_pending_pairing_results(state)
        }));
        let state = Arc::new(Mutex::new(app));

        let response = rhythm_os::handlers::handle_get_pair_device(&state, session_id);
        assert_eq!(response.status, 200);
        let status: rhythm_os::pairing::PairingResultStatus =
            serde_json::from_str(&response.body).unwrap();
        assert_eq!(
            status.result.unwrap().status,
            rhythm_os::pairing::PairingStatus::Complete
        );
        assert!(store.pending_activation_receipts().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn activation_outbox_accepts_an_existing_success_with_transient_warnings() {
        let root = temporary_dir("activation-outbox-warning");
        let store = LocalBleDeviceStore::load_shared(&root).unwrap();
        let session_id = "local-ble-warning-recovery";
        let fingerprint = rhythm_os::pairing::pairing_request_fingerprint(
            TEST_PAIRING_HMAC_KEY,
            HUB_TYPE,
            &serde_json::json!({}),
        )
        .unwrap();
        let committed = store
            .upsert_activation_until(
                LocalBleDevice::from_setup(
                    OREIN_OC02001_PROFILE_ID,
                    &ValidatedBleSetup {
                        stable_identity: "B1B2C3D4E5F6".to_string(),
                        metadata: BTreeMap::new(),
                    },
                    "02:00:00:00:00:21".to_string(),
                    BTreeMap::new(),
                    false,
                    1,
                ),
                session_id,
                &fingerprint,
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap();
        let mut app = AppState::default();
        app.data_dir = root.to_string_lossy().into_owned();
        app.storage = Some(Arc::new(
            rhythm_os::storage::FileStorage::new(root.to_str().unwrap()).unwrap(),
        ));
        let state = Arc::new(Mutex::new(app));
        let mut first_terminal = terminal_session_for_device(&committed).unwrap();
        first_terminal
            .warnings
            .push("Storage acknowledgement was degraded".to_string());
        complete_pairing_result_with_fingerprint(
            &state,
            session_id,
            HUB_TYPE,
            &fingerprint,
            &first_terminal,
        )
        .unwrap();

        reconcile_activation_receipts(&state, &store).unwrap();

        assert!(store.pending_activation_receipts().unwrap().is_empty());
        let persisted = rhythm_os::pairing::lookup_pairing_result(&state, session_id)
            .unwrap()
            .unwrap()
            .result
            .unwrap();
        assert_eq!(persisted.warnings, ["Storage acknowledgement was degraded"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restart_reconciliation_removes_only_uncommitted_local_ble_endpoints() {
        let mut app = AppState::default();
        for native_id in ["local-ble-active", "local-ble-orphan"] {
            app.canonical_registry.resolve(
                &DiscoveredIdentity {
                    native_id: native_id.to_string(),
                    room_id: None,
                    room_name: None,
                    name: "Button".to_string(),
                    device_type: DeviceType::Button,
                    hardware_ids: vec![HardwareId::serial(native_id)],
                    manufacturer: Some("Synthetic".to_string()),
                    model: Some("TEST".to_string()),
                },
                &hub_key(),
                1,
            );
        }
        let state = Arc::new(Mutex::new(app));

        reconcile_orphaned_canonical_projections(
            &state,
            &HashSet::from(["local-ble-active".to_string()]),
        )
        .unwrap();

        let app = state.lock().unwrap();
        assert!(app
            .canonical_registry
            .find_by_native_id(&hub_key(), "local-ble-active")
            .is_some());
        assert!(app
            .canonical_registry
            .find_by_native_id(&hub_key(), "local-ble-orphan")
            .is_none());
    }

    #[test]
    fn offline_unpair_removes_store_without_an_active_hub_or_bluez() {
        let root = temporary_dir("offline-unpair");
        let device = LocalBleDevice::from_setup(
            OREIN_OC02001_PROFILE_ID,
            &ValidatedBleSetup {
                stable_identity: "A1B2C3D4E5F6".to_string(),
                metadata: BTreeMap::from([
                    ("serial".to_string(), "sanitized-s".to_string()),
                    ("model".to_string(), "sanitized-m".to_string()),
                ]),
            },
            "02:00:00:00:00:01".to_string(),
            BTreeMap::new(),
            false,
            1,
        );
        LocalBleDeviceStore::load(&root)
            .unwrap()
            .upsert(device.clone())
            .unwrap();
        let mut app = AppState::default();
        app.data_dir = root.to_string_lossy().into_owned();
        app.canonical_registry.resolve(
            &DiscoveredIdentity {
                native_id: device.id.clone(),
                room_id: None,
                room_name: None,
                name: "Button".to_string(),
                device_type: DeviceType::Button,
                hardware_ids: vec![HardwareId::serial(&device.id)],
                manufacturer: Some("Orein/AiDot".to_string()),
                model: Some("OC02001-CR-B".to_string()),
            },
            &hub_key(),
            1,
        );
        let state = Arc::new(Mutex::new(app));

        let result = unpair(&state, &device.id).unwrap();

        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(
            result.completion_scope,
            Some(UnpairingCompletionScope::LocalStateOnly)
        );
        assert!(LocalBleDeviceStore::load(&root).unwrap().all().is_empty());
        assert!(state.lock().unwrap().hubs.is_empty());
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn factory_reset_latches_the_path_fence_without_an_active_hub() {
        let root = temporary_dir("offline-reset-fence");
        let mut app = AppState::default();
        app.data_dir = root.to_string_lossy().into_owned();
        let state = Arc::new(Mutex::new(app));

        quiesce_for_factory_reset(&state).unwrap();

        assert!(LocalBleDeviceStore::load_shared(&root).is_err());
        assert!(!root.join(crate::store::STORE_DIRECTORY).exists());

        // Production deliberately retains the fence until reboot. Explicitly
        // release this unique path so the process-global test registry does not
        // retain test-only state.
        crate::store::release_latched_reset_guard(&root);
        LocalBleDeviceStore::load_shared(&root).unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
