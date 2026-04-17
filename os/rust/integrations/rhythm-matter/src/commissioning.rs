//! Shared Matter commissioning orchestration.

use std::sync::Arc;

use anyhow::Result;
use log::{error, info};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{HardwareId, HubKey};
use rhythm_os::hub::HubType;
use rhythm_os::pairing::{PairedDeviceInfo, PairingSession, PairingStatus};
use rhythm_os::state::SharedState;
use serde_json::Value;

use crate::hub_state::MatterHubData;
use crate::transport::{
    CommissionedDevice, MatterCommissionRequest, MatterCommissioningNetwork,
    MatterCommissioningRendezvous, MatterCommissioningWifiCredentials, MatterTransport,
};

/// Parsed Matter pairing request owned by `rhythm-matter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatterPairingParams {
    /// Raw Matter setup payload. May be an `MT:` QR payload or a manual code.
    pub setup_payload: String,
    /// Matter network being commissioned.
    pub network: MatterCommissioningNetwork,
    /// How the commissioner reaches the device.
    pub rendezvous: MatterCommissioningRendezvous,
}

impl MatterPairingParams {
    /// Parse the integration-specific request payload.
    ///
    /// `setup_payload` is the preferred field name. The legacy `setup_code`
    /// field remains accepted for older clients.
    pub fn from_value(params: &Value) -> Result<Self> {
        let setup_payload = params
            .get("setup_payload")
            .or_else(|| params.get("setup_code"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Missing 'setup_payload' (or legacy 'setup_code') in pairing params"
                )
            })?;

        let network = match params.get("network").and_then(|value| value.as_str()) {
            None | Some("wifi") => MatterCommissioningNetwork::Wifi,
            Some(other) => anyhow::bail!(
                "Unsupported Matter network '{}'; only 'wifi' is currently supported",
                other
            ),
        };

        let rendezvous = match params.get("rendezvous").and_then(|value| value.as_str()) {
            None | Some("auto") => MatterCommissioningRendezvous::Auto,
            Some("ble") => MatterCommissioningRendezvous::Ble,
            Some("on_network") => MatterCommissioningRendezvous::OnNetwork,
            Some(other) => anyhow::bail!(
                "Unsupported Matter rendezvous '{}'; expected 'auto', 'ble', or 'on_network'",
                other
            ),
        };

        Ok(Self {
            setup_payload: setup_payload.to_string(),
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

    match transport.commission_light(&commission_request) {
        Ok(device) => build_success_session(state, &hub_data, device),
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
    let wifi = state
        .lock()
        .ok()
        .and_then(|state| state.storage.as_ref().and_then(|storage| {
            storage
                .load_commissioning_wifi_credentials()
                .ok()
                .flatten()
        }))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Matter Wi-Fi commissioning requires stored appliance Wi-Fi credentials; provision the appliance over Wi-Fi before pairing Matter lights"
            )
        })?;

    Ok(MatterCommissioningWifiCredentials {
        ssid: wifi.ssid,
        password: wifi.password,
    })
}

fn build_success_session(
    state: &SharedState,
    hub_data: &Arc<MatterHubData>,
    device: CommissionedDevice,
) -> Result<PairingSession> {
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
    store_device_capabilities(hub_data, &device, &device_id);
    register_canonical_identity(state, &hub_key, &device, &device_id, &device_name)?;

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

fn store_device_capabilities(
    hub_data: &Arc<MatterHubData>,
    device: &CommissionedDevice,
    device_id: &str,
) {
    let mut caps = crate::capabilities::capabilities_from_commissioned(device);
    crate::capabilities::enrich_from_db(&mut caps, device, rhythm_devices::builtin_db());

    if let Ok(mut device_caps) = hub_data.device_caps.lock() {
        device_caps.insert(device_id.to_string(), caps);
        info!(
            target: "sys",
            "Matter: stored caps for {} ({} devices tracked)",
            device_id,
            device_caps.len()
        );
    }
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
        room_id: device_id.to_string(),
        room_name: device_name.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_params_accept_legacy_setup_code() {
        let params = serde_json::json!({
            "setup_code": "34970112332"
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();

        assert_eq!(parsed.setup_payload, "34970112332");
        assert_eq!(parsed.network, MatterCommissioningNetwork::Wifi);
        assert_eq!(parsed.rendezvous, MatterCommissioningRendezvous::Auto);
    }

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
            "setup_payload": "3497-011-2332",
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        assert_eq!(parsed.rendezvous, MatterCommissioningRendezvous::Auto);
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
