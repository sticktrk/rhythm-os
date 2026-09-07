//! Linux registration of the vendor-neutral local-BLE profile host.

use std::collections::BTreeSet;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{
    ExternalLightHubIntegration, HubCredentials, HubDeviceProfileCapability, HubEvent,
    HubIntegrationCapability, HubProvider, HubType,
};
use rhythm_os::pairing::{PairingRequestContext, PairingSession, UnpairingResult};
use rhythm_os::state::SharedState;

use crate::bluez_profile::BluezLocalBleTransport;
use crate::lifecycle;
use crate::profile::profiles;
use crate::{HUB_ADDRESS, HUB_TYPE};

pub static INTEGRATION: BluezLocalBleIntegration = BluezLocalBleIntegration;

pub struct BluezLocalBleIntegration;

struct BluezLocalBleProvider;
static PROVIDER: BluezLocalBleProvider = BluezLocalBleProvider;

impl BluezLocalBleProvider {
    fn configure_with_deadline(
        &self,
        address: &str,
        credentials_json: &str,
        state: &SharedState,
        deadline: Option<Instant>,
    ) -> Result<()> {
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
            |state| {
                let transport = Arc::new(BluezLocalBleTransport::new()?);
                match deadline {
                    Some(deadline) => lifecycle::connect_until(state, transport, deadline),
                    None => lifecycle::connect(state, transport),
                }
            },
        )
    }
}

impl HubProvider for BluezLocalBleProvider {
    fn hub_type(&self) -> HubType {
        HubType::new(HUB_TYPE)
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        self.configure_with_deadline(address, credentials_json, state, None)
    }
}

impl BluezLocalBleIntegration {
    fn ensure_connected(&self, state: &SharedState, deadline: Instant) -> Result<()> {
        ensure_deadline(deadline, "hub connection")?;
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
            PROVIDER.configure_with_deadline(
                HUB_ADDRESS,
                &serde_json::json!({ "adapter": "default" }).to_string(),
                state,
                Some(deadline),
            )?;
            return Ok(());
        }
        let receiver = self.connect_and_start_with_deadline(state.clone(), &key, Some(deadline))?;
        state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .pending_hub_event_rxs
            .push(receiver);
        Ok(())
    }

    fn connect_and_start_with_deadline(
        &self,
        state: SharedState,
        _key: &HubKey,
        deadline: Option<Instant>,
    ) -> Result<Receiver<HubEvent>> {
        let transport = Arc::new(BluezLocalBleTransport::new()?);
        let (hub, event_rx) = match deadline {
            Some(deadline) => lifecycle::connect_until(&state, transport, deadline)?,
            None => lifecycle::connect(&state, transport)?,
        };
        let key = hub.hub_key.clone();
        let store = hub
            .data::<Arc<lifecycle::LocalBleHubData>>()
            .map(|data| data.store.clone())
            .ok_or_else(|| anyhow::anyhow!("local Bluetooth hub data is unavailable"))?;
        let mut pending_hub = Some(hub);
        let published = store.with_reset_safe_operation(|| {
            let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            if state.hubs.contains_key(&key) {
                return Ok(false);
            }
            state.hubs.insert(
                key.clone(),
                pending_hub
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("local Bluetooth hub was already published"))?,
            );
            state.set_hub_connected(&key, false);
            Ok(true)
        })?;
        if !published {
            drop(pending_hub.take());
            drop(event_rx);
            let (_closed_tx, closed_rx) = std::sync::mpsc::channel();
            return Ok(closed_rx);
        }
        Ok(event_rx)
    }
}

fn ensure_deadline(deadline: Instant, stage: &str) -> Result<()> {
    if deadline <= Instant::now() {
        anyhow::bail!("local Bluetooth pairing deadline expired before {stage}");
    }
    Ok(())
}

