//! Desktop Matter lifecycle using matc transport.
//!
//! Provides `ExternalLightHubIntegration` impl so rhythm-server can
//! register Matter as a hub alongside Hue and HA.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use anyhow::Result;
use log::info;

use rhythm_core::controller::LightController;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{HubEvent, HubProvider, HubType};
use rhythm_os::pairing::PairingSession;
use rhythm_os::state::SharedState;

use crate::controller::MatterLightController;
use crate::desktop_transport::MatcTransport;
use crate::hub_state::MatterHubData;
use crate::transport::MatterTransport;

/// Get the Matter data path from AppState.
fn matter_data_path(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.data_dir.is_empty() {
        return Err(anyhow::anyhow!("data_dir not configured on AppState"));
    }
    Ok(format!("{}/matter", s.data_dir))
}

/// Connect to the local Matter fabric and store the hub in state.
///
/// Loads or creates the Matter fabric in `{data_dir}/matter/`.
/// Returns the event receiver for the main event loop.
pub fn connect_and_start(state: SharedState, _key: &HubKey) -> Result<Receiver<HubEvent>> {
    let data_path = matter_data_path(&state)?;
    let transport = Arc::new(MatcTransport::load_or_create(&data_path)?);
    let (mut hub, event_rx) = crate::lifecycle::connect_matter(&state, transport.clone())?;

    // Wire discovery so POST /api/sync discovers Matter devices
    hub.discovery = Some(Arc::new(crate::discovery::MatterDiscovery::new(transport)));

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = HubKey::new(HubType::new("matter"), "local");
        s.hubs.insert(key, hub);
    }

    Ok(event_rx)
}

/// Create a `MatterLightController` for the composite controller.
pub fn create_controller(state: &SharedState, _key: &HubKey) -> Result<Arc<dyn LightController>> {
    let data_path = matter_data_path(state)?;
    let transport = Arc::new(MatcTransport::load_or_create(&data_path)?);

    let hub_data = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = HubKey::new(HubType::new("matter"), "local");
        s.hubs
            .get(&key)
            .and_then(|h| h.data::<Arc<MatterHubData>>())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Matter hub not connected"))?
    };

    let fade_ms = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.bulb_fade_atomic.clone()
    };

    Ok(Arc::new(MatterLightController::new(transport, hub_data, fade_ms)))
}

// ============================================================================
// HubProvider
// ============================================================================

struct MatterHubProvider;

impl HubProvider for MatterHubProvider {
    fn hub_type(&self) -> HubType {
        HubType::new("matter")
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        crate::provider::configure_matter_hub(address, credentials_json, state, |state| {
            let data_path = matter_data_path(state)?;
            let transport = Arc::new(MatcTransport::load_or_create(&data_path)?);
            crate::lifecycle::connect_matter(state, transport)
        })
    }
}

static MATTER_PROVIDER: MatterHubProvider = MatterHubProvider;

pub fn get_hub_provider() -> &'static dyn HubProvider {
    &MATTER_PROVIDER
}

// ============================================================================
// ExternalLightHubIntegration
// ============================================================================

pub struct MatterIntegration;

impl rhythm_os::hub::ExternalLightHubIntegration for MatterIntegration {
    fn hub_type(&self) -> &'static str {
        "matter"
    }

    fn provider(&self) -> &'static dyn HubProvider {
        get_hub_provider()
    }

    fn connect_and_start(&self, state: SharedState, key: &HubKey) -> Result<Receiver<HubEvent>> {
        connect_and_start(state, key)
    }

    fn ensure_runtime(&self, state: &SharedState) -> Result<()> {
        // Runtime creation handled by ensure_composite_runtime in rhythm-os
        Ok(())
    }

    fn create_controller(
        &self,
        state: &SharedState,
        key: &HubKey,
    ) -> Result<Arc<dyn LightController>> {
        create_controller(state, key)
    }

    fn start_pairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<PairingSession> {
        use rhythm_core::runtime::hub_registry::DeviceType;
        use rhythm_os::canonical::identity::HardwareId;
        use rhythm_os::pairing::{PairedDeviceInfo, PairingStatus};

        let setup_code = params
            .get("setup_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'setup_code' in pairing params"))?;

        let data_path = matter_data_path(state)?;
        let transport = MatcTransport::load_or_create(&data_path)?;

        match transport.commission(setup_code) {
            Ok(device) => {
                let device_id =
                    crate::lifecycle::format_device_id(device.node_id, device.light_endpoint);
                let device_name = format!("{} {}", device.vendor_name, device.product_name);

                info!(target: "sys", "Matter: paired {} (node {}, id={})",
                    device_name, device.node_id, device_id);

                // Register device in HubDeviceRegistry (self-roomed synthetic room)
                // and store capabilities for capability-aware command adaptation
                let hub_key = HubKey::new(HubType::new("matter"), "local");
                {
                    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                    if let Some(hub) = s.hubs.get(&hub_key) {
                        if let Some(hub_data) = hub.data::<Arc<MatterHubData>>() {
                            let mut reg = hub_data
                                .registry
                                .lock()
                                .map_err(|e| anyhow::anyhow!("{}", e))?;
                            reg.upsert_room(
                                &device_id,
                                &device_name,
                                &device_id,
                                &[device_id.clone()],
                            );
                            reg.set_area_lights(&device_id, vec![device_id.clone()]);
                        }
                    }
                }

                // Store device capabilities from commissioning data
                {
                    let caps =
                        crate::capabilities::capabilities_from_commissioned(&device);
                    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                    if let Some(hub) = s.hubs.get(&hub_key) {
                        if let Some(hub_data) = hub.data::<Arc<MatterHubData>>() {
                            if let Ok(mut dc) = hub_data.device_caps.lock() {
                                dc.insert(device_id.clone(), caps);
                                info!(target: "sys",
                                    "Matter: stored caps for {} ({} devices tracked)",
                                    device_id, dc.len()
                                );
                            }
                        }
                    }
                }

                // Register canonical device for cross-hub dedup
                {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let identity = rhythm_os::canonical::identity::DiscoveredIdentity {
                        native_id: device_id.clone(),
                        room_id: device_id.clone(),
                        room_name: device_name.clone(),
                        name: device_name.clone(),
                        device_type: DeviceType::Light,
                        hardware_ids: vec![HardwareId::matter(
                            &device.node_id.to_string(),
                        )],
                        manufacturer: Some(device.vendor_name.clone()),
                        model: Some(device.product_name.clone()),
                    };

                    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                    s.canonical_registry.resolve(&identity, &hub_key, now);
                }

                Ok(PairingSession {
                    hub_type: "matter".to_string(),
                    status: PairingStatus::Complete,
                    device: Some(PairedDeviceInfo {
                        device_id,
                        name: device_name,
                        device_type: DeviceType::Light,
                        manufacturer: Some(device.vendor_name),
                        model: Some(device.product_name),
                    }),
                    error: None,
                })
            }
            Err(e) => Ok(PairingSession {
                hub_type: "matter".to_string(),
                status: PairingStatus::Failed,
                device: None,
                error: Some(e.to_string()),
            }),
        }
    }
}

pub static INTEGRATION: MatterIntegration = MatterIntegration;
