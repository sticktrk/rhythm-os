//! Complete Hue lifecycle using reqwest transport (desktop/server targets).
//!
//! Provides everything a binary crate needs to run Hue as a plugin:
//! `connect_and_start`, `ensure_runtime`, `get_hub_provider`.
//! No platform-specific code needed in the consuming crate.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use log::warn;

use crate::hub_state::HueHubData;
use crate::registry::{HueDeviceRegistry, HueRegistrySnapshot};
use crate::reqwest_sse::start_reqwest_sse;
use crate::reqwest_transport::ReqwestHueTransport;
use crate::sse::HueSseConfig;
use crate::sse_liveness::HueSseLiveness;
use crate::transport::HueTransport;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{
    ActiveHub, ExternalLightHubIntegration, HubDeviceRoomAssignment,
    HubDeviceRoomAssignmentOutcome, HubEvent, HubIntegrationCapability, HubProvider, HubType,
    DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_SERIAL_SEARCH,
};
use rhythm_os::pairing::{
    PairedDeviceInfo, PairingSession, PairingStage, PairingStatus, UnpairingResult,
};
use rhythm_os::state::SharedState;

// ============================================================================
// Boot-time connection
// ============================================================================

/// Connect to the Hue bridge and store the hub in state.
///
/// Call this at startup when hub credentials are already configured.
/// Loads the registry snapshot from storage, connects SSE, and stores the hub
/// in state. Runtime creation is deferred to room sync.
///
/// Returns the event receiver for the main event loop.
pub fn connect_and_start(state: SharedState, key: &HubKey) -> Result<Receiver<HubEvent>> {
    let snapshot = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.storage
            .as_ref()
            .and_then(|st| st.load_hub_registry_for(key).ok().flatten())
            .and_then(|v| serde_json::from_value::<HueRegistrySnapshot>(v).ok())
    };

    let (hub, event_rx) = connect_hue_sse(&state, key, snapshot)?;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = hub.hub_key.clone();
        s.hubs.insert(key.clone(), hub);
        s.set_hub_connected(&key, false);
    }

    // Runtime creation is deferred to room sync (do_room_set → commands::ensure_runtime),
    // which uses ensure_composite_runtime on desktop. This ensures the CompositeController
    // is created with controllers for ALL connected hubs, enabling proper topology ID
    // → hub-native ID translation.

    Ok(event_rx)
}

// ============================================================================
// Runtime creation
// ============================================================================

/// Create the RhythmRuntime for the Hue hub using reqwest transport.
pub fn ensure_runtime(state: &SharedState) -> Result<()> {
    // Check early: skip if any runtime already exists (avoids creating a
    // ReqwestHueTransport whose reqwest::blocking::Client would panic on
    // drop if we're on a tokio worker thread and the runtime was a no-op).
    {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if s.hubs.values().any(|h| h.runtime.is_some()) {
            return Ok(());
        }
    }

    let bridge_ip = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_credentials
            .values()
            .find(|c| c.hub_type.as_ref().is_some_and(|t| t.as_str() == "hue"))
            .map(|c| c.address.clone())
            .unwrap_or_default()
    };

    let transport = ReqwestHueTransport::new(&bridge_ip)?;
    crate::hue_lifecycle::ensure_hue_runtime(state, transport)
}

// ============================================================================
// Controller creation (for CompositeController)
// ============================================================================

/// Create a type-erased Hue light controller for a specific hub key.
///
/// Extracts credentials and registry from state, builds a reqwest transport,
/// and returns the controller as `Arc<dyn HubLightController>`.
pub fn create_hue_controller(
    state: &SharedState,
    key: &HubKey,
) -> Result<std::sync::Arc<dyn rhythm_core::HubLightController>> {
    use crate::controller::HueLightController;
    use crate::hub_state::HueHubData;

    let (bridge_ip, username, registry, sse_liveness) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        let creds = s
            .hub_credentials
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("No Hue credentials for {}", key))?;
        let username = crate::provider::hue_username(creds)
            .ok_or_else(|| anyhow::anyhow!("Hue credentials missing username"))?
            .to_string();
        let bridge_ip = creds.address.clone();

        let hue_data = s.hubs.get(key).and_then(|h| h.data::<HueHubData>());
        let (reg, sse_liveness) = hue_data
            .map(|hue| (hue.registry.clone(), hue.sse_liveness.clone()))
            .ok_or_else(|| anyhow::anyhow!("Hue hub not active for {}", key))?;

        (bridge_ip, username, reg, sse_liveness)
    };

    let transport = ReqwestHueTransport::new(&bridge_ip)?;
    let controller = HueLightController::new(transport, username, registry)
        .with_capability_source(state.clone(), key.clone())
        .with_sse_liveness(sse_liveness);
    Ok(std::sync::Arc::new(controller))
}

// ============================================================================
// Hub provider
// ============================================================================

