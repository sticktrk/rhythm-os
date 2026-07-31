//! Canonical device registry and deduplication engine.
//!
//! The canonical registry is the authoritative source of truth for all
//! physical devices. It assigns each device a stable Rhythm UUID and
//! deduplicates across integrations using hardware identifiers.

use std::collections::{HashMap, HashSet};

use log::{debug, info};
use serde::{Deserialize, Serialize};

use rhythm_core::runtime::hub_registry::DeviceType;

use super::identity::{CanonicalDevice, DiscoveredIdentity, HardwareId, HubKey};
use super::triage::{
    CandidateMatch, MatchReason, TriageDiscoveredDevice, TriageEntry, TriageKind, TriageQueue,
    TriageStatus,
};

/// Result of resolving a discovered device against the canonical registry.
#[derive(Debug)]
pub enum ResolveResult {
    /// Device already registered on this hub — updated last_seen.
    AlreadyKnown { canonical_id: String },
    /// Previously-approved merge silently re-applied on re-sync.
    ReApproved { canonical_id: String },
    /// Cross-hub match found — queued for user approval.
    Queued { triage_entry_id: String },
    /// No match — created new canonical device (works immediately in its hub's silo).
    Created { canonical_id: String },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EndpointVisibilityReport {
    pub affected_device_ids: Vec<String>,
    pub hidden_device_ids: Vec<String>,
}

/// The canonical device registry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanonicalRegistry {
    /// All canonical devices, keyed by Rhythm UUID.
    devices: HashMap<String, CanonicalDevice>,
    /// Triage queue for ambiguous matches.
    triage: TriageQueue,
    /// Index: hardware ID value → canonical device ID (for fast lookup).
    #[serde(skip)]
    hw_index: HashMap<String, String>,
    /// Index: (hub_key display, native_id) → canonical device ID.
    #[serde(skip)]
    native_index: HashMap<(String, String), String>,
}

