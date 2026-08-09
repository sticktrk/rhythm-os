//! Complete Hue lifecycle using reqwest transport (desktop/server targets).
//!
//! Provides everything a binary crate needs to run Hue as a plugin:
//! `connect_and_start`, `ensure_runtime`, `get_hub_provider`.
//! No platform-specific code needed in the consuming crate.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
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
    ActiveHub, ExternalControllerReleaseReason, ExternalLightHubIntegration, HubCredentials,
    HubDeviceRoomAssignment, HubDeviceRoomAssignmentOutcome, HubEvent, HubIntegrationCapability,
    HubProvider, HubType, DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_BUTTON_SEARCH,
    DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_SERIAL_SEARCH,
};
use rhythm_os::pairing::{
    PairedDeviceInfo, PairingSession, PairingStage, PairingStatus, UnpairingResult,
};
use rhythm_os::state::SharedState;
use rhythm_os::storage::Storage;

const HUE_ADDRESS_MIGRATION_FROM_FIELD: &str = "address_migration_from";
const HUE_BUTTON_PROJECTION_TIMEOUT: Duration = Duration::from_secs(45);
const HUE_BUTTON_SYNC_SLOT_TIMEOUT: Duration = Duration::from_secs(5);

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
    let topology_lock = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .external_topology_transaction_lock
        .clone();
    let _topology_guard = topology_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("Failed to lock external topology transaction"))?;
    let bridge_id = verify_and_persist_connected_bridge_identity(&state, key)?;
    let authority_fenced = hue_authority_should_fence_on_connect(&state, key, &bridge_id)?;
    let operation_lock = crate::ownership::controller_operation_lock(&bridge_id);
    let _operation_guard = operation_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("Failed to lock Hue controller operation"))?;
    complete_pending_hue_address_migration(&state, key, &bridge_id)?;
    let (hub, event_rx) = start_hue_after_release_fence(&state, &bridge_id, || {
        let snapshot = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.storage
                .as_ref()
                .and_then(|st| st.load_hub_registry_for(key).ok().flatten())
                .and_then(|v| serde_json::from_value::<HueRegistrySnapshot>(v).ok())
        };
        connect_hue_sse(&state, key, snapshot)
    })?;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = hub.hub_key.clone();
        s.hubs.insert(key.clone(), hub);
        if authority_fenced {
            s.mark_external_controller_authority_pending(&key);
        }
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

    let (bridge_ip, username, bridge_id, registry, sse_liveness) = {
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
        let bridge_id = creds
            .get_str("bridge_id")
            .filter(|bridge_id| !bridge_id.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Hue credentials missing verified bridge identity"))?
            .to_string();

        (bridge_ip, username, bridge_id, reg, sse_liveness)
    };

    let transport = ReqwestHueTransport::new(&bridge_ip)?;
    let controller = HueLightController::new(transport, username, registry)
        .with_capability_source(state.clone(), key.clone())
        .with_sse_liveness(sse_liveness)
        .with_controller_operation_lock(crate::ownership::controller_operation_lock(&bridge_id));
    Ok(std::sync::Arc::new(controller))
}

