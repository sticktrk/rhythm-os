//! Server-driven room sync orchestrator.
//!
//! Discovers rooms and devices from the hub and diffs them against current
//! state. Adds new rooms, updates changed ones, removes stale ones, and
//! preserves user state (rhythm_enabled, offsets, etc.).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::Result;
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
    let Some(_guard) = try_acquire_hub_sync_guard(state, hub_key)? else {
        debug!(target: "room_sync", "Skipping sync for {}: already in progress", hub_key);
        return Ok(SyncReport::default());
    };

    let discovery = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hubs
            .get(hub_key)
            .and_then(|h| h.discovery.as_ref().cloned())
    }
    .ok_or_else(|| anyhow::anyhow!("No hub discovery for {}", hub_key))?;

    let report = sync_with_discovery(state, hub_key, discovery.as_ref(), discover_devices)?;
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
) -> Result<SyncReport> {
    // ========================================================================
    // Phase 1: Discover rooms
    // ========================================================================
    let discovered_rooms = discovery.discover_rooms()?;
    info!(target: "room_sync", "Discovered {} rooms from hub", discovered_rooms.len());

    // ========================================================================
    // Phase 2: Diff and apply source rooms into registry + topology only.
    // Runtime materialization is an explicit final sync phase.
    // ========================================================================
    let current_room_ids = {
        let registry = extract_registry_for(state, hub_key);
        let room_ids: HashSet<String> = registry
            .and_then(|r| {
                r.lock()
                    .ok()
                    .map(|reg| reg.rooms().into_iter().map(|r| r.id).collect())
            })
            .unwrap_or_default();
        room_ids
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
                warn!(target: "room_sync", "Failed to set room '{}': {}", room.name, e);
            }
        }
    }

    // Remove stale source rooms from the hub registry.
    //
    // If the hub has never reported any rooms, treat an empty discovery result
    // as "this hub doesn't do room discovery" and no-op. If it previously had
    // rooms, an empty discovery result means all source rooms disappeared and
    // should be pruned.
    let should_prune_stale_rooms = !discovered_ids.is_empty() || !current_room_ids.is_empty();
    if should_prune_stale_rooms {
        let stale_ids: Vec<String> = current_room_ids
            .difference(&discovered_ids)
            .cloned()
            .collect();

        let registry = extract_registry_for(state, hub_key);
        for room_id in stale_ids {
            info!(target: "room_sync", "Removing stale source room '{}'", room_id);
            let Some(registry) = registry.as_ref() else {
                warn!(target: "room_sync", "No hub registry available for stale room '{}'", room_id);
                continue;
            };
            match registry.lock() {
                Ok(mut reg) => {
                    reg.remove_room(&room_id);
                    report.rooms_removed += 1;
                }
                Err(_) => {
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
                commands::persist_topology(&s);
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
                warn!(target: "room_sync", "Identity discovery failed: {}", e);
                (Vec::new(), false)
            }
        };
        let discovered_native_ids: HashSet<String> = identities
            .iter()
            .map(|identity| identity.native_id.clone())
            .collect();
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
                    let result = s
                        .canonical_registry
                        .resolve(identity, &canonical_hub_key, now);
                    let canonical_id = match result {
                        ResolveResult::AlreadyKnown { canonical_id } => canonical_id,
                        ResolveResult::ReApproved { canonical_id } => canonical_id,
                        ResolveResult::Queued { .. } => continue,
                        ResolveResult::Created { canonical_id } => canonical_id,
                    };

                    let Some(hub_room_id) = identity.room_id.clone() else {
                        let still_unassigned = s
                            .canonical_registry
                            .get(&canonical_id)
                            .map(|device| device.room_id.is_none())
                            .unwrap_or(false);
                        if still_unassigned {
                            s.canonical_registry.assign_room(&canonical_id, None);
                            s.topology.ensure_standalone_device(&canonical_id);
                        }
                        continue;
                    };

                    canonical_room_devices
                        .entry(hub_room_id)
                        .or_default()
                        .push(canonical_id);
                }

                // ============================================================
                // Phase 4c: Topology sync
                // ============================================================
                for room in &discovered_rooms {
                    let canonical_device_ids = canonical_room_devices
                        .get(&room.id)
                        .cloned()
                        .unwrap_or_default();

                    let topo_room = DiscoveredTopologyRoom {
                        hub_room_id: room.id.clone(),
                        name: room.name.clone(),
                        control_id: room.grouped_light_id.clone(),
                        light_device_ids: room.device_ids.clone(),
                        canonical_device_ids: canonical_device_ids.clone(),
                    };
                    let canonical_registry = s.canonical_registry.clone();
                    let action = s.topology.sync_hub_room_with_registry(
                        &canonical_hub_key,
                        &topo_room,
                        &canonical_registry,
                    );
                    let rhythm_room_id = action.rhythm_room_id().to_string();
                    let canonical_room_assignments: Vec<(String, String)> = canonical_device_ids
                        .iter()
                        .map(|canonical_id| {
                            let assigned_room_id = s
                                .topology
                                .device_parent_room_id(canonical_id)
                                .map(str::to_string)
                                .or_else(|| {
                                    s.canonical_registry
                                        .get(canonical_id)
                                        .and_then(|device| device.room_id.clone())
                                })
                                .unwrap_or_else(|| rhythm_room_id.clone());
                            (canonical_id.clone(), assigned_room_id)
                        })
                        .collect();

                    for (canonical_id, assigned_room_id) in &canonical_room_assignments {
                        s.canonical_registry
                            .assign_room(canonical_id, Some(assigned_room_id));
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
                                    light_device_ids: room.device_ids.clone(),
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

                // Persist canonical registry and topology
                commands::persist_canonical(&s);
                commands::persist_topology(&s);
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
                    commands::persist_canonical(&s);
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
            if let Some(registry) = extract_registry_for(state, hub_key) {
                if let Ok(mut reg) = registry.lock() {
                    for ms in &motion_states {
                        reg.upsert_device(
                            &ms.sensor_id,
                            Some(&ms.room_id),
                            &[],
                            DeviceType::Motion,
                        );
                    }
                }
            }
            // Track Phase 5 sensors so stale removal doesn't undo them
            discovered_types.insert(DeviceType::Motion);
            for ms in &motion_states {
                discovered_device_ids.insert(ms.sensor_id.clone());
            }
            // Seed active sensors into AppState for the event loop to pick up.
            // Resolve to public source/target node IDs up front so startup
            // motion state follows the same node-control graph as live events.
            let active: Vec<(String, String)> = motion_states
                .iter()
                .filter(|ms| ms.is_active)
                .filter_map(|ms| {
                    commands::resolve_node_control_target(
                        state,
                        hub_key,
                        &ms.sensor_id,
                        &crate::topology::NodeControlKind::Motion,
                    )
                })
                .collect();
            if !active.is_empty() {
                if let Ok(mut s) = state.lock() {
                    s.pending_motion_seed = active;
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
    use crate::hub::{ActiveHub, HubType};
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

        sync_with_discovery(&state, &hub_key, &discovery, true).unwrap();

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

        sync_with_discovery(&state, &hub_key, &discovery, true).unwrap();

        assert!(
            registry_devices_for_room_contains(&state, &hub_key, &room_id, "motion-svc-1"),
            "roomless rediscovery must not clear the assigned Rhythm room"
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
}