impl Default for CanonicalRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CanonicalRegistry {
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
            triage: TriageQueue::new(),
            hw_index: HashMap::new(),
            native_index: HashMap::new(),
        }
    }

    /// Rebuild indices from the device map. Call after deserialization.
    pub fn rebuild_indices(&mut self) {
        self.hw_index.clear();
        self.native_index.clear();

        for (id, device) in &self.devices {
            for hw_id in &device.hardware_ids {
                self.hw_index.insert(hw_id.value().to_string(), id.clone());
            }
            for ep in &device.endpoints {
                self.native_index
                    .insert((ep.hub_key.to_string(), ep.native_id.clone()), id.clone());
            }
        }
    }

    /// Resolve a discovered device against the canonical registry.
    ///
    /// All cross-hub merging requires user approval. The algorithm:
    /// 1. Native ID lookup — already registered on this hub? (same-hub, no triage)
    /// 2. Exact hardware match — check if previously approved (re-apply silently),
    ///    otherwise queue high-confidence triage entry
    /// 3. Heuristic scoring — queue for user review if candidates found
    /// 4. Create new canonical device (silo-first: device works immediately)
    ///
    /// Phases 2 and 3 queue triage entries but always fall through to Phase 4
    /// so the device is created and functional in its hub's silo while awaiting
    /// user approval.
    pub fn resolve(
        &mut self,
        identity: &DiscoveredIdentity,
        hub_key: &HubKey,
        now: u64,
    ) -> ResolveResult {
        let room_name = identity
            .room_name
            .as_ref()
            .filter(|n| !n.is_empty())
            .cloned();

        // Phase 1: Native ID lookup (same hub, already known)
        let native_key = (hub_key.to_string(), identity.native_id.clone());
        if let Some(canonical_id) = self.native_index.get(&native_key).cloned() {
            if let Some(device) = self.devices.get_mut(&canonical_id) {
                if !device.is_removed() {
                    device.upsert_endpoint(
                        hub_key.clone(),
                        identity.native_id.clone(),
                        now,
                        room_name,
                    );
                    // Update hardware IDs if we have new ones
                    for hw_id in &identity.hardware_ids {
                        if !device.has_hardware_id(hw_id) {
                            device.hardware_ids.push(hw_id.clone());
                            self.hw_index
                                .insert(hw_id.value().to_string(), canonical_id.clone());
                        }
                    }
                    debug!(target: "canonical", "Already known: {} → {}", identity.native_id, canonical_id);
                    return ResolveResult::AlreadyKnown { canonical_id };
                }
            }
        }

        // Phase 2: Exact hardware match — requires user approval (unless previously approved)
        let mut hw_triage_queued = false;
        for hw_id in &identity.hardware_ids {
            if let Some(canonical_id) = self.hw_index.get(hw_id.value()).cloned() {
                if let Some(device) = self.devices.get(&canonical_id) {
                    // Skip soft-deleted devices and type mismatches
                    if device.is_removed() || device.device_type != identity.device_type {
                        continue;
                    }

                    // Same-hub silent re-key: a hardware match against an existing
                    // endpoint on the *same* hub with a *different* native_id is the
                    // same physical device whose hub-side rid has shifted (e.g. the
                    // Hue motion-sensor service-rid alignment). Update in place
                    // rather than queueing triage.
                    let stale_native_id = device
                        .endpoints
                        .iter()
                        .find(|ep| ep.hub_key == *hub_key && ep.native_id != identity.native_id)
                        .map(|ep| ep.native_id.clone());
                    if let Some(stale) = stale_native_id {
                        let device = self.devices.get_mut(&canonical_id).unwrap();
                        if let Some(ep) = device
                            .endpoints
                            .iter_mut()
                            .find(|ep| ep.hub_key == *hub_key && ep.native_id == stale)
                        {
                            ep.native_id = identity.native_id.clone();
                            ep.last_seen = now;
                            ep.active = true;
                            if room_name.is_some() {
                                ep.source_room_name = room_name.clone();
                            }
                        }
                        for new_hw in &identity.hardware_ids {
                            if !device.has_hardware_id(new_hw) {
                                device.hardware_ids.push(new_hw.clone());
                                self.hw_index
                                    .insert(new_hw.value().to_string(), canonical_id.clone());
                            }
                        }
                        self.native_index
                            .remove(&(hub_key.to_string(), stale.clone()));
                        self.native_index
                            .insert(native_key.clone(), canonical_id.clone());
                        info!(target: "canonical",
                            "Silently re-keyed endpoint on same hub: '{}' (HW: {}, {} → {})",
                            device.name, hw_id.value(), stale, identity.native_id
                        );
                        return ResolveResult::AlreadyKnown { canonical_id };
                    }

                    // Check if this exact merge was previously approved
                    let previously_approved = device.merge_history.iter().any(|m| {
                        m.source_hub == *hub_key && m.source_native_id == identity.native_id
                    });

                    if previously_approved {
                        // Re-apply silently (re-sync of approved merge)
                        let device = self.devices.get_mut(&canonical_id).unwrap();
                        device.upsert_endpoint(
                            hub_key.clone(),
                            identity.native_id.clone(),
                            now,
                            room_name,
                        );
                        // Add any new hardware IDs
                        for new_hw in &identity.hardware_ids {
                            if !device.has_hardware_id(new_hw) {
                                device.hardware_ids.push(new_hw.clone());
                                self.hw_index
                                    .insert(new_hw.value().to_string(), canonical_id.clone());
                            }
                        }
                        self.native_index.insert(native_key, canonical_id.clone());
                        info!(target: "canonical",
                            "Re-applied approved merge: '{}' → '{}' (HW: {})",
                            identity.name, device.name, hw_id.value()
                        );
                        return ResolveResult::ReApproved { canonical_id };
                    }

                    // Queue for user approval (high confidence)
                    if !self.triage.has_device(hub_key, &identity.native_id) {
                        let entry = TriageEntry {
                            id: format!("triage-{}-{}", now, identity.native_id),
                            kind: TriageKind::DeviceMerge,
                            discovered: TriageDiscoveredDevice::from(identity),
                            hub_key: hub_key.clone(),
                            candidate_matches: vec![CandidateMatch {
                                canonical_id: canonical_id.clone(),
                                name: device.name.clone(),
                                score: 100,
                                reasons: vec![MatchReason::ExactHardware],
                            }],
                            room_binding: None,
                            confidence: 100,
                            status: TriageStatus::Pending,
                            resolved_by: None,
                            created_at: now,
                            resolved_at: None,
                            canonical_id: None,
                        };
                        self.triage.add(entry);
                        info!(target: "canonical",
                            "Queued HW match for approval: '{}' ↔ '{}' ({})",
                            identity.name, device.name, hw_id.value()
                        );
                        hw_triage_queued = true;
                    }
                    // Don't return — fall through to Phase 4 to create silo device
                    break;
                }
            }
        }

        // Phase 3: Heuristic scoring — all matches require user approval
        if !hw_triage_queued && identity.hardware_ids.is_empty() {
            let candidates = self.heuristic_candidates(identity, hub_key);

            if !candidates.is_empty() && !self.triage.has_device(hub_key, &identity.native_id) {
                let confidence = candidates[0].score;
                let entry = TriageEntry {
                    id: format!("triage-{}-{}", now, identity.native_id),
                    kind: TriageKind::DeviceMerge,
                    discovered: TriageDiscoveredDevice::from(identity),
                    hub_key: hub_key.clone(),
                    candidate_matches: candidates,
                    room_binding: None,
                    confidence,
                    status: TriageStatus::Pending,
                    resolved_by: None,
                    created_at: now,
                    resolved_at: None,
                    canonical_id: None,
                };
                self.triage.add(entry);
                debug!(target: "canonical",
                    "Queued heuristic match for approval: '{}'",
                    identity.name
                );
            }
        }

        // Phase 4: Create new canonical device (silo-first)
        //
        // The device works immediately in its hub's silo. If a triage entry was
        // queued in Phase 2 or 3, the user can later approve the merge, at which
        // point the silo device's endpoint gets moved to the matched canonical device.
        let mut device = CanonicalDevice::new(
            identity.name.clone(),
            identity.device_type.clone(),
            identity.hardware_ids.clone(),
            now,
        );
        device.manufacturer = identity.manufacturer.clone();
        device.model = identity.model.clone();

        let canonical_id = device.id.clone();
        self.devices.insert(canonical_id.clone(), device);

        // Index hardware IDs
        for hw_id in &identity.hardware_ids {
            self.hw_index
                .insert(hw_id.value().to_string(), canonical_id.clone());
        }

        // Add the endpoint
        if let Some(device) = self.devices.get_mut(&canonical_id) {
            device.upsert_endpoint(hub_key.clone(), identity.native_id.clone(), now, room_name);
        }
        self.native_index.insert(native_key, canonical_id.clone());

        if hw_triage_queued {
            info!(target: "canonical",
                "Created silo device '{}' ({}) — pending cross-hub merge approval",
                identity.name, canonical_id
            );
        } else {
            info!(target: "canonical", "Created new device '{}' ({})", identity.name, canonical_id);
        }
        ResolveResult::Created { canonical_id }
    }

    /// Find heuristic match candidates from OTHER hubs based on name, type,
    /// manufacturer/model, and room context.
    ///
    /// Only considers devices that have at least one endpoint on a different hub
    /// than the discovering hub. Same-hub devices are never triage candidates.
    ///
    /// Returns scored candidates with reason codes. Only includes candidates
    /// that scored above the minimum threshold (3).
    fn heuristic_candidates(
        &self,
        identity: &DiscoveredIdentity,
        hub_key: &HubKey,
    ) -> Vec<CandidateMatch> {
        let mut candidates = Vec::new();
        let identity_room_lower = identity
            .room_name
            .as_deref()
            .map(str::to_lowercase)
            .unwrap_or_default();
        let has_room_name = !identity_room_lower.is_empty();

        for (id, device) in &self.devices {
            // Skip soft-deleted devices
            if device.is_removed() {
                continue;
            }
            // Must be same device type
            if device.device_type != identity.device_type {
                continue;
            }
            // Skip devices that only exist on the same hub (not cross-hub)
            let has_other_hub = device.endpoints.iter().any(|ep| &ep.hub_key != hub_key);
            if !has_other_hub {
                continue;
            }

            let mut score = 0u32;
            let mut reasons = Vec::new();

            reasons.push(MatchReason::SameDeviceType);

            // Room match: check if any endpoint's source_room_name matches
            if has_room_name {
                let room_match = device.endpoints.iter().any(|ep| {
                    ep.source_room_name
                        .as_ref()
                        .map(|n| n.to_lowercase() == identity_room_lower)
                        .unwrap_or(false)
                });
                if room_match {
                    score += 3;
                    reasons.push(MatchReason::SameRoom);
                }
            }

            // Name similarity (simple case-insensitive comparison)
            if device.name.to_lowercase() == identity.name.to_lowercase() {
                score += 4;
                reasons.push(MatchReason::ExactName);
            } else if device
                .name
                .to_lowercase()
                .contains(&identity.name.to_lowercase())
                || identity
                    .name
                    .to_lowercase()
                    .contains(&device.name.to_lowercase())
            {
                score += 2;
                reasons.push(MatchReason::PartialName);
            }

            // Manufacturer match
            if let (Some(a), Some(b)) = (&device.manufacturer, &identity.manufacturer) {
                if a.to_lowercase() == b.to_lowercase() {
                    score += 2;
                    reasons.push(MatchReason::SameManufacturer);
                }
            }

            // Model match
            if let (Some(a), Some(b)) = (&device.model, &identity.model) {
                if a.to_lowercase() == b.to_lowercase() {
                    score += 2;
                    reasons.push(MatchReason::SameModel);
                }
            }

            if score >= 3 {
                candidates.push(CandidateMatch {
                    canonical_id: id.clone(),
                    name: device.name.clone(),
                    score,
                    reasons,
                });
            }
        }

        // Sort by score descending
        candidates.sort_by(|a, b| b.score.cmp(&a.score));
        candidates
    }

    /// Complete a triage entry by merging the discovered device with a canonical device.
    pub fn complete_merge(
        &mut self,
        triage_entry_id: &str,
        canonical_id: &str,
        hub_key: &HubKey,
        now: u64,
    ) -> bool {
        let entry = match self.triage.get(triage_entry_id) {
            Some(e) => e.clone(),
            None => return false,
        };

        if let Some(device) = self.devices.get_mut(canonical_id) {
            let source_room = if entry.discovered.room_name.is_empty() {
                None
            } else {
                Some(entry.discovered.room_name.clone())
            };
            device.upsert_endpoint(
                hub_key.clone(),
                entry.discovered.native_id.clone(),
                now,
                source_room,
            );
            device.record_merge(
                hub_key.clone(),
                entry.discovered.native_id.clone(),
                "triage_confirmed".to_string(),
                now,
            );
            let native_key = (hub_key.to_string(), entry.discovered.native_id.clone());
            self.native_index
                .insert(native_key, canonical_id.to_string());
            self.triage
                .resolve(triage_entry_id, TriageStatus::Confirmed, "api", now);
            true
        } else {
            false
        }
    }

    /// Complete a triage entry by creating a new canonical device.
    pub fn complete_new_device(
        &mut self,
        triage_entry_id: &str,
        hub_key: &HubKey,
        now: u64,
    ) -> Option<String> {
        let entry = match self.triage.get(triage_entry_id) {
            Some(e) => e.clone(),
            None => return None,
        };

        let native_key = (hub_key.to_string(), entry.discovered.native_id.clone());
        if let Some(existing_id) = self.native_index.get(&native_key).cloned() {
            if let Some(device) = self.devices.get_mut(&existing_id) {
                if !device.is_removed() {
                    let source_room = if entry.discovered.room_name.is_empty() {
                        None
                    } else {
                        Some(entry.discovered.room_name.clone())
                    };
                    device.name = entry.discovered.name.clone();
                    if device.manufacturer.is_none() {
                        device.manufacturer = entry.discovered.manufacturer.clone();
                    }
                    if device.model.is_none() {
                        device.model = entry.discovered.model.clone();
                    }
                    device.upsert_endpoint(
                        hub_key.clone(),
                        entry.discovered.native_id.clone(),
                        now,
                        source_room,
                    );
                    self.triage.resolve_new(triage_entry_id, now);
                    return Some(existing_id);
                }
            }
        }

        let mut device = CanonicalDevice::new(
            entry.discovered.name.clone(),
            entry.discovered.device_type.clone(),
            vec![],
            now,
        );
        device.manufacturer = entry.discovered.manufacturer.clone();
        device.model = entry.discovered.model.clone();
        let source_room = if entry.discovered.room_name.is_empty() {
            None
        } else {
            Some(entry.discovered.room_name.clone())
        };
        device.upsert_endpoint(
            hub_key.clone(),
            entry.discovered.native_id.clone(),
            now,
            source_room,
        );
        let canonical_id = device.id.clone();

        let native_key = (hub_key.to_string(), entry.discovered.native_id);
        self.native_index.insert(native_key, canonical_id.clone());
        self.devices.insert(canonical_id.clone(), device);
        self.triage.resolve_new(triage_entry_id, now);

        Some(canonical_id)
    }

    /// Soft-delete a device (set removed_at, keep in registry for dedup).
    pub fn soft_remove(&mut self, device_id: &str, now: u64) -> bool {
        if let Some(device) = self.devices.get_mut(device_id) {
            device.removed_at = Some(now);
            self.triage.resolve_unassigned_for_device(device_id, now);
            true
        } else {
            false
        }
    }

    /// Queue an UnassignedDevice triage entry for a device with no room.
    ///
    /// Call this when a device is known to have no room assignment and the user
    /// should be prompted to assign one (e.g. after Matter commissioning).
    /// No-op if the device already has a pending or durable unassigned-device
    /// decision. This keeps explicit standalone/ignore choices stable across
    /// hub re-syncs.
    pub fn queue_unassigned(&mut self, device_id: &str, now: u64) {
        if self.triage.has_unassigned_decision(device_id) {
            return;
        }
        let (name, device_type, hub_key, native_id) = match self.devices.get(device_id) {
            Some(d) if !d.is_removed() => (
                d.name.clone(),
                d.device_type.clone(),
                d.endpoints.first().map(|ep| ep.hub_key.clone()),
                d.endpoints.first().map(|ep| ep.native_id.clone()),
            ),
            _ => return,
        };
        self.triage.add(super::triage::TriageEntry {
            id: format!("unassigned-{}-{}", device_id, now),
            kind: super::triage::TriageKind::UnassignedDevice,
            discovered: super::triage::TriageDiscoveredDevice {
                native_id: native_id.unwrap_or_default(),
                name,
                device_type,
                room_id: String::new(),
                room_name: String::new(),
                manufacturer: None,
                model: None,
            },
            hub_key: hub_key
                .unwrap_or_else(|| HubKey::new(crate::hub::HubType::new("unknown"), "unknown")),
            candidate_matches: vec![],
            room_binding: None,
            confidence: 0,
            status: super::triage::TriageStatus::Pending,
            resolved_by: None,
            created_at: now,
            resolved_at: None,
            canonical_id: Some(device_id.to_string()),
        });
    }

    // ---- Query methods ----

    /// Get all active (non-removed) canonical devices.
    pub fn devices(&self) -> impl Iterator<Item = &CanonicalDevice> {
        self.devices.values().filter(|d| !d.is_removed())
    }

    /// Get all devices including soft-deleted (for admin/debug).
    pub fn all_devices_including_removed(&self) -> impl Iterator<Item = &CanonicalDevice> {
        self.devices.values()
    }

    /// Get a device by canonical ID.
    pub fn get(&self, id: &str) -> Option<&CanonicalDevice> {
        self.devices.get(id)
    }

    /// Get a device by canonical ID (mutable).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut CanonicalDevice> {
        self.devices.get_mut(id)
    }

    /// Find a device by native ID on a specific hub.
    pub fn find_by_native_id(&self, hub_key: &HubKey, native_id: &str) -> Option<&CanonicalDevice> {
        let key = (hub_key.to_string(), native_id.to_string());
        self.native_index
            .get(&key)
            .and_then(|id| self.devices.get(id))
    }

    /// Find a device by hardware ID.
    pub fn find_by_hardware_id(&self, hw_id: &HardwareId) -> Option<&CanonicalDevice> {
        self.hw_index
            .get(hw_id.value())
            .and_then(|id| self.devices.get(id))
    }

    /// Check whether this hub currently has any active canonical endpoints.
    pub fn has_active_endpoints_for_hub(&self, hub_key: &HubKey) -> bool {
        self.devices.values().any(|device| {
            !device.is_removed()
                && device
                    .active_endpoints()
                    .any(|endpoint| &endpoint.hub_key == hub_key)
        })
    }

    /// Get all devices assigned to a room.
    pub fn devices_in_room(&self, room_id: &str) -> Vec<&CanonicalDevice> {
        self.devices
            .values()
            .filter(|d| d.room_id.as_deref() == Some(room_id))
            .collect()
    }

    /// Get all devices of a specific type.
    pub fn devices_by_type(&self, device_type: &DeviceType) -> Vec<&CanonicalDevice> {
        self.devices
            .values()
            .filter(|d| &d.device_type == device_type)
            .collect()
    }

    /// Assign a device to a room.
    ///
    /// When assigning to a room (`Some`), auto-resolves any pending
    /// `UnassignedDevice` triage entry. When unassigning (`None`), queues
    /// a new `UnassignedDevice` entry if one doesn't already exist.
    pub fn assign_room(&mut self, device_id: &str, room_id: Option<&str>) -> bool {
        if !self.devices.contains_key(device_id) {
            return false;
        }

        let device = self.devices.get_mut(device_id).unwrap();
        device.room_id = room_id.map(|s| s.to_string());

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if room_id.is_some() {
            self.triage.resolve_unassigned_for_device(device_id, now);
        } else {
            self.queue_unassigned(device_id, now);
        }

        true
    }

    /// Set the preferred endpoint for a device.
    pub fn set_preferred_endpoint(
        &mut self,
        device_id: &str,
        hub_key: &HubKey,
        native_id: &str,
    ) -> bool {
        if let Some(device) = self.devices.get_mut(device_id) {
            for ep in &mut device.endpoints {
                ep.preferred = ep.hub_key == *hub_key && ep.native_id == native_id;
            }
            true
        } else {
            false
        }
    }

    /// Mark missing endpoints for a hub inactive while retaining their identity
    /// mapping for future reactivation.
    pub fn deactivate_missing_endpoints_for_hub(
        &mut self,
        hub_key: &HubKey,
        discovered_native_ids: &HashSet<String>,
    ) -> EndpointVisibilityReport {
        let mut affected_device_ids = HashSet::new();
        let mut hidden_device_ids = HashSet::new();

        for (device_id, device) in &mut self.devices {
            if device.is_removed() {
                continue;
            }

            let mut changed = false;
            for endpoint in &mut device.endpoints {
                if endpoint.hub_key != *hub_key || !endpoint.active {
                    continue;
                }
                if discovered_native_ids.contains(&endpoint.native_id) {
                    continue;
                }
                endpoint.active = false;
                changed = true;
            }

            if !changed {
                continue;
            }

            affected_device_ids.insert(device_id.clone());
            if !device.has_active_endpoint() {
                hidden_device_ids.insert(device_id.clone());
            }
        }

        let mut affected_device_ids: Vec<String> = affected_device_ids.into_iter().collect();
        affected_device_ids.sort();
        let mut hidden_device_ids: Vec<String> = hidden_device_ids.into_iter().collect();
        hidden_device_ids.sort();

        EndpointVisibilityReport {
            affected_device_ids,
            hidden_device_ids,
        }
    }

    /// Get the triage queue.
    pub fn triage(&self) -> &TriageQueue {
        &self.triage
    }

    /// Get the triage queue (mutable).
    pub fn triage_mut(&mut self) -> &mut TriageQueue {
        &mut self.triage
    }

    /// Total number of canonical devices.
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    /// Remove a canonical device and clean up indices.
    pub fn remove_device(&mut self, device_id: &str) -> Option<CanonicalDevice> {
        if let Some(device) = self.devices.remove(device_id) {
            let removed_device_id = device.id.clone();
            // Auto-resolve any pending unassigned triage entry
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.triage.resolve_unassigned_for_device(device_id, now);

            // Clean up hw_index
            for hw_id in &device.hardware_ids {
                if self.hw_index.get(hw_id.value()) == Some(&removed_device_id) {
                    self.hw_index.remove(hw_id.value());
                }
            }
            // Clean up native_index
            for ep in &device.endpoints {
                let key = (ep.hub_key.to_string(), ep.native_id.clone());
                if self.native_index.get(&key) == Some(&removed_device_id) {
                    self.native_index.remove(&key);
                }
            }
            Some(device)
        } else {
            None
        }
    }

    /// Remove one integration endpoint without deleting other ways to reach
    /// the same physical device.
    ///
    /// Returns `(canonical_id, removed_whole_device)`. The canonical record is
    /// deleted only when the removed endpoint was its final endpoint.
    pub fn remove_endpoint(&mut self, hub_key: &HubKey, native_id: &str) -> Option<(String, bool)> {
        let native_key = (hub_key.to_string(), native_id.to_string());
        let canonical_id = self.native_index.remove(&native_key)?;
        let remove_whole_device = {
            let device = self.devices.get_mut(&canonical_id)?;
            device
                .endpoints
                .retain(|endpoint| &endpoint.hub_key != hub_key || endpoint.native_id != native_id);
            if !device.endpoints.is_empty() {
                let has_preferred = device.endpoints.iter().any(|endpoint| endpoint.preferred);
                if !has_preferred {
                    let first = device.endpoints.first_mut().expect("not empty");
                    first.preferred = true;
                }
                false
            } else {
                true
            }
        };
        if remove_whole_device {
            self.remove_device(&canonical_id);
        }
        Some((canonical_id, remove_whole_device))
    }

    /// Backfill UnassignedDevice triage entries for existing devices with no room.
    ///
    /// Called on startup after loading the registry to handle devices that
    /// predate this feature.
    pub fn backfill_unassigned_triage(&mut self, now: u64) {
        let ids: Vec<_> = self
            .devices
            .values()
            .filter(|d| d.room_id.is_none() && !d.is_removed())
            .map(|d| d.id.clone())
            .collect();

        for id in ids {
            self.queue_unassigned(&id, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::HubType;

    fn hue_key() -> HubKey {
        HubKey::new(HubType::new("hue"), "192.168.1.100")
    }

    fn ha_key() -> HubKey {
        HubKey::new(HubType::new("ha"), "192.168.1.200")
    }

    fn make_identity(native_id: &str, name: &str, hw_ids: Vec<HardwareId>) -> DiscoveredIdentity {
        DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: Some("room-1".to_string()),
            room_name: Some("Kitchen".to_string()),
            name: name.to_string(),
            device_type: DeviceType::Light,
            hardware_ids: hw_ids,
            manufacturer: Some("Signify".to_string()),
            model: Some("LCA001".to_string()),
        }
    }

    #[test]
    fn resolve_creates_new_device() {
        let mut reg = CanonicalRegistry::new();
        let identity = make_identity(
            "hue-light-1",
            "Kitchen Spot 1",
            vec![HardwareId::mac("00:17:88:01:09:ab:cd:ef")],
        );

        let result = reg.resolve(&identity, &hue_key(), 1000);
        assert!(matches!(result, ResolveResult::Created { .. }));
        assert_eq!(reg.device_count(), 1);
    }

    #[test]
    fn remove_endpoint_preserves_other_transport_and_reassigns_preference() {
        let mut reg = CanonicalRegistry::new();
        let identity = make_identity(
            "hue-light-1",
            "Kitchen Spot 1",
            vec![HardwareId::mac("00:17:88:01:09:ab:cd:ef")],
        );
        let canonical_id = match reg.resolve(&identity, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("unexpected resolve result: {other:?}"),
        };
        reg.get_mut(&canonical_id).unwrap().upsert_endpoint(
            ha_key(),
            "light.kitchen".to_string(),
            2000,
            None,
        );
        reg.rebuild_indices();

        assert_eq!(
            reg.remove_endpoint(&hue_key(), "hue-light-1"),
            Some((canonical_id.clone(), false))
        );
        let remaining = reg.get(&canonical_id).expect("canonical device remains");
        assert_eq!(remaining.endpoints.len(), 1);
        assert_eq!(remaining.preferred_endpoint().unwrap().hub_key, ha_key());
        assert!(reg.find_by_native_id(&hue_key(), "hue-light-1").is_none());
        assert_eq!(
            reg.find_by_native_id(&ha_key(), "light.kitchen")
                .unwrap()
                .id,
            canonical_id
        );

        assert_eq!(
            reg.remove_endpoint(&ha_key(), "light.kitchen"),
            Some((canonical_id.clone(), true))
        );
        assert!(reg.get(&canonical_id).is_none());
    }

    #[test]
    fn resolve_already_known_on_same_hub() {
        let mut reg = CanonicalRegistry::new();
        let identity = make_identity(
            "hue-light-1",
            "Kitchen Spot 1",
            vec![HardwareId::mac("00:17:88:01:09:ab:cd:ef")],
        );

        reg.resolve(&identity, &hue_key(), 1000);
        let result = reg.resolve(&identity, &hue_key(), 2000);

        assert!(matches!(result, ResolveResult::AlreadyKnown { .. }));
        assert_eq!(reg.device_count(), 1);
    }

    #[test]
    fn resolve_hw_match_queues_triage_and_creates_silo_device() {
        let mut reg = CanonicalRegistry::new();
        let mac = HardwareId::mac("00:17:88:01:09:ab:cd:ef");

        // First: Hue discovers it
        let hue_identity = make_identity("hue-light-1", "Kitchen Spot 1", vec![mac.clone()]);
        reg.resolve(&hue_identity, &hue_key(), 1000);

        // Second: HA discovers same device (same MAC) — should NOT auto-merge
        let ha_identity = make_identity("light.kitchen_spot_1", "Kitchen Spot 1", vec![mac]);
        let result2 = reg.resolve(&ha_identity, &ha_key(), 2000);

        // Creates a new silo device (not auto-merged)
        assert!(matches!(result2, ResolveResult::Created { .. }));
        assert_eq!(reg.device_count(), 2);

        // A high-confidence device merge triage entry was queued
        assert_eq!(reg.triage().pending_device_count(), 1);
        let merges = reg.triage().pending_by_kind(TriageKind::DeviceMerge);
        assert_eq!(merges[0].confidence, 100);
        assert_eq!(merges[0].kind, TriageKind::DeviceMerge);
        assert!(merges[0].candidate_matches[0]
            .reasons
            .contains(&MatchReason::ExactHardware));
    }

    #[test]
    fn resolve_does_not_merge_different_types() {
        let mut reg = CanonicalRegistry::new();
        let mac = HardwareId::mac("00:17:88:01:09:ab:cd:ef");

        // A light with this MAC
        let light = make_identity("hue-light-1", "Kitchen Spot", vec![mac.clone()]);
        reg.resolve(&light, &hue_key(), 1000);

        // A button with the same MAC (shouldn't happen in practice, but tests the guard)
        let mut button = make_identity("hue-button-1", "Kitchen Switch", vec![mac]);
        button.device_type = DeviceType::Button;
        let result = reg.resolve(&button, &ha_key(), 2000);

        // Should create a new device, not merge
        assert!(matches!(result, ResolveResult::Created { .. }));
        assert_eq!(reg.device_count(), 2);
    }

    #[test]
    fn resolve_queues_heuristic_match_and_creates_silo() {
        let mut reg = CanonicalRegistry::new();

        // First device: known, has hardware IDs
        let identity1 = make_identity(
            "hue-light-1",
            "Kitchen Spot 1",
            vec![HardwareId::mac("00:17:88:01:09:ab:cd:ef")],
        );
        reg.resolve(&identity1, &hue_key(), 1000);

        // Second device: no hardware IDs, same name + manufacturer + model
        // but DIFFERENT room → heuristic triage + silo device
        let identity2 = DiscoveredIdentity {
            native_id: "light.kitchen_spot_1".to_string(),
            room_id: Some("area-1".to_string()),
            room_name: Some("Lounge".to_string()),
            name: "Kitchen Spot 1".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![],
            manufacturer: Some("Signify".to_string()),
            model: Some("LCA001".to_string()),
        };

        let result = reg.resolve(&identity2, &ha_key(), 2000);
        // Creates silo device and queues heuristic triage
        assert!(matches!(result, ResolveResult::Created { .. }));
        assert_eq!(reg.device_count(), 2);
        assert_eq!(reg.triage().pending_device_count(), 1);
    }

    #[test]
    fn resolve_creates_when_no_match() {
        let mut reg = CanonicalRegistry::new();

        // First device
        let identity1 = make_identity(
            "hue-light-1",
            "Kitchen Spot 1",
            vec![HardwareId::mac("00:17:88:01:09:ab:cd:ef")],
        );
        reg.resolve(&identity1, &hue_key(), 1000);

        // Completely different device with different HW IDs
        let identity2 = make_identity(
            "hue-light-2",
            "Bedroom Lamp",
            vec![HardwareId::mac("00:11:22:33:44:55:66:77")],
        );
        let result = reg.resolve(&identity2, &hue_key(), 2000);

        assert!(matches!(result, ResolveResult::Created { .. }));
        assert_eq!(reg.device_count(), 2);
    }

    #[test]
    fn resolve_hw_match_reapplies_approved_merge() {
        let mut reg = CanonicalRegistry::new();
        let mac = HardwareId::mac("00:17:88:01:09:ab:cd:ef");

        // First: Hue discovers it
        let hue_identity = make_identity("hue-light-1", "Kitchen Spot 1", vec![mac.clone()]);
        let canonical_id = match reg.resolve(&hue_identity, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };

        // Simulate a previously-approved merge by adding merge_history
        let device = reg.get_mut(&canonical_id).unwrap();
        device.record_merge(
            ha_key(),
            "light.kitchen_spot_1".to_string(),
            "exact_hw_match:00:17:88:01:09:ab:cd:ef".to_string(),
            1500,
        );

        // HA rediscovers the same device on re-sync
        let ha_identity = make_identity("light.kitchen_spot_1", "Kitchen Spot 1", vec![mac]);
        let result = reg.resolve(&ha_identity, &ha_key(), 2000);

        // Should silently re-apply (not queue merge triage)
        assert!(
            matches!(result, ResolveResult::ReApproved { canonical_id: ref id } if id == &canonical_id),
            "Expected ReApproved, got {:?}",
            result
        );
        assert_eq!(reg.device_count(), 1);
        assert_eq!(reg.triage().pending_device_count(), 0);

        // Device now has two endpoints
        let device = reg.get(&canonical_id).unwrap();
        assert_eq!(device.endpoints.len(), 2);
    }

    #[test]
    fn complete_merge_adds_endpoint() {
        let mut reg = CanonicalRegistry::new();

        // Create a device via Hue
        let identity = make_identity(
            "hue-light-1",
            "Kitchen Spot 1",
            vec![HardwareId::mac("00:17:88:01:09:ab:cd:ef")],
        );
        let canonical_id = match reg.resolve(&identity, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };

        // Add a triage entry manually (simulating heuristic match)
        let entry = TriageEntry {
            id: "triage-1".to_string(),
            kind: TriageKind::DeviceMerge,
            discovered: TriageDiscoveredDevice {
                native_id: "light.kitchen_spot_1".to_string(),
                name: "Kitchen Spot 1".to_string(),
                device_type: DeviceType::Light,
                room_id: "area-1".to_string(),
                room_name: "Kitchen".to_string(),
                manufacturer: None,
                model: None,
            },
            hub_key: ha_key(),
            candidate_matches: vec![CandidateMatch {
                canonical_id: canonical_id.clone(),
                name: "Kitchen Spot 1".to_string(),
                score: 8,
                reasons: vec![MatchReason::ExactName, MatchReason::SameDeviceType],
            }],
            room_binding: None,
            confidence: 8,
            status: TriageStatus::Pending,
            resolved_by: None,
            created_at: 2000,
            resolved_at: None,
            canonical_id: None,
        };
        reg.triage_mut().add(entry);

        // Complete the merge
        assert!(reg.complete_merge("triage-1", &canonical_id, &ha_key(), 3000));

        // Device now has two endpoints
        let device = reg.get(&canonical_id).unwrap();
        assert_eq!(device.endpoints.len(), 2);
        // Merge recorded in history
        assert!(device
            .merge_history
            .iter()
            .any(|m| m.reason == "triage_confirmed"));

        // Triage entry is resolved
        let triage = reg.triage().get("triage-1").unwrap();
        assert_eq!(triage.status, TriageStatus::Confirmed);
        assert_eq!(triage.resolved_by.as_deref(), Some("api"));
    }

    #[test]
    fn complete_new_device_creates_separate() {
        let mut reg = CanonicalRegistry::new();

        // Add a triage entry
        let entry = TriageEntry {
            id: "triage-1".to_string(),
            kind: TriageKind::DeviceMerge,
            discovered: TriageDiscoveredDevice {
                native_id: "light.kitchen_spot_1".to_string(),
                name: "Kitchen Spot 1".to_string(),
                device_type: DeviceType::Light,
                room_id: "area-1".to_string(),
                room_name: "Kitchen".to_string(),
                manufacturer: None,
                model: None,
            },
            hub_key: ha_key(),
            candidate_matches: vec![],
            room_binding: None,
            confidence: 0,
            status: TriageStatus::Pending,
            resolved_by: None,
            created_at: 2000,
            resolved_at: None,
            canonical_id: None,
        };
        reg.triage_mut().add(entry);

        let new_id = reg.complete_new_device("triage-1", &ha_key(), 3000);
        assert!(new_id.is_some());

        let device = reg.get(&new_id.unwrap()).unwrap();
        assert_eq!(device.name, "Kitchen Spot 1");
        assert_eq!(device.endpoints.len(), 1);
        assert_eq!(device.created_at, 3000);
    }

    #[test]
    fn soft_remove_hides_from_active_queries() {
        let mut reg = CanonicalRegistry::new();
        let identity = make_identity("hue-light-1", "Kitchen Spot", vec![]);
        let canonical_id = match reg.resolve(&identity, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };

        assert_eq!(reg.devices().count(), 1);
        reg.soft_remove(&canonical_id, 2000);
        assert_eq!(reg.devices().count(), 0);
        assert_eq!(reg.all_devices_including_removed().count(), 1);
        // Device count includes removed (raw HashMap size)
        assert_eq!(reg.device_count(), 1);
    }

    #[test]
    fn find_by_native_id() {
        let mut reg = CanonicalRegistry::new();
        let identity = make_identity("hue-light-1", "Kitchen Spot 1", vec![]);
        reg.resolve(&identity, &hue_key(), 1000);

        assert!(reg.find_by_native_id(&hue_key(), "hue-light-1").is_some());
        assert!(reg.find_by_native_id(&ha_key(), "hue-light-1").is_none());
    }

    #[test]
    fn find_by_hardware_id() {
        let mut reg = CanonicalRegistry::new();
        let mac = HardwareId::mac("00:17:88:01:09:ab:cd:ef");
        let identity = make_identity("hue-light-1", "Kitchen Spot 1", vec![mac.clone()]);
        reg.resolve(&identity, &hue_key(), 1000);

        assert!(reg.find_by_hardware_id(&mac).is_some());
        assert!(reg
            .find_by_hardware_id(&HardwareId::mac("00:11:22:33:44:55:66:77"))
            .is_none());
    }

    #[test]
    fn assign_room() {
        let mut reg = CanonicalRegistry::new();
        let identity = make_identity("hue-light-1", "Kitchen Spot 1", vec![]);
        let canonical_id = match reg.resolve(&identity, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };

        assert!(reg.assign_room(&canonical_id, Some("rhythm-room-1")));
        assert_eq!(
            reg.get(&canonical_id).unwrap().room_id.as_deref(),
            Some("rhythm-room-1")
        );

        let in_room = reg.devices_in_room("rhythm-room-1");
        assert_eq!(in_room.len(), 1);
    }

    #[test]
    fn set_preferred_endpoint() {
        let mut reg = CanonicalRegistry::new();

        // Create device on Hue
        let hue_id = make_identity("hue-light-1", "Kitchen Spot", vec![]);
        let canonical_id = match reg.resolve(&hue_id, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };

        // Manually add a second endpoint (simulating an approved merge)
        let device = reg.get_mut(&canonical_id).unwrap();
        device.upsert_endpoint(ha_key(), "light.kitchen".to_string(), 2000, None);

        // Hue is currently preferred (it was first)
        assert!(
            reg.get(&canonical_id)
                .unwrap()
                .preferred_endpoint()
                .unwrap()
                .hub_key
                == hue_key()
        );

        // Switch preference to HA
        reg.set_preferred_endpoint(&canonical_id, &ha_key(), "light.kitchen");

        assert!(
            reg.get(&canonical_id)
                .unwrap()
                .preferred_endpoint()
                .unwrap()
                .hub_key
                == ha_key()
        );
    }

    #[test]
    fn remove_device_cleans_indices() {
        let mut reg = CanonicalRegistry::new();
        let mac = HardwareId::mac("00:17:88:01:09:ab:cd:ef");
        let identity = make_identity("hue-light-1", "Kitchen Spot", vec![mac.clone()]);

        let canonical_id = match reg.resolve(&identity, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };

        assert!(reg.find_by_hardware_id(&mac).is_some());
        assert!(reg.find_by_native_id(&hue_key(), "hue-light-1").is_some());

        reg.remove_device(&canonical_id);

        assert!(reg.find_by_hardware_id(&mac).is_none());
        assert!(reg.find_by_native_id(&hue_key(), "hue-light-1").is_none());
        assert_eq!(reg.device_count(), 0);
    }

    #[test]
    fn deactivate_missing_endpoints_hides_device_with_no_active_endpoints() {
        let mut reg = CanonicalRegistry::new();
        let identity = make_identity("hue-light-1", "Kitchen Spot", vec![]);
        let canonical_id = match reg.resolve(&identity, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };

        let discovered = HashSet::new();
        let report = reg.deactivate_missing_endpoints_for_hub(&hue_key(), &discovered);

        assert_eq!(report.affected_device_ids, vec![canonical_id.clone()]);
        assert_eq!(report.hidden_device_ids, vec![canonical_id.clone()]);
        let device = reg.get(&canonical_id).unwrap();
        assert!(!device.endpoints[0].active);
        assert!(reg.find_by_native_id(&hue_key(), "hue-light-1").is_some());
    }

    #[test]
    fn deactivate_missing_endpoints_keeps_cross_hub_device_visible() {
        let mut reg = CanonicalRegistry::new();
        let hue_identity = make_identity("hue-light-1", "Kitchen Spot", vec![]);
        let canonical_id = match reg.resolve(&hue_identity, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };
        reg.get_mut(&canonical_id).unwrap().upsert_endpoint(
            ha_key(),
            "light.kitchen".to_string(),
            1001,
            None,
        );

        let discovered = HashSet::new();
        let report = reg.deactivate_missing_endpoints_for_hub(&ha_key(), &discovered);

        assert_eq!(report.affected_device_ids, vec![canonical_id.clone()]);
        assert!(report.hidden_device_ids.is_empty());
        let device = reg.get(&canonical_id).unwrap();
        assert_eq!(device.active_endpoints().count(), 1);
        assert_eq!(device.preferred_endpoint().unwrap().hub_key, hue_key());
    }

    #[test]
    fn remove_device_keeps_reassigned_native_index() {
        let mut reg = CanonicalRegistry::new();
        let identity = make_identity("hue-light-1", "Kitchen Spot", vec![]);
        let canonical_id = match reg.resolve(&identity, &hue_key(), 1000) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };
        let silo_id = match reg.resolve(
            &make_identity("light.kitchen", "Kitchen Spot", vec![]),
            &ha_key(),
            2000,
        ) {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("expected Created"),
        };

        reg.get_mut(&canonical_id).unwrap().upsert_endpoint(
            ha_key(),
            "light.kitchen".to_string(),
            3000,
            None,
        );
        reg.native_index.insert(
            (ha_key().to_string(), "light.kitchen".to_string()),
            canonical_id.clone(),
        );

        reg.remove_device(&silo_id);

        assert_eq!(
            reg.find_by_native_id(&ha_key(), "light.kitchen")
                .unwrap()
                .id,
            canonical_id
        );
    }

    #[test]
    fn serialization_roundtrip() {
        let mut reg = CanonicalRegistry::new();
        let mac = HardwareId::mac("00:17:88:01:09:ab:cd:ef");
        let identity = make_identity("hue-light-1", "Kitchen Spot", vec![mac]);
        reg.resolve(&identity, &hue_key(), 1000);

        let json = serde_json::to_string(&reg).unwrap();
        let mut restored: CanonicalRegistry = serde_json::from_str(&json).unwrap();
        restored.rebuild_indices();

        assert_eq!(restored.device_count(), 1);
        assert!(restored
            .find_by_native_id(&hue_key(), "hue-light-1")
            .is_some());
    }

    // ---- Cross-hub heuristic triage tests ----

    /// Helper: create an identity with specific room context.
    fn make_room_identity(
        native_id: &str,
        name: &str,
        room_name: &str,
        hw_ids: Vec<HardwareId>,
    ) -> DiscoveredIdentity {
        DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: Some("room-1".to_string()),
            room_name: Some(room_name.to_string()),
            name: name.to_string(),
            device_type: DeviceType::Light,
            hardware_ids: hw_ids,
            manufacturer: Some("Signify".to_string()),
            model: Some("LCA001".to_string()),
        }
    }

    #[test]
    fn heuristic_same_room_exact_name_queues_for_approval() {
        let mut reg = CanonicalRegistry::new();

        // Hue creates device "Office" in room "Office"
        let hue_identity = make_room_identity("hue-gl-1", "Office", "Office", vec![]);
        reg.resolve(&hue_identity, &hue_key(), 1000);

        // HA discovers device "Office" in room "Office" — no HW IDs
        // Previously would auto-merge. Now queues for user approval.
        let ha_identity = make_room_identity("light.office", "Office", "Office", vec![]);
        let result = reg.resolve(&ha_identity, &ha_key(), 2000);

        // Creates silo device + queues merge triage
        assert!(matches!(result, ResolveResult::Created { .. }));
        assert_eq!(reg.device_count(), 2); // two separate devices
        assert_eq!(reg.triage().pending_device_count(), 1);
    }

    #[test]
    fn heuristic_same_room_different_name_queues() {
        let mut reg = CanonicalRegistry::new();

        let hue_identity = make_room_identity("hue-gl-1", "Hue Go", "Kitchen", vec![]);
        reg.resolve(&hue_identity, &hue_key(), 1000);

        let ha_identity = make_room_identity("light.kitchen_go", "Kitchen Go", "Kitchen", vec![]);
        let result = reg.resolve(&ha_identity, &ha_key(), 2000);

        // Creates silo device + queues merge triage
        assert!(matches!(result, ResolveResult::Created { .. }));
        assert_eq!(reg.triage().pending_device_count(), 1);
    }

    #[test]
    fn heuristic_different_room_queues() {
        let mut reg = CanonicalRegistry::new();

        let hue_identity = make_room_identity("hue-gl-1", "Office", "Office", vec![]);
        reg.resolve(&hue_identity, &hue_key(), 1000);

        let ha_identity = make_room_identity("light.bedroom_office", "Office", "Bedroom", vec![]);
        let result = reg.resolve(&ha_identity, &ha_key(), 2000);

        assert!(matches!(result, ResolveResult::Created { .. }));
        assert_eq!(reg.triage().pending_device_count(), 1);
    }

    #[test]
    fn heuristic_ambiguous_candidates_queues() {
        let mut reg = CanonicalRegistry::new();

        let hue1 = make_room_identity("hue-light-1", "Spot 1", "Kitchen", vec![]);
        let hue2 = make_room_identity("hue-light-2", "Spot 2", "Kitchen", vec![]);
        reg.resolve(&hue1, &hue_key(), 1000);
        reg.resolve(&hue2, &hue_key(), 1001);

        let ha_identity =
            make_room_identity("light.kitchen_spot", "Kitchen Spot", "Kitchen", vec![]);
        let result = reg.resolve(&ha_identity, &ha_key(), 2000);

        assert!(matches!(result, ResolveResult::Created { .. }));
        assert_eq!(reg.triage().pending_device_count(), 1);
    }

    #[test]
    fn heuristic_disambiguated_by_name_still_queues() {
        let mut reg = CanonicalRegistry::new();

        let hue1 = make_room_identity("hue-light-1", "Kitchen Spot 1", "Kitchen", vec![]);
        let hue2 = make_room_identity("hue-light-2", "Kitchen Spot 2", "Kitchen", vec![]);
        reg.resolve(&hue1, &hue_key(), 1000);
        reg.resolve(&hue2, &hue_key(), 1001);

        // Even with a clear best match, all cross-hub merges require approval
        let ha_identity =
            make_room_identity("light.kitchen_1", "Kitchen Spot 1", "Kitchen", vec![]);
        let result = reg.resolve(&ha_identity, &ha_key(), 2000);

        assert!(
            matches!(result, ResolveResult::Created { .. }),
            "Expected Created (no auto-merge), got {:?}",
            result
        );
        assert_eq!(reg.triage().pending_device_count(), 1);
    }

    #[test]
    fn heuristic_below_threshold_creates_without_triage() {
        let mut reg = CanonicalRegistry::new();

        let hue_identity = DiscoveredIdentity {
            native_id: "hue-light-1".to_string(),
            room_id: Some("room-1".to_string()),
            room_name: Some("Kitchen".to_string()),
            name: "Hue Bulb".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![],
            manufacturer: None,
            model: None,
        };
        reg.resolve(&hue_identity, &hue_key(), 1000);

        // SameRoom(3) only = 3 → at threshold, still queues
        let ha_identity = DiscoveredIdentity {
            native_id: "light.kitchen".to_string(),
            room_id: Some("area-1".to_string()),
            room_name: Some("Kitchen".to_string()),
            name: "Kitchen Light".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![],
            manufacturer: None,
            model: None,
        };
        let result = reg.resolve(&ha_identity, &ha_key(), 2000);

        // Score = 3 (SameRoom only) → meets threshold → queued
        assert!(matches!(result, ResolveResult::Created { .. }));
        assert_eq!(reg.triage().pending_device_count(), 1);
    }
}
