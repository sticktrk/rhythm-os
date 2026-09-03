//! Desktop Matter lifecycle using the local CHIP sidecar transport.

use std::collections::HashSet;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use anyhow::Result;
use log::{info, warn};
use rhythm_core::controller::HubLightController;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{HubEvent, HubProvider, HubType};
use rhythm_os::pairing::PairingSession;
use rhythm_os::state::SharedState;
use rhythm_os::topology::HubRoomBinding;

use crate::chip_transport::ChipTransport;
use crate::controller::{parse_group_control_id, MatterLightController};
use crate::groups::{MatterGroupController, MatterTopologyGroup};
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

fn remove_persisted_device_cache_entry(state: &SharedState, node_id: u64) -> Result<bool> {
    let data_path = matter_data_path(state)?;
    crate::device_store::remove_commissioned_node(data_path, node_id)
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

fn topology_group_specs(state: &SharedState, key: &HubKey) -> Result<Vec<MatterTopologyGroup>> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let mut specs = Vec::new();

    for room in state.topology.rooms() {
        let mut member_device_ids = Vec::new();
        for room_device in &room.devices {
            let Some(device) = state.canonical_registry.get(&room_device.device_id) else {
                continue;
            };
            if device.device_type != DeviceType::Light {
                continue;
            }
            let Some(endpoint) = device.preferred_endpoint() else {
                continue;
            };
            if &endpoint.hub_key != key {
                continue;
            }
            if crate::lifecycle::parse_device_id(&endpoint.native_id).is_some() {
                member_device_ids.push(endpoint.native_id.clone());
            }
        }

        member_device_ids.sort();
        member_device_ids.dedup();
        if member_device_ids.len() >= 2 {
            specs.push(MatterTopologyGroup {
                area_id: room.id.clone(),
                name: room.name.clone(),
                member_device_ids,
            });
        }
    }

    specs.sort_by(|left, right| left.area_id.cmp(&right.area_id));
    Ok(specs)
}

