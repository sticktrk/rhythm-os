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
    PairedDeviceInfo, PairingSession, PairingStage, PairingStatus, UnpairingCompletionScope,
    UnpairingResult,
};
use rhythm_os::state::SharedState;

use crate::discovery::LocalBleDiscovery;
use crate::profile::profile_by_id;
use crate::store::{LocalBleDevice, LocalBleDeviceStore};
use crate::transport::LocalBleTransport;
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
    let available = match deadline {
        Some(deadline) => transport.is_available_until(deadline)?,
        None => transport.is_available()?,
    };
    if !available {
        anyhow::bail!("Bluetooth adapter is unavailable");
    }

    let store = LocalBleDeviceStore::load_shared(data_dir(state)?)?;
    let active_device_ids = store
        .all()
        .into_iter()
        .map(|device| device.id.clone())
        .collect::<HashSet<_>>();
    reconcile_orphaned_canonical_projections(state, &active_device_ids)?;
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
    session_id: Option<&str>,
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
    rhythm_os::pairing::emit_pairing_progress(
        state,
        HUB_TYPE,
        session_id,
        PairingStatus::Searching,
        PairingStage::Searching,
        "Looking for the nearby Bluetooth device",
        None,
        None,
    );
    let association_deadline = association_deadline(deadline)?;
    let candidate = data
        .transport
        .associate_until(profile_id, &setup, association_deadline)
        .map_err(|_| anyhow::anyhow!("local Bluetooth association failed"))?;
    remaining_pairing_budget(deadline, "device finalization")?;
    rhythm_os::pairing::emit_pairing_progress(
        state,
        HUB_TYPE,
        session_id,
        PairingStatus::Commissioning,
        PairingStage::Finalizing,
        "Saving the Bluetooth device",
        None,
        None,
    );

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
    if data.store.upsert(device.clone()).is_err() {
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
    let warnings = data
        .store
        .durability_degraded()
        .then(|| {
            "Bluetooth association was saved, but storage durability acknowledgement is degraded"
                .to_string()
        })
        .into_iter()
        .collect();

    let _ = data.event_tx.send(HubEvent::DevicePaired {
        hub_key: None,
        device_id: device.id.clone(),
        name: projection.display_name.to_string(),
        device_type: projection.device_type.clone(),
    });
    let info = PairedDeviceInfo {
        device_id: device.id.clone(),
        name: projection.display_name.to_string(),
        device_type: projection.device_type.clone(),
        manufacturer: projection.manufacturer.map(str::to_string),
        model: projection.model.map(str::to_string),
    };
    Ok(PairingSession {
        hub_type: HUB_TYPE.to_string(),
        status: PairingStatus::Complete,
        device: Some(info.clone()),
        devices: vec![info],
        error: None,
        warnings,
        details: Some(serde_json::json!({ "profile_id": profile_id })),
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
    if let Some(storage) = state.storage.as_ref() {
        storage.save_canonical_registry(&serde_json::to_value(&state.canonical_registry)?)?;
    }
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
    let removed = store.remove(&native_id)?;
    let has_active_hub = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .hubs
        .contains_key(&hub_key());
    if has_active_hub {
        rhythm_os::commands::do_device_remove(state, &native_id, &hub_key())?;
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
    let Ok(data) = get_hub_data(state) else {
        return Ok(());
    };
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
    Ok(())
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
            .is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
}
