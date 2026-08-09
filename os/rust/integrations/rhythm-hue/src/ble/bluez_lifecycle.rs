//! Linux registration of Hue BLE as a first-class local hub integration.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use anyhow::Result;
use log::warn;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{
    ExternalLightHubIntegration, HubCredentials, HubEvent, HubIntegrationCapability, HubProvider,
    HubType, DEVICE_ONBOARDING_METHOD_HUE_BLE_NEARBY_SCAN,
};
use rhythm_os::pairing::{PairingSession, UnpairingResult};
use rhythm_os::state::SharedState;

use super::bluez::BluezHueBleTransport;
use super::lifecycle;
use super::types::{HueBleCommand, HueBlePairingRequest};
use super::{HUB_ADDRESS, HUB_TYPE};

pub static INTEGRATION: BluezHueBleIntegration = BluezHueBleIntegration;

pub struct BluezHueBleIntegration;

struct BluezHueBleProvider;
static PROVIDER: BluezHueBleProvider = BluezHueBleProvider;

impl HubProvider for BluezHueBleProvider {
    fn hub_type(&self) -> HubType {
        HubType::new(HUB_TYPE)
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        rhythm_os::lifecycle::configure_hub(
            state,
            address,
            credentials_json,
            |address, json| {
                let data = serde_json::from_str(json)
                    .unwrap_or_else(|_| serde_json::json!({ "adapter": "default" }));
                Ok(HubCredentials::new(HUB_TYPE, address, data))
            },
            |state, credentials| {
                let key = credentials.hub_key();
                let state = match state.lock() {
                    Ok(state) => state,
                    Err(_) => return false,
                };
                key.is_some_and(|key| {
                    state.hubs.contains_key(&key) && state.hub_credentials.contains_key(&key)
                })
            },
            |state| {
                let transport = Arc::new(BluezHueBleTransport::new()?);
                lifecycle::connect(state, transport)
            },
        )
    }
}

impl BluezHueBleIntegration {
    fn ensure_connected(&self, state: &SharedState) -> Result<()> {
        let key = lifecycle::hub_key();
        if state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .hubs
            .contains_key(&key)
        {
            return Ok(());
        }

        let has_credentials = state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .hub_credentials
            .contains_key(&key);
        if !has_credentials {
            rhythm_os::commands::do_hub_credentials(
                state,
                HUB_TYPE,
                HUB_ADDRESS,
                &serde_json::json!({ "adapter": "default" }),
            )?;
            return Ok(());
        }

        let receiver = self.connect_and_start(state.clone(), &key)?;
        state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .pending_hub_event_rxs
            .push(receiver);
        Ok(())
    }
}

fn start_unpairing_with<F>(
    state: &SharedState,
    params: &serde_json::Value,
    ensure_connected: F,
) -> Result<UnpairingResult>
where
    F: FnOnce(&SharedState) -> Result<()>,
{
    let device_id = params
        .get("device_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Missing 'device_id' in Hue BLE unpair request"))?;
    let force = params
        .get("force")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    match ensure_connected(state) {
        Ok(()) => lifecycle::unpair(state, device_id, force),
        Err(error) if force => {
            warn!(
                target: "pair",
                "Hue BLE adapter connection failed during forced removal; removing local metadata while preserving any on-disk BlueZ bond: {error:#}"
            );
            lifecycle::force_forget_offline(state, device_id)
        }
        Err(error) => Ok(UnpairingResult {
            hub_type: HUB_TYPE.to_string(),
            hub_address: Some(HUB_ADDRESS.to_string()),
            status: rhythm_os::pairing::PairingStatus::Failed,
            device_id: Some(device_id.to_string()),
            error: Some(format!("{error:#}")),
            completion_scope: None,
            warning: None,
        }),
    }
}