/// Synchronize generated Matter groups and topology bindings from assigned rooms.
pub fn sync_topology_groups(state: &SharedState, key: &HubKey) -> Result<()> {
    if key.hub_type.as_str() != "matter" {
        return Ok(());
    }

    let specs = topology_group_specs(state, key)?;
    info!(
        target: "sys",
        "Matter group topology sync: hub={} desired_rooms={}",
        key,
        specs.len()
    );

    let transport = get_transport(state)?;
    let hub_data = get_hub_data(state)?;
    let controller = MatterGroupController::new(transport, hub_data.registry.clone())
        .with_physical_group_provisioning(!crate::controller::matter_group_fanout_only_enabled());
    let groups = controller
        .sync_topology_groups(&specs)
        .map_err(|error| anyhow::anyhow!("Matter group sync failed: {}", error))?;
    let desired_area_ids = groups
        .iter()
        .map(|group| group.area_id.clone())
        .collect::<HashSet<_>>();

    let (changed, removed_bindings, updated_bindings) = {
        let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let mut changed = false;
        let removed_bindings = state
            .topology
            .remove_room_bindings_for_hub_where(key, |binding| {
                parse_group_control_id(&binding.control_id).is_some()
                    && !desired_area_ids.contains(&binding.hub_room_id)
            });
        if !removed_bindings.is_empty() {
            changed = true;
        }

        let mut updated_bindings = Vec::new();
        for group in &groups {
            let room_exists = state.topology.rooms().any(|room| room.id == group.area_id);
            let binding = HubRoomBinding {
                hub_key: key.clone(),
                hub_room_id: group.area_id.clone(),
                control_id: group.control_id.clone(),
                light_device_ids: group.members.clone(),
            };
            if state.topology.upsert_room_binding(&group.area_id, binding) {
                changed = true;
                updated_bindings.push((
                    group.area_id.clone(),
                    group.control_id.clone(),
                    group.members.len(),
                ));
            } else if !room_exists {
                warn!(
                    target: "sys",
                    "Matter group topology sync: skipping missing room={} control_id={} members={}",
                    group.area_id,
                    group.control_id,
                    group.members.len()
                );
            }
        }

        (changed, removed_bindings, updated_bindings)
    };

    for room_id in &removed_bindings {
        info!(
            target: "sys",
            "Matter group topology sync: removed stale binding room={}",
            room_id
        );
    }
    for (room_id, control_id, members) in &updated_bindings {
        info!(
            target: "sys",
            "Matter group topology sync: room={} control_id={} members={}",
            room_id,
            control_id,
            members
        );
    }
    info!(
        target: "sys",
        "Matter group topology sync complete: hub={} groups={} topology_changed={} stale_bindings={}",
        key,
        groups.len(),
        changed,
        removed_bindings.len()
    );

    Ok(())
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
            device_profiles: Vec::new(),
            supports_unpairing: true,
            unpairable_device_types: vec!["light".to_string()],
            supports_roomless_devices: true,
            blocks_room_readiness: true,
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

    fn sync_topology_groups(&self, state: &SharedState, key: &HubKey) -> Result<()> {
        sync_topology_groups(state, key)
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
        self.start_pairing_with_context(
            state,
            params,
            rhythm_os::pairing::PairingRequestContext::accepted_now(),
        )
    }

    fn start_pairing_with_context(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
        context: rhythm_os::pairing::PairingRequestContext,
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
        crate::commissioning::pair_device_with_context(
            state, transport, hub_data, &request, &context,
        )
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
        let preserve_recovery = params
            .get("archive")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let device_id = resolve_unpair_device_id(state, device_id)?;

        let (node_id, _endpoint) = crate::lifecycle::parse_device_id(&device_id)
            .ok_or_else(|| anyhow::anyhow!("Invalid Matter device ID: {}", device_id))?;

        if force {
            let persistent_removed = match remove_persisted_device_cache_entry(state, node_id) {
                Ok(removed) => removed,
                Err(error) => {
                    log::error!(
                        target: "pair",
                        "Matter: failed to force-remove node {} from persistent device cache: {:#}",
                        node_id,
                        error
                    );
                    return Ok(UnpairingResult {
                        hub_type: "matter".to_string(),
                        hub_address: Some("local".to_string()),
                        status: PairingStatus::Failed,
                        device_id: Some(device_id.to_string()),
                        error: Some(format!("{:#}", error)),
                        completion_scope: None,
                        warning: None,
                    });
                }
            };

            if persistent_removed {
                warn!(
                    target: "pair",
                    "Matter: force-removed node {} from persistent device cache without decommission RPC",
                    node_id
                );
            } else {
                warn!(
                    target: "pair",
                    "Matter: node {} was not present in persistent device cache during force remove",
                    node_id
                );
            }

            match get_hub_data(state) {
                Ok(hub_data) => {
                    warn!(
                        target: "pair",
                        "Matter: force-removing node {} locally without decommission RPC",
                        node_id
                    );
                    hub_data.finish_decommission(node_id, true);
                }
                Err(error) => {
                    warn!(
                        target: "pair",
                        "Matter: force-removing node {} locally without connected hub data: {:#}",
                        node_id,
                        error
                    );
                }
            }

            let warning = if preserve_recovery {
                None
            } else {
                delete_setup_recovery_for_node(state, node_id)
            };
            return Ok(UnpairingResult {
                hub_type: "matter".to_string(),
                hub_address: Some("local".to_string()),
                status: PairingStatus::Complete,
                device_id: Some(device_id.to_string()),
                error: None,
                completion_scope: None,
                warning,
            });
        }

        let transport = get_transport(state)?;
        let hub_data = get_hub_data(state)?;

        if hub_data.is_recently_decommissioned(node_id) {
            info!(
                target: "sys",
                "Matter: node {} was already decommissioned recently",
                node_id
            );
            let warning = if preserve_recovery {
                None
            } else {
                delete_setup_recovery_for_node(state, node_id)
            };
            return Ok(UnpairingResult {
                hub_type: "matter".to_string(),
                hub_address: Some("local".to_string()),
                status: PairingStatus::Complete,
                device_id: Some(device_id.to_string()),
                error: None,
                completion_scope: None,
                warning,
            });
        }

        if !hub_data.begin_decommission(node_id) {
            return Ok(UnpairingResult {
                hub_type: "matter".to_string(),
                hub_address: Some("local".to_string()),
                status: PairingStatus::Failed,
                device_id: Some(device_id.to_string()),
                error: Some(format!(
                    "Matter node {} is already being decommissioned",
                    node_id
                )),
                completion_scope: None,
                warning: None,
            });
        }

        match transport.decommission_device(node_id, false) {
            Ok(()) => {
                info!(target: "sys", "Matter: decommissioned node {}", node_id);
                hub_data.finish_decommission(node_id, true);

                let warning = if preserve_recovery {
                    None
                } else {
                    delete_setup_recovery_for_node(state, node_id)
                };
                Ok(UnpairingResult {
                    hub_type: "matter".to_string(),
                    hub_address: Some("local".to_string()),
                    status: PairingStatus::Complete,
                    device_id: Some(device_id.to_string()),
                    error: None,
                    completion_scope: None,
                    warning,
                })
            }
            Err(e) => {
                log::error!(target: "pair", "Matter decommission error: {:#}", e);
                hub_data.finish_decommission(node_id, false);
                Ok(UnpairingResult {
                    hub_type: "matter".to_string(),
                    hub_address: Some("local".to_string()),
                    status: PairingStatus::Failed,
                    device_id: Some(device_id.to_string()),
                    error: Some(format!("{:#}", e)),
                    completion_scope: None,
                    warning: None,
                })
            }
        }
    }

    fn load_pairing_recovery(
        &self,
        state: &SharedState,
        native_device_id: &str,
    ) -> Result<Option<rhythm_os::pairing::PairingRecoverySecret>> {
        let matter_hub_key = HubKey::new(HubType::new("matter"), "local");
        let is_registered = state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .canonical_registry
            .all_devices_including_removed()
            .any(|device| {
                device.endpoints.iter().any(|endpoint| {
                    endpoint.hub_key == matter_hub_key && endpoint.native_id == native_device_id
                })
            });
        if !is_registered {
            return Ok(None);
        }
        crate::setup_recovery::load_setup_payload(
            state,
            &configured_fabric_id(state),
            native_device_id,
        )
    }

    fn purge_pairing_recovery(&self, state: &SharedState, native_device_id: &str) -> Result<()> {
        let (node_id, _endpoint) = crate::lifecycle::parse_device_id(native_device_id)
            .ok_or_else(|| anyhow::anyhow!("Invalid Matter device ID: {}", native_device_id))?;
        crate::setup_recovery::delete_setup_payloads_for_node(
            state,
            &configured_fabric_id(state),
            node_id,
        )
    }

    fn run_device_test(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        crate::audition::run_audition(state, params)
    }

    fn save_device_test_report(
        &self,
        state: &SharedState,
        report: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        crate::audition::save_audition_report(state, report)
    }
}