/// Get the static Hue hub provider (reqwest transport).
pub fn get_hub_provider() -> &'static dyn HubProvider {
    static HUE: ReqwestHueHubProvider = ReqwestHueHubProvider;
    &HUE
}

/// Hub provider for Hue bridges using reqwest transport.
pub struct ReqwestHueHubProvider;

impl HubProvider for ReqwestHueHubProvider {
    fn hub_type(&self) -> HubType {
        HubType::new(HubType::HUE)
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        let configure_key = HubKey::new(HubType::new(HubType::HUE), address);
        crate::provider::configure_hue_hub(address, credentials_json, state, |state| {
            let snapshot = {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                s.storage
                    .as_ref()
                    .and_then(|st| st.load_hub_registry_for(&configure_key).ok().flatten())
                    .and_then(|v| serde_json::from_value::<HueRegistrySnapshot>(v).ok())
            };
            connect_hue_sse(state, &configure_key, snapshot)
        })?;

        // Runtime creation is deferred to room sync (do_room_set → commands::ensure_runtime).
        // do_configure_hub triggers auto-sync after this returns.

        Ok(())
    }
}

// ============================================================================
// ExternalLightHubIntegration — static integration for platform crate registries
// ============================================================================

/// Static integration instance for platform crate registries.
pub static INTEGRATION: HueIntegration = HueIntegration;

/// Hue integration using reqwest transport (desktop/server targets).
pub struct HueIntegration;