/// Refuse binary rollback while a bridge is still under Rhythm authority.
///
/// Automatic Hue restoration is intentionally disabled. A snapshot retained
/// after credential removal is archival and may survive rollback, but this
/// function never writes captured state back to a bridge.
pub fn restore_authoritative_bridges_before_binary_rollback(
    data_dir: &std::path::Path,
) -> Result<()> {
    let data_dir = data_dir
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Rhythm data directory is not UTF-8"))?;
    let storage = rhythm_os::storage::FileStorage::new(data_dir)?;
    let ownership_manifest_files = storage
        .load_integration_backup_files(true)?
        .into_iter()
        .filter(|file| {
            file.path.starts_with("hue/controller-ownership/by-bridge/")
                && file.path.ends_with(".json")
        })
        .collect::<Vec<_>>();
    let all_credentials = storage
        .load_all_hub_credentials()?
        .into_iter()
        .filter(|credentials| {
            credentials
                .hub_type
                .as_ref()
                .is_some_and(|hub_type| hub_type.as_str() == HubType::HUE)
        })
        .collect::<Vec<_>>();
    let blocking_manifest_count = ownership_manifest_files
        .iter()
        .filter(|file| {
            let Ok(candidate) =
                serde_json::from_str::<crate::ownership::HueControllerOwnership>(&file.content)
            else {
                return true;
            };
            match candidate.phase {
                crate::ownership::HueOwnershipPhase::SnapshotRetained => {
                    all_credentials.iter().any(|credentials| {
                        credentials.get_str("bridge_id") == Some(candidate.baseline().bridge_id())
                    })
                }
                crate::ownership::HueOwnershipPhase::Restored => false,
                _ => true,
            }
        })
        .count();
    if blocking_manifest_count != 0 {
        anyhow::bail!(
            "Binary rollback is blocked: {blocking_manifest_count} Hue bridge snapshot(s) still have active controller authority"
        );
    }
    Ok(())
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

fn validate_hue_backup_credentials<H: HueTransport + ?Sized>(
    transport: &H,
    credentials: &serde_json::Value,
) -> Result<()> {
    let username = credentials
        .get("username")
        .and_then(serde_json::Value::as_str)
        .filter(|username| !username.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Backup Hue recovery credentials are incomplete"))?;
    let expected_bridge_id = credentials
        .get("bridge_id")
        .and_then(serde_json::Value::as_str)
        .filter(|bridge_id| !bridge_id.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Backup Hue recovery identity is missing"))?;
    let connected_bridge_id = crate::ownership::connected_hue_bridge_id(transport, username)
        .context("Backup Hue bridge could not be authenticated before restore")?;
    if connected_bridge_id != expected_bridge_id {
        anyhow::bail!("Backup Hue credentials identify a different physical bridge");
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct SubmittedHueCredentials {
    username: String,
}

fn submitted_hue_username(credentials_json: &str) -> Result<String> {
    let submitted: SubmittedHueCredentials = serde_json::from_str(credentials_json)
        .map_err(|error| anyhow::anyhow!("Invalid Hue credentials: {}", error))?;
    if submitted.username.trim().is_empty() {
        anyhow::bail!("Invalid Hue credentials: username must not be empty");
    }
    Ok(submitted.username)
}

fn preflight_authenticated_hue_configuration(
    state: &SharedState,
    key: &HubKey,
    submitted_username: &str,
    connected_bridge_id: &str,
) -> Result<Option<HubKey>> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if state
        .hub_credentials
        .values()
        .any(|credentials| credentials.backup_restore_pending)
    {
        anyhow::bail!(
            "Hue backup restore is incomplete; retry backup restore or factory reset before configuration"
        );
    }
    let pending_migration_old_key = state
        .hub_credentials
        .get(key)
        .and_then(|credentials| credentials.get_str(HUE_ADDRESS_MIGRATION_FROM_FIELD))
        .filter(|address| !address.trim().is_empty())
        .map(|address| HubKey::new(key.hub_type.clone(), address));
    let target_is_resuming_address_migration = pending_migration_old_key.is_some();
    if state.external_controller_authority_pending.contains(key)
        && !target_is_resuming_address_migration
    {
        anyhow::bail!(
            "Hue controller authority is transitioning; retry after disconnect completes"
        );
    }
    if let Some(existing) = state.hub_credentials.get(key) {
        if let Some(expected_bridge_id) = existing
            .get_str("bridge_id")
            .filter(|bridge_id| !bridge_id.trim().is_empty())
        {
            if expected_bridge_id != connected_bridge_id {
                anyhow::bail!("Submitted Hue credentials identify a different physical bridge");
            }
        }
    }

    let configured_keys = state
        .hub_credentials
        .keys()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let mut authority_graph_keys = state.canonical_registry.referenced_hub_keys();
    authority_graph_keys.extend(state.topology.referenced_hub_keys());
    if authority_graph_keys.iter().any(|graph_key| {
        graph_key.hub_type.as_str() == HubType::HUE
            && !configured_keys.contains(graph_key)
            && pending_migration_old_key.as_ref() != Some(graph_key)
    }) {
        anyhow::bail!(
            "Existing Hue topology has no matching credential identity and must be recovered before configuration"
        );
    }

    let hue_credentials = state
        .hub_credentials
        .iter()
        .filter(|(_, credentials)| {
            credentials
                .hub_type
                .as_ref()
                .is_some_and(|hub_type| hub_type.as_str() == HubType::HUE)
        })
        .collect::<Vec<_>>();
    let mut matching_keys = hue_credentials
        .iter()
        .filter(|(_, credentials)| credentials.get_str("bridge_id") == Some(connected_bridge_id))
        .map(|(key, _)| (*key).clone())
        .collect::<Vec<_>>();
    matching_keys.sort_by_key(ToString::to_string);
    matching_keys.dedup();
    if matching_keys.len() > 1 {
        anyhow::bail!(
            "The physical Hue bridge is already registered under multiple addresses; disconnect the stale configuration before retrying"
        );
    }
    let legacy_keys = hue_credentials
        .iter()
        .filter(|(_, credentials)| {
            credentials
                .get_str("bridge_id")
                .is_none_or(|bridge_id| bridge_id.trim().is_empty())
        })
        .map(|(key, credentials)| ((*key).clone(), *credentials))
        .collect::<Vec<_>>();
    let matching_key = if let Some(matching_key) = matching_keys.into_iter().next() {
        if legacy_keys.iter().any(|(legacy_key, credentials)| {
            legacy_key != &matching_key
                && crate::provider::hue_username(credentials) == Some(submitted_username)
        }) {
            anyhow::bail!(
                "A legacy Hue configuration with the same credentials must be migrated before this bridge can be configured"
            );
        }
        Some(matching_key)
    } else if legacy_keys.iter().any(|(legacy_key, credentials)| {
        legacy_key == key && crate::provider::hue_username(credentials) == Some(submitted_username)
    }) {
        Some(key.clone())
    } else {
        let same_username_keys = legacy_keys
            .iter()
            .filter(|(_, credentials)| {
                crate::provider::hue_username(credentials) == Some(submitted_username)
            })
            .map(|(legacy_key, _)| legacy_key.clone())
            .collect::<Vec<_>>();
        match same_username_keys.as_slice() {
            [legacy_key] => Some(legacy_key.clone()),
            [] if legacy_keys.is_empty() => None,
            _ => anyhow::bail!(
                "Existing legacy Hue configuration could not be safely matched to this bridge"
            ),
        }
    };
    if let Some(matching_key) = matching_key.as_ref() {
        if state
            .external_controller_authority_pending
            .contains(matching_key)
            && !(matching_key == key && target_is_resuming_address_migration)
        {
            anyhow::bail!(
                "Hue controller authority is transitioning; retry after disconnect completes"
            );
        }
        if matching_key != key
            && (state.hub_credentials.contains_key(key)
                || state.hubs.contains_key(key)
                || state.canonical_registry.references_hub_key(key)
                || state.topology.references_hub_key(key))
        {
            anyhow::bail!("The target Hue address already belongs to another local hub identity");
        }
    }
    Ok(matching_key)
}

fn mark_hue_address_migration_pending(
    credentials: &mut HubCredentials,
    bridge_id: &str,
    old_key: &HubKey,
) {
    credentials.data["bridge_id"] = serde_json::Value::String(bridge_id.to_string());
    credentials.data[HUE_ADDRESS_MIGRATION_FROM_FIELD] =
        serde_json::Value::String(old_key.address.clone());
}

fn pending_hue_address_migration(
    credentials: &HubCredentials,
    new_key: &HubKey,
    bridge_id: &str,
) -> Result<Option<HubKey>> {
    if credentials.get_str("bridge_id") != Some(bridge_id) {
        anyhow::bail!("Stored Hue credentials identify a different physical bridge");
    }
    let Some(old_address) = credentials
        .get_str(HUE_ADDRESS_MIGRATION_FROM_FIELD)
        .filter(|address| !address.trim().is_empty())
    else {
        return Ok(None);
    };
    let old_key = HubKey::new(new_key.hub_type.clone(), old_address);
    if old_key == *new_key {
        anyhow::bail!("Hue address migration marker does not change the hub key");
    }
    Ok(Some(old_key))
}

fn stage_hue_address_migration(
    state: &SharedState,
    old_key: &HubKey,
    new_key: &HubKey,
    username: &str,
    bridge_id: &str,
) -> Result<()> {
    if old_key == new_key {
        return Ok(());
    }

    let (old_hub, composite, storage, registry_snapshot) = {
        let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let storage = state
            .storage
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Hue address migration requires durable storage"))?;
        let old_credentials = state
            .hub_credentials
            .get(old_key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("The prior Hue address is no longer configured"))?;
        match old_credentials.get_str("bridge_id") {
            Some(existing_bridge_id) if existing_bridge_id != bridge_id => {
                anyhow::bail!("The prior Hue address identifies a different physical bridge");
            }
            None if crate::provider::hue_username(&old_credentials) != Some(username) => {
                anyhow::bail!("The legacy Hue address could not be safely matched to this bridge");
            }
            _ => {}
        }
        if state.hub_credentials.contains_key(new_key)
            || state.hubs.contains_key(new_key)
            || state.canonical_registry.references_hub_key(new_key)
            || state.topology.references_hub_key(new_key)
        {
            anyhow::bail!("The target Hue address already belongs to another local hub identity");
        }
        if state.hub_sync_in_progress.contains(old_key)
            || state.hub_sync_in_progress.contains(new_key)
        {
            anyhow::bail!("Hue topology sync is in progress; retry the address migration");
        }

        let mut migrated_credentials = crate::provider::hue_credentials(&new_key.address, username);
        mark_hue_address_migration_pending(&mut migrated_credentials, bridge_id, old_key);
        let mut old_hub = state.hubs.remove(old_key);
        state.hub_credentials.remove(old_key);
        state
            .hub_credentials
            .insert(new_key.clone(), migrated_credentials);
        let all_credentials = state.hub_credentials.values().cloned().collect::<Vec<_>>();
        if let Err(error) = storage.save_all_hub_credentials(&all_credentials) {
            state.hub_credentials.remove(new_key);
            state
                .hub_credentials
                .insert(old_key.clone(), old_credentials);
            if let Some(old_hub) = old_hub.take() {
                state.hubs.insert(old_key.clone(), old_hub);
            }
            return Err(error.context("Failed to durably stage Hue address migration"));
        }

        let authority_required = state.external_controller_authority_is_required(old_key)
            || state.external_controller_authority_is_required(new_key)
            || state.external_controller_authority_is_enabled_for(new_key);
        state.set_external_controller_authority_required(old_key, false);
        state.set_external_controller_authority_required(new_key, authority_required);
        state.hub_connection_status.remove(old_key);
        state.hub_connection_status.remove(new_key);
        state.hub_connection_status.insert(new_key.clone(), false);
        state.external_controller_authority_pending.remove(old_key);
        state.external_controller_authority_pending.remove(new_key);
        state
            .external_controller_initial_sync_pending
            .remove(old_key);
        state
            .external_controller_initial_sync_pending
            .remove(new_key);
        state.mark_external_controller_initial_sync_pending(new_key);
        state.hub_seen_connected_once.remove(old_key);
        state.hub_seen_connected_once.remove(new_key);
        state.hub_sync_in_progress.remove(old_key);
        state.hub_sync_in_progress.remove(new_key);
        state.hub_reconnect_sync_at.remove(old_key);
        state.hub_reconnect_sync_at.remove(new_key);
        state.hub_pending_disconnect_at.remove(old_key);
        state.hub_pending_disconnect_at.remove(new_key);
        state.hub_startup_retry.remove(old_key);
        state.hub_startup_retry.remove(new_key);

        let old_key_string = old_key.to_string();
        let new_key_string = new_key.to_string();
        let stale_dispatch_ids = state
            .pending_integration_dispatches
            .iter()
            .filter_map(|(dispatch_id, dispatch)| {
                (dispatch.hub_key == old_key_string || dispatch.hub_key == new_key_string)
                    .then_some(*dispatch_id)
            })
            .collect::<Vec<_>>();
        for dispatch_id in stale_dispatch_ids {
            if let Some(dispatch) = state.pending_integration_dispatches.remove(&dispatch_id) {
                for command_id in dispatch.remaining_command_ids {
                    state.pending_integration_commands.remove(&(
                        dispatch.hub_key.clone(),
                        dispatch.controller_stream_id.clone(),
                        command_id,
                    ));
                }
                if let Some(count) = state.pending_node_dispatches.get_mut(&dispatch.node_id) {
                    if *count > 1 {
                        *count -= 1;
                    } else {
                        state.pending_node_dispatches.remove(&dispatch.node_id);
                    }
                }
            }
        }
        state
            .pending_integration_commands
            .retain(|(hub_key, _, _), _| hub_key != &old_key_string && hub_key != &new_key_string);
        state
            .early_integration_outcomes
            .retain(|(hub_key, _, _), _| hub_key != &old_key_string && hub_key != &new_key_string);

        let registry_snapshot = old_hub
            .as_ref()
            .and_then(|hub| hub.registry.as_ref())
            .and_then(|registry| {
                registry
                    .lock()
                    .ok()
                    .map(|registry| registry.snapshot_json())
            })
            .or_else(|| storage.load_hub_registry_for(old_key).ok().flatten());
        (
            old_hub,
            state.composite_controller.clone(),
            storage,
            registry_snapshot,
        )
    };

    if let Some(snapshot) = registry_snapshot {
        if let Err(error) = storage.save_hub_registry_for(new_key, &snapshot) {
            warn!(
                "Failed to migrate Hue registry cache to the new address: {}",
                error
            );
        }
    }
    if let Some(composite) = composite {
        composite.remove_controller(&old_key.to_string());
        rhythm_os::commands::rebuild_composite_routing(state);
    }
    if let Some(old_hub) = old_hub {
        old_hub
            .shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        std::thread::Builder::new()
            .name("hue-address-migration-drop".to_string())
            .spawn(move || drop(old_hub))
            .ok();
    }
    Ok(())
}

fn complete_pending_hue_address_migration(
    state: &SharedState,
    new_key: &HubKey,
    bridge_id: &str,
) -> Result<bool> {
    let shared_state = state;
    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let old_key = {
        let Some(credentials) = state.hub_credentials.get(new_key) else {
            return Ok(false);
        };
        pending_hue_address_migration(credentials, new_key, bridge_id)?
    };
    let Some(old_key) = old_key else {
        return Ok(false);
    };

    let authority_required = state.external_controller_authority_is_required(&old_key)
        || state.external_controller_authority_is_required(new_key)
        || state.external_controller_authority_is_enabled_for(new_key);
    state.set_external_controller_authority_required(&old_key, false);
    state.set_external_controller_authority_required(new_key, authority_required);

    let canonical_old = state.canonical_registry.references_hub_key(&old_key);
    let canonical_new = state.canonical_registry.references_hub_key(new_key);
    let topology_old = state.topology.references_hub_key(&old_key);
    let topology_new = state.topology.references_hub_key(new_key);
    if (canonical_old && canonical_new) || (topology_old && topology_new) {
        anyhow::bail!("Hue address migration found conflicting old and new controller references");
    }

    let previous_canonical = state.canonical_registry.clone();
    let previous_topology = state.topology.clone();
    state.canonical_registry.remap_hub_key(&old_key, new_key);
    state.topology.remap_hub_key(&old_key, new_key);
    if let Err(error) = rhythm_os::commands::save_authority_state(&state) {
        state.canonical_registry = previous_canonical;
        state.topology = previous_topology;
        return Err(error.context("Failed to durably migrate Hue topology to the new address"));
    }

    let storage = state
        .storage
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Hue address migration requires durable storage"))?;
    let previous_data = state
        .hub_credentials
        .get(new_key)
        .expect("credentials were checked above")
        .data
        .clone();
    state
        .hub_credentials
        .get_mut(new_key)
        .expect("credentials were checked above")
        .data
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Hue credentials have invalid provider data"))?
        .remove(HUE_ADDRESS_MIGRATION_FROM_FIELD);
    let all_credentials = state.hub_credentials.values().cloned().collect::<Vec<_>>();
    if let Err(error) = storage.save_all_hub_credentials(&all_credentials) {
        state
            .hub_credentials
            .get_mut(new_key)
            .expect("credentials remain installed")
            .data = previous_data;
        return Err(error.context("Failed to finalize durable Hue address migration"));
    }
    drop(state);
    rhythm_os::commands::rebuild_composite_routing(shared_state);
    Ok(true)
}

fn configure_authenticated_hue_hub<F>(
    address: &str,
    credentials_json: &str,
    username: &str,
    connected_bridge_id: &str,
    state: &SharedState,
    connect_fn: F,
) -> Result<()>
where
    F: FnOnce(&SharedState) -> Result<(ActiveHub, Receiver<HubEvent>)>,
{
    let configure_key = HubKey::new(HubType::new(HubType::HUE), address);
    let topology_lock = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .external_topology_transaction_lock
        .clone();
    let _topology_guard = topology_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("Failed to lock external topology transaction"))?;
    let operation_lock = crate::ownership::controller_operation_lock(connected_bridge_id);
    let _operation_guard = operation_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("Failed to lock Hue controller operation"))?;

    let matching_key = preflight_authenticated_hue_configuration(
        state,
        &configure_key,
        username,
        connected_bridge_id,
    )?;
    ensure_hue_start_is_not_release_fenced(state, connected_bridge_id)?;
    if let Some(old_key) = matching_key.filter(|old_key| old_key != &configure_key) {
        stage_hue_address_migration(
            state,
            &old_key,
            &configure_key,
            username,
            connected_bridge_id,
        )?;
    }
    // This must run before the generic provider rewrites credentials. The
    // durable marker is the crash-resume handoff between credential and
    // authority-state commits.
    complete_pending_hue_address_migration(state, &configure_key, connected_bridge_id)?;

    crate::provider::configure_hue_hub(address, credentials_json, state, |state| {
        persist_connected_bridge_identity(state, &configure_key, connected_bridge_id)?;
        connect_fn(state)
    })?;
    if hue_authority_should_fence_on_connect(state, &configure_key, connected_bridge_id)? {
        state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .mark_external_controller_authority_pending(&configure_key);
    }
    Ok(())
}

impl HubProvider for ReqwestHueHubProvider {
    fn hub_type(&self) -> HubType {
        HubType::new(HubType::HUE)
    }

    fn validate_backup_credentials(
        &self,
        address: &str,
        credentials: &serde_json::Value,
    ) -> Result<()> {
        let transport = ReqwestHueTransport::new(address)?;
        validate_hue_backup_credentials(&transport, credentials)
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        let configure_key = HubKey::new(HubType::new(HubType::HUE), address);
        let username = submitted_hue_username(credentials_json)?;
        let transport = ReqwestHueTransport::new(address)?;
        let bridge_id = crate::ownership::connected_hue_bridge_id(&transport, &username)?;
        configure_authenticated_hue_hub(
            address,
            credentials_json,
            &username,
            &bridge_id,
            state,
            |state| {
                let snapshot = {
                    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                    s.storage
                        .as_ref()
                        .and_then(|st| st.load_hub_registry_for(&configure_key).ok().flatten())
                        .and_then(|v| serde_json::from_value::<HueRegistrySnapshot>(v).ok())
                };
                connect_hue_sse(state, &configure_key, snapshot)
            },
        )?;

        // Runtime creation is deferred to room sync (do_room_set → commands::ensure_runtime).
        // do_configure_hub triggers auto-sync after this returns.

        Ok(())
    }
}

// ============================================================================
// ExternalLightHubIntegration — static integration for platform crate registries
// ============================================================================

#[derive(Clone)]
struct HueAuthorityContext {
    storage: Arc<dyn Storage>,
    bridge_ip: String,
    username: String,
}

fn hue_authority_context(state: &SharedState, key: &HubKey) -> Result<HueAuthorityContext> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let storage = state
        .storage
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Hue authority requires durable storage"))?;
    let hue = state
        .hubs
        .get(key)
        .and_then(|hub| hub.data::<HueHubData>())
        .ok_or_else(|| anyhow::anyhow!("The Hue controller is not active"))?;
    Ok(HueAuthorityContext {
        storage,
        bridge_ip: hue.bridge_ip.clone(),
        username: hue.username.clone(),
    })
}

fn hue_release_context(state: &SharedState, key: &HubKey) -> Result<Option<HueAuthorityContext>> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let Some(storage) = state.storage.clone() else {
        return Ok(None);
    };
    let Some(credentials) = state.hub_credentials.get(key) else {
        return Ok(None);
    };
    let Some(bridge_id) = credentials
        .get_str("bridge_id")
        .filter(|bridge_id| !bridge_id.trim().is_empty())
    else {
        // Authority acquisition persists stable bridge identity before it
        // creates an ownership manifest. Legacy credentials without that
        // identity cannot safely claim an arbitrary bridge-scoped manifest.
        return Ok(None);
    };
    if crate::ownership::load_controller_ownership(storage.as_ref(), bridge_id)?.is_none() {
        // This credential has no recovery state; another configured Hue
        // bridge may own a different manifest.
        return Ok(None);
    }
    let username = crate::provider::hue_username(credentials)
        .ok_or_else(|| anyhow::anyhow!("Hue recovery credentials are incomplete"))?
        .to_string();
    Ok(Some(HueAuthorityContext {
        storage,
        bridge_ip: credentials.address.clone(),
        username,
    }))
}