pub static INTEGRATION: MatterIntegration = MatterIntegration;

fn delete_setup_recovery_for_node(state: &SharedState, node_id: u64) -> Option<String> {
    match crate::setup_recovery::delete_setup_payloads_for_node(
        state,
        &configured_fabric_id(state),
        node_id,
    ) {
        Ok(()) => None,
        Err(error) => {
            warn!(
                target: "pair",
                "Matter setup recovery material could not be deleted: {}",
                error
            );
            Some(
                "The device was removed, but its saved Matter setup code could not be deleted."
                    .to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::{Mutex, OnceLock};

    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
    use rhythm_os::canonical::registry::ResolveResult;
    use rhythm_os::hub::{ActiveHub, ExternalLightHubIntegration, HubCredentials};
    use rhythm_os::pairing::PairingStatus;

    use crate::cloud_profiles::CloudMatterProfileCatalog;
    use crate::controller::MatterDeviceRegistry;
    use crate::transport::{
        CommissionedDevice, MatterColorMode, MatterCommissionRequest, MatterCommissioningNetwork,
        MatterCommissioningRendezvous, MatterCommissioningWifiCredentials, MatterDeviceInfo,
        MatterGroup, MatterGroupMember,
    };

    #[derive(Default)]
    struct FakeMatterTransport {
        decommission_calls: Mutex<Vec<(u64, bool)>>,
        decommission_error: Mutex<Option<String>>,
        configured_groups: Mutex<Vec<MatterGroup>>,
        removed_groups: Mutex<Vec<(u16, Vec<MatterGroupMember>)>>,
    }

    impl FakeMatterTransport {
        fn set_decommission_error(&self, error: impl Into<String>) {
            *self.decommission_error.lock().unwrap() = Some(error.into());
        }
    }

    impl MatterTransport for FakeMatterTransport {
        fn commission_light(
            &self,
            request: &MatterCommissionRequest,
        ) -> Result<CommissionedDevice> {
            Ok(commissioned_device(request.node_id))
        }

        fn decommission_device(&self, node_id: u64, force: bool) -> Result<()> {
            self.decommission_calls
                .lock()
                .unwrap()
                .push((node_id, force));
            if let Some(error) = self.decommission_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            Ok(())
        }

        fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
            Ok(vec![MatterDeviceInfo {
                node_id: 42,
                vendor_name: "Acme".to_string(),
                product_name: "Lamp".to_string(),
                reachable: true,
            }])
        }

        fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
            Ok(commissioned_device(node_id))
        }

        fn set_on_off(&self, _node_id: u64, _endpoint: u16, _on: bool) -> Result<()> {
            Ok(())
        }

        fn configure_group(&self, group: &MatterGroup) -> Result<()> {
            self.configured_groups.lock().unwrap().push(group.clone());
            Ok(())
        }

        fn remove_group(&self, group_id: u16, members: &[MatterGroupMember]) -> Result<()> {
            self.removed_groups
                .lock()
                .unwrap()
                .push((group_id, members.to_vec()));
            Ok(())
        }

        fn identify_light(&self, _node_id: u64, _endpoint: u16, _duration_secs: u16) -> Result<()> {
            Ok(())
        }

        fn set_brightness(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _level: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_color_temperature(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _kelvin: u16,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_xy(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _x: f32,
            _y: f32,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_hue_saturation(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _hue: u8,
            _saturation: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn read_on_off(&self, _node_id: u64, _endpoint: u16) -> Result<bool> {
            Ok(false)
        }
    }

    fn matter_key() -> HubKey {
        HubKey::new(HubType::new("matter"), "local")
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(rhythm_os::state::AppState::default()))
    }

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rhythm-matter-desktop-lifecycle-{name}-{nanos}"))
    }

    fn set_data_dir(state: &SharedState, name: &str) -> std::path::PathBuf {
        let dir = unique_test_dir(name);
        std::fs::create_dir_all(&dir).unwrap();
        state.lock().unwrap().data_dir = dir.display().to_string();
        dir
    }

    fn commissioned_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Acme".to_string(),
            product_name: "Lamp".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 2,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        }
    }

    fn hub_data() -> Arc<MatterHubData> {
        let (event_tx, _event_rx) = std::sync::mpsc::channel();
        Arc::new(MatterHubData {
            transport: OnceLock::new(),
            capture_dir: OnceLock::new(),
            registry: Arc::new(Mutex::new(MatterDeviceRegistry::new())),
            fabric_id: "default".to_string(),
            commissioned: Mutex::new(Vec::new()),
            next_node_id: AtomicU64::new(10),
            device_caps: Mutex::new(HashMap::new()),
            device_quirks: Mutex::new(HashMap::new()),
            device_profiles: Mutex::new(HashMap::new()),
            pending_turn_on_plans: Arc::new(Mutex::new(HashMap::new())),
            needs_audition: Arc::new(Mutex::new(HashSet::new())),
            readback: Arc::new(crate::hub_state::MatterReadbackCoordinator::default()),
            local_overrides: Mutex::new(crate::local_quirks::LocalMatterOverrides::default()),
            cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
            decommissioning: Mutex::new(HashSet::new()),
            recently_decommissioned: Mutex::new(HashMap::new()),
            node_proof_of_life: Arc::new(Mutex::new(HashMap::new())),
            on_off_observations: Arc::new(Mutex::new(HashMap::new())),
            attribute_report_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            event_tx,
        })
    }

    fn install_hub(state: &SharedState, hub_data: Arc<MatterHubData>) {
        let key = matter_key();
        let hub = ActiveHub {
            hub_type: key.hub_type.clone(),
            hub_key: key.clone(),
            runtime: None,
            hub_data: Box::new(hub_data),
            registry: None,
            discovery: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        state.lock().unwrap().hubs.insert(key, hub);
    }

    fn install_transport(
        hub_data: &Arc<MatterHubData>,
        transport: Arc<FakeMatterTransport>,
    ) -> Arc<FakeMatterTransport> {
        let transport_dyn: Arc<dyn MatterTransport> = transport.clone();
        assert!(hub_data.transport.set(transport_dyn).is_ok());
        transport
    }

    fn canonical_matter_device(state: &SharedState, native_id: &str) -> String {
        let identity = DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: None,
            room_name: None,
            name: "Matter Lamp".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter(&format!("vid-1-pid-2-{native_id}"))],
            manufacturer: Some("Acme".to_string()),
            model: Some("Lamp".to_string()),
        };
        let result = state
            .lock()
            .unwrap()
            .canonical_registry
            .resolve(&identity, &matter_key(), 1);
        match result {
            ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("expected new canonical device, got {other:?}"),
        }
    }

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn topology_group_specs_include_only_assigned_matter_lights() {
        let state = state();
        let room_id = state.lock().unwrap().topology.create_room("Kitchen");
        let lamp_one = canonical_matter_device(&state, "matter-42-2");
        let lamp_two = canonical_matter_device(&state, "matter-43-2");

        let hue_identity = DiscoveredIdentity {
            native_id: "hue-light-1".to_string(),
            room_id: None,
            room_name: None,
            name: "Hue Lamp".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::serial("hue-light-1")],
            manufacturer: Some("Hue".to_string()),
            model: Some("A19".to_string()),
        };
        let hue_id = match state.lock().unwrap().canonical_registry.resolve(
            &hue_identity,
            &HubKey::new(HubType::new("hue"), "bridge"),
            2,
        ) {
            ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("expected hue canonical device, got {other:?}"),
        };

        let button_identity = DiscoveredIdentity {
            native_id: "matter-button-1".to_string(),
            room_id: None,
            room_name: None,
            name: "Button".to_string(),
            device_type: DeviceType::Button,
            hardware_ids: vec![HardwareId::matter("button")],
            manufacturer: Some("Acme".to_string()),
            model: Some("Button".to_string()),
        };
        let button_id = match state.lock().unwrap().canonical_registry.resolve(
            &button_identity,
            &matter_key(),
            3,
        ) {
            ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("expected button canonical device, got {other:?}"),
        };

        {
            let mut state = state.lock().unwrap();
            assert!(state
                .topology
                .attach_device_user_override(&room_id, &lamp_one));
            assert!(state
                .topology
                .attach_device_user_override(&room_id, &lamp_two));
            assert!(state
                .topology
                .attach_device_user_override(&room_id, &hue_id));
            assert!(state
                .topology
                .attach_device_user_override(&room_id, &button_id));
        }

        let specs = topology_group_specs(&state, &matter_key()).unwrap();

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].area_id, room_id);
        assert_eq!(specs[0].name, "Kitchen");
        assert_eq!(
            specs[0].member_device_ids,
            vec!["matter-42-2".to_string(), "matter-43-2".to_string()]
        );
    }

    #[test]
    fn sync_topology_groups_publishes_logical_binding_without_physical_group_by_default() {
        let state = state();
        let data = hub_data();
        let transport = install_transport(&data, Arc::new(FakeMatterTransport::default()));
        install_hub(&state, data.clone());

        let room_id = state.lock().unwrap().topology.create_room("Kitchen");
        let stale_room_id = state.lock().unwrap().topology.create_room("Stale");
        let lamp_one = canonical_matter_device(&state, "matter-42-2");
        let lamp_two = canonical_matter_device(&state, "matter-43-2");
        {
            let mut state = state.lock().unwrap();
            assert!(state
                .topology
                .attach_device_user_override(&room_id, &lamp_one));
            assert!(state
                .topology
                .attach_device_user_override(&room_id, &lamp_two));
            state
                .topology
                .get_mut(&stale_room_id)
                .unwrap()
                .upsert_hub_room_binding(HubRoomBinding {
                    hub_key: matter_key(),
                    hub_room_id: "old-area".to_string(),
                    control_id: crate::controller::format_group_control_id(0x8001),
                    light_device_ids: vec!["matter-99-2".to_string()],
                });
        }

        sync_topology_groups(&state, &HubKey::new(HubType::new("hue"), "bridge")).unwrap();
        assert!(transport.configured_groups.lock().unwrap().is_empty());

        sync_topology_groups(&state, &matter_key()).unwrap();

        assert!(transport.configured_groups.lock().unwrap().is_empty());

        let state = state.lock().unwrap();
        let room = state.topology.get(&room_id).unwrap();
        let binding = room
            .hub_room_bindings
            .iter()
            .find(|binding| binding.hub_key == matter_key())
            .expect("Matter group sync should upsert a room binding");
        assert_eq!(binding.hub_room_id, room_id);
        assert!(parse_group_control_id(&binding.control_id).is_some());
        assert_eq!(
            binding.light_device_ids,
            vec!["matter-42-2".to_string(), "matter-43-2".to_string()]
        );
        let expected_group_id = parse_group_control_id(&binding.control_id);
        assert!(state
            .topology
            .get(&stale_room_id)
            .unwrap()
            .hub_room_bindings
            .is_empty());
        drop(state);

        let registry = data.registry.lock().unwrap();
        assert_eq!(
            registry.get_light_entities(&room_id),
            vec!["matter-42-2".to_string(), "matter-43-2".to_string()]
        );
        assert_eq!(
            registry
                .get_grouped_light_id(&room_id)
                .and_then(|control_id| parse_group_control_id(&control_id)),
            expected_group_id
        );
    }

    #[test]
    fn matter_data_path_and_fabric_id_follow_state() {
        let state = state();
        assert_eq!(
            string_error(matter_data_path(&state)),
            "data_dir not configured on AppState"
        );
        assert_eq!(configured_fabric_id(&state), "default");

        {
            let mut state = state.lock().unwrap();
            state.data_dir = "/tmp/rhythm".to_string();
            state.hub_credentials.insert(
                matter_key(),
                HubCredentials::new(
                    "matter",
                    "local",
                    serde_json::json!({ "fabric_id": "fabric-a" }),
                ),
            );
        }

        assert_eq!(matter_data_path(&state).unwrap(), "/tmp/rhythm/matter");
        assert_eq!(configured_fabric_id(&state), "fabric-a");
    }

    #[test]
    fn integration_capabilities_and_credentials_interceptor_branches() {
        let state = state();
        let integration = MatterIntegration;

        assert_eq!(integration.hub_type(), "matter");
        assert_eq!(integration.provider().hub_type().as_str(), "matter");

        let capabilities = integration.api_capabilities();
        assert_eq!(capabilities.hub_type, "matter");
        assert!(capabilities.configurable);
        assert!(capabilities.supports_unpairing);
        assert!(capabilities.supports_roomless_devices);
        assert!(capabilities
            .device_onboarding_methods
            .iter()
            .any(|method| method
                == rhythm_os::hub::DEVICE_ONBOARDING_METHOD_MATTER_ON_NETWORK_SETUP_CODE));

        assert!(integration
            .credentials_interceptor(&state, &serde_json::json!({ "hub_type": "hue" }))
            .is_none());
        assert!(integration
            .credentials_interceptor(
                &state,
                &serde_json::json!({
                    "hub_type": "matter",
                    "credentials": { "fabric_id": "custom" }
                })
            )
            .is_none());

        let intercepted = integration
            .credentials_interceptor(&state, &serde_json::json!({ "hub_type": "matter" }))
            .expect("matter empty credentials should be intercepted");
        assert_eq!(intercepted.unwrap_err(), "No hub provider registered");
    }

    #[test]
    fn resolve_unpair_device_id_accepts_native_or_canonical_matter_endpoint() {
        let state = state();
        assert_eq!(
            resolve_unpair_device_id(&state, "matter-42-2").unwrap(),
            "matter-42-2"
        );
        assert_eq!(
            string_error(resolve_unpair_device_id(&state, "missing")),
            "Invalid Matter device ID: missing"
        );

        let canonical_id = canonical_matter_device(&state, "matter-99-2");
        assert_eq!(
            resolve_unpair_device_id(&state, &canonical_id).unwrap(),
            "matter-99-2"
        );
    }

    #[test]
    fn resolve_unpair_device_id_rejects_canonical_device_without_matter_endpoint() {
        let state = state();
        let hue_identity = DiscoveredIdentity {
            native_id: "hue-light-1".to_string(),
            room_id: None,
            room_name: None,
            name: "Hue Lamp".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::serial("hue-light-1")],
            manufacturer: Some("Hue".to_string()),
            model: Some("A19".to_string()),
        };
        let canonical_id = match state.lock().unwrap().canonical_registry.resolve(
            &hue_identity,
            &HubKey::new(HubType::new("hue"), "bridge"),
            4,
        ) {
            ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("expected hue canonical device, got {other:?}"),
        };

        let error = resolve_unpair_device_id(&state, &canonical_id).unwrap_err();
        assert!(error.to_string().contains("has no Matter endpoint"));
    }

    #[test]
    fn create_controller_reports_missing_hub_data_or_transport() {
        let state = state();
        assert_eq!(
            string_error(create_controller(&state, &matter_key())),
            "Matter hub not connected"
        );

        let data = hub_data();
        install_hub(&state, data.clone());
        assert_eq!(
            string_error(create_controller(&state, &matter_key())),
            "Matter transport not initialized"
        );

        install_transport(&data, Arc::new(FakeMatterTransport::default()));
        let controller = create_controller(&state, &matter_key()).unwrap();
        assert_eq!(controller.name(), "Matter");
    }

    #[test]
    fn integration_trait_wrappers_delegate_to_matter_helpers() {
        let state = state();
        let integration = MatterIntegration;

        integration.ensure_runtime(&state).unwrap();
        integration.post_connect(&state, &matter_key());
        assert_eq!(
            string_error(integration.create_controller(&state, &matter_key())),
            "Matter hub not connected"
        );

        integration
            .sync_topology_groups(&state, &HubKey::new(HubType::new("hue"), "bridge"))
            .unwrap();

        assert!(integration
            .run_device_test(&state, &serde_json::json!({}))
            .is_err());
        assert!(integration
            .save_device_test_report(&state, &serde_json::json!({}))
            .is_err());
    }

    #[test]
    fn fake_matter_transport_covers_full_trait_surface() {
        let transport = FakeMatterTransport::default();
        let request = MatterCommissionRequest {
            setup_payload: "MT:TEST".to_string(),
            node_id: 123,
            network: MatterCommissioningNetwork::Wifi,
            rendezvous: MatterCommissioningRendezvous::OnNetwork,
            wifi_credentials: MatterCommissioningWifiCredentials {
                ssid: "wifi".to_string(),
                password: "secret".to_string(),
            },
        };

        assert_eq!(transport.commission_light(&request).unwrap().node_id, 123);
        assert_eq!(transport.list_devices().unwrap()[0].node_id, 42);
        assert_eq!(transport.probe_light(77).unwrap().node_id, 77);
        transport.set_on_off(77, 2, true).unwrap();
        transport.identify_light(77, 2, 1).unwrap();
        transport.set_brightness(77, 2, 128, Some(100)).unwrap();
        transport.set_color_temperature(77, 2, 3000, None).unwrap();
        transport.set_xy(77, 2, 0.25, 0.35, Some(50)).unwrap();
        transport.set_hue_saturation(77, 2, 10, 200, None).unwrap();
        assert!(!transport.read_on_off(77, 2).unwrap());

        let group = MatterGroup {
            group_id: 0x8001,
            name: "Kitchen".to_string(),
            members: vec![MatterGroupMember {
                node_id: 77,
                endpoint: 2,
            }],
        };
        transport.configure_group(&group).unwrap();
        transport
            .remove_group(group.group_id, &group.members)
            .unwrap();

        assert_eq!(transport.configured_groups.lock().unwrap()[0], group);
        assert_eq!(
            transport.removed_groups.lock().unwrap()[0],
            (
                0x8001,
                vec![MatterGroupMember {
                    node_id: 77,
                    endpoint: 2
                }]
            )
        );
    }

    #[test]
    fn start_unpairing_validates_params_and_missing_transport() {
        let state = state();
        let data_dir = set_data_dir(&state, "unpair-validation");
        let integration = MatterIntegration;

        assert_eq!(
            string_error(integration.start_unpairing(&state, &serde_json::json!({}))),
            "Missing 'device_id' in unpairing params"
        );
        assert_eq!(
            string_error(
                integration
                    .start_unpairing(&state, &serde_json::json!({ "device_id": "not-matter" }))
            ),
            "Invalid Matter device ID: not-matter"
        );

        install_hub(&state, hub_data());
        assert_eq!(
            string_error(
                integration
                    .start_unpairing(&state, &serde_json::json!({ "device_id": "matter-42-2" }))
            ),
            "Matter transport not initialized"
        );
        let result = integration
            .start_unpairing(
                &state,
                &serde_json::json!({ "device_id": "matter-42-2", "force": true }),
            )
            .unwrap();
        assert_eq!(result.status, PairingStatus::Complete);

        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn start_unpairing_tracks_success_recent_suppression_in_progress_and_failure() {
        let state = state();
        let data_dir = set_data_dir(&state, "unpair-tracking");
        let data = hub_data();
        let transport = install_transport(&data, Arc::new(FakeMatterTransport::default()));
        install_hub(&state, data.clone());
        let integration = MatterIntegration;

        let result = integration
            .start_unpairing(
                &state,
                &serde_json::json!({ "device_id": "matter-42-2", "force": true }),
            )
            .unwrap();
        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(result.device_id.as_deref(), Some("matter-42-2"));
        assert!(transport.decommission_calls.lock().unwrap().is_empty());
        assert!(data.is_recently_decommissioned(42));

        let result = integration
            .start_unpairing(
                &state,
                &serde_json::json!({ "device_id": "matter-42-2", "force": true }),
            )
            .unwrap();
        assert_eq!(result.status, PairingStatus::Complete);
        assert!(transport.decommission_calls.lock().unwrap().is_empty());

        assert!(data.begin_decommission(77));
        let result = integration
            .start_unpairing(&state, &serde_json::json!({ "device_id": "matter-77" }))
            .unwrap();
        assert_eq!(result.status, PairingStatus::Failed);
        assert_eq!(
            result.error.as_deref(),
            Some("Matter node 77 is already being decommissioned")
        );

        transport.set_decommission_error("sidecar rejected request");
        let result = integration
            .start_unpairing(&state, &serde_json::json!({ "device_id": "matter-88" }))
            .unwrap();
        assert_eq!(result.status, PairingStatus::Failed);
        assert_eq!(result.error.as_deref(), Some("sidecar rejected request"));
        assert!(!data.is_recently_decommissioned(88));
        assert_eq!(
            transport.decommission_calls.lock().unwrap().as_slice(),
            &[(88, false)]
        );

        let result = integration
            .start_unpairing(
                &state,
                &serde_json::json!({ "device_id": "matter-89", "force": true }),
            )
            .unwrap();
        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(result.device_id.as_deref(), Some("matter-89"));
        assert_eq!(result.error, None);
        assert!(data.is_recently_decommissioned(89));
        assert_eq!(
            transport.decommission_calls.lock().unwrap().as_slice(),
            &[(88, false)]
        );

        assert!(data.begin_decommission(90));
        let result = integration
            .start_unpairing(
                &state,
                &serde_json::json!({ "device_id": "matter-90", "force": true }),
            )
            .unwrap();
        assert_eq!(result.status, PairingStatus::Complete);
        assert!(data.is_recently_decommissioned(90));
        assert_eq!(
            transport.decommission_calls.lock().unwrap().as_slice(),
            &[(88, false)]
        );

        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn forced_unpair_removes_persisted_device_without_connected_hub() {
        let state = state();
        let data_dir = set_data_dir(&state, "force-persistent-remove");
        state.lock().unwrap().storage = Some(Arc::new(
            rhythm_os::storage::FileStorage::new(data_dir.to_str().unwrap()).unwrap(),
        ));
        crate::setup_recovery::save_setup_payload(&state, "default", 102, 1, "MT:UNPAIR-SECRET")
            .unwrap();
        let matter_dir = data_dir.join("matter");
        let devices_path = matter_dir.join("chip").join("devices.json");
        std::fs::create_dir_all(devices_path.parent().unwrap()).unwrap();
        std::fs::write(
            &devices_path,
            serde_json::to_string_pretty(&vec![commissioned_device(102), commissioned_device(103)])
                .unwrap(),
        )
        .unwrap();

        let result = MatterIntegration
            .start_unpairing(
                &state,
                &serde_json::json!({ "device_id": "matter-102", "force": true }),
            )
            .unwrap();

        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(result.device_id.as_deref(), Some("matter-102"));

        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&devices_path).unwrap()).unwrap();
        assert_eq!(persisted["schema_version"], 1);
        let devices = persisted["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["node_id"], 103);
        assert!(
            crate::setup_recovery::load_setup_payload(&state, "default", "matter-102",)
                .unwrap()
                .is_none()
        );

        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn archived_unpair_retains_recovery_until_explicit_purge() {
        let state = state();
        let data_dir = set_data_dir(&state, "archive-persistent-remove");
        state.lock().unwrap().storage = Some(Arc::new(
            rhythm_os::storage::FileStorage::new(data_dir.to_str().unwrap()).unwrap(),
        ));
        crate::setup_recovery::save_setup_payload(&state, "default", 104, 1, "MT:ARCHIVE-SECRET")
            .unwrap();
        let devices_path = data_dir.join("matter").join("chip").join("devices.json");
        std::fs::create_dir_all(devices_path.parent().unwrap()).unwrap();
        std::fs::write(
            &devices_path,
            serde_json::to_string_pretty(&vec![commissioned_device(104)]).unwrap(),
        )
        .unwrap();

        let integration = MatterIntegration;
        let result = integration
            .start_unpairing(
                &state,
                &serde_json::json!({
                    "device_id": "matter-104",
                    "force": true,
                    "archive": true
                }),
            )
            .unwrap();

        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(
            crate::setup_recovery::load_setup_payload(&state, "default", "matter-104")
                .unwrap()
                .unwrap()
                .setup_payload,
            "MT:ARCHIVE-SECRET"
        );

        integration
            .purge_pairing_recovery(&state, "matter-104")
            .unwrap();
        assert!(
            crate::setup_recovery::load_setup_payload(&state, "default", "matter-104")
                .unwrap()
                .is_none()
        );

        let _ = std::fs::remove_dir_all(data_dir);
    }
}
