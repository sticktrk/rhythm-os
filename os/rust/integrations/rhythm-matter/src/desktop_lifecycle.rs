//! Desktop Matter lifecycle using the local CHIP sidecar transport.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use anyhow::Result;
use log::info;
use rhythm_core::controller::HubLightController;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{HubEvent, HubProvider, HubType};
use rhythm_os::pairing::PairingSession;
use rhythm_os::state::SharedState;

use crate::chip_transport::ChipTransport;
use crate::controller::MatterLightController;
use crate::hub_state::MatterHubData;
use crate::transport::MatterTransport;

fn matter_data_path(state: &SharedState) -> Result<String> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if state.data_dir.is_empty() {
        return Err(anyhow::anyhow!("data_dir not configured on AppState"));
    }
    Ok(format!("{}/matter", state.data_dir))
}

fn get_hub_data(state: &SharedState) -> Result<Arc<MatterHubData>> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state
        .hubs
        .get(&hub_key)
        .ok_or_else(|| anyhow::anyhow!("Matter hub not connected"))?
        .data::<Arc<MatterHubData>>()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Matter hub data missing"))
}

fn get_transport(state: &SharedState) -> Result<Arc<dyn MatterTransport>> {
    let hub_data = get_hub_data(state)?;
    hub_data
        .transport
        .get()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Matter transport not initialized"))
}

fn configured_fabric_id(state: &SharedState) -> String {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    state
        .lock()
        .ok()
        .and_then(|state| state.hub_credentials.get(&hub_key).cloned())
        .and_then(|creds| crate::provider::matter_fabric_id(&creds).map(ToOwned::to_owned))
        .unwrap_or_else(|| "default".to_string())
}

fn resolve_unpair_device_id(state: &SharedState, device_id: &str) -> Result<String> {
    if crate::lifecycle::parse_device_id(device_id).is_some() {
        return Ok(device_id.to_string());
    }

    let matter_hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let device = state
        .canonical_registry
        .get(device_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid Matter device ID: {}", device_id))?;

    let endpoint = device
        .endpoints
        .iter()
        .find(|endpoint| {
            endpoint.hub_key == matter_hub_key
                && crate::lifecycle::parse_device_id(&endpoint.native_id).is_some()
        })
        .ok_or_else(|| {
            anyhow::anyhow!("Canonical device '{}' has no Matter endpoint", device_id)
        })?;

    Ok(endpoint.native_id.clone())
}

/// Connect to the local Matter fabric and store the hub in state.
pub fn connect_and_start(state: SharedState, _key: &HubKey) -> Result<Receiver<HubEvent>> {
    let data_path = matter_data_path(&state)?;
    let fabric_id = configured_fabric_id(&state);
    let transport: Arc<dyn MatterTransport> =
        Arc::new(ChipTransport::load_or_create(&data_path, &fabric_id)?);
    let (mut hub, event_rx) = crate::lifecycle::connect_matter(&state, transport.clone())?;
    let hub_data = hub
        .data::<Arc<MatterHubData>>()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Matter hub data missing"))?;
    let _ = hub_data.capture_dir.set(format!("{}/captures", data_path));

    hub.discovery = Some(Arc::new(crate::discovery::MatterDiscovery::new(
        transport, hub_data,
    )));

    {
        let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = HubKey::new(HubType::new("matter"), "local");
        state.hubs.insert(key.clone(), hub);
        state.set_hub_connected(&key, false);
    }

    Ok(event_rx)
}

/// Create a `MatterLightController` for the composite controller.
pub fn create_controller(
    state: &SharedState,
    _key: &HubKey,
) -> Result<Arc<dyn HubLightController>> {
    let transport = get_transport(state)?;
    let hub_data = get_hub_data(state)?;
    Ok(Arc::new(MatterLightController::new(transport, hub_data)))
}

struct MatterHubProvider;

impl HubProvider for MatterHubProvider {
    fn hub_type(&self) -> HubType {
        HubType::new("matter")
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        crate::provider::configure_matter_hub(address, credentials_json, state, |state| {
            let data_path = matter_data_path(state)?;
            let fabric_id = configured_fabric_id(state);
            let transport: Arc<dyn MatterTransport> =
                Arc::new(ChipTransport::load_or_create(&data_path, &fabric_id)?);
            let (hub, event_rx) = crate::lifecycle::connect_matter(state, transport)?;
            if let Some(hub_data) = hub.data::<Arc<MatterHubData>>().cloned() {
                let _ = hub_data.capture_dir.set(format!("{}/captures", data_path));
            }
            Ok((hub, event_rx))
        })
    }
}

static MATTER_PROVIDER: MatterHubProvider = MatterHubProvider;

pub fn get_hub_provider() -> &'static dyn HubProvider {
    &MATTER_PROVIDER
}

pub struct MatterIntegration;

