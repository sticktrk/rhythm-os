//! Linux registration of Orein/AiDot buttons as an appliance-local hub.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use anyhow::Result;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{
    ExternalLightHubIntegration, HubCredentials, HubEvent, HubIntegrationCapability, HubProvider,
    HubType, DEVICE_ONBOARDING_METHOD_AIDOT_BUTTON_QR,
};
use rhythm_os::pairing::{PairingSession, UnpairingResult};
use rhythm_os::state::SharedState;

use crate::bluez::BluezAidotTransport;
use crate::lifecycle;
use crate::{HUB_ADDRESS, HUB_TYPE};

pub static INTEGRATION: BluezAidotIntegration = BluezAidotIntegration;

pub struct BluezAidotIntegration;

struct BluezAidotProvider;
static PROVIDER: BluezAidotProvider = BluezAidotProvider;

impl HubProvider for BluezAidotProvider {
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
                let Some(key) = credentials.hub_key() else {
                    return false;
                };
                state.lock().ok().is_some_and(|state| {
                    state.hubs.contains_key(&key) && state.hub_credentials.contains_key(&key)
                })
            },
            |state| lifecycle::connect(state, Arc::new(BluezAidotTransport::new()?)),
        )
    }
}

impl BluezAidotIntegration {
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

impl ExternalLightHubIntegration for BluezAidotIntegration {
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
            device_onboarding_methods: vec![DEVICE_ONBOARDING_METHOD_AIDOT_BUTTON_QR.to_string()],
            supports_unpairing: true,
            supports_roomless_devices: true,
        }
    }

    fn connect_and_start(&self, state: SharedState, _key: &HubKey) -> Result<Receiver<HubEvent>> {
        let (hub, event_rx) = lifecycle::connect(&state, Arc::new(BluezAidotTransport::new()?))?;
        let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = hub.hub_key.clone();
        if state.hubs.contains_key(&key) {
            drop(state);
            drop(hub);
            drop(event_rx);
            let (_closed_tx, closed_rx) = std::sync::mpsc::channel();
            return Ok(closed_rx);
        }
        state.hubs.insert(key.clone(), hub);
        state.set_hub_connected(&key, false);
        Ok(event_rx)
    }

    fn ensure_runtime(&self, _state: &SharedState) -> Result<()> {
        Ok(())
    }

    fn create_controller(
        &self,
        _state: &SharedState,
        _key: &HubKey,
    ) -> Result<Arc<dyn rhythm_core::HubLightController>> {
        Ok(Arc::new(rhythm_core::NoOpController::new()))
    }

    fn start_pairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<PairingSession> {
        let setup_payload = params
            .get("setup_payload")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing 'setup_payload' in AiDot pairing request"))?;
        let session_id = params.get("session_id").and_then(serde_json::Value::as_str);
        rhythm_os::pairing::emit_pairing_progress(
            state,
            HUB_TYPE,
            session_id,
            rhythm_os::pairing::PairingStatus::Searching,
            rhythm_os::pairing::PairingStage::HubConnecting,
            "Preparing the Bluetooth adapter",
            None,
            None,
        );
        self.ensure_connected(state)?;
        lifecycle::pair(state, setup_payload, session_id)
    }

    fn start_unpairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<UnpairingResult> {
        let device_id = params
            .get("device_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing 'device_id' in AiDot unpair request"))?;
        self.ensure_connected(state)?;
        lifecycle::unpair(state, device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_advertises_qr_pairing_and_roomless_buttons() {
        let capability = INTEGRATION.api_capabilities();
        assert_eq!(capability.hub_type, HUB_TYPE);
        assert_eq!(
            capability.device_onboarding_methods,
            vec![DEVICE_ONBOARDING_METHOD_AIDOT_BUTTON_QR]
        );
        assert!(capability.supports_unpairing);
        assert!(capability.supports_roomless_devices);
    }
}
