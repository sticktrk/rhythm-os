//! Server-driven room sync orchestrator.
//!
//! Discovers rooms and devices from the hub and diffs them against current
//! state. Adds new rooms, updates changed ones, removes stale ones, and
//! preserves user state (rhythm_enabled, offsets, etc.).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use log::{debug, info, warn};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::HubRegistry;

use crate::canonical::identity::HubKey;
use crate::canonical::registry::ResolveResult;
use crate::canonical::triage::{
    RoomBindingProposal, TriageDiscoveredDevice, TriageEntry, TriageKind, TriageStatus,
};
use crate::commands::{self, RoomParams};
use crate::discovery::HubDiscovery;
use crate::state::SharedState;
use crate::topology::{DiscoveredTopologyRoom, SyncAction};

/// Summary of what changed during a sync.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub rooms_added: usize,
    pub rooms_updated: usize,
    pub rooms_removed: usize,
    pub devices_synced: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncFailurePolicy {
    BestEffort,
    FailClosedBeforeAuthority,
}

impl SyncFailurePolicy {
    fn fail_closed(self) -> bool {
        self == Self::FailClosedBeforeAuthority
    }
}

struct HubSyncGuard {
    state: SharedState,
    hub_key: HubKey,
}

impl Drop for HubSyncGuard {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            s.finish_hub_sync(&self.hub_key);
        }
    }
}

fn try_acquire_hub_sync_guard(
    state: &SharedState,
    hub_key: &HubKey,
) -> Result<Option<HubSyncGuard>> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if !s.begin_hub_sync(hub_key) {
        return Ok(None);
    }
    Ok(Some(HubSyncGuard {
        state: state.clone(),
        hub_key: hub_key.clone(),
    }))
}

fn acquire_hub_sync_guard_with_timeout(
    state: &SharedState,
    hub_key: &HubKey,
    timeout: Duration,
) -> Result<HubSyncGuard> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(guard) = try_acquire_hub_sync_guard(state, hub_key)? {
            return Ok(guard);
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "Timed out waiting for the existing {} sync to finish",
                hub_key
            );
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100)),
        );
    }
}

/// Discover rooms and devices from the hub and sync into engine + registry.
///
/// Preserves user state (rhythm_enabled, time_offset, brightness_offset,
/// soft_off, disabled) for existing rooms. New rooms default to rhythm_enabled=true.
pub fn sync_from_hub(state: &SharedState) -> Result<SyncReport> {
    sync_from_hub_with_options(state, true)
}

/// Sync rooms and devices from ALL connected hubs.
///
/// Iterates each hub with discovery and runs `sync_with_discovery` for each.
/// Returns a combined report. Errors from individual hubs are logged but don't
/// prevent syncing from other hubs.
pub fn sync_all_hubs(state: &SharedState) -> Result<SyncReport> {
    let discover_devices = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.platform.full_device_discovery
    };

    let hub_keys: Vec<_> = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hubs
            .iter()
            .filter_map(|(key, hub)| hub.discovery.as_ref().map(|_| key.clone()))
            .collect()
    };

    if hub_keys.is_empty() {
        return Err(anyhow::anyhow!("No hub discovery available"));
    }

    let mut combined = SyncReport::default();

    for key in &hub_keys {
        match sync_from_hub_for_key(state, key, discover_devices) {
            Ok(report) => {
                info!(target: "room_sync", "Hub {} sync: +{} ~{} -{} devices={}",
                    key, report.rooms_added, report.rooms_updated,
                    report.rooms_removed, report.devices_synced);
                combined.rooms_added += report.rooms_added;
                combined.rooms_updated += report.rooms_updated;
                combined.rooms_removed += report.rooms_removed;
                combined.devices_synced += report.devices_synced;
            }
            Err(e) => {
                warn!(target: "room_sync", "Hub {} sync failed: {}", key, e);
            }
        }
    }

    Ok(combined)
}

/// Like [`sync_from_hub`] but allows skipping device discovery.
///
/// On constrained blocking runtimes, the device endpoint response (~79
/// devices) can exceed the available heap when SSE TLS is already active.
/// Pass `discover_devices: false` to sync only rooms — the SSE stream will
/// register devices organically.
pub fn sync_from_hub_with_options(
    state: &SharedState,
    discover_devices: bool,
) -> Result<SyncReport> {
    // Extract the first active hub key
    let hub_key = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hubs
            .iter()
            .find_map(|(key, h)| h.discovery.as_ref().map(|_| key.clone()))
    }
    .ok_or_else(|| anyhow::anyhow!("No hub discovery available"))?;

    sync_from_hub_for_key(state, &hub_key, discover_devices)
}

/// Sync rooms and devices from a specific hub identified by key.
///
/// Like [`sync_from_hub_with_options`] but targets a specific hub rather than
/// picking the first one. Used after `do_hub_credentials` to sync the newly
/// configured hub.
pub fn sync_from_hub_for_key(
    state: &SharedState,
    hub_key: &HubKey,
    discover_devices: bool,
) -> Result<SyncReport> {
    let transaction_lock = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .external_topology_transaction_lock
        .clone();
    let report = {
        let _transaction = transaction_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("External topology transaction lock poisoned"))?;
        let Some(_guard) = try_acquire_hub_sync_guard(state, hub_key)? else {
            debug!(target: "room_sync", "Skipping sync for {}: already in progress", hub_key);
            return Ok(SyncReport::default());
        };
        sync_from_hub_for_key_acquired(
            state,
            hub_key,
            discover_devices,
            SyncFailurePolicy::BestEffort,
        )?
    };
    reconcile_external_controller_authority_after_sync(state, hub_key)?;
    Ok(report)
}

/// Build the complete desired graph required before taking authority over an
/// external grouped-room controller. Unlike ordinary refresh syncs, discovery
/// and apply failures are fatal so takeover cannot clear a bridge from a
/// partial view of its lights and rooms.
pub fn sync_from_hub_for_key_before_authority(
    state: &SharedState,
    hub_key: &HubKey,
    discover_devices: bool,
) -> Result<SyncReport> {
    if !discover_devices {
        anyhow::bail!(
            "Complete device discovery is required before taking external controller authority"
        );
    }
    let transaction_lock = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .external_topology_transaction_lock
        .clone();
    let _transaction = transaction_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("External topology transaction lock poisoned"))?;
    let _guard = acquire_hub_sync_guard_with_timeout(state, hub_key, Duration::from_secs(30))?;
    sync_from_hub_for_key_acquired(
        state,
        hub_key,
        discover_devices,
        SyncFailurePolicy::FailClosedBeforeAuthority,
    )
}

/// Run a fresh sync after waiting for any same-hub sync to finish.
///
/// Lifecycle operations that have already changed upstream state (such as a
/// Hue Bridge join/removal) use this instead of treating a busy sync as a
/// successful no-op.
pub fn sync_from_hub_for_key_wait(
    state: &SharedState,
    hub_key: &HubKey,
    discover_devices: bool,
    timeout: Duration,
) -> Result<SyncReport> {
    let transaction_lock = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .external_topology_transaction_lock
        .clone();
    let report = {
        let _transaction = transaction_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("External topology transaction lock poisoned"))?;
        let _guard = acquire_hub_sync_guard_with_timeout(state, hub_key, timeout)?;
        sync_from_hub_for_key_acquired(
            state,
            hub_key,
            discover_devices,
            SyncFailurePolicy::BestEffort,
        )?
    };
    reconcile_external_controller_authority_after_sync(state, hub_key)?;
    Ok(report)
}

/// A room discovered after prior all-Rhythm consent invalidates that complete
/// consent set. Reconcile only after dropping the topology transaction guard:
/// the integration callback acquires that same guard around capture/release.
fn reconcile_external_controller_authority_after_sync(
    state: &SharedState,
    hub_key: &HubKey,
) -> Result<()> {
    let callback = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if state.external_controller_initial_sync_is_pending(hub_key) {
            return Ok(());
        }
        state.reconcile_external_controller_authority_fn.clone()
    };
    if let Some(callback) = callback {
        callback(state, hub_key)?;
    }
    Ok(())
}

fn sync_from_hub_for_key_acquired(
    state: &SharedState,
    hub_key: &HubKey,
    discover_devices: bool,
    failure_policy: SyncFailurePolicy,
) -> Result<SyncReport> {
    if state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .authority_state_recovery_required
    {
        anyhow::bail!("Authoritative topology state requires recovery");
    }
    let discovery = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hubs
            .get(hub_key)
            .and_then(|h| h.discovery.as_ref().cloned())
    }
    .ok_or_else(|| anyhow::anyhow!("No hub discovery for {}", hub_key))?;

    let report = sync_with_discovery(
        state,
        hub_key,
        discovery.as_ref(),
        discover_devices,
        failure_policy,
    )?;
    commands::reconcile_runtime_from_state(state)?;
    Ok(report)
}