impl rhythm_os::hub::ExternalLightHubIntegration for MatterIntegration {
    fn hub_type(&self) -> &'static str {
        "matter"
    }

    fn provider(&self) -> &'static dyn HubProvider {
        get_hub_provider()
    }

    fn api_capabilities(&self) -> rhythm_os::hub::HubIntegrationCapability {
        let mut device_onboarding_methods =
            vec![rhythm_os::hub::DEVICE_ONBOARDING_METHOD_MATTER_ON_NETWORK_SETUP_CODE.to_string()];
        if !cfg!(target_os = "macos") {
            device_onboarding_methods.push(
                rhythm_os::hub::DEVICE_ONBOARDING_METHOD_MATTER_BLE_WIFI_COMMISSIONING.to_string(),
            );
        }

        rhythm_os::hub::HubIntegrationCapability {
            hub_type: "matter".to_string(),
            configurable: true,
            device_onboarding_methods,
            supports_unpairing: true,
            supports_roomless_devices: true,
        }
    }

    fn connect_and_start(&self, state: SharedState, key: &HubKey) -> Result<Receiver<HubEvent>> {
        connect_and_start(state, key)
    }

    fn ensure_runtime(&self, _state: &SharedState) -> Result<()> {
        Ok(())
    }

    fn create_controller(
        &self,
        state: &SharedState,
        key: &HubKey,
    ) -> Result<Arc<dyn HubLightController>> {
        create_controller(state, key)
    }

    fn post_connect(&self, _state: &SharedState, _key: &HubKey) {
        // Capability probing is done during pairing and on explicit probe calls.
    }

    fn credentials_interceptor(
        &self,
        state: &SharedState,
        body: &serde_json::Value,
    ) -> Option<Result<String, String>> {
        let hub_type = body.get("hub_type").and_then(|value| value.as_str())?;
        if hub_type != "matter" {
            return None;
        }

        let creds_empty = body
            .get("credentials")
            .map(|value| {
                value.is_null()
                    || value
                        .as_object()
                        .map(|object| object.is_empty())
                        .unwrap_or(false)
            })
            .unwrap_or(true);
        if !creds_empty {
            return None;
        }

        let credentials = serde_json::json!({ "fabric_id": "default" });

        match rhythm_os::commands::do_hub_credentials(state, "matter", "local", &credentials) {
            Ok(()) => {
                let hub_connected = state
                    .lock()
                    .map(|state| state.has_any_connected_hub())
                    .unwrap_or(false);

                let hub_key = HubKey::new(HubType::new("matter"), "local");
                self.post_connect(state, &hub_key);

                Some(Ok(format!(r#"{{"hub_connected":{}}}"#, hub_connected)))
            }
            Err(e) => Some(Err(e.to_string())),
        }
    }

    fn start_pairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<PairingSession> {
        let request = crate::commissioning::MatterPairingParams::from_value(params)?;
        rhythm_os::pairing::emit_pairing_progress(
            state,
            "matter",
            request.session_id.as_deref(),
            rhythm_os::pairing::PairingStatus::Searching,
            rhythm_os::pairing::PairingStage::HubConnecting,
            "Preparing Matter hub",
            None,
            None,
        );
        crate::commissioning::ensure_matter_hub_connected(state)?;

        let transport = get_transport(state)?;
        let hub_data = get_hub_data(state)?;

        rhythm_os::pairing::emit_pairing_progress(
            state,
            "matter",
            request.session_id.as_deref(),
            rhythm_os::pairing::PairingStatus::Searching,
            rhythm_os::pairing::PairingStage::Searching,
            "Searching for Matter device",
            None,
            None,
        );
        crate::commissioning::pair_device(state, transport, hub_data, &request)
    }

    fn start_unpairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<rhythm_os::pairing::UnpairingResult> {
        use rhythm_os::pairing::{PairingStatus, UnpairingResult};

        let device_id = params
            .get("device_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'device_id' in unpairing params"))?;
        let force = params
            .get("force")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let device_id = resolve_unpair_device_id(state, device_id)?;

        let (node_id, _endpoint) = crate::lifecycle::parse_device_id(&device_id)
            .ok_or_else(|| anyhow::anyhow!("Invalid Matter device ID: {}", device_id))?;

        let transport = get_transport(state)?;
        let hub_data = get_hub_data(state)?;

        match transport.decommission_device(node_id, force) {
            Ok(()) => {
                info!(target: "sys", "Matter: decommissioned node {}", node_id);
                hub_data.remove_device(node_id);
                let matter_hub_key = HubKey::new(HubType::new("matter"), "local");
                rhythm_os::commands::do_device_hard_remove(
                    state,
                    &device_id,
                    Some(&matter_hub_key),
                )?;

                Ok(UnpairingResult {
                    hub_type: "matter".to_string(),
                    status: PairingStatus::Complete,
                    device_id: Some(device_id.to_string()),
                    error: None,
                })
            }
            Err(e) => {
                log::error!(target: "pair", "Matter decommission error: {:#}", e);
                Ok(UnpairingResult {
                    hub_type: "matter".to_string(),
                    status: PairingStatus::Failed,
                    device_id: Some(device_id.to_string()),
                    error: Some(format!("{:#}", e)),
                })
            }
        }
    }
}

pub static INTEGRATION: MatterIntegration = MatterIntegration;