fn persist_connected_bridge_identity(
    state: &SharedState,
    key: &HubKey,
    bridge_id: &str,
) -> Result<()> {
    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let previous_data = state
        .hub_credentials
        .get(key)
        .ok_or_else(|| anyhow::anyhow!("Hue credentials are unavailable"))?
        .data
        .clone();
    if let Some(expected) = previous_data
        .get("bridge_id")
        .and_then(serde_json::Value::as_str)
    {
        if expected != bridge_id {
            anyhow::bail!("Stored Hue credentials identify a different physical bridge");
        }
        return Ok(());
    }
    let credentials = state
        .hub_credentials
        .get_mut(key)
        .expect("credentials were checked above");
    let data = credentials
        .data
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Hue credentials have invalid provider data"))?;
    data.insert(
        "bridge_id".to_string(),
        serde_json::Value::String(bridge_id.to_string()),
    );
    if let Some(storage) = state.storage.as_ref() {
        let all_credentials = state.hub_credentials.values().cloned().collect::<Vec<_>>();
        if let Err(error) = storage.save_all_hub_credentials(&all_credentials) {
            state
                .hub_credentials
                .get_mut(key)
                .expect("credentials remain installed")
                .data = previous_data;
            return Err(error.context("Failed to durably bind Hue credentials to the bridge"));
        }
    }
    Ok(())
}

fn verify_and_persist_connected_bridge_identity(
    state: &SharedState,
    key: &HubKey,
) -> Result<String> {
    let (bridge_ip, username) = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let credentials = state
            .hub_credentials
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("No hub credentials configured for {}", key))?;
        let username = crate::provider::hue_username(credentials)
            .ok_or_else(|| anyhow::anyhow!("Hue credentials not configured"))?
            .to_string();
        (credentials.address.clone(), username)
    };
    let transport = ReqwestHueTransport::new(&bridge_ip)?;
    let bridge_id = crate::ownership::connected_hue_bridge_id(&transport, &username)?;
    persist_connected_bridge_identity(state, key, &bridge_id)?;
    Ok(bridge_id)
}

fn ensure_hue_start_phase_is_not_release_fenced(
    phase: Option<crate::ownership::HueOwnershipPhase>,
) -> Result<()> {
    if matches!(
        phase,
        Some(
            crate::ownership::HueOwnershipPhase::Restoring
                | crate::ownership::HueOwnershipPhase::RestoreIncomplete
                | crate::ownership::HueOwnershipPhase::Restored
                | crate::ownership::HueOwnershipPhase::ReleasePending
        )
    ) {
        anyhow::bail!(
            "Hue bridge release is incomplete; retry disconnecting Hue before reconnecting it"
        );
    }
    Ok(())
}

fn ensure_hue_start_is_not_release_fenced(state: &SharedState, bridge_id: &str) -> Result<()> {
    let storage = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .storage
        .clone();
    let phase = match storage {
        Some(storage) => crate::ownership::load_controller_ownership(storage.as_ref(), bridge_id)?
            .map(|ownership| ownership.phase),
        None => None,
    };
    ensure_hue_start_phase_is_not_release_fenced(phase)
}

fn hue_authority_should_fence_on_connect(
    state: &SharedState,
    key: &HubKey,
    bridge_id: &str,
) -> Result<bool> {
    let (rollout_enabled, storage) = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            state.external_controller_authority_is_enabled_for(key),
            state.storage.clone(),
        )
    };
    if rollout_enabled {
        return Ok(true);
    }

    let Some(storage) = storage else {
        return Ok(false);
    };
    // Disabling new acquisition must not make an ownership epoch created by
    // an earlier build look unmanaged. Preserve its write fence so release or
    // staff recovery can still run without opening ordinary light routing.
    Ok(crate::ownership::load_controller_ownership(storage.as_ref(), bridge_id)?.is_some())
}

