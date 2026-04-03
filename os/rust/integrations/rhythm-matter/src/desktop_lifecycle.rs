//! Desktop Matter lifecycle using matc transport.
//!
//! Provides `ExternalLightHubIntegration` impl so rhythm-server can
//! register Matter as a hub alongside Hue and HA.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use anyhow::Result;
use log::info;
#[allow(unused_imports)]
use log::warn;

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

/// Get the shared transport from the hub's MatterHubData.
fn get_transport(state: &SharedState) -> Result<Arc<MatcTransport>> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let hub = s
        .hubs
        .get(&hub_key)
        .ok_or_else(|| anyhow::anyhow!("Matter hub not connected"))?;
    let hub_data = hub
        .data::<Arc<MatterHubData>>()
        .ok_or_else(|| anyhow::anyhow!("Matter hub data missing"))?;
    hub_data
        .transport
        .get()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Matter transport not initialized"))
}

/// Connect to the local Matter fabric and store the hub in state.
///
/// Loads or creates the Matter fabric in `{data_dir}/matter/`.
/// Returns the event receiver for the main event loop.
pub fn connect_and_start(state: SharedState, _key: &HubKey) -> Result<Receiver<HubEvent>> {
    let data_path = matter_data_path(&state)?;
    let transport = Arc::new(MatcTransport::load_or_create(&data_path)?);
    let (mut hub, event_rx) = crate::lifecycle::connect_matter(&state, transport.clone())?;

    // Store the shared transport in hub_data so all code paths use one DM
    if let Some(hub_data) = hub.data::<Arc<MatterHubData>>() {
        let _ = hub_data.transport.set(transport.clone());
    }

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
///
/// Uses the shared transport from `MatterHubData` instead of creating a new
/// DeviceManager (which would conflict on port 5555).
pub fn create_controller(state: &SharedState, _key: &HubKey) -> Result<Arc<dyn LightController>> {
    let transport = get_transport(state)?;

    let hub_data = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = HubKey::new(HubType::new("matter"), "local");
        s.hubs
            .get(&key)
            .and_then(|h| h.data::<Arc<MatterHubData>>())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Matter hub not connected"))?
    };

    Ok(Arc::new(MatterLightController::new(transport, hub_data)))
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
            let (hub, rx) = crate::lifecycle::connect_matter(state, transport.clone())?;

            // Store transport in hub_data
            if let Some(hub_data) = hub.data::<Arc<MatterHubData>>() {
                let _ = hub_data.transport.set(transport);
            }

            Ok((hub, rx))
        })
    }
}

static MATTER_PROVIDER: MatterHubProvider = MatterHubProvider;

pub fn get_hub_provider() -> &'static dyn HubProvider {
    &MATTER_PROVIDER
}

// ============================================================================
// Device probing
// ============================================================================

/// Probe a commissioned device's capabilities via transport read_attribute calls.
///
/// Reads Basic Information (0x0028) and Color Control (0x0300) clusters to
/// reconstruct a `CommissionedDevice` with accurate capabilities.
#[allow(dead_code)]
fn probe_device_caps(
    transport: &dyn MatterTransport,
    node_id: u64,
    endpoint: u16,
) -> Result<crate::transport::CommissionedDevice> {
    use crate::transport::{CommissionedDevice, MatterColorMode};

    let vendor_name =
        read_string_via_transport(transport, node_id, 0, 0x0028, 1).unwrap_or_default();
    let product_name =
        read_string_via_transport(transport, node_id, 0, 0x0028, 2).unwrap_or_default();
    let vendor_id = read_u16_via_transport(transport, node_id, 0, 0x0028, 4).unwrap_or(0);
    let product_id = read_u16_via_transport(transport, node_id, 0, 0x0028, 5).unwrap_or(0);

    // Probe Color Control capabilities
    let capabilities_raw =
        read_u16_via_transport(transport, node_id, endpoint, 0x0300, 0x400A).unwrap_or(0);

    let mut color_modes = Vec::new();
    if capabilities_raw & 0x01 != 0 {
        color_modes.push(MatterColorMode::HueSaturation);
    }
    if capabilities_raw & 0x08 != 0 {
        color_modes.push(MatterColorMode::Xy);
    }
    if capabilities_raw & 0x10 != 0 {
        color_modes.push(MatterColorMode::ColorTemperature);
    }

    // Fallback: read ColorMode attribute if capabilities was 0
    if color_modes.is_empty() {
        let color_mode =
            read_u8_via_transport(transport, node_id, endpoint, 0x0300, 0x0008).unwrap_or(2);
        match color_mode {
            0 => color_modes.push(MatterColorMode::HueSaturation),
            1 => color_modes.push(MatterColorMode::Xy),
            _ => color_modes.push(MatterColorMode::ColorTemperature),
        }
    }

    // CT range in mireds → kelvin
    let min_mireds = read_u16_via_transport(transport, node_id, endpoint, 0x0300, 0x400C).ok();
    let max_mireds = read_u16_via_transport(transport, node_id, endpoint, 0x0300, 0x400D).ok();

    let min_kelvin = max_mireds
        .filter(|&m| m > 0)
        .map(|m| (1_000_000u32 / m as u32) as u16);
    let max_kelvin = min_mireds
        .filter(|&m| m > 0)
        .map(|m| (1_000_000u32 / m as u32) as u16);

    Ok(CommissionedDevice {
        node_id,
        vendor_name,
        product_name,
        vendor_id,
        product_id,
        serial_number: None,
        light_endpoint: endpoint,
        color_modes,
        min_kelvin,
        max_kelvin,
    })
}

