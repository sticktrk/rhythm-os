//! Shared Matter commissioning orchestration.

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use log::{error, info, warn};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{HardwareId, HubKey};
use rhythm_os::hub::HubType;
use rhythm_os::pairing::{PairedDeviceInfo, PairingSession, PairingStage, PairingStatus};
use rhythm_os::state::SharedState;
use serde_json::Value;

use crate::hub_state::MatterHubData;
use crate::transport::{
    CommissionedDevice, MatterCommissionRequest, MatterCommissioningNetwork,
    MatterCommissioningRendezvous, MatterCommissioningWifiCredentials, MatterSubscriptionTarget,
    MatterTransport, DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
    DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
};

/// Parsed Matter pairing request owned by `rhythm-matter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatterPairingParams {
    /// Raw Matter setup payload. May be an `MT:` QR payload or a manual code.
    pub setup_payload: String,
    /// Optional client-generated pairing session ID for SSE correlation.
    pub session_id: Option<String>,
    /// Matter network being commissioned.
    pub network: MatterCommissioningNetwork,
    /// How the commissioner reaches the device.
    pub rendezvous: MatterCommissioningRendezvous,
}

impl MatterPairingParams {
    /// Parse the integration-specific request payload.
    ///
    /// When clients omit `rendezvous`, manual setup codes default to
    /// on-network commissioning while QR payloads keep the existing auto
    /// behavior.
    pub fn from_value(params: &Value) -> Result<Self> {
        let setup_payload = params
            .get("setup_payload")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'setup_payload' in pairing params"))?;
        let session_id = params
            .get("session_id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let network = match params.get("network").and_then(|value| value.as_str()) {
            None | Some("wifi") => MatterCommissioningNetwork::Wifi,
            Some(other) => anyhow::bail!(
                "Unsupported Matter network '{}'; only 'wifi' is currently supported",
                other
            ),
        };

        let rendezvous = match params.get("rendezvous").and_then(|value| value.as_str()) {
            None => default_rendezvous_for_payload(setup_payload),
            Some("auto") => MatterCommissioningRendezvous::Auto,
            Some("ble") => MatterCommissioningRendezvous::Ble,
            Some("on_network") => MatterCommissioningRendezvous::OnNetwork,
            Some(other) => anyhow::bail!(
                "Unsupported Matter rendezvous '{}'; expected 'auto', 'ble', or 'on_network'",
                other
            ),
        };

        Ok(Self {
            setup_payload: setup_payload.to_string(),
            session_id,
            network,
            rendezvous,
        })
    }

    /// Build the transport-facing request for a specific local node ID.
    pub fn to_commission_request(
        &self,
        node_id: u64,
        wifi_credentials: MatterCommissioningWifiCredentials,
    ) -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_payload: self.setup_payload.clone(),
            node_id,
            network: self.network,
            rendezvous: self.rendezvous,
            wifi_credentials,
        }
    }
}

fn default_rendezvous_for_payload(setup_payload: &str) -> MatterCommissioningRendezvous {
    if is_qr_setup_payload(setup_payload) {
        MatterCommissioningRendezvous::Auto
    } else {
        MatterCommissioningRendezvous::OnNetwork
    }
}

fn is_qr_setup_payload(setup_payload: &str) -> bool {
    setup_payload
        .get(..3)
        .map(|prefix| prefix.eq_ignore_ascii_case("MT:"))
        .unwrap_or(false)
}

/// Ensure the local Matter hub exists before commissioning.
pub fn ensure_matter_hub_connected(state: &SharedState) -> Result<()> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let already_connected = state
        .lock()
        .map(|state| state.hubs.contains_key(&hub_key))
        .unwrap_or(false);

    if !already_connected {
        info!(target: "pair", "Matter hub not connected, auto-bootstrapping...");
        let credentials = serde_json::json!({ "fabric_id": "default" });
        rhythm_os::commands::do_hub_credentials(state, "matter", "local", &credentials)?;
    }

    Ok(())
}

/// Commission a Matter-over-WiFi light using the shared transport.
pub fn pair_device(
    state: &SharedState,
    transport: Arc<dyn MatterTransport>,
    hub_data: Arc<MatterHubData>,
    request: &MatterPairingParams,
) -> Result<PairingSession> {
    let wifi_credentials = load_commissioning_wifi_credentials(state)?;
    let node_id = hub_data.reserve_node_id();
    let commission_request = request.to_commission_request(node_id, wifi_credentials);
    rhythm_os::pairing::emit_pairing_progress(
        state,
        "matter",
        request.session_id.as_deref(),
        PairingStatus::Commissioning,
        PairingStage::Commissioning,
        "Commissioning Matter device",
        None,
        None,
    );

    match transport.commission_light(&commission_request) {
        Ok(device) => {
            build_success_session(state, &hub_data, device, request.session_id.as_deref())
        }
        Err(error) => {
            error!(target: "pair", "Matter commissioning error: {:#}", error);
            Ok(PairingSession {
                hub_type: "matter".to_string(),
                status: PairingStatus::Failed,
                device: None,
                error: Some(summarize_commissioning_error(&error)),
            })
        }
    }
}

