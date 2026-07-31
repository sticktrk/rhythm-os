//! Shared persistence, pairing, and canonical-device lifecycle.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::pairing::{
    PairedDeviceInfo, PairingSession, PairingStage, PairingStatus, UnpairingCompletionScope,
    UnpairingResult,
};
use rhythm_os::state::SharedState;

use crate::discovery::AidotButtonDiscovery;
use crate::protocol::AidotSetupCode;
use crate::store::{AidotButtonDevice, AidotButtonStore};
use crate::transport::AidotBleTransport;
use crate::{HUB_ADDRESS, HUB_TYPE};

const ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(60);

pub struct AidotHubData {
    pub transport: Arc<dyn AidotBleTransport>,
    pub store: Arc<AidotButtonStore>,
    event_tx: Sender<HubEvent>,
    _monitor_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
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

pub fn get_hub_data(state: &SharedState) -> Result<Arc<AidotHubData>> {
    let key = hub_key();
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .hubs
        .get(&key)
        .and_then(|hub| hub.data::<Arc<AidotHubData>>())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Orein/AiDot button integration is not connected"))
}

pub fn connect(
    state: &SharedState,
    transport: Arc<dyn AidotBleTransport>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    if !transport
        .is_available()
        .context("Bluetooth adapter is unavailable")?
    {
        anyhow::bail!("Bluetooth adapter is unavailable");
    }

    let store = Arc::new(AidotButtonStore::load(data_dir(state)?)?);
    let key = hub_key();
    let snapshot = state
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
        .and_then(|value| serde_json::from_value(value).ok());
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let monitor_thread = Arc::new(Mutex::new(None));

    let data_transport = transport.clone();
    let data_store = store.clone();
    let data_monitor_thread = monitor_thread.clone();
    let data_event_tx = event_tx.clone();
    let monitor_transport = transport;
    let monitor_store = store.clone();
    let monitor_thread_slot = monitor_thread;
    let (mut hub, event_rx) = rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new(HUB_TYPE),
        key,
        true,
        snapshot,
        move |_registry| {
            Box::new(Arc::new(AidotHubData {
                transport: data_transport,
                store: data_store,
                event_tx: data_event_tx,
                _monitor_thread: data_monitor_thread,
            }))
        },
        move |registry, shutdown| {
            if let Ok(mut registry) = registry.lock() {
                for device in monitor_store.all() {
                    if !registry.has_device(&device.id) {
                        registry.upsert_device(
                            &device.id,
                            None,
                            &[(device.button_id(), 1)],
                            DeviceType::Button,
                        );
                    }
                }
            }
            let _ = event_tx.send(HubEvent::Connected { hub_key: None });
            match monitor_transport.start_monitor(
                monitor_store,
                registry,
                event_tx.clone(),
                shutdown.clone(),
            ) {
                Ok(thread) => {
                    if let Ok(mut slot) = monitor_thread_slot.lock() {
                        *slot = Some(thread);
                    }
                }
                Err(error) => {
                    let _ = event_tx.send(HubEvent::Disconnected {
                        hub_key: None,
                        reason: format!("AiDot button monitor could not start: {error:#}"),
                    });
                }
            }
            event_rx
        },
    )?;
    hub.discovery = Some(Arc::new(AidotButtonDiscovery::new(store)));
    Ok((hub, event_rx))
}