impl ExternalLightHubIntegration for HueIntegration {
    fn hub_type(&self) -> &'static str {
        HubType::HUE
    }
    fn provider(&self) -> &'static dyn HubProvider {
        get_hub_provider()
    }
    fn connect_and_start(&self, state: SharedState, key: &HubKey) -> Result<Receiver<HubEvent>> {
        connect_and_start(state, key)
    }
    fn ensure_runtime(&self, state: &SharedState) -> Result<()> {
        ensure_runtime(state)
    }
    fn create_controller(
        &self,
        state: &SharedState,
        key: &HubKey,
    ) -> Result<std::sync::Arc<dyn rhythm_core::HubLightController>> {
        create_hue_controller(state, key)
    }

    fn api_capabilities(&self) -> HubIntegrationCapability {
        HubIntegrationCapability {
            hub_type: HubType::HUE.to_string(),
            configurable: true,
            device_onboarding_methods: vec![
                DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_SERIAL_SEARCH.to_string()
            ],
            device_profiles: Vec::new(),
            supports_unpairing: true,
            supports_roomless_devices: true,
            blocks_room_readiness: true,
        }
    }

    fn start_pairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<PairingSession> {
        let raw_serial = params
            .get("serial")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing Hue bulb serial"))?;
        let serial = crate::transport::normalize_hue_bridge_serial(raw_serial)?;
        let session_id = params
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|session_id| !session_id.is_empty());
        let requested_address = params
            .get("hub_address")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|address| !address.is_empty());
        let (key, bridge_ip, username) = bridge_pairing_target(state, requested_address)?;
        rhythm_os::pairing::emit_pairing_progress(
            state,
            HubType::HUE,
            session_id,
            PairingStatus::Searching,
            PairingStage::Searching,
            "Asking the Hue Bridge to search for the bulb",
            None,
            None,
        );
        let transport = ReqwestHueTransport::new(&bridge_ip)?;
        let found = transport.search_new_lights(&username, &serial)?;
        if found.is_empty() {
            anyhow::bail!(
                "The Hue Bridge did not find a new bulb with serial {serial}. Keep the bulb powered on nearby and try again"
            );
        }

        rhythm_os::pairing::emit_pairing_progress(
            state,
            HubType::HUE,
            session_id,
            PairingStatus::Commissioning,
            PairingStage::Finalizing,
            format!(
                "Hue Bridge found {} {}; refreshing rooms and lights",
                found.len(),
                if found.len() == 1 { "bulb" } else { "bulbs" }
            ),
            None,
            None,
        );
        let found_ids = found
            .iter()
            .map(|light| light.legacy_id.clone())
            .collect::<BTreeSet<_>>();
        let mut projected_devices = None;
        let mut projection_error = format!(
            "Hue V2 has not projected V1 lights {}",
            found_ids.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        const PROJECTION_ATTEMPTS: usize = 15;
        for attempt in 0..PROJECTION_ATTEMPTS {
            match transport
                .get_resources(&username, "light")
                .and_then(|resources| v2_device_ids_for_legacy_lights(&resources, &found_ids))
            {
                Ok(mapped) => {
                    let missing = found_ids
                        .iter()
                        .filter(|legacy_id| !mapped.contains_key(*legacy_id))
                        .cloned()
                        .collect::<Vec<_>>();
                    if missing.is_empty() {
                        let expected_device_ids = mapped.into_values().collect::<BTreeSet<_>>();
                        match rhythm_os::room_sync::sync_from_hub_for_key_wait(
                            state,
                            &key,
                            true,
                            Duration::from_secs(30),
                        ) {
                            Ok(_) => match exact_bridge_light_projection(
                                bridge_light_devices(state, &key),
                                &expected_device_ids,
                            ) {
                                Ok(devices) => {
                                    projected_devices = Some(devices);
                                    break;
                                }
                                Err(error) => projection_error = error.to_string(),
                            },
                            Err(error) => {
                                projection_error = format!("Hue Bridge refresh failed: {error:#}");
                            }
                        }
                    } else {
                        projection_error = format!(
                            "Hue V2 has not projected V1 light IDs {}",
                            missing.join(", ")
                        );
                    }
                }
                Err(error) => {
                    projection_error = format!("Hue V2 light projection lookup failed: {error:#}");
                }
            }
            if attempt + 1 < PROJECTION_ATTEMPTS {
                // V1 can report search completion before the exact new light
                // and its owner device are visible through V2.
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let mut devices = projected_devices.ok_or_else(|| {
            anyhow::anyhow!(
                "Hue Bridge found the bulb, but Rhythm could not confirm its exact V2 device projection after refresh: {projection_error}"
            )
        })?;
        devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        Ok(PairingSession {
            hub_type: HubType::HUE.to_string(),
            status: PairingStatus::Complete,
            device: devices.first().cloned(),
            devices,
            error: None,
            failure_stage: None,
            warnings: Vec::new(),
            details: None,
        })
    }

    fn start_unpairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<UnpairingResult> {
        start_bridge_unpairing_with(
            state,
            params,
            |target, force| {
                let transport = ReqwestHueTransport::new(&target.bridge_ip)?;
                let exists = transport.device_exists(&target.username, &target.native_id)?;
                if force {
                    if exists {
                        anyhow::bail!(
                            "Hue Bridge still owns device {}; force cannot forget a live Bridge device because it would reappear on the next sync",
                            target.native_id
                        );
                    }
                    return Ok(());
                }
                if exists {
                    transport.remove_light_device(&target.username, &target.native_id)?;
                }
                if transport.device_exists(&target.username, &target.native_id)? {
                    anyhow::bail!(
                        "Hue Bridge still reports device {} after removal",
                        target.native_id
                    );
                }
                Ok(())
            },
            |state, key| {
                rhythm_os::room_sync::sync_from_hub_for_key_wait(
                    state,
                    key,
                    true,
                    Duration::from_secs(30),
                )
                .map(|_| ())
            },
        )
    }

    fn prepare_device_room_assignment(
        &self,
        state: &SharedState,
        assignment: &HubDeviceRoomAssignment,
    ) -> Result<HubDeviceRoomAssignmentOutcome> {
        if assignment.device_type != rhythm_core::runtime::hub_registry::DeviceType::Light {
            return Ok(HubDeviceRoomAssignmentOutcome::Unchanged);
        }

        let target_hub_room_id = target_hub_room_id_for_assignment(assignment)?;

        let (bridge_ip, username) = {
            let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let hue = state
                .hubs
                .get(&assignment.hub_key)
                .and_then(|hub| hub.data::<HueHubData>())
                .ok_or_else(|| anyhow::anyhow!("Hue hub is not active: {}", assignment.hub_key))?;
            (hue.bridge_ip.clone(), hue.username.clone())
        };
        let transport = ReqwestHueTransport::new(&bridge_ip)?;
        let rollback = crate::room_membership::reassign_device_room(
            &transport,
            &username,
            &assignment.native_device_id,
            target_hub_room_id,
        )?;
        Ok(HubDeviceRoomAssignmentOutcome::Reassigned {
            target_hub_room_id: target_hub_room_id.map(str::to_string),
            rollback: Box::new(move || rollback.rollback(&transport, &username)),
        })
    }
}

fn target_hub_room_id_for_assignment(assignment: &HubDeviceRoomAssignment) -> Result<Option<&str>> {
    Ok(match assignment.target_rhythm_room_id.as_deref() {
        Some(target_room_id) => match assignment.target_hub_room_ids.as_slice() {
            [target_hub_room_id] => Some(target_hub_room_id.as_str()),
            [] => None,
            _ => {
                return Err(anyhow::anyhow!(
                    "Cannot move Hue light to Rhythm room '{}': it maps to multiple rooms on this Hue bridge",
                    target_room_id
                ));
            }
        },
        None => None,
    })
}

fn bridge_pairing_target(
    state: &SharedState,
    requested_address: Option<&str>,
) -> Result<(HubKey, String, String)> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let mut candidates = state
        .hubs
        .iter()
        .filter(|(key, _)| key.hub_type.as_str() == HubType::HUE)
        .filter(|(key, _)| {
            requested_address.is_none_or(|address| key.address.eq_ignore_ascii_case(address))
        })
        .filter_map(|(key, hub)| {
            hub.data::<HueHubData>()
                .map(|data| (key.clone(), data.bridge_ip.clone(), data.username.clone()))
        })
        .collect::<Vec<_>>();
    match candidates.len() {
        0 if requested_address.is_some() => anyhow::bail!(
            "The requested Hue Bridge is not connected: {}",
            requested_address.unwrap_or_default()
        ),
        0 => anyhow::bail!("Connect a Hue Bridge before adding a bulb by serial"),
        1 => Ok(candidates.pop().expect("one candidate")),
        _ => anyhow::bail!(
            "Multiple Hue Bridges are connected; provide hub_address for the bridge that should add the bulb"
        ),
    }
}