impl ExternalLightHubIntegration for BluezLocalBleIntegration {
    fn hub_type(&self) -> &'static str {
        HUB_TYPE
    }

    fn provider(&self) -> &'static dyn HubProvider {
        &PROVIDER
    }

    fn api_capabilities(&self) -> HubIntegrationCapability {
        let device_onboarding_methods = profiles()
            .iter()
            .flat_map(|profile| profile.descriptor().onboarding_methods)
            .map(str::to_string)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        HubIntegrationCapability {
            hub_type: HUB_TYPE.to_string(),
            configurable: false,
            device_onboarding_methods,
            device_profiles: profiles()
                .iter()
                .map(|profile| {
                    let descriptor = profile.descriptor();
                    HubDeviceProfileCapability {
                        id: descriptor.id.to_string(),
                        compatible_profile_ids: descriptor
                            .compatible_profile_ids
                            .into_iter()
                            .map(str::to_string)
                            .collect(),
                        device_type: descriptor.device_type.to_string(),
                        display_name: descriptor.display_name.to_string(),
                        // The declarative profile host is structurally limited
                        // to event/input devices. Stateful output devices such
                        // as bulbs are rich drivers with real controllers.
                        input_only: true,
                        onboarding_methods: descriptor
                            .onboarding_methods
                            .into_iter()
                            .map(str::to_string)
                            .collect(),
                        nearby_service_uuids: Vec::new(),
                        cloud_broker: None,
                    }
                })
                .collect(),
            supports_unpairing: true,
            unpairable_device_types: profiles()
                .iter()
                .map(|profile| profile.descriptor().device_type.to_string())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            supports_roomless_devices: true,
            blocks_room_readiness: false,
        }
    }

    fn connect_and_start(&self, state: SharedState, _key: &HubKey) -> Result<Receiver<HubEvent>> {
        self.connect_and_start_with_deadline(state, _key, None)
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
        self.start_pairing_with_context(state, params, PairingRequestContext::accepted_now())
    }

    fn start_pairing_with_context(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
        context: PairingRequestContext,
    ) -> Result<PairingSession> {
        // Include handler validation, adapter admission, and the durable
        // idempotency write in the same absolute server budget. Starting a new
        // clock here could otherwise outlive the app's reconciliation window.
        let deadline = context.deadline_after(lifecycle::PAIRING_SERVER_SLA);
        ensure_deadline(deadline, "request admission")?;
        let profile_id = params
            .get("profile_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing local Bluetooth profile_id"))?;
        let setup = params
            .get("setup")
            .ok_or_else(|| anyhow::anyhow!("missing local Bluetooth setup fields"))?;
        let session_id = params
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing local Bluetooth pairing session_id"))?;
        let request_fingerprint = params
            .get("request_fingerprint")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!("missing local Bluetooth pairing request fingerprint")
            })?;
        rhythm_os::pairing::emit_pairing_progress(
            state,
            HUB_TYPE,
            Some(session_id),
            rhythm_os::pairing::PairingStatus::Searching,
            rhythm_os::pairing::PairingStage::HubConnecting,
            "Preparing the Bluetooth adapter",
            None,
            None,
        );
        self.ensure_connected(state, deadline)?;
        ensure_deadline(deadline, "association")?;
        lifecycle::pair(
            state,
            profile_id,
            setup,
            session_id,
            request_fingerprint,
            deadline,
        )
    }

    fn reconcile_pairing_results(&self, state: &SharedState) -> Result<()> {
        lifecycle::reconcile_pending_pairing_results(state)
    }

    fn start_unpairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<UnpairingResult> {
        let device_id = params
            .get("device_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing local Bluetooth device_id"))?;
        lifecycle::unpair(state, device_id)
    }
}

pub fn quiesce_for_factory_reset(state: &SharedState) -> Result<()> {
    lifecycle::quiesce_for_factory_reset(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::OREIN_OC02001_PROFILE_ID;

    #[test]
    fn pairing_budget_includes_handler_preamble_time() {
        let state = Arc::new(std::sync::Mutex::new(rhythm_os::state::AppState::default()));
        let accepted_at = Instant::now() - lifecycle::PAIRING_SERVER_SLA;

        let error = INTEGRATION
            .start_pairing_with_context(
                &state,
                &serde_json::json!({}),
                PairingRequestContext::new(accepted_at),
            )
            .unwrap_err();

        assert!(error.to_string().contains("request admission"));
    }

    #[test]
    fn capability_advertises_registered_profile_without_blocking_room_readiness() {
        let capability = INTEGRATION.api_capabilities();
        assert_eq!(capability.hub_type, HUB_TYPE);
        assert_eq!(
            capability.device_onboarding_methods,
            vec![rhythm_os::hub::DEVICE_ONBOARDING_METHOD_LOCAL_BLE_QR]
        );
        assert_eq!(capability.device_profiles.len(), 1);
        assert_eq!(capability.device_profiles[0].id, OREIN_OC02001_PROFILE_ID);
        assert!(capability.device_profiles[0]
            .compatible_profile_ids
            .is_empty());
        assert_eq!(capability.device_profiles[0].device_type, "button");
        assert!(capability.device_profiles[0].input_only);
        assert!(capability.supports_unpairing);
        assert!(capability.supports_roomless_devices);
        assert!(!capability.blocks_room_readiness);
    }
}