/// Read a string attribute via `MatterTransport::read_attribute`.
#[allow(dead_code)]
fn read_string_via_transport(
    transport: &dyn MatterTransport,
    node_id: u64,
    endpoint: u16,
    cluster: u16,
    attr_id: u16,
) -> Result<String> {
    let data = transport.read_attribute(node_id, endpoint, cluster, attr_id)?;
    Ok(String::from_utf8(data).unwrap_or_default())
}

/// Read a u16 attribute via `MatterTransport::read_attribute`.
#[allow(dead_code)]
fn read_u16_via_transport(
    transport: &dyn MatterTransport,
    node_id: u64,
    endpoint: u16,
    cluster: u16,
    attr_id: u16,
) -> Result<u16> {
    let data = transport.read_attribute(node_id, endpoint, cluster, attr_id)?;
    if data.len() >= 2 {
        Ok(u16::from_le_bytes([data[0], data[1]]))
    } else if data.len() == 1 {
        Ok(data[0] as u16)
    } else {
        Err(anyhow::anyhow!("Empty attribute response"))
    }
}

/// Read a u8 attribute via `MatterTransport::read_attribute`.
#[allow(dead_code)]
fn read_u8_via_transport(
    transport: &dyn MatterTransport,
    node_id: u64,
    endpoint: u16,
    cluster: u16,
    attr_id: u16,
) -> Result<u8> {
    let data = transport.read_attribute(node_id, endpoint, cluster, attr_id)?;
    data.first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Empty attribute response"))
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

    fn ensure_runtime(&self, _state: &SharedState) -> Result<()> {
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

    fn post_connect(&self, _state: &SharedState, _key: &HubKey) {
        // Capability probing skipped on startup — the controller falls back
        // to ExtendedColor defaults for devices without cached caps.
        // Caps are populated during pairing (commission_device) and don't
        // need to be re-probed on every restart.
    }

    fn credentials_interceptor(
        &self,
        state: &SharedState,
        body: &serde_json::Value,
    ) -> Option<Result<String, String>> {
        let hub_type = body.get("hub_type").and_then(|v| v.as_str())?;
        if hub_type != "matter" {
            return None;
        }

        // Only intercept when credentials are empty/null (auto-bootstrap request)
        let creds_empty = body
            .get("credentials")
            .map(|v| v.is_null() || v.as_object().map(|o| o.is_empty()).unwrap_or(false))
            .unwrap_or(true);
        if !creds_empty {
            return None;
        }

        let credentials = serde_json::json!({ "fabric_id": "default" });

        match rhythm_os::commands::do_hub_credentials(state, "matter", "local", &credentials) {
            Ok(()) => {
                let hub_connected = state.lock().map(|s| s.has_any_hub()).unwrap_or(false);

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
        use rhythm_core::runtime::hub_registry::DeviceType;
        use rhythm_os::canonical::identity::HardwareId;
        use rhythm_os::pairing::{PairedDeviceInfo, PairingStatus};

        let setup_code = params
            .get("setup_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'setup_code' in pairing params"))?;

        // Ensure the Matter hub is connected before commissioning.
        // If not already connected, store credentials and connect so the
        // device registry and capabilities are available after pairing.
        {
            let hub_key = HubKey::new(HubType::new("matter"), "local");
            let already_connected = state
                .lock()
                .map(|s| s.hubs.contains_key(&hub_key))
                .unwrap_or(false);
            if !already_connected {
                info!(target: "pair", "Matter hub not connected, auto-bootstrapping...");
                let credentials = serde_json::json!({ "fabric_id": "default" });
                rhythm_os::commands::do_hub_credentials(state, "matter", "local", &credentials)?;
            }
        }

        // Get the shared transport and derive next node ID from commissioned devices.
        // Using the shared transport avoids creating a second DeviceManager that
        // would conflict on port 5555.
        let (transport, next_node_id) = {
            let hub_key = HubKey::new(HubType::new("matter"), "local");
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            if let Some(hub) = s.hubs.get(&hub_key) {
                if let Some(hub_data) = hub.data::<Arc<MatterHubData>>() {
                    let tr = hub_data
                        .transport
                        .get()
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Matter transport not initialized"))?;
                    let max = hub_data
                        .commissioned
                        .lock()
                        .map(|c| c.iter().map(|d| d.node_id).max().unwrap_or(99))
                        .unwrap_or(99);
                    (tr, max + 1)
                } else {
                    return Err(anyhow::anyhow!("Matter hub data missing"));
                }
            } else {
                return Err(anyhow::anyhow!("Matter hub not connected"));
            }
        };

        // Commission using the shared transport's drop-and-recreate flow.
        // This temporarily drops the existing DM to free port 5555, commissions
        // on a dedicated thread, then reloads the DM.
        match transport.commission_device(setup_code, next_node_id) {
            Ok(device) => {
                let device_id =
                    crate::lifecycle::format_device_id(device.node_id, device.light_endpoint);
                let device_name = format!("{} {}", device.vendor_name, device.product_name);

                info!(target: "sys", "Matter: paired {} (node {}, id={})",
                    device_name, device.node_id, device_id);

                // Store device capabilities from commissioning data.
                // No room is created here — the user assigns Matter devices
                // to rooms via the app's topology system.
                let hub_key = HubKey::new(HubType::new("matter"), "local");

                // Store device capabilities from commissioning data
                {
                    let mut caps = crate::capabilities::capabilities_from_commissioned(&device);
                    crate::capabilities::enrich_from_db(
                        &mut caps,
                        &device,
                        rhythm_devices::builtin_db(),
                    );
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
                        hardware_ids: vec![HardwareId::matter(&device.node_id.to_string())],
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
            Err(e) => {
                log::error!(target: "pair", "Matter commissioning error: {:#}", e);
                Ok(PairingSession {
                    hub_type: "matter".to_string(),
                    status: PairingStatus::Failed,
                    device: None,
                    error: Some(format!("{:#}", e)),
                })
            }
        }
    }

    fn start_unpairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<rhythm_os::pairing::UnpairingResult> {
        use rhythm_os::pairing::{PairingStatus, UnpairingResult};

        let device_id = params
            .get("device_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'device_id' in unpairing params"))?;
        let force = params
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let (node_id, _endpoint) = crate::lifecycle::parse_device_id(device_id)
            .ok_or_else(|| anyhow::anyhow!("Invalid Matter device ID: {}", device_id))?;

        let hub_key = HubKey::new(HubType::new("matter"), "local");

        // Get transport + hub_data from the connected hub
        let (transport, hub_data) = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let hub = s
                .hubs
                .get(&hub_key)
                .ok_or_else(|| anyhow::anyhow!("Matter hub not connected"))?;
            let hd = hub
                .data::<Arc<MatterHubData>>()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Matter hub data missing"))?;
            let tr = hd
                .transport
                .get()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Matter transport not initialized"))?;
            (tr, hd)
        };

        // Protocol-specific: decommission from fabric
        match transport.decommission_device(node_id, force) {
            Ok(ota_ok) => {
                info!(target: "sys", "Matter: decommissioned node {} (ota={})", node_id, ota_ok);

                // Integration-specific: clean up MatterHubData
                hub_data.remove_device(node_id);

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