fn summarize_commissioning_error(error: &anyhow::Error) -> String {
    let detail = format!("{:#}", error);
    let lower = detail.to_ascii_lowercase();

    if lower.contains("gatt write characteristic operation failed") {
        return "Matter BLE commissioning reached the bulb, but macOS CoreBluetooth failed the GATT write. This matches the current official Matter controller behavior on this host. Try Linux/BlueZ or the appliance target for real commissioning.".to_string();
    }

    if lower.contains("connectiondelegate timeout") || lower.contains("discovery timed out") {
        return "Matter BLE commissioning timed out while discovering the bulb from this host. Factory-reset the bulb, keep it close to the machine, and if it still fails, try Linux/BlueZ or the appliance target.".to_string();
    }

    if lower.contains("matter wi-fi commissioning requires stored appliance wi-fi credentials") {
        return "Matter pairing needs stored appliance Wi-Fi credentials on the server before a light can be commissioned.".to_string();
    }

    detail
}

fn load_commissioning_wifi_credentials(
    state: &SharedState,
) -> Result<MatterCommissioningWifiCredentials> {
    let wifi = match load_stored_commissioning_wifi_credentials(state)? {
        Some(wifi) => wifi,
        None => load_platform_commissioning_wifi_credentials(state)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Matter Wi-Fi commissioning requires stored appliance Wi-Fi credentials; provision the appliance over Wi-Fi before pairing Matter lights"
            )
        })?,
    };

    Ok(MatterCommissioningWifiCredentials {
        ssid: wifi.ssid,
        password: wifi.password,
    })
}

fn load_stored_commissioning_wifi_credentials(
    state: &SharedState,
) -> Result<Option<rhythm_os::provisioning::WifiCredentials>> {
    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    let Some(storage) = state.storage.as_ref() else {
        return Ok(None);
    };

    storage
        .load_commissioning_wifi_credentials()
        .context("loading stored appliance Wi-Fi credentials")
}

fn load_platform_commissioning_wifi_credentials(
    state: &SharedState,
) -> Result<Option<rhythm_os::provisioning::WifiCredentials>> {
    let provider = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
        .commissioning_wifi_credentials_provider
        .clone();
    let Some(provider) = provider else {
        return Ok(None);
    };

    let creds = provider().context("loading platform appliance Wi-Fi credentials")?;
    if let Some(creds) = creds.as_ref() {
        persist_commissioning_wifi_credentials(state, creds);
        info!(
            target: "pair",
            "Recovered Matter commissioning Wi-Fi credentials from platform network config for SSID '{}'",
            creds.ssid
        );
    }
    Ok(creds)
}

fn persist_commissioning_wifi_credentials(
    state: &SharedState,
    creds: &rhythm_os::provisioning::WifiCredentials,
) {
    let result = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))
        .and_then(|state| {
            let storage = state
                .storage
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
            storage.save_commissioning_wifi_credentials(creds)
        });

    if let Err(error) = result {
        warn!(
            target: "pair",
            "Failed to persist recovered Matter commissioning Wi-Fi credentials: {:#}",
            error
        );
    }
}