fn start_hue_after_release_fence<T>(
    state: &SharedState,
    bridge_id: &str,
    start: impl FnOnce() -> Result<T>,
) -> Result<T> {
    ensure_hue_start_is_not_release_fenced(state, bridge_id)?;
    start()
}

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

    fn requires_grouped_room_control(&self) -> bool {
        false
    }

    fn requires_external_controller_authority(&self) -> bool {
        true
    }

    fn sync_topology_groups(&self, _state: &SharedState, _key: &HubKey) -> Result<()> {
        // Hue discovery remains authoritative. Ordinary sync never infers
        // native room identity from a local name; explicit user mutations use
        // the dedicated source-room lifecycle hooks below.
        Ok(())
    }

    fn delete_source_room(
        &self,
        state: &SharedState,
        binding: &rhythm_os::topology::HubRoomBinding,
    ) -> Result<()> {
        let context = hue_authority_context(state, &binding.hub_key)?;
        let transport = ReqwestHueTransport::new(&context.bridge_ip)?;
        let bridge_id = crate::ownership::connected_hue_bridge_id(&transport, &context.username)?;
        let operation_lock = crate::ownership::controller_operation_lock(&bridge_id);
        let _operation = operation_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock Hue controller operations"))?;
        crate::managed_rooms::delete_source_room(
            &transport,
            &context.username,
            &binding.hub_room_id,
        )
    }

    fn rename_device(
        &self,
        state: &SharedState,
        key: &HubKey,
        native_device_id: &str,
        name: &str,
    ) -> Result<()> {
        let context = hue_authority_context(state, key)?;
        let transport = ReqwestHueTransport::new(&context.bridge_ip)?;
        let bridge_id = crate::ownership::connected_hue_bridge_id(&transport, &context.username)?;
        let operation_lock = crate::ownership::controller_operation_lock(&bridge_id);
        let _operation = operation_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock Hue controller operations"))?;
        transport.rename_device(&context.username, native_device_id, name)
    }

    fn reconcile_external_controller_authority(
        &self,
        state: &SharedState,
        key: &HubKey,
    ) -> Result<()> {
        let context = hue_authority_context(state, key)?;
        let transport = ReqwestHueTransport::new(&context.bridge_ip)?;
        let bridge_id = crate::ownership::connected_hue_bridge_id(&transport, &context.username)?;
        persist_connected_bridge_identity(state, key, &bridge_id)?;
        if crate::ownership::load_controller_ownership(context.storage.as_ref(), &bridge_id)?
            .is_some_and(|ownership| {
                matches!(
                    ownership.phase,
                    crate::ownership::HueOwnershipPhase::Restoring
                        | crate::ownership::HueOwnershipPhase::RestoreIncomplete
                        | crate::ownership::HueOwnershipPhase::Restored
                        | crate::ownership::HueOwnershipPhase::ReleasePending
                )
            })
        {
            anyhow::bail!("Hue bridge release has started; takeover cannot be resumed");
        }
        let operation_lock = crate::ownership::controller_operation_lock(&bridge_id);
        let _operation = operation_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock Hue controller operations"))?;
        let _ = crate::ownership::acquire_authoritative_control_best_effort(
            context.storage.as_ref(),
            key,
            &transport,
            &context.username,
        );
        Ok(())
    }

    fn release_external_controller_authority(
        &self,
        state: &SharedState,
        key: &HubKey,
        _reason: ExternalControllerReleaseReason,
    ) -> Result<()> {
        let Some(context) = hue_release_context(state, key)? else {
            return Ok(());
        };
        let transport = ReqwestHueTransport::new(&context.bridge_ip)?;
        let bridge_id = crate::ownership::connected_hue_bridge_id(&transport, &context.username)?;
        let operation_lock = crate::ownership::controller_operation_lock(&bridge_id);
        let _operation = operation_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock Hue controller operations"))?;
        let Some(ownership) = crate::ownership::release_authoritative_control(
            context.storage.as_ref(),
            key,
            &transport,
            &context.username,
        )?
        else {
            return Ok(());
        };
        if ownership.phase != crate::ownership::HueOwnershipPhase::ReleasePending {
            anyhow::bail!("Hue controller release did not retain its captured snapshot");
        }
        // The retained snapshot is the release result. The command layer may
        // now remove credentials without any restoration write to the bridge.
        Ok(())
    }

    fn finalize_external_controller_release(
        &self,
        state: &SharedState,
        key: &HubKey,
    ) -> Result<()> {
        let (storage, bridge_id) = {
            let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let Some(storage) = state.storage.clone() else {
                return Ok(());
            };
            let Some(credentials) = state.hub_credentials.get(key) else {
                return Ok(());
            };
            let Some(bridge_id) = credentials
                .get_str("bridge_id")
                .filter(|bridge_id| !bridge_id.trim().is_empty())
            else {
                return Ok(());
            };
            (storage, bridge_id.to_string())
        };
        let Some(_ownership) =
            crate::ownership::load_controller_ownership(storage.as_ref(), &bridge_id)?
        else {
            return Ok(());
        };
        crate::ownership::finalize_released_control(storage.as_ref(), &bridge_id)
    }

    fn api_capabilities(&self) -> HubIntegrationCapability {
        HubIntegrationCapability {
            hub_type: HubType::HUE.to_string(),
            configurable: true,
            device_onboarding_methods: vec![
                DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_SERIAL_SEARCH.to_string(),
                DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_BUTTON_SEARCH.to_string(),
            ],
            device_profiles: Vec::new(),
            supports_unpairing: true,
            unpairable_device_types: vec![
                "light".to_string(),
                "button".to_string(),
                "motion".to_string(),
            ],
            supports_roomless_devices: true,
            blocks_room_readiness: true,
        }
    }

    fn start_pairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<PairingSession> {
        let device_kind = params
            .get("device_kind")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("light");
        if device_kind == "button" {
            return start_bridge_button_pairing(state, params);
        }
        if device_kind != "light" {
            anyhow::bail!("Unsupported Hue Bridge pairing device kind");
        }
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
                            Ok(_) => match exact_bridge_device_projection(
                                bridge_devices(
                                    state,
                                    &key,
                                    rhythm_core::runtime::hub_registry::DeviceType::Light,
                                ),
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
        reconcile_paired_light_names(state, &key, &mut devices)?;
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
                let bridge_id =
                    crate::ownership::connected_hue_bridge_id(&transport, &target.username)?;
                let operation_lock = crate::ownership::controller_operation_lock(&bridge_id);
                let _operation = operation_lock
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Failed to lock Hue controller operations"))?;
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
                    let storage = state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("lock"))?
                        .storage
                        .clone();
                    ensure_physical_hue_unpair_allowed(storage.as_deref(), &bridge_id)?;
                    transport.remove_device(&target.username, &target.native_id)?;
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
        _state: &SharedState,
        _assignment: &HubDeviceRoomAssignment,
    ) -> Result<HubDeviceRoomAssignmentOutcome> {
        Ok(HubDeviceRoomAssignmentOutcome::Unchanged)
    }
}

#[cfg(test)]
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