/// Internal sync implementation that takes a discovery reference directly.
///
/// Interleaved order to minimize peak memory during sync:
/// 1. Discover rooms (builds device→room cache inside discovery impl)
/// 2. Process rooms (first room triggers runtime creation)
/// 3. Discover devices (uses cached device→room mapping, skips rooms re-fetch)
/// 4. Process devices
/// 5. Persist state
/// 6. Release discovery resources (drops discovery TLS)
fn sync_with_discovery(
    state: &SharedState,
    hub_key: &HubKey,
    discovery: &dyn HubDiscovery,
    discover_devices: bool,
    failure_policy: SyncFailurePolicy,
) -> Result<SyncReport> {
    // ========================================================================
    // Phase 1: Discover rooms
    // ========================================================================
    let discovered_rooms = discovery.discover_rooms()?;
    let source_room_names_authoritative = discovery.room_names_are_authoritative();
    info!(target: "room_sync", "Discovered {} rooms from hub", discovered_rooms.len());

    // ========================================================================
    // Phase 2: Diff and apply source rooms into registry + topology only.
    // Runtime materialization is an explicit final sync phase.
    // ========================================================================
    let mut complete_room_graph_applied = true;
    let current_room_ids = match extract_registry_for(state, hub_key) {
        Some(registry) => match registry.lock() {
            Ok(registry) => registry.rooms().into_iter().map(|room| room.id).collect(),
            Err(_) => {
                complete_room_graph_applied = false;
                HashSet::new()
            }
        },
        None => {
            complete_room_graph_applied = false;
            HashSet::new()
        }
    };

    let discovered_ids: HashSet<String> = discovered_rooms.iter().map(|r| r.id.clone()).collect();

    let mut report = SyncReport {
        rooms_added: 0,
        rooms_updated: 0,
        rooms_removed: 0,
        devices_synced: 0,
    };

    for room in &discovered_rooms {
        let is_new = !current_room_ids.contains(&room.id);

        let params = RoomParams {
            id: room.id.clone(),
            name: room.name.clone(),
            grouped_light_id: room.grouped_light_id.clone(),
            rhythm_enabled: true,
            disabled: false,
            state: None,
            device_ids: room.device_ids.clone(),
        };

        match commands::do_room_set(state, &params, hub_key, false, false) {
            Ok(_) => {
                if is_new {
                    info!(target: "room_sync", "Added room '{}' ({})", room.name, room.id);
                    report.rooms_added += 1;
                } else {
                    report.rooms_updated += 1;
                }
            }
            Err(e) => {
                if failure_policy.fail_closed() {
                    return Err(e).with_context(|| {
                        format!("Failed to apply discovered room on {}", hub_key)
                    });
                }
                complete_room_graph_applied = false;
                warn!(target: "room_sync", "Failed to set room '{}': {}", room.name, e);
            }
        }
    }

    // Remove stale source rooms from the hub registry.
    //
    // For normal room-aware hubs, an empty discovery result after previously
    // having source rooms means all source rooms disappeared. Roomless hubs
    // such as Matter intentionally report no rooms; preserve their existing
    // synthetic/source room mappings so a transient device probe failure does
    // not erase persisted routing.
    let supports_roomless_devices = hub_supports_roomless_devices(state, hub_key);
    let should_prune_stale_rooms =
        !discovered_ids.is_empty() || (!current_room_ids.is_empty() && !supports_roomless_devices);
    if should_prune_stale_rooms {
        let stale_ids: Vec<String> = current_room_ids
            .difference(&discovered_ids)
            .cloned()
            .collect();

        let registry = extract_registry_for(state, hub_key);
        for room_id in stale_ids {
            info!(target: "room_sync", "Removing stale source room '{}'", room_id);
            let Some(registry) = registry.as_ref() else {
                if failure_policy.fail_closed() {
                    anyhow::bail!(
                        "No hub registry available for stale-room removal on {}",
                        hub_key
                    );
                }
                complete_room_graph_applied = false;
                warn!(target: "room_sync", "No hub registry available for stale room '{}'", room_id);
                continue;
            };
            match registry.lock() {
                Ok(mut reg) => {
                    reg.remove_room(&room_id);
                    report.rooms_removed += 1;
                }
                Err(_) => {
                    if failure_policy.fail_closed() {
                        anyhow::bail!("Failed to apply stale-room removal on {}", hub_key);
                    }
                    complete_room_graph_applied = false;
                    warn!(target: "room_sync", "Failed to lock hub registry for stale room '{}'", room_id);
                }
            }
        }
    }

    if should_prune_stale_rooms {
        let discovered_room_ids: Vec<String> = discovered_rooms
            .iter()
            .map(|room| room.id.clone())
            .collect();
        let removed_bindings = {
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let removed_bindings = s
                .topology
                .remove_stale_bindings(hub_key, &discovered_room_ids);
            if !removed_bindings.is_empty() {
                commands::save_topology(&s)?;
            }
            removed_bindings
        };

        if !removed_bindings.is_empty() {
            info!(
                target: "room_sync",
                "Removed stale source bindings for {} topology rooms on {}",
                removed_bindings.len(),
                hub_key
            );
        }
    }

    if complete_room_graph_applied && !discovered_rooms.is_empty() {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        complete_room_graph_applied = discovered_rooms
            .iter()
            .all(|room| state.topology.find_by_hub_room(hub_key, &room.id).is_some());
    }

    // A legacy bridge marker exists only to carry pre-policy approval through
    // its first complete room discovery. Convert that temporary approval into
    // explicit per-room decisions before controller reconciliation, then
    // commit the marker retirement with the ordinary authority-state writer.
    // Any best-effort room apply failure retains the marker for a later retry.
    if complete_room_graph_applied {
        let materialized = {
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let topology_before = s.topology.clone();
            if !s
                .topology
                .materialize_grandfathered_external_room_automation_decisions(hub_key)
            {
                false
            } else if let Err(error) = commands::save_topology(&s) {
                s.topology = topology_before;
                return Err(error)
                    .context("Failed to durably materialize legacy Hue room automation authority");
            } else {
                true
            }
        };
        if materialized {
            info!(
                target: "room_sync",
                "Materialized legacy Hue room automation authority for {}",
                hub_key
            );
        }
    }

    // ========================================================================
    // Phase 3+4: Discover and apply devices (buttons + motion sensors)
    // ========================================================================
    // Skippable on constrained blocking runtimes where the device endpoint
    // response can OOM the heap.
    // The SSE stream registers devices organically as button/motion events arrive.
    // Collect which device types were discovered so stale removal
    // only affects types that the hub actually enumerates.
    // E.g. HA discovers only Motion (not Buttons), so on-demand
    // registered Buttons must not be removed as stale.
    let mut discovered_types: HashSet<rhythm_core::runtime::hub_registry::DeviceType> =
        HashSet::new();
    let mut discovered_device_ids: HashSet<String> = HashSet::new();

    if discover_devices {
        let discovered_devices = match discovery.discover_devices() {
            Ok(d) => d,
            Err(e) => {
                if failure_policy.fail_closed() {
                    return Err(e)
                        .with_context(|| format!("Device discovery failed for {}", hub_key));
                }
                warn!(target: "room_sync", "Device discovery failed: {}", e);
                Vec::new()
            }
        };

        discovered_types.extend(discovered_devices.iter().map(|d| d.device_type.clone()));
        discovered_device_ids.extend(discovered_devices.iter().map(|d| d.device_id.clone()));

        for device in &discovered_devices {
            let effective_room_id = registry_room_id_for_discovered_device(state, hub_key, device);
            if let Err(e) = commands::do_device_set(
                state,
                &device.device_id,
                effective_room_id.as_deref(),
                &device.buttons,
                device.device_type.clone(),
                hub_key,
                false,
            ) {
                if failure_policy.fail_closed() {
                    return Err(e).with_context(|| {
                        format!("Failed to apply discovered device on {}", hub_key)
                    });
                }
                warn!(target: "room_sync", "Failed to set device '{}': {}", device.device_id, e);
            }
            report.devices_synced += 1;
        }
    }

    // ========================================================================
    // Phase 4b: Canonical device resolution
    // ========================================================================
    // Discover full device identities (names, MAC addresses, manufacturer/model)
    // and resolve each through the canonical registry. Only runs when device
    // discovery is enabled (desktop only — constrained blocking targets do not
    // persist canonical state).
    if discover_devices {
        let canonical_hub_key = hub_key.clone();
        let (identities, identities_fresh) = match discovery.discover_identities() {
            Ok(ids) => (ids, true),
            Err(e) => {
                if failure_policy.fail_closed() {
                    return Err(e)
                        .with_context(|| format!("Identity discovery failed for {}", hub_key));
                }
                warn!(target: "room_sync", "Identity discovery failed: {}", e);
                (Vec::new(), false)
            }
        };
        let discovered_native_ids: HashSet<String> = identities
            .iter()
            .map(|identity| identity.native_id.clone())
            .collect();
        let discovered_endpoint_capabilities: HashMap<String, serde_json::Value> = identities
            .iter()
            .filter_map(|identity| {
                discovery
                    .endpoint_capabilities(&identity.native_id)
                    .map(|capabilities| (identity.native_id.clone(), capabilities))
            })
            .collect();
        let has_typed_light_identities = identities
            .iter()
            .any(|identity| identity.device_type == DeviceType::Light);
        let had_active_endpoints = state
            .lock()
            .ok()
            .map(|s| {
                s.canonical_registry
                    .has_active_endpoints_for_hub(&canonical_hub_key)
            })
            .unwrap_or(false);
        let should_process_identities =
            identities_fresh && (!discovered_native_ids.is_empty() || had_active_endpoints);

        if should_process_identities {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let mut canonical_room_devices: HashMap<String, Vec<String>> = HashMap::new();
            let mut room_light_device_ids: HashMap<String, Vec<String>> = HashMap::new();
            let mut new_light_canonical_ids: HashSet<String> = HashSet::new();
            let mut sleep_default_nodes_to_seed: Vec<String> = Vec::new();
            let mut canonically_assigned_roomless_inputs: Vec<(String, String)> = Vec::new();

            {
                let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

                // Prune triage entries resolved more than 7 days ago
                let seven_days = 7 * 24 * 60 * 60;
                if now > seven_days {
                    s.canonical_registry
                        .triage_mut()
                        .prune_resolved(now - seven_days);
                }

                // Dismiss pending DeviceMerge triage entries whose native_id
                // is no longer in the discovered identity set (e.g. HA virtual
                // group entities that are no longer produced).
                // RoomBinding entries are NOT checked here — they use
                // room_binding.hub_room_id, not discovered.native_id.
                let stale_triage: Vec<String> = s
                    .canonical_registry
                    .triage()
                    .pending()
                    .iter()
                    .filter(|e| {
                        e.hub_key == canonical_hub_key
                            && e.kind == crate::canonical::triage::TriageKind::DeviceMerge
                            && !discovered_native_ids.contains(&e.discovered.native_id)
                    })
                    .map(|e| e.id.clone())
                    .collect();
                for entry_id in &stale_triage {
                    s.canonical_registry.triage_mut().dismiss(entry_id, now);
                    info!(target: "room_sync", "Dismissed stale triage entry '{}'", entry_id);
                }

                for identity in &identities {
                    if identity.device_type == DeviceType::Light {
                        if let Some(hub_room_id) = identity.room_id.as_ref() {
                            room_light_device_ids
                                .entry(hub_room_id.clone())
                                .or_default()
                                .push(identity.native_id.clone());
                        }
                    }

                    let result = s
                        .canonical_registry
                        .resolve(identity, &canonical_hub_key, now);
                    let (canonical_id, created_canonical_device) = match result {
                        ResolveResult::AlreadyKnown { canonical_id } => (canonical_id, false),
                        ResolveResult::ReApproved { canonical_id } => (canonical_id, false),
                        ResolveResult::Queued { .. } => continue,
                        ResolveResult::Created { canonical_id } => (canonical_id, true),
                    };
                    if let Some(capabilities) =
                        discovered_endpoint_capabilities.get(&identity.native_id)
                    {
                        if let Some(endpoint) = s
                            .canonical_registry
                            .get_mut(&canonical_id)
                            .and_then(|device| {
                                device.endpoints.iter_mut().find(|endpoint| {
                                    endpoint.hub_key == canonical_hub_key
                                        && endpoint.native_id == identity.native_id
                                })
                            })
                        {
                            endpoint.capabilities = Some(capabilities.clone());
                        }
                    }
                    if created_canonical_device && identity.device_type == DeviceType::Light {
                        new_light_canonical_ids.insert(canonical_id.clone());
                    }

                    let Some(hub_room_id) = identity.room_id.clone() else {
                        let canonical_room_id = s
                            .canonical_registry
                            .get(&canonical_id)
                            .and_then(|device| device.room_id.clone());
                        match canonical_room_id {
                            None => {
                                s.canonical_registry.assign_room(&canonical_id, None);
                                s.topology.ensure_standalone_device(&canonical_id);
                                if new_light_canonical_ids.contains(&canonical_id) {
                                    sleep_default_nodes_to_seed.push(canonical_id.clone());
                                }
                            }
                            Some(canonical_room_id)
                                if identity.device_type != DeviceType::Light =>
                            {
                                canonically_assigned_roomless_inputs
                                    .push((canonical_id.clone(), canonical_room_id));
                            }
                            Some(_) => {}
                        }
                        continue;
                    };

                    canonical_room_devices
                        .entry(hub_room_id)
                        .or_default()
                        .push(canonical_id);
                }
                for ids in canonical_room_devices.values_mut() {
                    ids.sort();
                    ids.dedup();
                }
                for ids in room_light_device_ids.values_mut() {
                    ids.sort();
                    ids.dedup();
                }

                // ============================================================
                // Phase 4c: Topology sync
                // ============================================================
                for room in &discovered_rooms {
                    let canonical_device_ids = canonical_room_devices
                        .get(&room.id)
                        .cloned()
                        .unwrap_or_default();
                    let light_device_ids = if has_typed_light_identities {
                        room_light_device_ids
                            .get(&room.id)
                            .cloned()
                            .unwrap_or_default()
                    } else {
                        room.device_ids.clone()
                    };

                    let topo_room = DiscoveredTopologyRoom {
                        hub_room_id: room.id.clone(),
                        name: room.name.clone(),
                        source_name_authoritative: source_room_names_authoritative,
                        control_id: room.grouped_light_id.clone(),
                        light_device_ids: light_device_ids.clone(),
                        canonical_device_ids: canonical_device_ids.clone(),
                    };
                    let canonical_registry = s.canonical_registry.clone();
                    let action = s.topology.sync_hub_room_with_registry(
                        &canonical_hub_key,
                        &topo_room,
                        &canonical_registry,
                    );
                    let rhythm_room_id = action.rhythm_room_id().to_string();
                    let canonical_room_assignments: Vec<(String, Option<String>)> =
                        canonical_device_ids
                            .iter()
                            .map(|canonical_id| {
                                let explicitly_standalone = s
                                    .topology
                                    .get_device_node(canonical_id)
                                    .is_some_and(|node| {
                                        node.parent_id.is_none()
                                            && node.placement
                                                == crate::topology::DevicePlacement::UserOverride
                                    });
                                let assigned_room_id = if explicitly_standalone {
                                    None
                                } else {
                                    s.topology
                                        .device_parent_room_id(canonical_id)
                                        .map(str::to_string)
                                        .or_else(|| {
                                            s.canonical_registry
                                                .get(canonical_id)
                                                .and_then(|device| device.room_id.clone())
                                        })
                                        .or_else(|| Some(rhythm_room_id.clone()))
                                };
                                (canonical_id.clone(), assigned_room_id)
                            })
                            .collect();

                    for (canonical_id, assigned_room_id) in &canonical_room_assignments {
                        s.canonical_registry
                            .assign_room(canonical_id, assigned_room_id.as_deref());
                        if new_light_canonical_ids.contains(canonical_id) {
                            if let Some(assigned_room_id) = assigned_room_id {
                                sleep_default_nodes_to_seed.push(assigned_room_id.clone());
                            }
                        }
                    }

                    // Queue room binding proposals for cross-hub name matches
                    if let SyncAction::CreatedWithProposal {
                        proposed_target_id,
                        proposed_target_name,
                        candidate_rooms,
                        ..
                    } = action
                    {
                        if !s
                            .canonical_registry
                            .triage()
                            .has_room_binding(&canonical_hub_key, &room.id)
                        {
                            let entry = TriageEntry {
                                id: format!("room-triage-{}-{}", now, room.id),
                                kind: TriageKind::RoomBinding,
                                discovered: TriageDiscoveredDevice::default(),
                                hub_key: canonical_hub_key.clone(),
                                candidate_matches: vec![],
                                room_binding: Some(RoomBindingProposal {
                                    hub_room_id: room.id.clone(),
                                    hub_room_name: room.name.clone(),
                                    control_id: room.grouped_light_id.clone(),
                                    light_device_ids,
                                    canonical_device_ids,
                                    target_rhythm_room_id: proposed_target_id,
                                    target_rhythm_room_name: proposed_target_name,
                                    candidate_rooms,
                                }),
                                confidence: 80,
                                status: TriageStatus::Pending,
                                resolved_by: None,
                                created_at: now,
                                resolved_at: None,
                                canonical_id: None,
                            };
                            s.canonical_registry.triage_mut().add(entry);
                            info!(
                                target: "room_sync",
                                "Queued room binding proposal: hub room '{}' → candidates",
                                room.name
                            );
                        }
                    }
                }

                canonically_assigned_roomless_inputs.sort();
                canonically_assigned_roomless_inputs.dedup();
                let mut repaired_roomless_inputs = 0usize;
                for (canonical_id, canonical_room_id) in &canonically_assigned_roomless_inputs {
                    let user_overridden =
                        s.topology
                            .get_device_node(canonical_id)
                            .is_some_and(|node| {
                                node.placement == crate::topology::DevicePlacement::UserOverride
                            });
                    if user_overridden
                        || s.topology.device_parent_room_id(canonical_id)
                            == Some(canonical_room_id.as_str())
                    {
                        continue;
                    }

                    if s.topology
                        .attach_device_hub_default(canonical_room_id, canonical_id)
                    {
                        repaired_roomless_inputs += 1;
                    } else {
                        warn!(
                            target: "room_sync",
                            "Could not restore roomless input '{}' to missing canonical room '{}'",
                            canonical_id,
                            canonical_room_id
                        );
                    }
                }
                if repaired_roomless_inputs > 0 {
                    info!(
                        target: "room_sync",
                        "Restored {} roomless inputs to their canonical rooms",
                        repaired_roomless_inputs
                    );
                }

                // Persist canonical registry and topology
                commands::save_authority_state(&s)?;
            }

            sleep_default_nodes_to_seed.sort();
            sleep_default_nodes_to_seed.dedup();
            for node_id in &sleep_default_nodes_to_seed {
                commands::ensure_sleep_mode_hard_off_default(state, node_id);
            }

            let (affected_devices, hidden_devices) = commands::reconcile_hub_endpoint_visibility(
                state,
                &canonical_hub_key,
                &discovered_native_ids,
            )?;
            if affected_devices > 0 {
                info!(
                    target: "room_sync",
                    "Canonical endpoint visibility: {} devices updated, {} hidden",
                    affected_devices,
                    hidden_devices
                );
                crate::state::emit_server_event(
                    state,
                    crate::server_event::ServerEvent::NodesChanged,
                );
            }

            info!(
                target: "room_sync",
                "Canonical resolution: {} identities processed, {} topology rooms synced",
                identities.len(),
                discovered_rooms.len()
            );

            // Phase 4d (engine remap) eliminated: do_room_set now uses
            // translate_or_create to add rooms with topology IDs from the
            // start, so no post-hoc remap is needed.
        }

        // ============================================================
        // Phase 4e: Hub-configured device triage
        // ============================================================
        // Check for devices with native hub automation (e.g. Hue
        // behavior_instances). These conflict with Rhythm and should
        // be surfaced as triage items.
        match discovery.discover_configured_devices() {
            Ok(mappings) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                // Collect currently-configured device IDs for stale entry cleanup
                let configured_device_ids: HashSet<&str> =
                    mappings.iter().map(|(_, d)| d.as_str()).collect();

                let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

                // Create triage entries for newly-configured devices
                for (_, device_id) in &mappings {
                    if s.canonical_registry
                        .triage()
                        .has_hub_configured(&canonical_hub_key, device_id)
                    {
                        continue; // already pending
                    }

                    // Look up device info from canonical registry
                    let (name, device_type) = s
                        .canonical_registry
                        .find_by_native_id(&canonical_hub_key, device_id)
                        .map(|cd| (cd.name.clone(), cd.device_type.clone()))
                        .unwrap_or_else(|| (device_id.clone(), DeviceType::Button));

                    let entry = TriageEntry {
                        id: format!("hub-configured-{}-{}", now, device_id),
                        kind: TriageKind::HubConfigured,
                        discovered: TriageDiscoveredDevice {
                            native_id: device_id.clone(),
                            name,
                            device_type,
                            room_id: String::new(),
                            room_name: String::new(),
                            manufacturer: None,
                            model: None,
                        },
                        hub_key: canonical_hub_key.clone(),
                        candidate_matches: vec![],
                        room_binding: None,
                        confidence: 0,
                        status: TriageStatus::Pending,
                        resolved_by: None,
                        created_at: now,
                        resolved_at: None,
                        canonical_id: None,
                    };
                    s.canonical_registry.triage_mut().add(entry);
                    info!(
                        target: "room_sync",
                        "Queued HubConfigured triage: device '{}' has native automation",
                        device_id
                    );
                }

                // Auto-resolve stale HubConfigured entries for devices that
                // no longer have behavior_instances (user removed them in Hue app)
                let stale_ids: Vec<String> = s
                    .canonical_registry
                    .triage()
                    .pending_by_kind(TriageKind::HubConfigured)
                    .iter()
                    .filter(|e| {
                        e.hub_key == canonical_hub_key
                            && !configured_device_ids.contains(e.discovered.native_id.as_str())
                    })
                    .map(|e| e.id.clone())
                    .collect();
                for entry_id in &stale_ids {
                    s.canonical_registry.triage_mut().resolve(
                        entry_id,
                        TriageStatus::Confirmed,
                        "auto",
                        now,
                    );
                    info!(
                        target: "room_sync",
                        "Auto-resolved HubConfigured triage '{}': behavior removed",
                        entry_id
                    );
                }

                if !mappings.is_empty() || !stale_ids.is_empty() {
                    commands::save_canonical(&s)?;
                }
            }
            Err(e) => {
                warn!(target: "room_sync",
                    "Behavior instance discovery failed (non-fatal): {}", e);
            }
        }
    }

    // ========================================================================
    // Phase 5: Prefetch motion sensor state
    // ========================================================================
    // Some hubs (HA) don't replay current sensor state on event subscription.
    // If motion was active before startup, the event loop won't see it.
    // discover_motion_state() returns current sensor states so we can seed
    // the motion timer system before the event loop starts.
    match discovery.discover_motion_state() {
        Ok(motion_states) if !motion_states.is_empty() => {
            // Register all discovered motion sensors in the device registry
            let motion_registry_rooms: Vec<(String, Option<String>)> = motion_states
                .iter()
                .map(|ms| {
                    let device = crate::discovery::DiscoveredDevice {
                        device_id: ms.sensor_id.clone(),
                        room_id: ms.room_id.clone(),
                        buttons: vec![],
                        device_type: DeviceType::Motion,
                    };
                    (
                        ms.sensor_id.clone(),
                        registry_room_id_for_discovered_device(state, hub_key, &device),
                    )
                })
                .collect();
            if let Some(registry) = extract_registry_for(state, hub_key) {
                if let Ok(mut reg) = registry.lock() {
                    for (sensor_id, room_id) in &motion_registry_rooms {
                        reg.upsert_device(sensor_id, room_id.as_deref(), &[], DeviceType::Motion);
                    }
                }
            }
            // Track Phase 5 sensors so stale removal doesn't undo them
            discovered_types.insert(DeviceType::Motion);
            for ms in &motion_states {
                discovered_device_ids.insert(ms.sensor_id.clone());
            }
            // Seed every discovered sensor (active or not) into AppState so
            // the event loop can resume timers for rooms whose lights are
            // already on at boot. Resolve to public source/target node IDs
            // up front so startup motion state follows the same node-control
            // graph as live events.
            let motion_timer_restores = state
                .lock()
                .ok()
                .map(|s| s.motion_timer_restores.clone())
                .unwrap_or_default();
            let mut used_restore_source_ids = Vec::new();
            let seeds: Vec<crate::state::MotionSeedEntry> = motion_states
                .iter()
                .filter_map(|ms| {
                    commands::resolve_node_control_target(
                        state,
                        hub_key,
                        &ms.sensor_id,
                        &crate::topology::NodeControlKind::Motion,
                    )
                    .map(|(source_node_id, target_node_id)| {
                        let restore = motion_timer_restores
                            .get(&source_node_id)
                            .filter(|entry| entry.target_node_id == target_node_id);
                        if restore.is_some() {
                            used_restore_source_ids.push(source_node_id.clone());
                        }
                        crate::state::MotionSeedEntry {
                            source_node_id,
                            target_node_id,
                            is_active: ms.is_active,
                            stopped_at_epoch_ms: restore
                                .and_then(|entry| {
                                    (!ms.is_active).then_some(entry.stopped_at_epoch_ms)
                                })
                                .flatten(),
                            motion_owned: restore.map(|entry| entry.motion_owned),
                            warning_active: restore
                                .is_some_and(|entry| !ms.is_active && entry.warning_active),
                        }
                    })
                })
                .collect();
            if !seeds.is_empty() {
                if let Ok(mut s) = state.lock() {
                    s.pending_motion_seed.extend(seeds);
                    for source_id in used_restore_source_ids {
                        s.motion_timer_restores.remove(&source_id);
                    }
                }
            }
            info!(target: "room_sync", "Motion prefetch: {} sensors, {} active",
                motion_states.len(),
                motion_states.iter().filter(|m| m.is_active).count());
        }
        Err(e) => {
            warn!(target: "room_sync", "Motion state prefetch failed: {}", e);
        }
        _ => {} // Empty or Ok with no sensors
    }

    // ========================================================================
    // Phase 6: Remove stale devices
    // ========================================================================
    // Runs after Phase 5 so motion sensors discovered via prefetch aren't
    // falsely removed. Only removes device types that were actually discovered.
    if discover_devices && !discovered_types.is_empty() {
        let current_typed = get_all_typed_device_ids_with_types(state, hub_key);
        for (device_id, device_type) in &current_typed {
            if discovered_types.contains(device_type) && !discovered_device_ids.contains(device_id)
            {
                if let Err(e) = commands::do_device_remove(state, device_id, hub_key) {
                    if failure_policy.fail_closed() {
                        return Err(e).with_context(|| {
                            format!("Failed to apply stale-device removal on {}", hub_key)
                        });
                    }
                    warn!(target: "room_sync", "Failed to remove stale device '{}': {}", device_id, e);
                }
            }
        }
    }

    // Persist once after all changes
    commands::persist_state(state);

    // Release discovery transport's TLS connection now that sync is complete.
    // On constrained blocking runtimes this can free ~12KB of heap before the
    // first periodic tick.
    discovery.release_resources();

    info!(target: "room_sync",
        "Sync complete: {} added, {} updated, {} removed, {} devices",
        report.rooms_added, report.rooms_updated, report.rooms_removed,
        report.devices_synced
    );

    Ok(report)
}