fn build_success_session(
    state: &SharedState,
    hub_data: &Arc<MatterHubData>,
    device: CommissionedDevice,
    session_id: Option<&str>,
) -> Result<PairingSession> {
    rhythm_os::pairing::emit_pairing_progress(
        state,
        "matter",
        session_id,
        PairingStatus::Commissioning,
        PairingStage::Finalizing,
        "Finalizing paired Matter device",
        None,
        None,
    );

    let device_id = crate::lifecycle::format_device_id(device.node_id, device.light_endpoint);
    let device_name = format!("{} {}", device.vendor_name, device.product_name);
    let hub_key = HubKey::new(HubType::new("matter"), "local");

    info!(
        target: "sys",
        "Matter: paired {} (node {}, id={})",
        device_name,
        device.node_id,
        device_id
    );

    hub_data.record_commissioned_device(&device);
    store_device_metadata(hub_data, &device, &device_id);
    subscribe_paired_device(hub_data, &device, &device_id);
    if let Err(error) = crate::capture::persist_device_capture(hub_data, &device, "pair") {
        warn!(
            target: "sys",
            "Matter: failed to persist pair capture for {}: {}",
            device_id,
            error
        );
    }
    register_canonical_identity(state, &hub_key, &device, &device_id, &device_name)?;
    materialize_unassigned_canonical_device(state, &hub_key, &device_id)?;

    let _ = hub_data.event_tx.send(
        crate::events::device_paired_event(
            device.node_id,
            &device.vendor_name,
            &device.product_name,
        )
        .with_hub_key(hub_key),
    );

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

fn subscribe_paired_device(
    hub_data: &Arc<MatterHubData>,
    device: &CommissionedDevice,
    device_id: &str,
) {
    let Some(transport) = hub_data.transport.get() else {
        return;
    };
    let target = MatterSubscriptionTarget {
        node_id: device.node_id,
        endpoint: device.light_endpoint,
    };
    if let Err(error) = transport.subscribe_on_off(
        &[target],
        DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
        DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
    ) {
        warn!(
            target: "sys",
            "Matter: failed to subscribe paired device {} for live On/Off reports: {}",
            device_id,
            error
        );
    }
}

pub(crate) fn store_device_metadata(
    hub_data: &Arc<MatterHubData>,
    device: &CommissionedDevice,
    device_id: &str,
) {
    let mut caps = build_device_capabilities(device);
    let mut quirks = build_device_quirks(device);
    if let Ok(cloud_profiles) = hub_data.cloud_profiles.lock() {
        cloud_profiles.apply_to_device(device, &mut caps, &mut quirks);
    }

    if let Ok(mut device_caps) = hub_data.device_caps.lock() {
        device_caps.insert(device_id.to_string(), caps);
        info!(
            target: "sys",
            "Matter: stored caps for {} ({} devices tracked)",
            device_id,
            device_caps.len()
        );
    }

    if let Ok(mut device_quirks) = hub_data.device_quirks.lock() {
        device_quirks.insert(device_id.to_string(), quirks);
    }
}

pub(crate) fn build_device_capabilities(
    device: &CommissionedDevice,
) -> rhythm_devices::LightCapabilities {
    let mut caps = crate::capabilities::capabilities_from_commissioned(device);
    crate::capabilities::enrich_from_db(&mut caps, device, rhythm_devices::builtin_db());
    caps
}

pub(crate) fn build_device_quirks(device: &CommissionedDevice) -> Vec<rhythm_devices::DeviceQuirk> {
    crate::capabilities::quirks_from_db(device, rhythm_devices::builtin_db())
}

fn register_canonical_identity(
    state: &SharedState,
    hub_key: &HubKey,
    device: &CommissionedDevice,
    device_id: &str,
    device_name: &str,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let identity = rhythm_os::canonical::identity::DiscoveredIdentity {
        native_id: device_id.to_string(),
        room_id: None,
        room_name: None,
        name: device_name.to_string(),
        device_type: DeviceType::Light,
        hardware_ids: vec![HardwareId::matter(&device.node_id.to_string())],
        manufacturer: Some(device.vendor_name.clone()),
        model: Some(device.product_name.clone()),
    };

    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state.canonical_registry.resolve(&identity, hub_key, now);
    Ok(())
}

fn materialize_unassigned_canonical_device(
    state: &SharedState,
    hub_key: &HubKey,
    device_id: &str,
) -> Result<()> {
    let state_guard = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let (canonical_id, already_assigned) = state_guard
        .canonical_registry
        .find_by_native_id(hub_key, device_id)
        .map(|device| (device.id.clone(), device.room_id.is_some()))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Canonical device missing after Matter pairing: {}",
                device_id
            )
        })?;

    drop(state_guard);

    if already_assigned {
        rhythm_os::commands::reconcile_runtime_from_state(state)?;
    } else {
        rhythm_os::commands::do_canonical_assign_room(state, &canonical_id, None)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_params_accept_raw_mt_payload() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
            "network": "wifi",
            "rendezvous": "ble"
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        let request = parsed.to_commission_request(
            123,
            MatterCommissioningWifiCredentials {
                ssid: "wifi".to_string(),
                password: "secret".to_string(),
            },
        );

        assert_eq!(request.setup_payload, "MT:Y.K908OC16750648G00");
        assert_eq!(request.node_id, 123);
        assert_eq!(request.rendezvous, MatterCommissioningRendezvous::Ble);
    }

    #[test]
    fn pairing_params_default_to_auto_rendezvous() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        assert_eq!(parsed.rendezvous, MatterCommissioningRendezvous::Auto);
    }

    #[test]
    fn pairing_params_accept_session_id_for_progress_correlation() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
            "session_id": "pair-1",
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        assert_eq!(parsed.session_id.as_deref(), Some("pair-1"));
    }

    #[test]
    fn commissioning_error_summarizes_darwin_gatt_failures() {
        let error = anyhow::anyhow!(
            "commissioning Matter light: Ble Error 0x00000407: GATT write characteristic operation failed"
        );

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("CoreBluetooth failed the GATT write"));
        assert!(message.contains("Linux/BlueZ"));
    }

    #[test]
    fn commissioning_error_summarizes_ble_timeouts() {
        let error = anyhow::anyhow!("commissioning Matter light: ConnectionDelegate timeout");

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("timed out while discovering the bulb"));
    }
}