pub fn pair(
    state: &SharedState,
    setup_payload: &str,
    session_id: Option<&str>,
) -> Result<PairingSession> {
    let setup = AidotSetupCode::parse(setup_payload)?;
    let data = get_hub_data(state)?;
    rhythm_os::pairing::emit_pairing_progress(
        state,
        HUB_TYPE,
        session_id,
        PairingStatus::Searching,
        PairingStage::Searching,
        "Looking for the button from its QR code",
        None,
        None,
    );
    let candidate = data
        .transport
        .associate(&setup, ASSOCIATION_TIMEOUT)
        .context("associating Orein/AiDot button")?;
    rhythm_os::pairing::emit_pairing_progress(
        state,
        HUB_TYPE,
        session_id,
        PairingStatus::Commissioning,
        PairingStage::Finalizing,
        "Saving the button",
        None,
        None,
    );

    let id = AidotButtonDevice::stable_id(&setup.ble_identity);
    let previous = data.store.get_by_id(&id);
    let device = AidotButtonDevice {
        id: id.clone(),
        ble_identity: setup.ble_identity,
        serial_metadata: setup.serial_metadata,
        model_metadata: setup.model_metadata,
        observed_address: candidate.observed_address,
        last_counter: candidate
            .initial_counter
            .or_else(|| previous.as_ref().and_then(|device| device.last_counter)),
        associated_at_epoch_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    data.store.upsert(device.clone())?;

    let existing_room = state.lock().ok().and_then(|state| {
        state
            .canonical_registry
            .find_by_native_id(&hub_key(), &device.id)
            .and_then(|device| device.room_id.clone())
    });
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
            &[(device.button_id(), 1)],
            DeviceType::Button,
            &hub_key(),
            true,
        )?;
        rhythm_os::commands::do_canonical_assign_room(
            state,
            &canonical_id,
            existing_room.as_deref(),
        )
    });
    if let Err(error) = project {
        match previous {
            Some(previous) => {
                let _ = data.store.upsert(previous);
            }
            None => {
                let _ = data.store.remove(&device.id);
            }
        }
        if !had_canonical_endpoint {
            let _ = rhythm_os::commands::do_device_endpoint_remove(state, &device.id, &hub_key());
        }
        return Err(error).context("projecting paired Orein/AiDot button");
    }

    let _ = data.event_tx.send(HubEvent::DevicePaired {
        hub_key: None,
        device_id: device.id.clone(),
        name: device.display_name().to_string(),
        device_type: DeviceType::Button,
    });
    let info = PairedDeviceInfo {
        device_id: device.id.clone(),
        name: device.display_name().to_string(),
        device_type: DeviceType::Button,
        manufacturer: Some("Orein/AiDot".to_string()),
        model: Some("OC02001-CR-B".to_string()),
    };
    Ok(PairingSession {
        hub_type: HUB_TYPE.to_string(),
        status: PairingStatus::Complete,
        device: Some(info.clone()),
        devices: vec![info],
        error: None,
        warnings: Vec::new(),
        details: None,
    })
}

fn register_canonical_identity(state: &SharedState, device: &AidotButtonDevice) -> Result<String> {
    let identity = DiscoveredIdentity {
        native_id: device.id.clone(),
        room_id: None,
        room_name: None,
        name: device.display_name().to_string(),
        device_type: DeviceType::Button,
        hardware_ids: vec![HardwareId::mac(&device.ble_identity)],
        manufacturer: Some("Orein/AiDot".to_string()),
        model: Some("OC02001-CR-B".to_string()),
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
        .ok_or_else(|| anyhow::anyhow!("Canonical AiDot endpoint was not registered"))?;
    if let Some(storage) = state.storage.as_ref() {
        storage.save_canonical_registry(&serde_json::to_value(&state.canonical_registry)?)?;
    }
    Ok(canonical_id)
}

pub fn resolve_native_id(state: &SharedState, requested_id: &str) -> Result<String> {
    if requested_id.starts_with("aidot-ble-") {
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
        .ok_or_else(|| anyhow::anyhow!("Device has no Orein/AiDot button endpoint"))
}

pub fn unpair(state: &SharedState, requested_id: &str) -> Result<UnpairingResult> {
    let native_id = resolve_native_id(state, requested_id)?;
    let data = get_hub_data(state)?;
    let removed = data.store.remove(&native_id)?;
    // Always clean the live registry, including a retry after metadata was
    // durably removed but an earlier registry write failed.
    rhythm_os::commands::do_device_remove(state, &native_id, &hub_key())?;
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
