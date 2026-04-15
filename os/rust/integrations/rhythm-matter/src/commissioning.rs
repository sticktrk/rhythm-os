//! Shared Matter commissioning orchestration.
//!
//! The lifecycle wrappers stay thin and platform-specific. The integration
//! crate owns request parsing, hub bootstrap, node-id allocation, capability
//! registration, and canonical identity updates.

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
    MatterCommissioningRendezvous, MatterTransport,
};

/// Parsed Matter pairing request owned by `rhythm-matter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatterPairingParams {
    /// Manual Matter pairing code.
    pub setup_code: String,
    /// Matter network being commissioned.
    pub network: MatterCommissioningNetwork,
    /// How the commissioner reaches the device.
    pub rendezvous: MatterCommissioningRendezvous,
}

impl MatterPairingParams {
    /// Parse the integration-specific request payload.
    ///
    /// `setup_payload` is the preferred field name. The legacy `setup_code`
    /// field remains accepted for existing clients.
    pub fn from_value(params: &Value) -> Result<Self> {
        let setup_code = params
            .get("setup_payload")
            .or_else(|| params.get("setup_code"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Missing 'setup_payload' (or legacy 'setup_code') in pairing params"
                )
            })?;

        if setup_code.starts_with("MT:") {
            anyhow::bail!(
                "Matter QR payloads are not supported yet; pass the manual pairing code instead"
            );
        }

        let network = match params.get("network").and_then(|v| v.as_str()) {
            None | Some("wifi") => MatterCommissioningNetwork::Wifi,
            Some(other) => anyhow::bail!(
                "Unsupported Matter network '{}'; only 'wifi' is currently supported",
                other
            ),
        };

        let rendezvous = match params.get("rendezvous").and_then(|v| v.as_str()) {
            None | Some("on_network") => MatterCommissioningRendezvous::OnNetwork,
            Some("ble") => anyhow::bail!(
                "Matter BLE rendezvous is not implemented by the current backend; the device must already be IP-reachable"
            ),
            Some(other) => anyhow::bail!(
                "Unsupported Matter rendezvous '{}'; only 'on_network' is currently supported",
                other
            ),
        };

        Ok(Self {
            setup_code: setup_code.to_string(),
            network,
            rendezvous,
        })
    }

    /// Build the transport-facing request for a specific local node ID.
    pub fn to_commission_request(&self, node_id: u64) -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_code: self.setup_code.clone(),
            node_id,
            network: self.network,
            rendezvous: self.rendezvous,
        }
    }
}

/// Ensure the local Matter hub exists before commissioning.
pub fn ensure_matter_hub_connected(state: &SharedState) -> Result<()> {
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

    Ok(())
}

/// Commission a Matter-over-WiFi device using the shared transport.
pub fn pair_device<T: MatterTransport + ?Sized + 'static>(
    state: &SharedState,
    transport: Arc<T>,
    hub_data: Arc<MatterHubData>,
    request: &MatterPairingParams,
) -> Result<PairingSession> {
    let commission_request = request.to_commission_request(hub_data.next_node_id());

    match transport.commission_request(&commission_request) {
        Ok(device) => build_success_session(state, &hub_data, device),
        Err(e) => {
            error!(target: "pair", "Matter commissioning error: {:#}", e);
            Ok(PairingSession {
                hub_type: "matter".to_string(),
                status: PairingStatus::Failed,
                device: None,
                error: Some(format!("{:#}", e)),
            })
        }
    }
}

fn build_success_session(
    state: &SharedState,
    hub_data: &Arc<MatterHubData>,
    device: CommissionedDevice,
) -> Result<PairingSession> {
    let device_id = crate::lifecycle::format_device_id(device.node_id, device.light_endpoint);
    let device_name = format!("{} {}", device.vendor_name, device.product_name);
    let hub_key = HubKey::new(HubType::new("matter"), "local");

    info!(target: "sys", "Matter: paired {} (node {}, id={})",
        device_name, device.node_id, device_id);

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

    if let Ok(mut dc) = hub_data.device_caps.lock() {
        dc.insert(device_id.to_string(), caps);
        info!(target: "sys",
            "Matter: stored caps for {} ({} devices tracked)",
            device_id, dc.len()
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

    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    s.canonical_registry.resolve(&identity, hub_key, now);
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

        assert_eq!(parsed.setup_code, "34970112332");
        assert_eq!(parsed.network, MatterCommissioningNetwork::Wifi);
        assert_eq!(parsed.rendezvous, MatterCommissioningRendezvous::OnNetwork);
    }

    #[test]
    fn pairing_params_accept_setup_payload() {
        let params = serde_json::json!({
            "setup_payload": "3497-011-2332",
            "network": "wifi",
            "rendezvous": "on_network"
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        let request = parsed.to_commission_request(123);

        assert_eq!(request.setup_code, "3497-011-2332");
        assert_eq!(request.node_id, 123);
    }

    #[test]
    fn pairing_params_reject_qr_payloads() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00"
        });

        let err = MatterPairingParams::from_value(&params).unwrap_err();
        assert!(err.to_string().contains("manual pairing code"));
    }

    #[test]
    fn pairing_params_reject_ble_rendezvous() {
        let params = serde_json::json!({
            "setup_payload": "34970112332",
            "rendezvous": "ble"
        });

        let err = MatterPairingParams::from_value(&params).unwrap_err();
        assert!(err.to_string().contains("BLE rendezvous"));
    }
}