fn legacy_id_from_v2_id_v1(id_v1: &str) -> Option<&str> {
    let id = id_v1.strip_prefix("/lights/")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

fn v2_device_ids_for_legacy_lights(
    value: &serde_json::Value,
    requested_legacy_ids: &BTreeSet<String>,
) -> Result<BTreeMap<String, String>> {
    let errors = value
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("GET light returned an invalid Hue V2 response"))?;
    if !errors.is_empty() {
        anyhow::bail!(
            "GET light returned Hue errors: {}",
            serde_json::Value::Array(errors.clone())
        );
    }
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("GET light returned a Hue V2 response without data"))?;

    let mut mapped = BTreeMap::new();
    for light in data {
        let Some(legacy_id) = light
            .get("id_v1")
            .and_then(serde_json::Value::as_str)
            .and_then(legacy_id_from_v2_id_v1)
        else {
            continue;
        };
        if !requested_legacy_ids.contains(legacy_id) {
            continue;
        }
        if light
            .pointer("/owner/rtype")
            .and_then(serde_json::Value::as_str)
            != Some("device")
        {
            continue;
        }
        let Some(device_id) = light
            .pointer("/owner/rid")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if let Some(previous) = mapped.insert(legacy_id.to_string(), device_id.to_string()) {
            if previous != device_id {
                anyhow::bail!(
                    "Hue V1 light {legacy_id} maps to multiple Hue V2 devices ({previous}, {device_id})"
                );
            }
        }
    }
    Ok(mapped)
}

fn exact_bridge_light_projection(
    devices: Vec<PairedDeviceInfo>,
    expected_device_ids: &BTreeSet<String>,
) -> Result<Vec<PairedDeviceInfo>> {
    if expected_device_ids.is_empty() {
        anyhow::bail!("Hue Bridge pairing produced no exact V2 device IDs");
    }
    let projected = devices
        .into_iter()
        .filter(|device| expected_device_ids.contains(&device.device_id))
        .collect::<Vec<_>>();
    let projected_ids = projected
        .iter()
        .map(|device| device.device_id.clone())
        .collect::<BTreeSet<_>>();
    let missing = expected_device_ids
        .difference(&projected_ids)
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        anyhow::bail!(
            "Rhythm sync did not project exact Hue V2 device IDs {}",
            missing.join(", ")
        );
    }
    Ok(projected)
}

#[derive(Clone, Debug)]
struct BridgeUnpairTarget {
    hub_key: HubKey,
    bridge_ip: String,
    username: String,
    native_id: String,
}

fn bridge_unpair_target(
    state: &SharedState,
    requested_id: &str,
    requested_address: Option<&str>,
) -> Result<BridgeUnpairTarget> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let canonical = state
        .canonical_registry
        .get(requested_id)
        .filter(|device| !device.is_removed());
    let mut endpoints = if let Some(device) = canonical {
        device
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.hub_key.hub_type.as_str() == HubType::HUE)
            .filter(|endpoint| {
                requested_address
                    .is_none_or(|address| endpoint.hub_key.address.eq_ignore_ascii_case(address))
            })
            .map(|endpoint| (endpoint.hub_key.clone(), endpoint.native_id.clone()))
            .collect::<Vec<_>>()
    } else {
        state
            .canonical_registry
            .devices()
            .flat_map(|device| device.endpoints.iter())
            .filter(|endpoint| endpoint.hub_key.hub_type.as_str() == HubType::HUE)
            .filter(|endpoint| endpoint.native_id == requested_id)
            .filter(|endpoint| {
                requested_address
                    .is_none_or(|address| endpoint.hub_key.address.eq_ignore_ascii_case(address))
            })
            .map(|endpoint| (endpoint.hub_key.clone(), endpoint.native_id.clone()))
            .collect::<Vec<_>>()
    };
    endpoints.sort_by(|left, right| {
        left.0
            .to_string()
            .cmp(&right.0.to_string())
            .then_with(|| left.1.cmp(&right.1))
    });
    endpoints.dedup();

    if canonical.is_some() && endpoints.is_empty() {
        anyhow::bail!(
            "Canonical device {requested_id} has no Hue Bridge endpoint{}",
            requested_address
                .map(|address| format!(" on {address}"))
                .unwrap_or_default()
        );
    }

    if endpoints.is_empty() {
        endpoints = state
            .hubs
            .iter()
            .filter(|(key, hub)| {
                key.hub_type.as_str() == HubType::HUE
                    && hub.data::<HueHubData>().is_some()
                    && requested_address
                        .is_none_or(|address| key.address.eq_ignore_ascii_case(address))
            })
            .map(|(key, _)| (key.clone(), requested_id.to_string()))
            .collect();
    }

    let (hub_key, native_id) = match endpoints.as_slice() {
        [] if requested_address.is_some() => anyhow::bail!(
            "The requested Hue Bridge is not connected: {}",
            requested_address.unwrap_or_default()
        ),
        [] => anyhow::bail!("Connect a Hue Bridge before removing a Bridge light"),
        [only] => only.clone(),
        _ => anyhow::bail!(
            "The Hue device is reachable through multiple Bridges; provide hub_address for the exact endpoint"
        ),
    };
    let hue = state
        .hubs
        .get(&hub_key)
        .and_then(|hub| hub.data::<HueHubData>())
        .ok_or_else(|| anyhow::anyhow!("Hue Bridge is not connected: {}", hub_key.address))?;
    Ok(BridgeUnpairTarget {
        hub_key,
        bridge_ip: hue.bridge_ip.clone(),
        username: hue.username.clone(),
        native_id,
    })
}