fn start_bridge_button_pairing(
    state: &SharedState,
    params: &serde_json::Value,
) -> Result<PairingSession> {
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
        "Asking the Hue Bridge to search for a button or switch",
        None,
        None,
    );
    let transport = ReqwestHueTransport::new(&bridge_ip)?;
    let found = transport.search_new_sensors(&username)?;
    if found.is_empty() {
        anyhow::bail!(
            "The Hue Bridge did not find a new button or switch. Put the accessory in pairing mode and try again"
        );
    }

    rhythm_os::pairing::emit_pairing_progress(
        state,
        HubType::HUE,
        session_id,
        PairingStatus::Commissioning,
        PairingStage::Finalizing,
        "Hue Bridge search finished; confirming the new button or switch",
        None,
        None,
    );
    let found_ids = found
        .iter()
        .map(|sensor| sensor.legacy_id.clone())
        .collect::<BTreeSet<_>>();
    let mut projected_devices = None;
    let mut projection_error = "Hue V2 has not projected a button resource".to_string();
    let projection_deadline = Instant::now() + HUE_BUTTON_PROJECTION_TIMEOUT;
    const PROJECTION_ATTEMPTS: usize = 15;
    for attempt in 0..PROJECTION_ATTEMPTS {
        match transport
            .get_resources(&username, "button")
            .and_then(|resources| v2_device_ids_for_legacy_buttons(&resources, &found_ids))
        {
            Ok(mapped) if mapped.is_empty() => {
                projection_error =
                    "The Hue Bridge found sensors, but none exposed a button service".to_string();
            }
            Ok(mapped) => {
                let expected_device_ids = mapped.into_values().collect::<BTreeSet<_>>();
                match rhythm_os::room_sync::sync_from_hub_for_key_wait(
                    state,
                    &key,
                    true,
                    HUE_BUTTON_SYNC_SLOT_TIMEOUT,
                ) {
                    Ok(_) => match exact_bridge_device_projection(
                        bridge_devices(
                            state,
                            &key,
                            rhythm_core::runtime::hub_registry::DeviceType::Button,
                        ),
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
            }
            Err(error) => {
                projection_error = format!("Hue V2 button projection lookup failed: {error:#}");
            }
        }
        if attempt + 1 < PROJECTION_ATTEMPTS && Instant::now() < projection_deadline {
            std::thread::sleep(Duration::from_secs(1));
        } else {
            break;
        }
    }
    let mut devices = projected_devices.ok_or_else(|| {
        anyhow::anyhow!(
            "Hue Bridge search completed, but Rhythm could not confirm an exact button or switch after refresh: {projection_error}"
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
        0 => anyhow::bail!("Connect a Hue Bridge before adding a device"),
        1 => Ok(candidates.pop().expect("one candidate")),
        _ => anyhow::bail!(
            "Multiple Hue Bridges are connected; provide hub_address for the bridge that should add the device"
        ),
    }
}

fn legacy_id_from_v2_id_v1(id_v1: &str) -> Option<&str> {
    let id = id_v1.strip_prefix("/lights/")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

fn legacy_sensor_id_from_v2_id_v1(id_v1: &str) -> Option<&str> {
    let id = id_v1.strip_prefix("/sensors/")?;
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

fn v2_device_ids_for_legacy_buttons(
    value: &serde_json::Value,
    requested_legacy_ids: &BTreeSet<String>,
) -> Result<BTreeMap<String, String>> {
    let errors = value
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("GET button returned an invalid Hue V2 response"))?;
    if !errors.is_empty() {
        anyhow::bail!("GET button returned {} Hue error(s)", errors.len());
    }
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("GET button returned a Hue V2 response without data"))?;

    let mut mapped = BTreeMap::new();
    for button in data {
        let Some(legacy_id) = button
            .get("id_v1")
            .and_then(serde_json::Value::as_str)
            .and_then(legacy_sensor_id_from_v2_id_v1)
        else {
            continue;
        };
        if !requested_legacy_ids.contains(legacy_id) {
            continue;
        }
        if button
            .pointer("/owner/rtype")
            .and_then(serde_json::Value::as_str)
            != Some("device")
        {
            continue;
        }
        let Some(device_id) = button
            .pointer("/owner/rid")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if let Some(previous) = mapped.insert(legacy_id.to_string(), device_id.to_string()) {
            if previous != device_id {
                anyhow::bail!("Hue V1 sensor {legacy_id} maps to multiple Hue V2 button devices");
            }
        }
    }
    Ok(mapped)
}

fn exact_bridge_device_projection(
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

fn ensure_physical_hue_unpair_allowed(
    storage: Option<&dyn Storage>,
    bridge_id: &str,
) -> Result<()> {
    let Some(storage) = storage else {
        return Ok(());
    };
    if crate::ownership::load_controller_ownership(storage, bridge_id)?.is_some() {
        anyhow::bail!("Release Rhythm control of this Hue Bridge before removing a Bridge device");
    }
    Ok(())
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
        [] => anyhow::bail!("Connect a Hue Bridge before removing a Bridge device"),
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

fn bridge_devices(
    state: &SharedState,
    key: &HubKey,
    device_type: rhythm_core::runtime::hub_registry::DeviceType,
) -> Vec<PairedDeviceInfo> {
    let Ok(state) = state.lock() else {
        return Vec::new();
    };
    state
        .canonical_registry
        .devices()
        .filter(|device| !device.is_removed() && device.device_type == device_type)
        .filter_map(|device| {
            let endpoint = device
                .active_endpoints()
                .find(|endpoint| &endpoint.hub_key == key)?;
            Some(PairedDeviceInfo {
                device_id: endpoint.native_id.clone(),
                name: device.name.clone(),
                device_type: device.device_type.clone(),
                manufacturer: device.manufacturer.clone(),
                model: device.model.clone(),
            })
        })
        .collect()
}

fn reconcile_paired_light_names(
    state: &SharedState,
    key: &HubKey,
    devices: &mut [PairedDeviceInfo],
) -> Result<()> {
    let transaction_lock = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .external_topology_transaction_lock
        .clone();
    let _transaction = transaction_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("External topology transaction lock poisoned"))?;
    let mut scope = rhythm_os::device_naming::LightNameReconciliationScope::default();
    {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        for device in devices.iter() {
            if let Some(canonical) = state
                .canonical_registry
                .find_by_native_id(key, &device.device_id)
            {
                scope.include_device(canonical.id.clone());
            }
        }
    }
    rhythm_os::device_naming::reconcile_automatic_light_names(state, &scope)?;

    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    for device in devices {
        if let Some(canonical) = state
            .canonical_registry
            .find_by_native_id(key, &device.device_id)
        {
            device.name = canonical.name.clone();
        }
    }
    Ok(())
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
                let transport = Arc::new(transport);
                // Discovery remains bridge-native even while Rhythm suppresses
                // Hue automations. Ownership manifests are recovery/audit data,
                // never a room allowlist or permission to project scenes.
                hub.discovery = Some(Arc::new(crate::discovery::HueDiscovery::new(
                    transport, username,
                )));
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

    use std::sync::atomic::{AtomicUsize, Ordering};

    use rhythm_core::{
        InputEvent, LightProfileConfig, LightingCommand, ModeConfig, RestoredRoomState,
        RoomSnapshot, RuntimeHandle, SolarTime,
    };
    use rhythm_os::hub::HubCredentials;
    use rhythm_os::state::AppState;
    use rhythm_os::storage::{
        FileStorage, StoredAuthorityState, StoredLightProfiles, StoredLocation, StoredSettings,
    };
    use rhythm_os::topology::HubRoomBinding;

    use crate::provider::hue_credentials;
    use crate::test_support::SpyHueTransport;

    struct ToggleAuthorityStorage {
        inner: FileStorage,
        fail_authority_save: AtomicBool,
    }

    impl ToggleAuthorityStorage {
        fn new(path: &std::path::Path) -> Self {
            Self {
                inner: FileStorage::new(path.to_str().unwrap()).unwrap(),
                fail_authority_save: AtomicBool::new(false),
            }
        }

        fn set_fail_authority_save(&self, fail: bool) {
            self.fail_authority_save.store(fail, Ordering::SeqCst);
        }
    }

    impl Storage for ToggleAuthorityStorage {
        fn load_rooms(&self) -> Result<rhythm_core::room::RoomManager> {
            self.inner.load_rooms()
        }

        fn save_rooms(&self, rooms: &rhythm_core::room::RoomManager) -> Result<()> {
            self.inner.save_rooms(rooms)
        }

        fn load_light_profiles(&self) -> Result<StoredLightProfiles> {
            self.inner.load_light_profiles()
        }

        fn save_light_profiles(&self, config: &StoredLightProfiles) -> Result<()> {
            self.inner.save_light_profiles(config)
        }

        fn load_location(&self) -> Result<StoredLocation> {
            self.inner.load_location()
        }

        fn save_location(&self, location: &StoredLocation) -> Result<()> {
            self.inner.save_location(location)
        }

        fn load_settings(&self) -> Result<StoredSettings> {
            self.inner.load_settings()
        }

        fn save_settings(&self, settings: &StoredSettings) -> Result<()> {
            self.inner.save_settings(settings)
        }

        fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>> {
            self.inner.load_all_hub_credentials()
        }

        fn save_all_hub_credentials(&self, credentials: &[HubCredentials]) -> Result<()> {
            self.inner.save_all_hub_credentials(credentials)
        }

        fn load_hub_registry_for(&self, key: &HubKey) -> Result<Option<serde_json::Value>> {
            self.inner.load_hub_registry_for(key)
        }

        fn save_hub_registry_for(&self, key: &HubKey, data: &serde_json::Value) -> Result<()> {
            self.inner.save_hub_registry_for(key, data)
        }

        fn load_authority_state(&self) -> Result<Option<StoredAuthorityState>> {
            self.inner.load_authority_state()
        }

        fn save_authority_state(&self, authority: &StoredAuthorityState) -> Result<()> {
            if self.fail_authority_save.load(Ordering::SeqCst) {
                anyhow::bail!("injected authority-state commit failure");
            }
            self.inner.save_authority_state(authority)
        }
    }

    #[test]
    fn backup_credentials_are_bound_to_the_live_physical_bridge() {
        let transport = SpyHueTransport::new();
        transport.set_resource_response(
            "bridge",
            serde_json::json!({"data": [{"id": "bridge-1"}], "errors": []}),
        );

        validate_hue_backup_credentials(
            &transport,
            &serde_json::json!({"username": "user-1", "bridge_id": "bridge-1"}),
        )
        .unwrap();

        let error = validate_hue_backup_credentials(
            &transport,
            &serde_json::json!({"username": "user-1", "bridge_id": "bridge-2"}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("different physical bridge"));
        assert!(!error.to_string().contains("bridge-1"));
        assert!(!error.to_string().contains("bridge-2"));
    }

    #[test]
    fn same_key_credential_rotation_requires_the_authenticated_bridge_identity() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        let mut existing = hue_credentials("192.0.2.10", "old-user");
        existing.data["bridge_id"] = serde_json::json!("bridge-stable");
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), existing);

        let error =
            preflight_authenticated_hue_configuration(&state, &key, "old-user", "bridge-other")
                .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Submitted Hue credentials identify a different physical bridge"
        );
        assert_eq!(
            state
                .lock()
                .unwrap()
                .hub_credentials
                .get(&key)
                .and_then(crate::provider::hue_username),
            Some("old-user")
        );

        assert_eq!(
            preflight_authenticated_hue_configuration(&state, &key, "old-user", "bridge-stable",)
                .unwrap(),
            Some(key)
        );
    }

    #[test]
    fn legacy_same_key_rejects_username_change_without_stable_identity() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), hue_credentials(&key.address, "legacy-user"));

        let error = configure_authenticated_hue_hub(
            &key.address,
            r#"{"username":"replacement-user"}"#,
            "replacement-user",
            "replacement-bridge",
            &state,
            |_| panic!("legacy identity mismatch must not connect"),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("legacy Hue configuration could not be safely matched"));
        assert_eq!(
            state
                .lock()
                .unwrap()
                .hub_credentials
                .get(&key)
                .and_then(crate::provider::hue_username),
            Some("legacy-user")
        );
    }

    #[test]
    fn unbacked_hue_authority_graph_blocks_a_second_configuration_key() {
        let state = shared_state();
        let stale_key = hue_key("192.0.2.10");
        let new_key = hue_key("192.0.2.11");
        add_hue_light_endpoint(&state, &stale_key, "stale-light");

        let error = configure_authenticated_hue_hub(
            &new_key.address,
            r#"{"username":"new-user"}"#,
            "new-user",
            "new-bridge",
            &state,
            |_| panic!("unbacked topology must not be rebound implicitly"),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("topology has no matching credential identity"));
        let state = state.lock().unwrap();
        assert!(state.hub_credentials.is_empty());
        assert!(state.canonical_registry.references_hub_key(&stale_key));
        assert!(!state.canonical_registry.references_hub_key(&new_key));
    }

    #[test]
    fn hue_start_phase_fence_blocks_release_phases_only() {
        use crate::ownership::HueOwnershipPhase;

        for phase in [
            HueOwnershipPhase::Restoring,
            HueOwnershipPhase::RestoreIncomplete,
            HueOwnershipPhase::Restored,
            HueOwnershipPhase::ReleasePending,
        ] {
            let error = ensure_hue_start_phase_is_not_release_fenced(Some(phase)).unwrap_err();
            assert_eq!(
                error.to_string(),
                "Hue bridge release is incomplete; retry disconnecting Hue before reconnecting it"
            );
        }

        for phase in [
            HueOwnershipPhase::Captured,
            HueOwnershipPhase::Clearing,
            HueOwnershipPhase::ClearIncomplete,
            HueOwnershipPhase::Active,
            HueOwnershipPhase::SnapshotRetained,
        ] {
            ensure_hue_start_phase_is_not_release_fenced(Some(phase)).unwrap();
        }
        ensure_hue_start_phase_is_not_release_fenced(None).unwrap();
    }

    #[test]
    fn disabled_rollout_fences_only_an_existing_ownership_epoch() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-rollout-fence-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = Arc::new(FileStorage::new(path.to_str().unwrap()).unwrap());
        let key = HubKey::new(HubType::new(HubType::HUE), "bridge.local");
        let mut app = AppState::default();
        app.storage = Some(storage.clone());
        let state = Arc::new(Mutex::new(app));

        assert!(!hue_authority_should_fence_on_connect(&state, &key, "bridge-rollout").unwrap());

        state
            .lock()
            .unwrap()
            .set_external_controller_authority_enabled(key.hub_type.clone(), true);
        assert!(hue_authority_should_fence_on_connect(&state, &key, "bridge-rollout").unwrap());

        state
            .lock()
            .unwrap()
            .set_external_controller_authority_enabled(key.hub_type.clone(), false);
        let manifest = serde_json::json!({
            "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
            "phase": "active",
            "baseline": {
                "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
                "capture_id": "capture-rollout",
                "bridge_id": "bridge-rollout",
                "v2_resources": {},
                "v1_resources": {}
            },
            "managed_rooms": {},
            "managed_scenes": {},
            "restored_resource_ids": {},
            "receipts": {}
        });
        storage
            .save_integration_state_file(
                &crate::ownership::hue_controller_ownership_path("bridge-rollout").unwrap(),
                &serde_json::to_string(&manifest).unwrap(),
            )
            .unwrap();

        assert!(hue_authority_should_fence_on_connect(&state, &key, "bridge-rollout").unwrap());

        drop(state);
        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn release_context_requires_the_credentials_exact_bridge_manifest() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-exact-release-manifest-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = Arc::new(FileStorage::new(path.to_str().unwrap()).unwrap());
        let key = hue_key("192.0.2.10");
        let write_manifest = |bridge_id: &str| {
            let manifest = serde_json::json!({
                "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
                "phase": "active",
                "baseline": {
                    "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
                    "capture_id": format!("capture-{bridge_id}"),
                    "bridge_id": bridge_id,
                    "v2_resources": {},
                    "v1_resources": {}
                },
                "managed_rooms": {},
                "managed_scenes": {},
                "restored_resource_ids": {},
                "receipts": {}
            });
            storage
                .save_integration_state_file(
                    &crate::ownership::hue_controller_ownership_path(bridge_id).unwrap(),
                    &serde_json::to_string(&manifest).unwrap(),
                )
                .unwrap();
        };
        write_manifest("bridge-other");

        let mut app = AppState {
            storage: Some(storage.clone()),
            ..Default::default()
        };
        app.hub_credentials
            .insert(key.clone(), hue_credentials(&key.address, "release-user"));
        let state = Arc::new(Mutex::new(app));

        assert!(hue_release_context(&state, &key).unwrap().is_none());
        state
            .lock()
            .unwrap()
            .hub_credentials
            .get_mut(&key)
            .unwrap()
            .data["bridge_id"] = serde_json::json!("bridge-expected");
        assert!(hue_release_context(&state, &key).unwrap().is_none());

        write_manifest("bridge-expected");
        let context = hue_release_context(&state, &key)
            .unwrap()
            .expect("exact bridge manifest should enable release");
        assert_eq!(context.bridge_ip, key.address);
        assert_eq!(context.username, "release-user");

        drop(context);
        drop(state);
        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn release_finalization_noops_without_an_exact_bridge_manifest() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-optional-release-finalization-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let key = hue_key("192.0.2.10");
        let state = shared_state();

        INTEGRATION
            .finalize_external_controller_release(&state, &key)
            .unwrap();

        let storage = Arc::new(FileStorage::new(path.to_str().unwrap()).unwrap());
        state.lock().unwrap().storage = Some(storage.clone());
        INTEGRATION
            .finalize_external_controller_release(&state, &key)
            .unwrap();

        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), hue_credentials(&key.address, "release-user"));
        INTEGRATION
            .finalize_external_controller_release(&state, &key)
            .unwrap();

        state
            .lock()
            .unwrap()
            .hub_credentials
            .get_mut(&key)
            .unwrap()
            .data["bridge_id"] = serde_json::json!("bridge-without-manifest");
        INTEGRATION
            .finalize_external_controller_release(&state, &key)
            .unwrap();

        drop(state);
        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn binary_rollback_preserves_credentialless_retained_snapshot() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-credentialless-rollback-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
        let manifest = serde_json::json!({
            "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
            "phase": "snapshot_retained",
            "baseline": {
                "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
                "capture_id": "capture-credentialless-rollback",
                "bridge_id": "bridge-credentialless-rollback",
                "v2_resources": {},
                "v1_resources": {}
            },
            "managed_rooms": {},
            "managed_scenes": {},
            "restored_resource_ids": {},
            "receipts": {}
        });
        storage
            .save_integration_state_file(
                &crate::ownership::hue_controller_ownership_path("bridge-credentialless-rollback")
                    .unwrap(),
                &serde_json::to_string(&manifest).unwrap(),
            )
            .unwrap();
        assert!(storage.load_all_hub_credentials().unwrap().is_empty());

        restore_authoritative_bridges_before_binary_rollback(&path).unwrap();

        let retained =
            crate::ownership::load_controller_ownership(&storage, "bridge-credentialless-rollback")
                .unwrap()
                .unwrap();
        assert_eq!(
            retained.phase,
            crate::ownership::HueOwnershipPhase::SnapshotRetained
        );
        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn binary_rollback_blocks_active_snapshot_without_restoring_bridge() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-active-rollback-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
        let manifest = serde_json::json!({
            "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
            "phase": "active",
            "baseline": {
                "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
                "capture_id": "capture-active-rollback",
                "bridge_id": "bridge-active-rollback",
                "v2_resources": {},
                "v1_resources": {}
            },
            "managed_rooms": {},
            "managed_scenes": {},
            "receipts": {}
        });
        storage
            .save_integration_state_file(
                &crate::ownership::hue_controller_ownership_path("bridge-active-rollback").unwrap(),
                &serde_json::to_string(&manifest).unwrap(),
            )
            .unwrap();

        let error = restore_authoritative_bridges_before_binary_rollback(&path).unwrap_err();

        assert!(error.to_string().contains("active controller authority"));
        assert!(
            crate::ownership::load_controller_ownership(&storage, "bridge-active-rollback")
                .unwrap()
                .is_some()
        );
        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn durable_release_fence_prevents_hue_start_and_preserves_credentials() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-start-release-fence-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = Arc::new(FileStorage::new(path.to_str().unwrap()).unwrap());
        let manifest = serde_json::json!({
            "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
            "phase": "restored",
            "baseline": {
                "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
                "capture_id": "capture-1",
                "bridge_id": "bridge-1",
                "v2_resources": {},
                "v1_resources": {}
            },
            "managed_rooms": {},
            "managed_scenes": {},
            "restored_resource_ids": {},
            "receipts": {}
        });
        storage
            .save_integration_state_file(
                &crate::ownership::hue_controller_ownership_path("bridge-1").unwrap(),
                &serde_json::to_string(&manifest).unwrap(),
            )
            .unwrap();

        let state = shared_state();
        let key = hue_key("192.0.2.10");
        {
            let mut guard = state.lock().unwrap();
            let mut credentials = hue_credentials("192.0.2.10", "user-123");
            credentials.data["bridge_id"] = serde_json::json!("bridge-1");
            guard.hub_credentials.insert(key.clone(), credentials);
            guard.storage = Some(storage.clone());
        }
        let started = AtomicBool::new(false);
        let configure_error = configure_authenticated_hue_hub(
            &key.address,
            r#"{"username":"user-123"}"#,
            "user-123",
            "bridge-1",
            &state,
            |_| panic!("release-fenced configure must not connect"),
        )
        .unwrap_err();
        assert!(configure_error
            .to_string()
            .contains("retry disconnecting Hue"));
        let error = start_hue_after_release_fence(&state, "bridge-1", || {
            started.store(true, Ordering::Relaxed);
            Ok(())
        })
        .unwrap_err();

        assert!(error.to_string().contains("retry disconnecting Hue"));
        assert!(!started.load(Ordering::Relaxed));
        let guard = state.lock().unwrap();
        assert!(guard.hubs.is_empty());
        assert_eq!(
            guard
                .hub_credentials
                .get(&key)
                .and_then(|credentials| credentials.get_str("bridge_id")),
            Some("bridge-1")
        );
        drop(guard);
        drop(state);
        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn pending_same_key_controller_blocks_reconfiguration_without_a_manifest() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        {
            let mut guard = state.lock().unwrap();
            guard
                .hub_credentials
                .insert(key.clone(), hue_credentials("192.0.2.10", "user-123"));
            guard.mark_external_controller_authority_pending(&key);
        }

        let error = configure_authenticated_hue_hub(
            &key.address,
            r#"{"username":"user-123"}"#,
            "user-123",
            "bridge-1",
            &state,
            |_| panic!("pending configure must not connect"),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Hue controller authority is transitioning; retry after disconnect completes"
        );
        assert_eq!(
            state
                .lock()
                .unwrap()
                .hub_credentials
                .get(&key)
                .and_then(crate::provider::hue_username),
            Some("user-123")
        );
    }

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
            vec![
                DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_SERIAL_SEARCH,
                DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_BUTTON_SEARCH,
            ]
        );
        assert!(caps.supports_unpairing);
        assert_eq!(
            caps.unpairable_device_types,
            vec!["light", "button", "motion"]
        );
        assert!(caps.supports_roomless_devices);
    }

    #[test]
    fn hue_authority_is_decoupled_from_native_topology_ownership() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        {
            let mut app = state.lock().unwrap();
            let mut room = rhythm_os::topology::TopologyRoom::new("rhythm-bedroom", "Bedroom");
            room.hub_room_bindings.push(HubRoomBinding {
                hub_key: key.clone(),
                hub_room_id: "hue-bedroom".to_string(),
                control_id: "hue-bedroom-group".to_string(),
                light_device_ids: vec!["hue-light-1".to_string()],
            });
            app.topology.insert_room(room);
        }
        let topology_before = {
            let app = state.lock().unwrap();
            serde_json::to_value(&app.topology).unwrap()
        };

        assert!(!INTEGRATION.requires_grouped_room_control());
        assert!(INTEGRATION.requires_external_controller_authority());
        INTEGRATION.sync_topology_groups(&state, &key).unwrap();
        let outcome = INTEGRATION
            .prepare_device_room_assignment(
                &state,
                &HubDeviceRoomAssignment {
                    hub_key: key,
                    native_device_id: "hue-light-1".to_string(),
                    device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                    preferred_for_control: true,
                    target_rhythm_room_id: Some("rhythm-bedroom".to_string()),
                    target_hub_room_ids: vec!["hue-bedroom".to_string()],
                },
            )
            .unwrap();

        assert!(matches!(outcome, HubDeviceRoomAssignmentOutcome::Unchanged));
        let topology_after = {
            let app = state.lock().unwrap();
            serde_json::to_value(&app.topology).unwrap()
        };
        assert_eq!(topology_after, topology_before);
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
    fn bridge_button_pairing_maps_only_exact_new_sensor_ids_to_button_owners() {
        let requested = BTreeSet::from(["21".to_string(), "22".to_string()]);
        let mapped = v2_device_ids_for_legacy_buttons(
            &serde_json::json!({
                "errors": [],
                "data": [
                    {
                        "id": "button-a",
                        "id_v1": "/sensors/21",
                        "owner": {"rtype": "device", "rid": "device-a"}
                    },
                    {
                        "id": "button-unrelated",
                        "id_v1": "/sensors/23",
                        "owner": {"rtype": "device", "rid": "device-unrelated"}
                    },
                    {
                        "id": "malformed-owner",
                        "id_v1": "/sensors/22",
                        "owner": {"rtype": "room", "rid": "room-a"}
                    }
                ]
            }),
            &requested,
        )
        .unwrap();
        assert_eq!(
            mapped,
            BTreeMap::from([("21".to_string(), "device-a".to_string())])
        );
        assert!(!mapped.contains_key("22"));
        assert!(!mapped.contains_key("23"));
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
        assert!(exact_bridge_device_projection(vec![unrelated], &expected).is_err());
        assert!(exact_bridge_device_projection(Vec::new(), &BTreeSet::new()).is_err());

        let exact = PairedDeviceInfo {
            device_id: "device-a".to_string(),
            name: "New lamp".to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            manufacturer: Some("Signify".to_string()),
            model: Some("LCA009".to_string()),
        };
        let projected = exact_bridge_device_projection(vec![exact.clone()], &expected).unwrap();
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
    fn pairing_name_reconciliation_only_touches_projected_devices() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        let paired_id = add_hue_light_endpoint(&state, &key, "hue-paired");
        let unrelated_id = add_hue_light_endpoint(&state, &key, "hue-unrelated");
        let renamed_native_ids = Arc::new(Mutex::new(Vec::<String>::new()));
        state.lock().unwrap().rename_hub_device_fn = Some(Arc::new({
            let renamed_native_ids = renamed_native_ids.clone();
            move |_, _, native_id, _| {
                renamed_native_ids
                    .lock()
                    .unwrap()
                    .push(native_id.to_string());
                Ok(())
            }
        }));
        let mut paired_devices = bridge_light_devices(&state, &key)
            .into_iter()
            .filter(|device| device.device_id == "hue-paired")
            .collect::<Vec<_>>();

        reconcile_paired_light_names(&state, &key, &mut paired_devices).unwrap();

        let state = state.lock().unwrap();
        assert!(state
            .canonical_registry
            .get(&paired_id)
            .unwrap()
            .name
            .starts_with("Hue Zig "));
        assert_eq!(
            state.canonical_registry.get(&unrelated_id).unwrap().name,
            "Test Hue lamp"
        );
        assert!(paired_devices[0].name.starts_with("Hue Zig "));
        assert_eq!(
            renamed_native_ids.lock().unwrap().as_slice(),
            ["hue-paired"]
        );
    }

    fn install_address_migration_graph(state: &SharedState, old_key: &HubKey) -> (String, String) {
        let canonical_id = add_hue_light_endpoint(state, old_key, "hue-light-1");
        let room_id = {
            let mut state = state.lock().unwrap();
            let room_id = state.topology.create_room("Office");
            assert!(state
                .topology
                .attach_device_user_override(&room_id, &canonical_id));
            assert!(state.topology.upsert_managed_room_binding(
                &room_id,
                HubRoomBinding {
                    hub_key: old_key.clone(),
                    hub_room_id: "hue-office".to_string(),
                    control_id: "grouped-office".to_string(),
                    light_device_ids: vec!["hue-light-1".to_string()],
                },
            ));
            state
                .topology
                .set_grouped_room_control_required(old_key, true);
            state.set_external_controller_authority_required(old_key, true);
            rhythm_os::commands::save_authority_state(&state).unwrap();
            room_id
        };
        (canonical_id, room_id)
    }

    #[test]
    fn authenticated_address_migration_replaces_active_old_key_and_group_routes() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-address-migration-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = Arc::new(FileStorage::new(path.to_str().unwrap()).unwrap());
        let state = shared_state();
        let old_key = hue_key("192.0.2.10");
        let new_key = hue_key("192.0.2.11");
        let mut old_credentials = hue_credentials(&old_key.address, "old-user");
        old_credentials.data["bridge_id"] = serde_json::json!("bridge-stable");
        storage
            .save_all_hub_credentials(&[old_credentials.clone()])
            .unwrap();

        let ownership_path =
            crate::ownership::hue_controller_ownership_path("bridge-stable").unwrap();
        let ownership_json = serde_json::to_string(&serde_json::json!({
            "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
            "phase": "active",
            "baseline": {
                "schema_version": crate::ownership::HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
                "capture_id": "capture-stable",
                "bridge_id": "bridge-stable",
                "v2_resources": {},
                "v1_resources": {}
            },
            "managed_rooms": {},
            "managed_scenes": {},
            "restored_resource_ids": {},
            "receipts": {}
        }))
        .unwrap();
        storage
            .save_integration_state_file(&ownership_path, &ownership_json)
            .unwrap();

        let old_hub = active_hue_hub(old_key.clone());
        let old_shutdown = old_hub.shutdown.clone();
        let composite = Arc::new(rhythm_core::CompositeController::new());
        composite.register_controller(
            &old_key.to_string(),
            Arc::new(rhythm_core::NoOpController::new()),
        );
        {
            let mut state = state.lock().unwrap();
            state.storage = Some(storage.clone());
            state
                .hub_credentials
                .insert(old_key.clone(), old_credentials);
            state.hubs.insert(old_key.clone(), old_hub);
            state.set_hub_connected(&old_key, true);
            state.note_hub_connected_event(&old_key);
            state.note_hub_reconnect_sync(&old_key);
            state.note_hub_pending_disconnect(&old_key, Duration::from_secs(30));
            state.composite_controller = Some(composite.clone());
        }
        let (canonical_id, room_id) = install_address_migration_graph(&state, &old_key);

        configure_authenticated_hue_hub(
            &new_key.address,
            r#"{"username":"new-user"}"#,
            "new-user",
            "bridge-stable",
            &state,
            |_| {
                let (sender, receiver) = std::sync::mpsc::channel();
                drop(sender);
                Ok((active_hue_hub(new_key.clone()), receiver))
            },
        )
        .unwrap();

        assert!(old_shutdown.load(Ordering::SeqCst));
        assert_eq!(composite.controller_count(), 0);
        {
            let state = state.lock().unwrap();
            assert_eq!(state.hub_credentials.len(), 1);
            assert_eq!(state.hubs.len(), 1);
            assert!(!state.hub_credentials.contains_key(&old_key));
            assert!(!state.hubs.contains_key(&old_key));
            let credentials = state.hub_credentials.get(&new_key).unwrap();
            assert_eq!(credentials.get_str("bridge_id"), Some("bridge-stable"));
            assert_eq!(crate::provider::hue_username(credentials), Some("new-user"));
            assert_eq!(credentials.get_str(HUE_ADDRESS_MIGRATION_FROM_FIELD), None);
            assert!(!state.hub_connection_status.contains_key(&old_key));
            assert!(!state.hub_seen_connected_once.contains(&old_key));
            assert!(!state.hub_reconnect_sync_at.contains_key(&old_key));
            assert!(!state.hub_pending_disconnect_at.contains_key(&old_key));
            assert!(!state.hub_startup_retry.contains_key(&old_key));
            assert!(!state
                .external_controller_authority_pending
                .contains(&old_key));
            assert!(!state
                .external_controller_initial_sync_pending
                .contains(&old_key));
            assert!(!state.external_controller_authority_is_required(&old_key));
            assert!(state.external_controller_authority_is_required(&new_key));
            assert!(state
                .external_controller_authority_pending
                .contains(&new_key));
            assert!(state
                .canonical_registry
                .find_by_native_id(&old_key, "hue-light-1")
                .is_none());
            assert_eq!(
                state
                    .canonical_registry
                    .find_by_native_id(&new_key, "hue-light-1")
                    .map(|device| device.id.as_str()),
                Some(canonical_id.as_str())
            );
            assert!(!state.topology.references_hub_key(&old_key));
            assert!(state.topology.references_hub_key(&new_key));
            assert!(state
                .topology
                .room_binding_is_managed(&room_id, &new_key, "hue-office"));
            assert_eq!(
                state
                    .topology
                    .composite_routing(&state.canonical_registry)
                    .get(&room_id),
                Some(&vec![(
                    new_key.to_string(),
                    rhythm_core::HubDispatchTarget::Group {
                        room_id: "hue-office".to_string(),
                        control_id: "grouped-office".to_string(),
                    },
                )])
            );
        }

        let durable_credentials = storage.load_all_hub_credentials().unwrap();
        assert_eq!(durable_credentials.len(), 1);
        assert_eq!(durable_credentials[0].hub_key(), Some(new_key.clone()));
        assert_eq!(
            durable_credentials[0].get_str(HUE_ADDRESS_MIGRATION_FROM_FIELD),
            None
        );
        let durable_authority = storage.load_authority_state().unwrap().unwrap();
        let durable_authority_json = serde_json::to_string(&durable_authority).unwrap();
        assert!(durable_authority_json.contains(&new_key.address));
        assert!(!durable_authority_json.contains(&old_key.address));
        assert_eq!(
            storage
                .load_integration_state_file(&ownership_path)
                .unwrap(),
            Some(ownership_json)
        );

        drop(state);
        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn authority_commit_failure_rolls_back_graph_and_configure_resumes_marker() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-address-migration-rollback-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = Arc::new(ToggleAuthorityStorage::new(&path));
        let state = shared_state();
        let old_key = hue_key("192.0.2.20");
        let new_key = hue_key("192.0.2.21");
        let mut old_credentials = hue_credentials(&old_key.address, "old-user");
        old_credentials.data["bridge_id"] = serde_json::json!("bridge-resume");
        storage
            .save_all_hub_credentials(&[old_credentials.clone()])
            .unwrap();
        let old_hub = active_hue_hub(old_key.clone());
        let old_shutdown = old_hub.shutdown.clone();
        {
            let mut state = state.lock().unwrap();
            state.storage = Some(storage.clone());
            state
                .hub_credentials
                .insert(old_key.clone(), old_credentials);
            state.hubs.insert(old_key.clone(), old_hub);
        }
        let (_, room_id) = install_address_migration_graph(&state, &old_key);
        storage.set_fail_authority_save(true);

        let error = configure_authenticated_hue_hub(
            &new_key.address,
            r#"{"username":"new-user"}"#,
            "new-user",
            "bridge-resume",
            &state,
            |_| panic!("connection must wait for the authority-state commit"),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Failed to durably migrate Hue topology"));
        assert!(old_shutdown.load(Ordering::SeqCst));
        {
            let state = state.lock().unwrap();
            assert!(state.canonical_registry.references_hub_key(&old_key));
            assert!(!state.canonical_registry.references_hub_key(&new_key));
            assert!(state.topology.references_hub_key(&old_key));
            assert!(!state.topology.references_hub_key(&new_key));
            assert!(state.hubs.is_empty());
            assert_eq!(state.hub_credentials.len(), 1);
            assert_eq!(
                state.hub_credentials.get(&new_key).and_then(|credentials| {
                    credentials.get_str(HUE_ADDRESS_MIGRATION_FROM_FIELD)
                }),
                Some(old_key.address.as_str())
            );
        }
        let staged = storage.load_all_hub_credentials().unwrap();
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].hub_key(), Some(new_key.clone()));
        assert_eq!(
            staged[0].get_str(HUE_ADDRESS_MIGRATION_FROM_FIELD),
            Some(old_key.address.as_str())
        );

        storage.set_fail_authority_save(false);
        configure_authenticated_hue_hub(
            &new_key.address,
            r#"{"username":"new-user"}"#,
            "new-user",
            "bridge-resume",
            &state,
            |_| {
                let (sender, receiver) = std::sync::mpsc::channel();
                drop(sender);
                Ok((active_hue_hub(new_key.clone()), receiver))
            },
        )
        .unwrap();
        {
            let state = state.lock().unwrap();
            assert!(!state.canonical_registry.references_hub_key(&old_key));
            assert!(state.canonical_registry.references_hub_key(&new_key));
            assert!(!state.topology.references_hub_key(&old_key));
            assert!(state.topology.references_hub_key(&new_key));
            assert!(state
                .topology
                .room_binding_is_managed(&room_id, &new_key, "hue-office"));
            assert!(state.hubs.contains_key(&new_key));
            assert_eq!(
                state.hub_credentials.get(&new_key).and_then(|credentials| {
                    credentials.get_str(HUE_ADDRESS_MIGRATION_FROM_FIELD)
                }),
                None
            );
        }

        drop(state);
        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn address_migration_rejects_an_active_old_key_topology_sync() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-address-migration-sync-race-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = Arc::new(FileStorage::new(path.to_str().unwrap()).unwrap());
        let state = shared_state();
        let old_key = hue_key("192.0.2.30");
        let new_key = hue_key("192.0.2.31");
        let mut credentials = hue_credentials(&old_key.address, "old-user");
        credentials.data["bridge_id"] = serde_json::json!("bridge-syncing");
        storage
            .save_all_hub_credentials(&[credentials.clone()])
            .unwrap();
        {
            let mut state = state.lock().unwrap();
            state.storage = Some(storage.clone());
            state.hub_credentials.insert(old_key.clone(), credentials);
            assert!(state.begin_hub_sync(&old_key));
        }

        let error = configure_authenticated_hue_hub(
            &new_key.address,
            r#"{"username":"new-user"}"#,
            "new-user",
            "bridge-syncing",
            &state,
            |_| panic!("migration must not race an active topology sync"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("topology sync is in progress"));
        {
            let state = state.lock().unwrap();
            assert!(state.hub_credentials.contains_key(&old_key));
            assert!(!state.hub_credentials.contains_key(&new_key));
            assert!(state.hub_sync_in_progress.contains(&old_key));
        }
        let durable = storage.load_all_hub_credentials().unwrap();
        assert_eq!(durable.len(), 1);
        assert_eq!(durable[0].hub_key(), Some(old_key));

        drop(state);
        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn legacy_same_username_address_migration_binds_stable_id_without_duplication() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-legacy-address-migration-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = Arc::new(FileStorage::new(path.to_str().unwrap()).unwrap());
        let state = shared_state();
        let old_key = hue_key("192.0.2.40");
        let new_key = hue_key("192.0.2.41");
        let credentials = hue_credentials(&old_key.address, "legacy-user");
        storage
            .save_all_hub_credentials(&[credentials.clone()])
            .unwrap();
        {
            let mut state = state.lock().unwrap();
            state.storage = Some(storage.clone());
            state.hub_credentials.insert(old_key.clone(), credentials);
            state
                .hubs
                .insert(old_key.clone(), active_hue_hub(old_key.clone()));
        }
        install_address_migration_graph(&state, &old_key);

        let mismatch = configure_authenticated_hue_hub(
            &new_key.address,
            r#"{"username":"different-user"}"#,
            "different-user",
            "bridge-upgraded",
            &state,
            |_| panic!("an unmatched legacy credential must block configuration"),
        )
        .unwrap_err();
        assert!(mismatch
            .to_string()
            .contains("legacy Hue configuration could not be safely matched"));
        assert!(state.lock().unwrap().hub_credentials.contains_key(&old_key));

        configure_authenticated_hue_hub(
            &new_key.address,
            r#"{"username":"legacy-user"}"#,
            "legacy-user",
            "bridge-upgraded",
            &state,
            |_| {
                let (sender, receiver) = std::sync::mpsc::channel();
                drop(sender);
                Ok((active_hue_hub(new_key.clone()), receiver))
            },
        )
        .unwrap();

        let state = state.lock().unwrap();
        assert_eq!(state.hub_credentials.len(), 1);
        assert_eq!(state.hubs.len(), 1);
        assert!(!state.hub_credentials.contains_key(&old_key));
        assert!(!state.hubs.contains_key(&old_key));
        assert_eq!(
            state
                .hub_credentials
                .get(&new_key)
                .and_then(|credentials| credentials.get_str("bridge_id")),
            Some("bridge-upgraded")
        );
        assert!(!state.canonical_registry.references_hub_key(&old_key));
        assert!(state.canonical_registry.references_hub_key(&new_key));
        assert!(!state.topology.references_hub_key(&old_key));
        assert!(state.topology.references_hub_key(&new_key));
        drop(state);

        let durable = storage.load_all_hub_credentials().unwrap();
        assert_eq!(durable.len(), 1);
        assert_eq!(durable[0].hub_key(), Some(new_key));
        assert_eq!(durable[0].get_str("bridge_id"), Some("bridge-upgraded"));

        drop(storage);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn physical_unpair_is_blocked_while_bridge_ownership_manifest_exists() {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-unpair-authority-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
        assert!(ensure_physical_hue_unpair_allowed(Some(&storage), "bridge-unpair").is_ok());

        let transport = SpyHueTransport::new();
        for (resource_type, data) in [
            ("bridge", serde_json::json!([{"id": "bridge-unpair"}])),
            ("device", serde_json::json!([{"id": "bulb"}])),
            ("light", serde_json::json!([])),
            ("behavior_instance", serde_json::json!([])),
            ("room", serde_json::json!([])),
            ("zone", serde_json::json!([])),
            ("scene", serde_json::json!([])),
            ("smart_scene", serde_json::json!([])),
        ] {
            transport.set_resource_response(
                resource_type,
                serde_json::json!({"data": data, "errors": []}),
            );
        }
        transport.set_v1_response("rules", serde_json::json!({}));
        transport.set_v1_response("schedules", serde_json::json!({}));
        crate::ownership::acquire_authoritative_control(
            &storage,
            &hue_key("192.0.2.10"),
            &transport,
            "user",
        )
        .unwrap();

        let error = ensure_physical_hue_unpair_allowed(Some(&storage), "bridge-unpair")
            .expect_err("an open ownership epoch must block physical removal");
        assert!(error.to_string().contains("Release Rhythm control"));

        drop(storage);
        let _ = std::fs::remove_dir_all(path);
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
            preferred_for_control: true,
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

        state
            .lock()
            .unwrap()
            .hubs
            .insert(key.clone(), active_hue_hub(key.clone()));
        assert_eq!(
            string_error(create_hue_controller(&state, &key)),
            "Hue credentials missing verified bridge identity"
        );
    }

    #[test]
    fn create_hue_controller_builds_controller_with_bridge_operation_lock() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        let bridge_id = "bridge-controller-operation-lock";
        {
            let mut guard = state.lock().unwrap();
            let mut credentials = hue_credentials("192.0.2.10", "user-123");
            credentials.data["bridge_id"] = serde_json::json!(bridge_id);
            guard.hub_credentials.insert(key.clone(), credentials);
            guard.hubs.insert(key.clone(), active_hue_hub(key.clone()));
        }
        let operation_lock = crate::ownership::controller_operation_lock(bridge_id);
        let owners_before_controller = Arc::strong_count(&operation_lock);

        let controller = create_hue_controller(&state, &key).unwrap();

        assert_eq!(Arc::strong_count(&controller), 1);
        assert_eq!(
            Arc::strong_count(&operation_lock),
            owners_before_controller + 1
        );
        drop(controller);
        assert_eq!(Arc::strong_count(&operation_lock), owners_before_controller);
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