impl ExternalLightHubIntegration for BluezHueBleIntegration {
    fn hub_type(&self) -> &'static str {
        HUB_TYPE
    }

    fn provider(&self) -> &'static dyn HubProvider {
        &PROVIDER
    }

    fn api_capabilities(&self) -> HubIntegrationCapability {
        HubIntegrationCapability {
            hub_type: HUB_TYPE.to_string(),
            configurable: false,
            device_onboarding_methods: vec![
                DEVICE_ONBOARDING_METHOD_HUE_BLE_NEARBY_SCAN.to_string()
            ],
            device_profiles: Vec::new(),
            supports_unpairing: true,
            unpairable_device_types: vec!["light".to_string()],
            supports_roomless_devices: true,
            blocks_room_readiness: true,
        }
    }

    fn connect_and_start(&self, state: SharedState, _key: &HubKey) -> Result<Receiver<HubEvent>> {
        let transport = Arc::new(BluezHueBleTransport::new()?);
        let (hub, event_rx) = lifecycle::connect(&state, transport)?;
        {
            let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let key = hub.hub_key.clone();
            if state.hubs.contains_key(&key) {
                // Stored-hub bootstrap and an HTTP pairing request can race
                // during boot. Keep the first live hub so its in-memory
                // device store cannot be replaced by a stale second load.
                drop(state);
                drop(hub);
                drop(event_rx);
                let (_closed_tx, closed_rx) = std::sync::mpsc::channel();
                return Ok(closed_rx);
            }
            state.hubs.insert(key.clone(), hub);
            state.set_hub_connected(&key, false);
        }
        Ok(event_rx)
    }

    fn ensure_runtime(&self, _state: &SharedState) -> Result<()> {
        Ok(())
    }

    fn create_controller(
        &self,
        state: &SharedState,
        _key: &HubKey,
    ) -> Result<Arc<dyn rhythm_core::HubLightController>> {
        lifecycle::create_controller(state)
    }

    fn start_pairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<PairingSession> {
        let request = HueBlePairingRequest::from_value(params)?;
        rhythm_os::pairing::emit_pairing_progress(
            state,
            HUB_TYPE,
            request.session_id.as_deref(),
            rhythm_os::pairing::PairingStatus::Searching,
            rhythm_os::pairing::PairingStage::HubConnecting,
            "Preparing the Bluetooth adapter",
            None,
            None,
        );
        self.ensure_connected(state)?;
        lifecycle::pair(state, &request)
    }

    fn start_unpairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<UnpairingResult> {
        start_unpairing_with(state, params, |state| self.ensure_connected(state))
    }

    fn run_device_test(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.ensure_connected(state)?;
        let data = lifecycle::get_hub_data(state)?;
        let requested_id = params
            .get("device_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing Hue BLE device_id"))?;
        let native_id = lifecycle::resolve_native_id(state, requested_id)?;
        let device = data
            .store
            .get(&native_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Hue BLE device: {native_id}"))?;
        let action = params
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("state");
        match action {
            "state" => Ok(serde_json::to_value(data.transport.read_state(&device)?)?),
            "identify" => {
                data.transport.identify(&device)?;
                Ok(serde_json::json!({ "ok": true }))
            }
            "effect" => {
                let effect = params
                    .get("effect")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Missing Hue BLE effect"))?;
                let effect = serde_json::from_value(effect)?;
                let effect_speed = params
                    .get("speed")
                    .and_then(serde_json::Value::as_u64)
                    .map(|speed| speed.min(255) as u8);
                data.transport.apply_command(
                    &device,
                    &HueBleCommand {
                        on: Some(true),
                        effect: Some(effect),
                        effect_speed,
                        ..Default::default()
                    },
                )?;
                Ok(serde_json::json!({ "ok": true }))
            }
            other => anyhow::bail!(
                "Unknown Hue BLE test action '{other}' (expected state, identify, or effect)"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::store::HueBleDeviceStore;
    use crate::ble::types::{HueBleCapabilities, HueBleDevice};
    use std::sync::{Arc, Mutex};

    #[test]
    fn capability_advertises_code_less_nearby_scan() {
        let capabilities = INTEGRATION.api_capabilities();
        assert_eq!(capabilities.hub_type, HUB_TYPE);
        assert_eq!(
            capabilities.device_onboarding_methods,
            vec![DEVICE_ONBOARDING_METHOD_HUE_BLE_NEARBY_SCAN]
        );
        assert!(capabilities.supports_unpairing);
        assert!(capabilities.supports_roomless_devices);
    }

    fn test_device() -> HueBleDevice {
        HueBleDevice {
            id: "hue-ble-001788010c765ba7".to_string(),
            address: "EA:84:C2:50:A8:65".to_string(),
            address_type: "random".to_string(),
            eui64: "001788010c765ba7".to_string(),
            name: "Hue color lamp".to_string(),
            manufacturer: "Signify Netherlands B.V.".to_string(),
            model: "LCA013".to_string(),
            firmware: "1.0".to_string(),
            capabilities: HueBleCapabilities {
                dimming: true,
                ..Default::default()
            },
            paired_at_epoch_secs: 1,
            last_state: None,
        }
    }

    fn offline_state(label: &str) -> (SharedState, std::path::PathBuf) {
        let unique = format!(
            "rhythm-hue-ble-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let data_dir = std::env::temp_dir().join(unique);
        let mut app_state = rhythm_os::state::AppState::default();
        app_state.data_dir = data_dir.to_string_lossy().into_owned();
        (Arc::new(Mutex::new(app_state)), data_dir)
    }

    #[test]
    fn unavailable_adapter_requires_force_and_preserves_metadata() {
        let (state, data_dir) = offline_state("offline-graceful");
        let device = test_device();
        let store = HueBleDeviceStore::load(&data_dir).unwrap();
        store.upsert(device.clone()).unwrap();

        let result =
            start_unpairing_with(&state, &serde_json::json!({"device_id": device.id}), |_| {
                anyhow::bail!("Bluetooth adapter is unavailable")
            })
            .unwrap();

        assert_eq!(result.status, rhythm_os::pairing::PairingStatus::Failed);
        assert!(HueBleDeviceStore::load(&data_dir)
            .unwrap()
            .get(&device.id)
            .is_some());
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn unavailable_adapter_force_removes_local_metadata() {
        let (state, data_dir) = offline_state("offline-force");
        let device = test_device();
        let store = HueBleDeviceStore::load(&data_dir).unwrap();
        store.upsert(device.clone()).unwrap();

        let result = start_unpairing_with(
            &state,
            &serde_json::json!({"device_id": device.id, "force": true}),
            |_| anyhow::bail!("Bluetooth adapter is unavailable"),
        )
        .unwrap();

        assert_eq!(result.status, rhythm_os::pairing::PairingStatus::Complete);
        let reloaded = HueBleDeviceStore::load(&data_dir).unwrap();
        assert!(reloaded.get(&device.id).is_none());
        assert_eq!(reloaded.tombstone(&device.id), Some(device));
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn unavailable_adapter_force_quarantines_corrupt_metadata() {
        let (state, data_dir) = offline_state("offline-corrupt-force");
        let store_dir = data_dir.join("hue_ble");
        std::fs::create_dir_all(&store_dir).unwrap();
        std::fs::write(store_dir.join("devices.json"), b"{bad").unwrap();
        let device = test_device();

        let result = start_unpairing_with(
            &state,
            &serde_json::json!({"device_id": device.id, "force": true}),
            |_| anyhow::bail!("Bluetooth adapter is unavailable"),
        )
        .unwrap();

        assert_eq!(result.status, rhythm_os::pairing::PairingStatus::Complete);
        assert!(!store_dir.join("devices.json").exists());
        assert!(HueBleDeviceStore::load(&data_dir)
            .unwrap()
            .blocks_paired_orphan_adoption());
        assert_eq!(
            std::fs::read_dir(&store_dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt."))
                .count(),
            1
        );
        std::fs::remove_dir_all(data_dir).unwrap();
    }
}