fn start_bridge_unpairing_with<Remote, Sync>(
    state: &SharedState,
    params: &serde_json::Value,
    remote_unpair: Remote,
    sync_hub: Sync,
) -> Result<UnpairingResult>
where
    Remote: FnOnce(&BridgeUnpairTarget, bool) -> Result<()>,
    Sync: FnOnce(&SharedState, &HubKey) -> Result<()>,
{
    let requested_id = params
        .get("device_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Missing Hue device_id"))?;
    let requested_address = params
        .get("hub_address")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty());
    let force = params
        .get("force")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let target = bridge_unpair_target(state, requested_id, requested_address)?;
    let failed = |error: anyhow::Error| UnpairingResult {
        hub_type: HubType::HUE.to_string(),
        hub_address: Some(target.hub_key.address.clone()),
        status: PairingStatus::Failed,
        device_id: Some(target.native_id.clone()),
        error: Some(format!("{error:#}")),
        completion_scope: None,
        warning: None,
    };

    if let Err(error) = remote_unpair(&target, force) {
        return Ok(failed(error));
    }
    if let Err(error) = sync_hub(state, &target.hub_key) {
        return Ok(failed(error.context(format!(
            "Hue Bridge device was removed, but syncing {} failed",
            target.hub_key.address
        ))));
    }
    Ok(UnpairingResult {
        hub_type: HubType::HUE.to_string(),
        hub_address: Some(target.hub_key.address),
        status: PairingStatus::Complete,
        device_id: Some(target.native_id),
        error: None,
        completion_scope: None,
        warning: None,
    })
}