/// Determine the room mapping to store in the hub registry for a discovered
/// typed device.
///
/// Hub-native rooms stay authoritative when the hub reports one. For roomless
/// devices, preserve a user-assigned canonical Rhythm room so a later sync does
/// not clear routing that was created through the assignment endpoint.
fn registry_room_id_for_discovered_device(
    state: &SharedState,
    hub_key: &HubKey,
    device: &crate::discovery::DiscoveredDevice,
) -> Option<String> {
    if device.room_id.is_some() {
        return device.room_id.clone();
    }

    state.lock().ok().and_then(|s| {
        s.canonical_registry
            .find_by_native_id(hub_key, &device.device_id)
            .and_then(|canonical| canonical.room_id.as_ref())
            .filter(|room_id| s.topology.get(room_id).is_some())
            .cloned()
    })
}

fn hub_supports_roomless_devices(state: &SharedState, hub_key: &HubKey) -> bool {
    state
        .lock()
        .ok()
        .map(|s| {
            s.hub_capabilities.iter().any(|capability| {
                capability.hub_type == hub_key.hub_type.as_str()
                    && capability.supports_roomless_devices
            })
        })
        .unwrap_or(false)
}

/// Extract registry for a specific hub.
fn extract_registry_for(
    state: &SharedState,
    hub_key: &HubKey,
) -> Option<Arc<Mutex<dyn HubRegistry>>> {
    state.lock().ok().and_then(|s| s.hub_registry_for(hub_key))
}