fn bridge_light_devices(state: &SharedState, key: &HubKey) -> Vec<PairedDeviceInfo> {
    let Ok(state) = state.lock() else {
        return Vec::new();
    };
    state
        .canonical_registry
        .devices()
        .filter(|device| {
            !device.is_removed()
                && device.device_type == rhythm_core::runtime::hub_registry::DeviceType::Light
        })
        .filter_map(|device| {
            let endpoint = device
                .active_endpoints()
                .find(|endpoint| &endpoint.hub_key == key)?;
            Some(PairedDeviceInfo {
                device_id: endpoint.native_id.clone(),
                name: device.name.clone(),
                device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                manufacturer: device.manufacturer.clone(),
                model: device.model.clone(),
            })
        })
        .collect()
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Connect Hue SSE with reqwest transport, including discovery.
fn connect_hue_sse(
    state: &SharedState,
    key: &HubKey,
    snapshot: Option<HueRegistrySnapshot>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let (mut hub, event_rx) = crate::hue_lifecycle::connect_hue_sse(
        state,
        key.clone(),
        snapshot,
        |config, registry, shutdown, sse_liveness| {
            start_event_stream(
                config.bridge_ip,
                config.username,
                registry,
                shutdown,
                sse_liveness,
            )
        },
    )?;

    // Attach Hue discovery to the hub
    if let Some(hue_data) = hub.data::<HueHubData>() {
        let bridge_ip = hue_data.bridge_ip.clone();
        let username = hue_data.username.clone();
        match ReqwestHueTransport::new(&bridge_ip) {
            Ok(transport) => {
                let discovery = crate::discovery::HueDiscovery::new(Arc::new(transport), username);
                hub.discovery = Some(Arc::new(discovery));
            }
            Err(e) => {
                warn!(target: "sys", "Failed to create discovery transport: {}", e);
            }
        }
    }

    Ok((hub, event_rx))
}

/// Start the SSE event stream using reqwest + a shared translator thread.
fn start_event_stream(
    bridge_ip: String,
    username: String,
    registry: Arc<Mutex<HueDeviceRegistry>>,
    shutdown: Arc<AtomicBool>,
    sse_liveness: Arc<HueSseLiveness>,
) -> Receiver<HubEvent> {
    let sse_config = HueSseConfig {
        bridge_ip,
        username,
    };

    let sse_rx = start_reqwest_sse(sse_config, shutdown.clone(), sse_liveness);

    crate::events::start_event_translator(sse_rx, registry, shutdown, None, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    use rhythm_core::{
        InputEvent, LightProfileConfig, LightingCommand, ModeConfig, RestoredRoomState,
        RoomSnapshot, RuntimeHandle, SolarTime,
    };
    use rhythm_os::hub::HubCredentials;
    use rhythm_os::state::AppState;

    use crate::provider::hue_credentials;

    struct NoopRuntime;

    impl RuntimeHandle for NoopRuntime {
        fn handle_event(&self, _: &InputEvent) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn sync_rooms(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_solar(&self, _: SolarTime) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_light_profile_config(&self, _: LightProfileConfig) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_mode_configs(&self, _: Vec<ModeConfig>) -> anyhow::Result<()> {
            Ok(())
        }

        fn periodic_tick_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }

        fn engine_room_snapshot(&self, _: &str) -> Option<RoomSnapshot> {
            None
        }

        fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
            Vec::new()
        }

        fn restore_room_state(&self, _: &str, _: RestoredRoomState) {}

        fn add_room(&self, _: &str, _: &str) {}

        fn remove_room(&self, _: &str) {}

        fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }

        fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn apply_room_command(&self, _: &str, _: LightingCommand) -> anyhow::Result<()> {
            Ok(())
        }

        fn lights_off_room(&self, _: &str, _: Option<u32>) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_power_save(&self, _: bool) -> Vec<String> {
            Vec::new()
        }

        fn is_power_save(&self) -> bool {
            false
        }

        fn set_room_brightness(&self, _: &str, _: u8) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_room_time_offset(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }

        fn idle_brightness(&self) -> u8 {
            1
        }

        fn soft_off_tick_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn any_lights_on(&self, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn current_hour(&self) -> f32 {
            12.0
        }

        fn set_light_profile(&self, _: &str) -> bool {
            true
        }

        fn active_light_profile_id(&self) -> String {
            rhythm_core::RHYTHM_PROFILE_ID.to_string()
        }

        fn available_light_profiles(&self) -> Vec<(String, String)> {
            vec![(
                rhythm_core::RHYTHM_PROFILE_ID.to_string(),
                "Rhythm".to_string(),
            )]
        }
    }

    fn shared_state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    fn hue_key(address: &str) -> HubKey {
        HubKey::new(HubType::new(HubType::HUE), address)
    }

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    fn active_hue_hub(key: HubKey) -> ActiveHub {
        let registry = Arc::new(Mutex::new(HueDeviceRegistry::with_options(false)));
        let bridge_ip = key.address.clone();
        ActiveHub {
            hub_type: HubType::new(HubType::HUE),
            hub_key: key,
            runtime: None,
            hub_data: Box::new(HueHubData {
                bridge_ip,
                username: "user-123".to_string(),
                registry,
                sse_liveness: Arc::new(HueSseLiveness::default()),
            }),
            registry: None,
            discovery: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn provider_and_integration_report_hue_metadata() {
        let provider = get_hub_provider();
        assert_eq!(provider.hub_type().as_str(), HubType::HUE);
        assert_eq!(INTEGRATION.hub_type(), HubType::HUE);
        assert_eq!(INTEGRATION.provider().hub_type().as_str(), HubType::HUE);
        let caps = INTEGRATION.api_capabilities();
        assert_eq!(caps.hub_type, HubType::HUE);
        assert!(caps.configurable);
        assert_eq!(
            caps.device_onboarding_methods,
            vec![DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_SERIAL_SEARCH]
        );
        assert!(caps.supports_unpairing);
        assert!(caps.supports_roomless_devices);
    }

    #[test]
    fn bridge_pairing_maps_only_exact_v1_results_to_v2_owner_devices() {
        let requested = BTreeSet::from(["12".to_string(), "14".to_string(), "99".to_string()]);
        let mapped = v2_device_ids_for_legacy_lights(
            &serde_json::json!({
                "errors": [],
                "data": [
                    {
                        "id": "light-a",
                        "id_v1": "/lights/12",
                        "owner": {"rtype": "device", "rid": "device-a"}
                    },
                    {
                        "id": "light-unrelated",
                        "id_v1": "/lights/13",
                        "owner": {"rtype": "device", "rid": "device-unrelated"}
                    },
                    {
                        "id": "light-a-2",
                        "id_v1": "/lights/14",
                        "owner": {"rtype": "device", "rid": "device-a"}
                    }
                ]
            }),
            &requested,
        )
        .unwrap();
        assert_eq!(
            mapped,
            BTreeMap::from([
                ("12".to_string(), "device-a".to_string()),
                ("14".to_string(), "device-a".to_string())
            ])
        );
        assert!(!mapped.contains_key("13"));
        assert!(!mapped.contains_key("99"));
    }

    #[test]
    fn bridge_pairing_requires_nonempty_exact_canonical_projection() {
        let expected = BTreeSet::from(["device-a".to_string()]);
        let unrelated = PairedDeviceInfo {
            device_id: "device-unrelated".to_string(),
            name: "Other lamp".to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            manufacturer: None,
            model: None,
        };
        assert!(exact_bridge_light_projection(vec![unrelated], &expected).is_err());
        assert!(exact_bridge_light_projection(Vec::new(), &BTreeSet::new()).is_err());

        let exact = PairedDeviceInfo {
            device_id: "device-a".to_string(),
            name: "New lamp".to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            manufacturer: Some("Signify".to_string()),
            model: Some("LCA009".to_string()),
        };
        let projected = exact_bridge_light_projection(vec![exact.clone()], &expected).unwrap();
        assert_eq!(projected, vec![exact]);
    }

    #[test]
    fn bridge_serial_search_requires_an_explicit_target_when_multiple_are_connected() {
        let state = shared_state();
        let first = hue_key("192.0.2.10");
        let second = hue_key("192.0.2.11");
        {
            let mut state = state.lock().unwrap();
            state
                .hubs
                .insert(first.clone(), active_hue_hub(first.clone()));
            state
                .hubs
                .insert(second.clone(), active_hue_hub(second.clone()));
        }

        let error = bridge_pairing_target(&state, None).unwrap_err();
        assert!(error.to_string().contains("Multiple Hue Bridges"));

        let (selected, address, username) =
            bridge_pairing_target(&state, Some("192.0.2.11")).unwrap();
        assert_eq!(selected, second);
        assert_eq!(address, "192.0.2.11");
        assert_eq!(username, "user-123");
    }

    fn add_hue_light_endpoint(state: &SharedState, key: &HubKey, native_id: &str) -> String {
        use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
        use rhythm_os::canonical::registry::ResolveResult;

        let mut state = state.lock().unwrap();
        let identity = DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: None,
            room_name: None,
            name: "Test Hue lamp".to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            hardware_ids: vec![HardwareId::serial(&format!("serial-{native_id}"))],
            manufacturer: Some("Signify".to_string()),
            model: Some("LCA009".to_string()),
        };
        match state.canonical_registry.resolve(&identity, key, 1) {
            ResolveResult::AlreadyKnown { canonical_id }
            | ResolveResult::ReApproved { canonical_id }
            | ResolveResult::Created { canonical_id } => canonical_id,
            ResolveResult::Queued { .. } => panic!("unexpected triage"),
        }
    }

    #[test]
    fn bridge_unpair_resolves_canonical_id_and_returns_exact_hub_address() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        state
            .lock()
            .unwrap()
            .hubs
            .insert(key.clone(), active_hue_hub(key.clone()));
        let canonical_id = add_hue_light_endpoint(&state, &key, "device-a");

        let result = start_bridge_unpairing_with(
            &state,
            &serde_json::json!({
                "device_id": canonical_id,
                "hub_address": "192.0.2.10"
            }),
            |target, force| {
                assert!(!force);
                assert_eq!(target.hub_key, key);
                assert_eq!(target.native_id, "device-a");
                assert_eq!(target.bridge_ip, "192.0.2.10");
                assert_eq!(target.username, "user-123");
                Ok(())
            },
            |_, sync_key| {
                assert_eq!(sync_key, &key);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(result.hub_address.as_deref(), Some("192.0.2.10"));
        assert_eq!(result.device_id.as_deref(), Some("device-a"));
    }

    #[test]
    fn bridge_unpair_requires_hub_address_for_multiple_bridge_endpoints() {
        let state = shared_state();
        let first = hue_key("192.0.2.10");
        let second = hue_key("192.0.2.11");
        {
            let mut state = state.lock().unwrap();
            state
                .hubs
                .insert(first.clone(), active_hue_hub(first.clone()));
            state
                .hubs
                .insert(second.clone(), active_hue_hub(second.clone()));
        }
        let canonical_id = add_hue_light_endpoint(&state, &first, "device-a");
        state
            .lock()
            .unwrap()
            .canonical_registry
            .get_mut(&canonical_id)
            .unwrap()
            .upsert_endpoint(second.clone(), "device-a-via-second".to_string(), 2, None);

        let ambiguous = bridge_unpair_target(&state, &canonical_id, None).unwrap_err();
        assert!(ambiguous.to_string().contains("multiple Bridges"));

        let target = bridge_unpair_target(&state, &canonical_id, Some("192.0.2.11")).unwrap();
        assert_eq!(target.hub_key, second);
        assert_eq!(target.native_id, "device-a-via-second");
    }

    #[test]
    fn bridge_unpair_returns_failed_for_upstream_error_and_live_force_target() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        state
            .lock()
            .unwrap()
            .hubs
            .insert(key.clone(), active_hue_hub(key.clone()));
        add_hue_light_endpoint(&state, &key, "device-a");

        let failure = start_bridge_unpairing_with(
            &state,
            &serde_json::json!({"device_id": "device-a"}),
            |_, force| {
                assert!(!force);
                anyhow::bail!("bridge rejected delete")
            },
            |_, _| panic!("failed upstream removal must not sync"),
        )
        .unwrap();
        assert_eq!(failure.status, PairingStatus::Failed);
        assert!(failure
            .error
            .as_deref()
            .is_some_and(|error| error.contains("bridge rejected delete")));

        let force_failure = start_bridge_unpairing_with(
            &state,
            &serde_json::json!({"device_id": "device-a", "force": true}),
            |_, force| {
                assert!(force);
                anyhow::bail!("Hue Bridge still owns device device-a")
            },
            |_, _| panic!("live force target must not sync"),
        )
        .unwrap();
        assert_eq!(force_failure.status, PairingStatus::Failed);
        assert!(force_failure
            .error
            .as_deref()
            .is_some_and(|error| error.contains("still owns")));

        let force_absent = start_bridge_unpairing_with(
            &state,
            &serde_json::json!({"device_id": "device-a", "force": true}),
            |_, force| {
                assert!(force);
                Ok(())
            },
            |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(force_absent.status, PairingStatus::Complete);
    }

    #[test]
    fn hue_light_move_accepts_no_native_target_or_exactly_one() {
        let assignment = |target_hub_room_ids: Vec<String>| HubDeviceRoomAssignment {
            hub_key: hue_key("192.0.2.10"),
            native_device_id: "light-device".to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            target_rhythm_room_id: Some("office".to_string()),
            target_hub_room_ids,
        };

        assert_eq!(
            target_hub_room_id_for_assignment(&assignment(Vec::new())).unwrap(),
            None
        );
        assert_eq!(
            target_hub_room_id_for_assignment(&assignment(vec!["hue-office".to_string()])).unwrap(),
            Some("hue-office")
        );

        let ambiguous = string_error(target_hub_room_id_for_assignment(&assignment(vec![
            "hue-office-a".to_string(),
            "hue-office-b".to_string(),
        ])));
        assert!(ambiguous.contains("maps to multiple rooms"));
    }

    #[test]
    fn connect_and_start_and_private_connect_report_missing_credentials() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");

        assert!(string_error(connect_and_start(state.clone(), &key))
            .contains("No hub credentials configured"));
        assert!(string_error(connect_hue_sse(&state, &key, None))
            .contains("No hub credentials configured"));
    }

    #[test]
    fn ensure_runtime_returns_when_runtime_already_exists() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        state
            .lock()
            .unwrap()
            .hubs
            .insert(key, active_hue_hub(hue_key("192.0.2.10")));

        state
            .lock()
            .unwrap()
            .hubs
            .values_mut()
            .next()
            .unwrap()
            .runtime = Some(Arc::new(NoopRuntime));

        ensure_runtime(&state).unwrap();
    }

    #[test]
    fn create_hue_controller_validates_credentials_and_active_hub_before_transport_use() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");

        assert_eq!(
            string_error(create_hue_controller(&state, &key)),
            format!("No Hue credentials for {}", key)
        );

        state.lock().unwrap().hub_credentials.insert(
            key.clone(),
            HubCredentials::new(HubType::HUE, "192.0.2.10", serde_json::json!({})),
        );
        assert_eq!(
            string_error(create_hue_controller(&state, &key)),
            "Hue credentials missing username"
        );

        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), hue_credentials("192.0.2.10", "user-123"));
        assert_eq!(
            string_error(create_hue_controller(&state, &key)),
            format!("Hue hub not active for {}", key)
        );
    }

    #[test]
    fn create_hue_controller_builds_controller_for_active_hub() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        {
            let mut guard = state.lock().unwrap();
            guard
                .hub_credentials
                .insert(key.clone(), hue_credentials("192.0.2.10", "user-123"));
            guard.hubs.insert(key.clone(), active_hue_hub(key.clone()));
        }

        let controller = create_hue_controller(&state, &key).unwrap();

        assert_eq!(Arc::strong_count(&controller), 1);
    }

    #[test]
    fn reqwest_provider_rejects_invalid_credentials_without_connecting() {
        let state = shared_state();
        let provider = ReqwestHueHubProvider;

        let error = provider
            .configure("192.0.2.10", "{}", &state)
            .unwrap_err()
            .to_string();

        assert!(error.contains("Invalid Hue credentials"));
        assert!(state.lock().unwrap().hubs.is_empty());
    }
}