/// Get all managed (typed) device IDs with their types from the registry.
///
/// Only returns devices with a `DeviceType` (Button, Motion) — not light
/// entity IDs which are room children managed by `upsert_room`.
/// When `hub_key` is provided, only returns devices from that hub's registry.
fn get_all_typed_device_ids_with_types(
    state: &SharedState,
    hub_key: &HubKey,
) -> Vec<(String, rhythm_core::runtime::hub_registry::DeviceType)> {
    let registry = extract_registry_for(state, hub_key);
    registry
        .and_then(|r| {
            r.lock().ok().map(|reg| {
                let mut devices = Vec::new();
                for room in reg.rooms() {
                    for (device_id, dt) in reg.devices_for_room_typed(&room.id) {
                        devices.push((device_id, dt));
                    }
                }
                devices
            })
        })
        .unwrap_or_default()
}

/// Poll the hub for each room's on/off state and populate observed power state.
///
/// Called after `sync_from_hub()` to fill in the initial light state so
/// the UI shows correct on/off status immediately instead of waiting for
/// the first periodic tick (~60s).
pub fn poll_initial_light_state(state: &SharedState) {
    let runtime = {
        let Ok(s) = state.lock() else { return };
        s.hub_runtime()
    };
    let Some(runtime) = runtime else { return };

    let snapshots = commands::refresh_all_lights_on_cache_for_runtime(
        state,
        &runtime,
        crate::state::ObservedPowerSource::SyncPoll,
    );
    if snapshots.is_empty() {
        return;
    }

    let mut on_count = 0usize;
    let mut room_count = 0usize;
    for snap in &snapshots {
        if !snap.kind.is_room() {
            continue;
        }

        room_count += 1;
        if state
            .lock()
            .ok()
            .and_then(|s| {
                s.room_observed_power
                    .get(&snap.id)
                    .map(|observed| observed.lights_on)
            })
            .unwrap_or(false)
        {
            on_count += 1;
        }
    }

    debug!(
        target: "room_sync",
        "Initial light state: {}/{} rooms have lights on",
        on_count,
        room_count
    );

    // Emit SSE so any already-connected clients get the initial state
    {
        let events: Vec<_> = snapshots
            .iter()
            .map(|s| {
                crate::commands::build_node_state_event(
                    state,
                    &rhythm_core::NodeSnapshot::from_room_snapshot(s.clone()),
                )
            })
            .collect();
        if !events.is_empty() {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::NodeState { nodes: events },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};
    use crate::hub::{ActiveHub, HubIntegrationCapability, HubType};
    use crate::state::AppState;
    use rhythm_core::runtime::hub_registry::DeviceType;

    struct MockDiscovery {
        rooms: Vec<DiscoveredRoom>,
        devices: Vec<DiscoveredDevice>,
    }

    impl HubDiscovery for MockDiscovery {
        fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
            Ok(self
                .rooms
                .iter()
                .map(|r| DiscoveredRoom {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    grouped_light_id: r.grouped_light_id.clone(),
                    device_ids: r.device_ids.clone(),
                })
                .collect())
        }

        fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
            Ok(self
                .devices
                .iter()
                .map(|d| DiscoveredDevice {
                    device_id: d.device_id.clone(),
                    room_id: d.room_id.clone(),
                    buttons: d.buttons.clone(),
                    device_type: d.device_type.clone(),
                })
                .collect())
        }
    }

    // Note: Full integration tests require a runtime + registry setup.
    // These are basic struct / trait validation tests.

    #[test]
    fn test_mock_discovery_returns_rooms() {
        let discovery = MockDiscovery {
            rooms: vec![DiscoveredRoom {
                id: "room-1".to_string(),
                name: "Living Room".to_string(),
                grouped_light_id: "gl-1".to_string(),
                device_ids: vec![],
            }],
            devices: vec![],
        };

        let rooms = discovery.discover_rooms().unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].name, "Living Room");
    }

    #[test]
    fn test_mock_discovery_returns_typed_devices() {
        let discovery = MockDiscovery {
            rooms: vec![],
            devices: vec![
                DiscoveredDevice {
                    device_id: "btn1".to_string(),
                    room_id: Some("r1".to_string()),
                    buttons: vec![("b1".to_string(), 1)],
                    device_type: DeviceType::Button,
                },
                DiscoveredDevice {
                    device_id: "ms1".to_string(),
                    room_id: Some("r1".to_string()),
                    buttons: vec![],
                    device_type: DeviceType::Motion,
                },
            ],
        };

        let devices = discovery.discover_devices().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].device_type, DeviceType::Button);
        assert_eq!(devices[1].device_type, DeviceType::Motion);
    }

    #[test]
    fn required_sync_waits_for_and_then_owns_the_exact_hub_slot() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let hub_key = HubKey::new(HubType::new(HubType::HUE), "bridge-wait");
        assert!(state.lock().unwrap().begin_hub_sync(&hub_key));

        let releasing_state = state.clone();
        let releasing_key = hub_key.clone();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            releasing_state
                .lock()
                .unwrap()
                .finish_hub_sync(&releasing_key);
        });

        let guard = acquire_hub_sync_guard_with_timeout(&state, &hub_key, Duration::from_secs(1))
            .expect("required sync should wait for the active sync");
        assert!(
            state
                .lock()
                .unwrap()
                .hub_sync_in_progress
                .contains(&hub_key),
            "the required sync should own the same hub slot before it starts discovery"
        );

        drop(guard);
        release.join().unwrap();
        assert!(!state
            .lock()
            .unwrap()
            .hub_sync_in_progress
            .contains(&hub_key));
    }

    #[test]
    fn authority_recovery_fence_blocks_discovery_before_state_replacement() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().authority_state_recovery_required = true;
        let hub_key = HubKey::new(HubType::new(HubType::HUE), "bridge-recovery");

        let error = sync_from_hub_for_key(&state, &hub_key, true).unwrap_err();

        assert!(error.to_string().contains("requires recovery"));
        assert!(!state
            .lock()
            .unwrap()
            .hub_sync_in_progress
            .contains(&hub_key));
    }

    #[test]
    fn authority_takeover_never_skips_device_identity_discovery() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let hub_key = HubKey::new(HubType::new(HubType::HUE), "bridge-partial");

        let error = sync_from_hub_for_key_before_authority(&state, &hub_key, false).unwrap_err();

        assert!(error.to_string().contains("Complete device discovery"));
    }

    #[test]
    fn completed_refresh_reconciles_authority_after_the_topology_guard_is_released() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let hub_key = HubKey::new(HubType::new(HubType::HUE), "bridge-refresh");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let calls = calls.clone();
            state
                .lock()
                .unwrap()
                .reconcile_external_controller_authority_fn = Some(Arc::new(move |state, _| {
                let transaction = state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("lock"))?
                    .external_topology_transaction_lock
                    .clone();
                let _guard = transaction
                    .try_lock()
                    .map_err(|_| anyhow::anyhow!("topology guard was still held after refresh"))?;
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }));
        }

        reconcile_external_controller_authority_after_sync(&state, &hub_key).unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        state
            .lock()
            .unwrap()
            .mark_external_controller_initial_sync_pending(&hub_key);
        reconcile_external_controller_authority_after_sync(&state, &hub_key).unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn required_sync_times_out_instead_of_reporting_busy_noop_as_success() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let hub_key = HubKey::new(HubType::new(HubType::HUE), "bridge-busy");
        assert!(state.lock().unwrap().begin_hub_sync(&hub_key));

        let started = Instant::now();
        let error = match acquire_hub_sync_guard_with_timeout(
            &state,
            &hub_key,
            Duration::from_millis(20),
        ) {
            Ok(_) => panic!("required sync must not turn a busy slot into an empty success"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("Timed out waiting"));
        assert!(started.elapsed() < Duration::from_millis(200));
        state.lock().unwrap().finish_hub_sync(&hub_key);
    }

    #[test]
    fn sync_preserves_assigned_room_for_roomless_typed_device() {
        let hub_type = HubType::new(HubType::HUE);
        let hub_key = HubKey::new(hub_type.clone(), "bridge-1");
        let registry: Arc<Mutex<dyn rhythm_core::HubRegistry>> = Arc::new(Mutex::new(
            crate::registry::HubDeviceRegistry::with_options(false),
        ));

        let mut app = AppState::default();
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key: hub_key.clone(),
                runtime: None,
                hub_data: Box::new(()),
                registry: Some(registry),
                discovery: None,
                shutdown: Default::default(),
            },
        );
        let state: SharedState = Arc::new(Mutex::new(app));

        let discovery = MockDiscovery {
            rooms: vec![],
            devices: vec![DiscoveredDevice {
                device_id: "motion-svc-1".to_string(),
                room_id: None,
                buttons: vec![],
                device_type: DeviceType::Motion,
            }],
        };

        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let canonical_id = {
            let s = state.lock().unwrap();
            s.canonical_registry
                .find_by_native_id(&hub_key, "motion-svc-1")
                .expect("roomless motion should create a canonical device")
                .id
                .clone()
        };
        let room_id = state.lock().unwrap().topology.create_room("Hallway");
        commands::do_canonical_assign_room(&state, &canonical_id, Some(&room_id)).unwrap();

        assert!(
            registry_devices_for_room_contains(&state, &hub_key, &room_id, "motion-svc-1"),
            "assignment should route the motion sensor before the next sync"
        );

        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        assert!(
            registry_devices_for_room_contains(&state, &hub_key, &room_id, "motion-svc-1"),
            "roomless rediscovery must not clear the assigned Rhythm room"
        );
    }

    #[test]
    fn roomless_hub_empty_room_discovery_preserves_existing_source_rooms() {
        let hub_type = HubType::new(HubType::MATTER);
        let hub_key = HubKey::new(hub_type.clone(), "local");
        let mut registry_impl = crate::registry::HubDeviceRegistry::with_options(false);
        let device_ids = vec!["matter-102".to_string()];
        registry_impl.upsert_room("matter-102", "Matter 102", "matter-102", &device_ids);
        let registry: Arc<Mutex<dyn rhythm_core::HubRegistry>> =
            Arc::new(Mutex::new(registry_impl));

        let mut app = AppState::default();
        let mut capability = HubIntegrationCapability::new(HubType::MATTER);
        capability.supports_roomless_devices = true;
        app.hub_capabilities.push(capability);
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key: hub_key.clone(),
                runtime: None,
                hub_data: Box::new(()),
                registry: Some(registry),
                discovery: None,
                shutdown: Default::default(),
            },
        );
        let state: SharedState = Arc::new(Mutex::new(app));

        let discovery = MockDiscovery {
            rooms: vec![],
            devices: vec![],
        };

        let report = sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        assert_eq!(report.rooms_removed, 0);
        assert!(
            state
                .lock()
                .unwrap()
                .hub_registry_for(&hub_key)
                .expect("matter registry should exist")
                .lock()
                .unwrap()
                .rooms()
                .iter()
                .any(|room| room.id == "matter-102"),
            "empty Matter room discovery must not prune existing source rooms"
        );
    }

    fn registry_devices_for_room_contains(
        state: &SharedState,
        hub_key: &HubKey,
        room_id: &str,
        device_id: &str,
    ) -> bool {
        state
            .lock()
            .unwrap()
            .hub_registry_for(hub_key)
            .expect("hub registry should exist")
            .lock()
            .unwrap()
            .devices_for_room(room_id)
            .iter()
            .any(|id| id == device_id)
    }

    /// Discovery that lets the test populate `discover_identities` directly
    /// instead of relying on the trait's `discover_devices` fallback (which
    /// produces empty room_name + manufacturer / model and exercises a
    /// different canonical resolve path).
    struct IdentityDiscovery {
        rooms: Vec<DiscoveredRoom>,
        identities: Vec<crate::canonical::identity::DiscoveredIdentity>,
    }

    impl HubDiscovery for IdentityDiscovery {
        fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
            Ok(self
                .rooms
                .iter()
                .map(|r| DiscoveredRoom {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    grouped_light_id: r.grouped_light_id.clone(),
                    device_ids: r.device_ids.clone(),
                })
                .collect())
        }

        fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
            Ok(self
                .identities
                .iter()
                .map(|d| DiscoveredDevice {
                    device_id: d.native_id.clone(),
                    room_id: d.room_id.clone(),
                    buttons: vec![],
                    device_type: d.device_type.clone(),
                })
                .collect())
        }

        fn discover_identities(
            &self,
        ) -> Result<Vec<crate::canonical::identity::DiscoveredIdentity>> {
            Ok(self.identities.clone())
        }
    }

    struct CapabilityIdentityDiscovery {
        rooms: Vec<DiscoveredRoom>,
        identities: Vec<crate::canonical::identity::DiscoveredIdentity>,
        endpoint_capabilities: HashMap<String, serde_json::Value>,
    }

    impl HubDiscovery for CapabilityIdentityDiscovery {
        fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
            Ok(self
                .rooms
                .iter()
                .map(|room| DiscoveredRoom {
                    id: room.id.clone(),
                    name: room.name.clone(),
                    grouped_light_id: room.grouped_light_id.clone(),
                    device_ids: room.device_ids.clone(),
                })
                .collect())
        }

        fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
            Ok(Vec::new())
        }

        fn discover_identities(
            &self,
        ) -> Result<Vec<crate::canonical::identity::DiscoveredIdentity>> {
            Ok(self.identities.clone())
        }

        fn endpoint_capabilities(&self, native_id: &str) -> Option<serde_json::Value> {
            self.endpoint_capabilities.get(native_id).cloned()
        }
    }

    fn make_identity(
        native_id: &str,
        hub_room_id: &str,
        hub_room_name: &str,
        name: &str,
        device_type: DeviceType,
    ) -> crate::canonical::identity::DiscoveredIdentity {
        crate::canonical::identity::DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: Some(hub_room_id.to_string()),
            room_name: Some(hub_room_name.to_string()),
            name: name.to_string(),
            device_type,
            hardware_ids: vec![],
            manufacturer: None,
            model: None,
        }
    }

    fn install_test_hub() -> (HubKey, SharedState) {
        let hub_type = HubType::new(HubType::HUE);
        let hub_key = HubKey::new(hub_type.clone(), "bridge-1");
        let registry: Arc<Mutex<dyn rhythm_core::HubRegistry>> = Arc::new(Mutex::new(
            crate::registry::HubDeviceRegistry::with_options(false),
        ));

        let mut app = AppState::default();
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key: hub_key.clone(),
                runtime: None,
                hub_data: Box::new(()),
                registry: Some(registry),
                discovery: None,
                shutdown: Default::default(),
            },
        );
        let state: SharedState = Arc::new(Mutex::new(app));
        (hub_key, state)
    }

    #[test]
    fn sync_persists_normalized_endpoint_capabilities_with_canonical_identity() {
        let (hub_key, state) = install_test_hub();
        let discovery = CapabilityIdentityDiscovery {
            rooms: vec![DiscoveredRoom {
                id: "future-room".to_string(),
                name: "Future Room".to_string(),
                grouped_light_id: "future-group".to_string(),
                device_ids: vec!["future-light".to_string()],
            }],
            identities: vec![make_identity(
                "future-light",
                "future-room",
                "Future Room",
                "Future Hue light",
                DeviceType::Light,
            )],
            endpoint_capabilities: HashMap::from([(
                "future-light".to_string(),
                serde_json::json!({
                    "light_capabilities": {
                        "color_temperature": {
                            "min_kelvin": 2000,
                            "max_kelvin": 6536
                        }
                    }
                }),
            )]),
        };

        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let state = state.lock().unwrap();
        let device = state
            .canonical_registry
            .find_by_native_id(&hub_key, "future-light")
            .expect("identity should resolve");
        let endpoint = device
            .endpoints
            .iter()
            .find(|endpoint| endpoint.hub_key == hub_key)
            .expect("Hue endpoint should be retained");
        assert_eq!(
            endpoint
                .capabilities
                .as_ref()
                .and_then(|value| value.pointer("/light_capabilities/color_temperature")),
            Some(&serde_json::json!({
                "min_kelvin": 2000,
                "max_kelvin": 6536
            }))
        );
        let serialized = serde_json::to_string(&state.canonical_registry).unwrap();
        drop(state);

        let mut restored: crate::canonical::registry::CanonicalRegistry =
            serde_json::from_str(&serialized).unwrap();
        restored.rebuild_indices();
        assert_eq!(
            restored
                .find_by_native_id(&hub_key, "future-light")
                .and_then(|device| {
                    device
                        .endpoints
                        .iter()
                        .find(|endpoint| endpoint.hub_key == hub_key)
                })
                .and_then(|endpoint| endpoint.capabilities.as_ref())
                .and_then(|value| value.pointer("/light_capabilities/color_temperature")),
            Some(&serde_json::json!({
                "min_kelvin": 2000,
                "max_kelvin": 6536
            })),
            "normalized Hue capabilities must survive authority-state restart"
        );
    }

    #[test]
    fn complete_room_discovery_materializes_legacy_hue_authority_without_device_discovery() {
        let (hub_key, state) = install_test_hub();
        {
            let mut state = state.lock().unwrap();
            state.topology = crate::topology::RoomTopologyStore::legacy_empty();
            state
                .topology
                .migrate_legacy_external_room_automation_policy(std::slice::from_ref(&hub_key));
        }
        let discovery = MockDiscovery {
            rooms: vec![DiscoveredRoom {
                id: "hue-office".to_string(),
                name: "Office".to_string(),
                grouped_light_id: "grouped-office".to_string(),
                device_ids: Vec::new(),
            }],
            devices: Vec::new(),
        };

        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            false,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let mut state = state.lock().unwrap();
        let room_id = state
            .topology
            .find_by_hub_room(&hub_key, "hue-office")
            .expect("discovered Hue room should be bound")
            .id
            .clone();
        assert_eq!(
            state
                .topology
                .external_room_automation_owner(&room_id, &hub_key),
            Some(crate::topology::ExternalRoomAutomationOwner::Rhythm)
        );
        assert!(state
            .topology
            .external_hub_has_full_rhythm_consent(&hub_key));
        assert!(
            !state
                .topology
                .materialize_grandfathered_external_room_automation_decisions(&hub_key),
            "the sync should already have retired the bootstrap marker"
        );

        let later = state.topology.sync_hub_room(
            &hub_key,
            &DiscoveredTopologyRoom {
                hub_room_id: "hue-later".to_string(),
                name: "Later".to_string(),
                control_id: "grouped-later".to_string(),
                light_device_ids: Vec::new(),
                canonical_device_ids: Vec::new(),
                source_name_authoritative: true,
            },
        );
        assert_eq!(
            state
                .topology
                .external_room_automation_owner(later.rhythm_room_id(), &hub_key),
            None
        );
    }

    #[test]
    fn failed_best_effort_room_apply_retains_legacy_hue_marker() {
        let hub_key = HubKey::new(HubType::new(HubType::HUE), "missing-hub");
        let mut app = AppState {
            topology: crate::topology::RoomTopologyStore::legacy_empty(),
            ..Default::default()
        };
        app.topology
            .migrate_legacy_external_room_automation_policy(std::slice::from_ref(&hub_key));
        let state: SharedState = Arc::new(Mutex::new(app));
        let discovery = MockDiscovery {
            rooms: vec![DiscoveredRoom {
                id: "hue-office".to_string(),
                name: "Office".to_string(),
                grouped_light_id: "grouped-office".to_string(),
                device_ids: Vec::new(),
            }],
            devices: Vec::new(),
        };

        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            false,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let state = state.lock().unwrap();
        assert!(state.topology.references_hub_key(&hub_key));
        assert!(!state.topology.structurally_references_hub_key(&hub_key));
        assert!(state
            .topology
            .find_by_hub_room(&hub_key, "hue-office")
            .is_none());
    }

    /// Issue #302 field reproducer: legacy Hue authority takeover can leave
    /// input devices canonically assigned while topology still marks them as
    /// standalone. Hue then reports the inputs without a native room, and the
    /// next sync must repair topology from the retained canonical room.
    #[test]
    fn roomless_hue_motion_inputs_repair_from_canonical_rooms() {
        let (hub_key, state) = install_test_hub();

        let rooms = || {
            vec![
                DiscoveredRoom {
                    id: "guest-bath-hue-id".to_string(),
                    name: "Guest Bath".to_string(),
                    grouped_light_id: "guest-bath-gl".to_string(),
                    device_ids: vec!["guest-bath-light".to_string()],
                },
                DiscoveredRoom {
                    id: "staircase-hue-id".to_string(),
                    name: "Staircase".to_string(),
                    grouped_light_id: "staircase-gl".to_string(),
                    device_ids: vec!["staircase-light".to_string()],
                },
            ]
        };
        let initial_discovery = IdentityDiscovery {
            rooms: rooms(),
            identities: vec![
                make_identity(
                    "guest-bath-light",
                    "guest-bath-hue-id",
                    "Guest Bath",
                    "Guest Bath light",
                    DeviceType::Light,
                ),
                make_identity(
                    "staircase-light",
                    "staircase-hue-id",
                    "Staircase",
                    "Staircase light",
                    DeviceType::Light,
                ),
                make_identity(
                    "guest-bath-motion",
                    "guest-bath-hue-id",
                    "Guest Bath",
                    "Guest Bath motion",
                    DeviceType::Motion,
                ),
                make_identity(
                    "stair-bottom-motion",
                    "staircase-hue-id",
                    "Staircase",
                    "Stair Bottom",
                    DeviceType::Motion,
                ),
                make_identity(
                    "stairs-top-motion",
                    "staircase-hue-id",
                    "Staircase",
                    "Stairs Top",
                    DeviceType::Motion,
                ),
            ],
        };

        sync_with_discovery(
            &state,
            &hub_key,
            &initial_discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let expected_assignments: Vec<(String, String)> = {
            let s = state.lock().unwrap();
            [
                ("guest-bath-motion", "guest-bath-hue-id"),
                ("stair-bottom-motion", "staircase-hue-id"),
                ("stairs-top-motion", "staircase-hue-id"),
            ]
            .into_iter()
            .map(|(native_id, hub_room_id)| {
                let canonical_id = s
                    .canonical_registry
                    .find_by_native_id(&hub_key, native_id)
                    .expect("motion sensor should resolve to a canonical device")
                    .id
                    .clone();
                let room_id = s
                    .topology
                    .find_by_hub_room(&hub_key, hub_room_id)
                    .expect("Hue room should resolve to a topology room")
                    .id
                    .clone();
                (canonical_id, room_id)
            })
            .collect()
        };

        // Reproduce the persisted split seen in DBG-612F97F7: the app's
        // canonical room is intact, but routing topology says standalone.
        {
            let mut s = state.lock().unwrap();
            for (canonical_id, _) in &expected_assignments {
                s.topology.ensure_standalone_device(canonical_id);
            }
        }

        let roomless_motion =
            |native_id: &str, name: &str| crate::canonical::identity::DiscoveredIdentity {
                native_id: native_id.to_string(),
                room_id: None,
                room_name: None,
                name: name.to_string(),
                device_type: DeviceType::Motion,
                hardware_ids: vec![],
                manufacturer: None,
                model: None,
            };
        let roomless_discovery = IdentityDiscovery {
            rooms: rooms(),
            identities: vec![
                make_identity(
                    "guest-bath-light",
                    "guest-bath-hue-id",
                    "Guest Bath",
                    "Guest Bath light",
                    DeviceType::Light,
                ),
                make_identity(
                    "staircase-light",
                    "staircase-hue-id",
                    "Staircase",
                    "Staircase light",
                    DeviceType::Light,
                ),
                roomless_motion("guest-bath-motion", "Guest Bath motion"),
                roomless_motion("stair-bottom-motion", "Stair Bottom"),
                roomless_motion("stairs-top-motion", "Stairs Top"),
            ],
        };

        sync_with_discovery(
            &state,
            &hub_key,
            &roomless_discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let serialized_topology = {
            let s = state.lock().unwrap();
            for (canonical_id, expected_room_id) in &expected_assignments {
                let node = s
                    .topology
                    .get_device_node(canonical_id)
                    .expect("roomless motion should remain a first-class topology node");
                assert_eq!(node.parent_id.as_deref(), Some(expected_room_id.as_str()));
                assert_eq!(
                    s.topology.effective_control_target(
                        canonical_id,
                        &crate::topology::NodeControlKind::Motion
                    ),
                    Some(expected_room_id.clone()),
                    "roomless Hue motion should route to its canonical current room"
                );
            }
            serde_json::to_string(&s.topology).unwrap()
        };

        let restored: crate::topology::RoomTopologyStore =
            serde_json::from_str(&serialized_topology).unwrap();
        for (canonical_id, expected_room_id) in &expected_assignments {
            assert_eq!(
                restored
                    .get_device_node(canonical_id)
                    .and_then(|node| node.parent_id.as_deref()),
                Some(expected_room_id.as_str()),
                "repaired motion routing must survive authority-state reload"
            );
        }
    }

    #[test]
    fn sync_filters_room_binding_light_ids_from_typed_identities() {
        let (hub_key, state) = install_test_hub();

        let discovery = IdentityDiscovery {
            rooms: vec![
                DiscoveredRoom {
                    id: "stairwell-hue-id".to_string(),
                    name: "Stairwell".to_string(),
                    grouped_light_id: "stairwell-gl".to_string(),
                    device_ids: vec![
                        "hue-light-1".to_string(),
                        "hue-button-device-1".to_string(),
                        "hue-motion-device-1".to_string(),
                    ],
                },
                DiscoveredRoom {
                    id: "sensor-only-hue-id".to_string(),
                    name: "Sensor Only".to_string(),
                    grouped_light_id: "sensor-only-gl".to_string(),
                    device_ids: vec![
                        "hue-button-device-2".to_string(),
                        "hue-motion-device-2".to_string(),
                    ],
                },
            ],
            identities: vec![
                make_identity(
                    "hue-light-1",
                    "stairwell-hue-id",
                    "Stairwell",
                    "Stairwell light",
                    DeviceType::Light,
                ),
                make_identity(
                    "hue-button-device-1",
                    "stairwell-hue-id",
                    "Stairwell",
                    "Stairwell dimmer",
                    DeviceType::Button,
                ),
                make_identity(
                    "hue-motion-service-1",
                    "stairwell-hue-id",
                    "Stairwell",
                    "Stairwell motion",
                    DeviceType::Motion,
                ),
                make_identity(
                    "hue-button-device-2",
                    "sensor-only-hue-id",
                    "Sensor Only",
                    "Sensor Only dimmer",
                    DeviceType::Button,
                ),
                make_identity(
                    "hue-motion-service-2",
                    "sensor-only-hue-id",
                    "Sensor Only",
                    "Sensor Only motion",
                    DeviceType::Motion,
                ),
            ],
        };

        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let s = state.lock().unwrap();
        let room = s
            .topology
            .find_by_hub_room(&hub_key, "stairwell-hue-id")
            .expect("stairwell should be mapped into topology");
        let binding = room
            .binding_for_hub_room(&hub_key, "stairwell-hue-id")
            .expect("stairwell should retain its hub binding");

        assert_eq!(
            binding.light_device_ids,
            vec!["hue-light-1".to_string()],
            "Hue room children include controls and sensors, but binding light ids must not"
        );
        assert_eq!(
            room.devices.len(),
            3,
            "typed canonical devices should still be attached to the topology room"
        );

        let sensor_only_room = s
            .topology
            .find_by_hub_room(&hub_key, "sensor-only-hue-id")
            .expect("sensor-only room should be mapped into topology");
        let sensor_only_binding = sensor_only_room
            .binding_for_hub_room(&hub_key, "sensor-only-hue-id")
            .expect("sensor-only room should retain its hub binding");

        assert!(
            sensor_only_binding.light_device_ids.is_empty(),
            "rooms with typed identity discovery but no lights must not fall back to all Hue children"
        );
    }

    #[test]
    fn light_unassigned_by_user_stays_canonically_standalone_after_rediscovery() {
        let (hub_key, state) = install_test_hub();
        let discovery = IdentityDiscovery {
            rooms: vec![DiscoveredRoom {
                id: "office-hue-id".to_string(),
                name: "Office".to_string(),
                grouped_light_id: "office-gl".to_string(),
                device_ids: vec!["hue-light-1".to_string()],
            }],
            identities: vec![make_identity(
                "hue-light-1",
                "office-hue-id",
                "Office",
                "Desk bulb",
                DeviceType::Light,
            )],
        };

        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();
        let canonical_id = state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key, "hue-light-1")
            .unwrap()
            .id
            .clone();

        commands::do_canonical_assign_room(&state, &canonical_id, None).unwrap();
        // Simulate Hue out-of-band drift: discovery still reports the bulb in
        // the managed Office room. Rhythm's explicit standalone placement must
        // remain authoritative at both topology and canonical layers.
        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let state = state.lock().unwrap();
        let node = state.topology.get_device_node(&canonical_id).unwrap();
        assert_eq!(node.parent_id, None);
        assert_eq!(
            node.placement,
            crate::topology::DevicePlacement::UserOverride
        );
        assert_eq!(
            state
                .canonical_registry
                .get(&canonical_id)
                .unwrap()
                .room_id,
            None,
            "canonical fallback must not turn an explicit standalone bulb back into an assigned bulb"
        );
    }

    /// Issue #43 reproducer: unassigning a motion sensor (parent=None) marks
    /// it Standalone, but the next sync re-attaches it to the hub-default
    /// room as HubDefault — so the user's "remove from room" action silently
    /// reverts.
    #[test]
    fn motion_sensor_unassigned_does_not_get_reattached_by_sync() {
        let (hub_key, state) = install_test_hub();

        let discovery = IdentityDiscovery {
            rooms: vec![DiscoveredRoom {
                id: "balcony-hue-id".to_string(),
                name: "Balcony".to_string(),
                grouped_light_id: "balcony-gl".to_string(),
                device_ids: vec![],
            }],
            identities: vec![make_identity(
                "hue-motion-1",
                "balcony-hue-id",
                "Balcony",
                "Master",
                DeviceType::Motion,
            )],
        };

        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let canonical_id = state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key, "hue-motion-1")
            .expect("master sensor should resolve to a canonical device")
            .id
            .clone();

        // User unassigns the device — equivalent to PUT /parent with parent_id: null.
        commands::do_canonical_assign_room(&state, &canonical_id, None).unwrap();

        {
            let s = state.lock().unwrap();
            let node = s
                .topology
                .get_device_node(&canonical_id)
                .expect("master device node after unassign");
            assert_eq!(node.parent_id, None);
            // Explicit user-unassign is a UserOverride with no parent, so the
            // next sync's hub-default re-attach skips it.
            assert_eq!(
                node.placement,
                crate::topology::DevicePlacement::UserOverride
            );
        }

        // Next hub sync. Hue still reports the sensor in Balcony.
        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let s = state.lock().unwrap();
        let node = s
            .topology
            .get_device_node(&canonical_id)
            .expect("master device node after resync");
        assert_eq!(
            node.parent_id, None,
            "user's unassignment must not be reverted by hub rediscovery"
        );
        assert_eq!(
            node.placement,
            crate::topology::DevicePlacement::UserOverride,
            "placement must stay UserOverride+None after rediscovery"
        );
    }

    /// Issue #43: a motion sensor moved out of its hub-default Rhythm room
    /// should not revert to that room on the next sync from the hub.
    #[test]
    fn motion_sensor_moved_out_of_hub_room_stays_put_after_resync() {
        let (hub_key, state) = install_test_hub();

        let discovery = IdentityDiscovery {
            rooms: vec![DiscoveredRoom {
                id: "balcony-hue-id".to_string(),
                name: "Balcony".to_string(),
                grouped_light_id: "balcony-gl".to_string(),
                device_ids: vec![],
            }],
            identities: vec![make_identity(
                "hue-motion-1",
                "balcony-hue-id",
                "Balcony",
                "Master",
                DeviceType::Motion,
            )],
        };

        // Initial sync: motion sensor lands in Balcony as hub-default.
        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let canonical_id = state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key, "hue-motion-1")
            .expect("master sensor should resolve to a canonical device")
            .id
            .clone();
        let balcony_topology_id = state
            .lock()
            .unwrap()
            .topology
            .find_by_hub_room(&hub_key, "balcony-hue-id")
            .expect("balcony should be mapped into topology")
            .id
            .clone();

        {
            let s = state.lock().unwrap();
            let node = s
                .topology
                .get_device_node(&canonical_id)
                .expect("master device node");
            assert_eq!(
                node.parent_id.as_deref(),
                Some(balcony_topology_id.as_str())
            );
            assert_eq!(node.placement, crate::topology::DevicePlacement::HubDefault);
        }

        // User creates a separate Rhythm room and moves the sensor into it.
        let master_room_id = state.lock().unwrap().topology.create_room("Master Room");
        commands::do_canonical_assign_room(&state, &canonical_id, Some(&master_room_id)).unwrap();

        {
            let s = state.lock().unwrap();
            let node = s
                .topology
                .get_device_node(&canonical_id)
                .expect("master device node after move");
            assert_eq!(node.parent_id.as_deref(), Some(master_room_id.as_str()));
            assert_eq!(
                node.placement,
                crate::topology::DevicePlacement::UserOverride
            );
            assert_eq!(
                s.canonical_registry
                    .get(&canonical_id)
                    .unwrap()
                    .room_id
                    .as_deref(),
                Some(master_room_id.as_str())
            );
        }

        // Hue still reports the sensor in Balcony. The next sync must not
        // reset the user's chosen Rhythm room.
        sync_with_discovery(
            &state,
            &hub_key,
            &discovery,
            true,
            SyncFailurePolicy::BestEffort,
        )
        .unwrap();

        let s = state.lock().unwrap();
        let node = s
            .topology
            .get_device_node(&canonical_id)
            .expect("master device node after resync");
        assert_eq!(
            node.parent_id.as_deref(),
            Some(master_room_id.as_str()),
            "topology must keep the sensor in the user's chosen room"
        );
        assert_eq!(
            node.placement,
            crate::topology::DevicePlacement::UserOverride,
            "placement must stay UserOverride after rediscovery"
        );
        assert_eq!(
            s.canonical_registry
                .get(&canonical_id)
                .unwrap()
                .room_id
                .as_deref(),
            Some(master_room_id.as_str()),
            "canonical room_id must follow topology, not the hub's reported room"
        );
    }
}
