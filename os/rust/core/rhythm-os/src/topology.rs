//! Room topology store.
//!
//! Rhythm's own room hierarchy, independent of any hub. Hub rooms (Hue rooms,
//! HA areas) are "source rooms" used for bootstrapping and control routing.
//! Rhythm rooms are what the user sees and manages.
//!
//! ## Future: Composite Controller
//!
//! The topology is designed for cross-hub rooms where a single Rhythm room
//! contains devices from multiple hubs (e.g., HA switch + Hue bulb). The
//! endgame architecture:
//!
//! - ONE `RhythmEngine` runtime with a `CompositeController` implementing
//!   `LightController` (in rhythm-core)
//! - `CompositeController` holds per-hub controllers keyed by `HubKey`
//! - `turn_on(room_id, cmd)` → look up `TopologyRoom::hub_targets` → fan out
//!   to each hub's controller via `HubControlTarget`
//! - `any_lights_on(room_id)` → OR across all hub controllers
//! - `HubControlTarget::topology_aligned` determines whether the controller
//!   can use efficient grouped commands or must fall back to per-device
//!   addressing
//! - Room IDs are server-assigned (topology room IDs), not hub-native

use std::collections::HashMap;

use log::{debug, info};
use serde::{Deserialize, Serialize};

use crate::canonical::identity::HubKey;

/// How a device was assigned to a room.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlacement {
    /// Auto-assigned from hub room membership, updated on re-sync.
    HubDefault,
    /// User explicitly placed this device here — survives re-sync.
    UserOverride,
}

/// A device assigned to a Rhythm room.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomDevice {
    /// Canonical device ID.
    pub device_id: String,
    /// How this device ended up in this room.
    pub placement: DevicePlacement,
}

/// A hub-specific control target for a Rhythm room.
///
/// Each hub that has lights in a Rhythm room gets a control target. The
/// composite controller uses these to fan out commands to multiple hubs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubControlTarget {
    /// Which hub instance this target routes to.
    pub hub_key: HubKey,
    /// Hub-native room ID (Hue room UUID, HA area_id).
    pub hub_room_id: String,
    /// Hub-native control ID (Hue `grouped_light` resource, HA area_id).
    pub control_id: String,
    /// Light device IDs on this hub for `any_lights_on` checks.
    pub light_device_ids: Vec<String>,
    /// Whether this target's device membership still matches the canonical room.
    ///
    /// `true` means the hub-native room grouping matches the Rhythm room — the
    /// composite controller can use efficient grouped commands. `false` means a
    /// user moved devices, so the controller must fall back to per-device
    /// addressing via `DeviceAddressable`.
    #[serde(default = "default_true")]
    pub topology_aligned: bool,
}

fn default_true() -> bool {
    true
}

/// A Rhythm room — independent of any hub's room hierarchy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopologyRoom {
    /// Stable Rhythm room UUID.
    pub id: String,
    /// User-overridable display name.
    pub name: String,
    /// Hub control targets — one per hub that has lights in this room.
    pub hub_targets: Vec<HubControlTarget>,
    /// Devices assigned to this room.
    pub devices: Vec<RoomDevice>,
    /// Whether the user has customized this room (rename, merge, split, move devices).
    #[serde(default)]
    pub user_customized: bool,
    /// Original hub name, used for cross-hub name matching during sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_name: Option<String>,
}

impl TopologyRoom {
    /// Create a new empty room.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: id.into(),
            name: name.clone(),
            hub_targets: Vec::new(),
            devices: Vec::new(),
            user_customized: false,
            bootstrap_name: Some(name),
        }
    }

    /// Find a hub target by hub key.
    pub fn target_for_hub(&self, hub_key: &HubKey) -> Option<&HubControlTarget> {
        self.hub_targets.iter().find(|t| &t.hub_key == hub_key)
    }

    /// Find a hub target by hub room ID.
    pub fn target_for_hub_room(
        &self,
        hub_key: &HubKey,
        hub_room_id: &str,
    ) -> Option<&HubControlTarget> {
        self.hub_targets
            .iter()
            .find(|t| &t.hub_key == hub_key && t.hub_room_id == hub_room_id)
    }

    /// Add or update a hub control target.
    pub fn upsert_hub_target(&mut self, target: HubControlTarget) {
        if let Some(existing) = self
            .hub_targets
            .iter_mut()
            .find(|t| t.hub_key == target.hub_key && t.hub_room_id == target.hub_room_id)
        {
            existing.control_id = target.control_id;
            existing.light_device_ids = target.light_device_ids;
        } else {
            self.hub_targets.push(target);
        }
    }

    /// Remove a hub target.
    pub fn remove_hub_target(&mut self, hub_key: &HubKey, hub_room_id: &str) {
        self.hub_targets
            .retain(|t| !(&t.hub_key == hub_key && t.hub_room_id == hub_room_id));
    }

    /// Add a device with HubDefault placement (no-op if already UserOverride).
    pub fn add_device_hub_default(&mut self, device_id: &str) {
        if !self.devices.iter().any(|d| d.device_id == device_id) {
            self.devices.push(RoomDevice {
                device_id: device_id.to_string(),
                placement: DevicePlacement::HubDefault,
            });
        }
    }

    /// Add or update a device with UserOverride placement.
    pub fn set_device_user_override(&mut self, device_id: &str) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.device_id == device_id) {
            d.placement = DevicePlacement::UserOverride;
        } else {
            self.devices.push(RoomDevice {
                device_id: device_id.to_string(),
                placement: DevicePlacement::UserOverride,
            });
        }
        self.user_customized = true;
    }

    /// Remove a device from this room.
    pub fn remove_device(&mut self, device_id: &str) {
        self.devices.retain(|d| d.device_id != device_id);
    }

    /// Remove all HubDefault devices (keeping UserOverride).
    pub fn clear_hub_default_devices(&mut self) {
        self.devices
            .retain(|d| d.placement == DevicePlacement::UserOverride);
    }

    /// Check if this room has any hub targets.
    pub fn has_targets(&self) -> bool {
        !self.hub_targets.is_empty()
    }

    /// Check if this room has devices.
    pub fn has_devices(&self) -> bool {
        !self.devices.is_empty()
    }
}

/// A discovered hub room for topology sync.
pub struct DiscoveredTopologyRoom {
    /// Hub-native room ID.
    pub hub_room_id: String,
    /// Room name from the hub.
    pub name: String,
    /// Hub-native control ID (grouped_light, area_id).
    pub control_id: String,
    /// Light device IDs in this room.
    pub light_device_ids: Vec<String>,
    /// Canonical device IDs in this room (already resolved).
    pub canonical_device_ids: Vec<String>,
}

/// Record of an approved cross-hub room binding.
///
/// When a user approves binding a hub room to an existing Rhythm room, this
/// record is stored so the binding re-applies silently on re-sync.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomBindingRecord {
    /// Hub that was bound.
    pub hub_key: HubKey,
    /// Hub-native room ID that was bound.
    pub hub_room_id: String,
    /// Rhythm room ID it was bound to.
    pub rhythm_room_id: String,
    /// When the binding was approved (Unix timestamp seconds).
    pub approved_at: u64,
}

/// The room topology store — Rhythm's authoritative room registry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RoomTopologyStore {
    /// All Rhythm rooms, keyed by Rhythm room UUID.
    rooms: HashMap<String, TopologyRoom>,
    /// Approved cross-hub room bindings (survives re-sync).
    #[serde(default)]
    approved_bindings: Vec<RoomBindingRecord>,
    /// Index: (hub_key display, hub_room_id) → Rhythm room ID.
    #[serde(skip)]
    hub_room_index: HashMap<(String, String), String>,
}

impl RoomTopologyStore {
    pub fn new() -> Self {
        Self {
            rooms: HashMap::new(),
            approved_bindings: Vec::new(),
            hub_room_index: HashMap::new(),
        }
    }

    /// Rebuild indices from the room map. Call after deserialization.
    pub fn rebuild_indices(&mut self) {
        self.hub_room_index.clear();
        for (room_id, room) in &self.rooms {
            for target in &room.hub_targets {
                self.hub_room_index.insert(
                    (target.hub_key.to_string(), target.hub_room_id.clone()),
                    room_id.clone(),
                );
            }
        }
    }

    /// Sync a discovered hub room into the topology.
    ///
    /// Room sync rules:
    /// 1. Already mapped by hub room ID → update targets + HubDefault devices
    /// 2. Name matches existing room:
    ///    a. Previously-approved binding → re-bind silently
    ///    b. No approval → create separate room + return `CreatedWithProposal`
    /// 3. No match → create new Rhythm room
    /// 4. (Stale removal done separately via `remove_stale_targets`)
    pub fn sync_hub_room(
        &mut self,
        hub_key: &HubKey,
        discovered: &DiscoveredTopologyRoom,
    ) -> SyncAction {
        let index_key = (hub_key.to_string(), discovered.hub_room_id.clone());

        // Rule 1: Already mapped by hub room ID
        if let Some(rhythm_room_id) = self.hub_room_index.get(&index_key).cloned() {
            // Check if this room was freshly created by translate_or_create in the
            // same sync cycle (1 hub target, no canonical devices yet). If so, we
            // need to check for cross-hub name matches — translate_or_create doesn't
            // do name detection, so without this check triage proposals would never
            // be generated for same-name rooms across hubs.
            let freshly_created = self
                .rooms
                .get(&rhythm_room_id)
                .map(|room| room.hub_targets.len() == 1 && room.devices.is_empty())
                .unwrap_or(false);

            if freshly_created {
                let name_lower = discovered.name.to_lowercase();
                let name_matches: Vec<(String, String)> = self
                    .rooms
                    .iter()
                    .filter(|(id, room)| {
                        *id != &rhythm_room_id
                            && (room.name.to_lowercase() == name_lower
                                || room
                                    .bootstrap_name
                                    .as_ref()
                                    .map(|n| n.to_lowercase() == name_lower)
                                    .unwrap_or(false))
                            && room.target_for_hub(hub_key).is_none()
                    })
                    .map(|(id, room)| (id.clone(), room.name.clone()))
                    .collect();

                if !name_matches.is_empty() {
                    // Check for approved binding (Rule 2a path)
                    let approved = self
                        .approved_bindings
                        .iter()
                        .find(|b| b.hub_key == *hub_key && b.hub_room_id == discovered.hub_room_id)
                        .cloned();

                    if let Some(binding) = approved {
                        let target_id = binding.rhythm_room_id.clone();
                        if let Some(target_room) = self.rooms.get_mut(&target_id) {
                            target_room.upsert_hub_target(HubControlTarget {
                                hub_key: hub_key.clone(),
                                hub_room_id: discovered.hub_room_id.clone(),
                                control_id: discovered.control_id.clone(),
                                light_device_ids: discovered.light_device_ids.clone(),
                                topology_aligned: true,
                            });
                            for dev_id in &discovered.canonical_device_ids {
                                target_room.add_device_hub_default(dev_id);
                            }
                            // Remove the duplicate room created by translate_or_create
                            self.rooms.remove(&rhythm_room_id);
                            self.hub_room_index.insert(index_key, target_id.clone());
                            info!(target: "topology",
                                "Re-applied approved binding (freshly created): hub room '{}' → Rhythm room '{}' ({})",
                                discovered.name, binding.rhythm_room_id, target_id
                            );
                            return SyncAction::Bound {
                                rhythm_room_id: target_id,
                            };
                        }
                    }

                    // Rule 2b path: update the freshly-created room, return proposal
                    if let Some(room) = self.rooms.get_mut(&rhythm_room_id) {
                        room.upsert_hub_target(HubControlTarget {
                            hub_key: hub_key.clone(),
                            hub_room_id: discovered.hub_room_id.clone(),
                            control_id: discovered.control_id.clone(),
                            light_device_ids: discovered.light_device_ids.clone(),
                            topology_aligned: true,
                        });
                        room.devices.retain(|d| {
                            d.placement == DevicePlacement::UserOverride
                                || discovered.canonical_device_ids.contains(&d.device_id)
                        });
                        for dev_id in &discovered.canonical_device_ids {
                            room.add_device_hub_default(dev_id);
                        }
                        if !room.user_customized {
                            room.name = discovered.name.clone();
                        }
                    }

                    let (proposed_id, proposed_name) = name_matches[0].clone();
                    info!(target: "topology",
                        "Freshly created room '{}' ({}) has cross-hub name match with '{}', queuing binding proposal",
                        discovered.name, rhythm_room_id, proposed_name
                    );
                    return SyncAction::CreatedWithProposal {
                        rhythm_room_id,
                        proposed_target_id: proposed_id,
                        proposed_target_name: proposed_name,
                        candidate_rooms: name_matches,
                    };
                }
            }

            // Normal Rule 1 update (established room from previous sync)
            if let Some(room) = self.rooms.get_mut(&rhythm_room_id) {
                // Update the target
                room.upsert_hub_target(HubControlTarget {
                    hub_key: hub_key.clone(),
                    hub_room_id: discovered.hub_room_id.clone(),
                    control_id: discovered.control_id.clone(),
                    light_device_ids: discovered.light_device_ids.clone(),
                    topology_aligned: true,
                });

                // Update HubDefault devices (don't touch UserOverride)
                room.devices.retain(|d| {
                    d.placement == DevicePlacement::UserOverride
                        || discovered.canonical_device_ids.contains(&d.device_id)
                });
                for dev_id in &discovered.canonical_device_ids {
                    room.add_device_hub_default(dev_id);
                }

                if !room.user_customized {
                    room.name = discovered.name.clone();
                }

                debug!(target: "topology", "Updated room '{}' ({})", room.name, rhythm_room_id);
                return SyncAction::Updated { rhythm_room_id };
            }
        }

        // Rule 2: Name match against existing rooms (room not yet in hub_room_index)
        let name_lower = discovered.name.to_lowercase();
        let name_matches: Vec<(String, String)> = self
            .rooms
            .iter()
            .filter(|(_, room)| {
                let matches_bootstrap = room
                    .bootstrap_name
                    .as_ref()
                    .map(|n| n.to_lowercase() == name_lower)
                    .unwrap_or(false);
                let matches_name = room.name.to_lowercase() == name_lower;
                (matches_bootstrap || matches_name) && room.target_for_hub(hub_key).is_none()
            })
            .map(|(id, room)| (id.clone(), room.name.clone()))
            .collect();

        if !name_matches.is_empty() {
            // Rule 2a: Check if a previously-approved binding exists
            let approved = self
                .approved_bindings
                .iter()
                .find(|b| b.hub_key == *hub_key && b.hub_room_id == discovered.hub_room_id);

            if let Some(binding) = approved {
                let target_id = binding.rhythm_room_id.clone();
                if let Some(room) = self.rooms.get_mut(&target_id) {
                    room.upsert_hub_target(HubControlTarget {
                        hub_key: hub_key.clone(),
                        hub_room_id: discovered.hub_room_id.clone(),
                        control_id: discovered.control_id.clone(),
                        light_device_ids: discovered.light_device_ids.clone(),
                        topology_aligned: true,
                    });

                    for dev_id in &discovered.canonical_device_ids {
                        room.add_device_hub_default(dev_id);
                    }

                    self.hub_room_index.insert(index_key, target_id.clone());

                    info!(target: "topology",
                        "Re-applied approved binding: hub room '{}' → Rhythm room '{}' ({})",
                        discovered.name, room.name, target_id
                    );
                    return SyncAction::Bound {
                        rhythm_room_id: target_id,
                    };
                } else {
                    // Target room was deleted — clean up stale binding
                    info!(target: "topology",
                        "Approved binding target '{}' no longer exists, removing stale binding",
                        target_id
                    );
                    self.revoke_binding(hub_key, &discovered.hub_room_id);
                    // Fall through to Rule 2b to create a new proposal
                }
            }

            // Rule 2b: Create separate room + return proposal for user approval
            let (proposed_id, proposed_name) = name_matches[0].clone();

            let rhythm_room_id = super::canonical::identity::generate_uuid_public();
            let mut room = TopologyRoom::new(rhythm_room_id.clone(), &discovered.name);
            room.upsert_hub_target(HubControlTarget {
                hub_key: hub_key.clone(),
                hub_room_id: discovered.hub_room_id.clone(),
                control_id: discovered.control_id.clone(),
                light_device_ids: discovered.light_device_ids.clone(),
                topology_aligned: true,
            });

            for dev_id in &discovered.canonical_device_ids {
                room.add_device_hub_default(dev_id);
            }

            self.hub_room_index
                .insert(index_key, rhythm_room_id.clone());
            self.rooms.insert(rhythm_room_id.clone(), room);

            info!(target: "topology",
                "Created separate room '{}' ({}) — name matches '{}', queuing binding proposal",
                discovered.name, rhythm_room_id, proposed_name
            );
            return SyncAction::CreatedWithProposal {
                rhythm_room_id,
                proposed_target_id: proposed_id,
                proposed_target_name: proposed_name,
                candidate_rooms: name_matches,
            };
        }

        // Rule 3: Create new Rhythm room
        let rhythm_room_id = super::canonical::identity::generate_uuid_public();
        let mut room = TopologyRoom::new(rhythm_room_id.clone(), &discovered.name);
        room.upsert_hub_target(HubControlTarget {
            hub_key: hub_key.clone(),
            hub_room_id: discovered.hub_room_id.clone(),
            control_id: discovered.control_id.clone(),
            light_device_ids: discovered.light_device_ids.clone(),
            topology_aligned: true,
        });

        for dev_id in &discovered.canonical_device_ids {
            room.add_device_hub_default(dev_id);
        }

        self.hub_room_index
            .insert(index_key, rhythm_room_id.clone());
        self.rooms.insert(rhythm_room_id.clone(), room);

        info!(target: "topology", "Created Rhythm room '{}' ({})", discovered.name, rhythm_room_id);
        SyncAction::Created { rhythm_room_id }
    }

    /// Remove stale hub targets for a hub that no longer reports certain rooms.
    ///
    /// Rule 4: Never delete Rhythm rooms — only remove hub targets.
    pub fn remove_stale_targets(
        &mut self,
        hub_key: &HubKey,
        current_hub_room_ids: &[String],
    ) -> Vec<String> {
        let current_set: std::collections::HashSet<&String> = current_hub_room_ids.iter().collect();
        let mut affected = Vec::new();

        for (room_id, room) in &mut self.rooms {
            let had_target = room.hub_targets.len();
            room.hub_targets
                .retain(|t| &t.hub_key != hub_key || current_set.contains(&t.hub_room_id));

            if room.hub_targets.len() < had_target {
                affected.push(room_id.clone());
            }
        }

        // Clean up hub_room_index: remove entries for this hub whose room IDs are stale
        let hub_str = hub_key.to_string();
        let current_set_owned: std::collections::HashSet<String> =
            current_hub_room_ids.iter().cloned().collect();
        self.hub_room_index.retain(|(key, hub_room_id), _| {
            key != &hub_str || current_set_owned.contains(hub_room_id)
        });

        affected
    }

    // ---- Query methods ----

    /// Get all rooms.
    pub fn rooms(&self) -> impl Iterator<Item = &TopologyRoom> {
        self.rooms.values()
    }

    /// Get a room by Rhythm room ID.
    pub fn get(&self, room_id: &str) -> Option<&TopologyRoom> {
        self.rooms.get(room_id)
    }

    /// Get a room by Rhythm room ID (mutable).
    pub fn get_mut(&mut self, room_id: &str) -> Option<&mut TopologyRoom> {
        self.rooms.get_mut(room_id)
    }

    /// Find the Rhythm room that contains a specific hub room.
    pub fn find_by_hub_room(&self, hub_key: &HubKey, hub_room_id: &str) -> Option<&TopologyRoom> {
        let key = (hub_key.to_string(), hub_room_id.to_string());
        self.hub_room_index
            .get(&key)
            .and_then(|id| self.rooms.get(id))
    }

    /// Translate a hub-native room ID to a Rhythm room ID.
    pub fn translate_room_id(&self, hub_key: &HubKey, hub_room_id: &str) -> Option<&str> {
        let key = (hub_key.to_string(), hub_room_id.to_string());
        self.hub_room_index.get(&key).map(|s| s.as_str())
    }

    /// Translate a hub-native room ID to a Rhythm room ID without requiring a hub key.
    ///
    /// Searches all hub room index entries for a matching hub_room_id. This works
    /// because hub-native room IDs are globally unique (Hue UUIDs, HA area slugs).
    /// Used for events where hub_key is not available (e.g., button_resolve).
    pub fn translate_room_id_any_hub(&self, hub_room_id: &str) -> Option<&str> {
        self.hub_room_index
            .iter()
            .find(|((_, hrid), _)| hrid == hub_room_id)
            .map(|(_, rhythm_id)| rhythm_id.as_str())
    }

    /// Get the routing table: rhythm_room_id → Vec<(hub_key, control_id)>.
    pub fn routing_table(&self) -> HashMap<String, Vec<(HubKey, String)>> {
        let mut table = HashMap::new();
        for (room_id, room) in &self.rooms {
            let targets: Vec<_> = room
                .hub_targets
                .iter()
                .map(|t| (t.hub_key.clone(), t.control_id.clone()))
                .collect();
            if !targets.is_empty() {
                table.insert(room_id.clone(), targets);
            }
        }
        table
    }

    /// Total number of rooms.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    // ---- Room management operations ----

    /// Create a new empty room.
    pub fn create_room(&mut self, name: impl Into<String>) -> String {
        let id = super::canonical::identity::generate_uuid_public();
        let mut room = TopologyRoom::new(id.clone(), name);
        room.user_customized = true;
        self.rooms.insert(id.clone(), room);
        id
    }

    /// Rename a room.
    pub fn rename_room(&mut self, room_id: &str, name: impl Into<String>) -> bool {
        if let Some(room) = self.rooms.get_mut(room_id) {
            room.name = name.into();
            room.user_customized = true;
            true
        } else {
            false
        }
    }

    /// Merge two rooms. All hub targets and devices from `source_id` are moved to `target_id`.
    /// The source room is removed.
    pub fn merge_rooms(&mut self, target_id: &str, source_id: &str) -> bool {
        let source = match self.rooms.remove(source_id) {
            Some(s) => s,
            None => return false,
        };

        let target = match self.rooms.get_mut(target_id) {
            Some(t) => t,
            None => {
                // Put source back
                self.rooms.insert(source_id.to_string(), source);
                return false;
            }
        };

        // Move hub targets
        for source_target in source.hub_targets {
            // Update index
            let key = (
                source_target.hub_key.to_string(),
                source_target.hub_room_id.clone(),
            );
            self.hub_room_index.insert(key, target_id.to_string());
            target.upsert_hub_target(source_target);
        }

        // Move devices (preserving placement)
        for device in source.devices {
            if !target
                .devices
                .iter()
                .any(|d| d.device_id == device.device_id)
            {
                target.devices.push(device);
            }
        }

        target.user_customized = true;
        true
    }

    /// Move a device from one room to another.
    ///
    /// Sets `UserOverride` placement on the device and marks all hub targets
    /// in both rooms as `topology_aligned: false` since the canonical room
    /// membership now diverges from hub-native grouping.
    pub fn move_device(&mut self, device_id: &str, from_room: &str, to_room: &str) -> bool {
        // Remove from source and mark its targets as misaligned
        if let Some(room) = self.rooms.get_mut(from_room) {
            room.remove_device(device_id);
            for target in &mut room.hub_targets {
                target.topology_aligned = false;
            }
        } else {
            return false;
        }

        // Add to target with UserOverride and mark its targets as misaligned
        if let Some(room) = self.rooms.get_mut(to_room) {
            room.set_device_user_override(device_id);
            for target in &mut room.hub_targets {
                target.topology_aligned = false;
            }
            true
        } else {
            false
        }
    }

    /// Insert a room directly (used for deserialization / testing).
    pub fn insert_room(&mut self, room: TopologyRoom) {
        for target in &room.hub_targets {
            self.hub_room_index.insert(
                (target.hub_key.to_string(), target.hub_room_id.clone()),
                room.id.clone(),
            );
        }
        self.rooms.insert(room.id.clone(), room);
    }

    // ---- Approved binding management ----

    /// Record an approved room binding (user confirmed merge).
    pub fn approve_binding(
        &mut self,
        hub_key: HubKey,
        hub_room_id: String,
        rhythm_room_id: String,
        now: u64,
    ) {
        // Don't add duplicates
        if !self
            .approved_bindings
            .iter()
            .any(|b| b.hub_key == hub_key && b.hub_room_id == hub_room_id)
        {
            self.approved_bindings.push(RoomBindingRecord {
                hub_key,
                hub_room_id,
                rhythm_room_id,
                approved_at: now,
            });
        }
    }

    /// Remove an approved binding (user revoked).
    pub fn revoke_binding(&mut self, hub_key: &HubKey, hub_room_id: &str) {
        self.approved_bindings
            .retain(|b| !(&b.hub_key == hub_key && b.hub_room_id == hub_room_id));
    }

    /// Check if a binding was previously approved.
    pub fn has_approved_binding(&self, hub_key: &HubKey, hub_room_id: &str) -> bool {
        self.approved_bindings
            .iter()
            .any(|b| &b.hub_key == hub_key && b.hub_room_id == hub_room_id)
    }

    /// Get all approved bindings.
    pub fn approved_bindings(&self) -> &[RoomBindingRecord] {
        &self.approved_bindings
    }

    /// Translate a hub-native room ID to a topology room ID, creating the
    /// topology entry if none exists yet.
    ///
    /// This is a simplified version of [`sync_hub_room`] that only ensures a
    /// topology entry exists — no canonical device resolution, no binding
    /// proposals. Used by [`do_room_set`](crate::commands::do_room_set) so
    /// rooms enter the engine with topology IDs from the start, eliminating
    /// the need for a later remap phase.
    ///
    /// The full [`sync_hub_room`] (Phase 4c) later enriches the same entry
    /// with canonical devices and detects cross-hub binding candidates.
    pub fn translate_or_create(
        &mut self,
        hub_key: &HubKey,
        hub_room_id: &str,
        room_name: &str,
        control_id: &str,
        light_device_ids: &[String],
    ) -> String {
        let index_key = (hub_key.to_string(), hub_room_id.to_string());

        // Rule 1: Already mapped — return existing topology room ID.
        if let Some(rhythm_room_id) = self.hub_room_index.get(&index_key) {
            return rhythm_room_id.clone();
        }

        // Rule 2a: Check for a previously-approved binding.
        let approved_target = self
            .approved_bindings
            .iter()
            .find(|b| b.hub_key == *hub_key && b.hub_room_id == hub_room_id)
            .map(|b| b.rhythm_room_id.clone());

        if let Some(target_id) = approved_target {
            if let Some(room) = self.rooms.get_mut(&target_id) {
                room.upsert_hub_target(HubControlTarget {
                    hub_key: hub_key.clone(),
                    hub_room_id: hub_room_id.to_string(),
                    control_id: control_id.to_string(),
                    light_device_ids: light_device_ids.to_vec(),
                    topology_aligned: true,
                });
                self.hub_room_index.insert(index_key, target_id.clone());
                debug!(target: "topology",
                    "translate_or_create: re-applied approved binding '{}' → '{}'",
                    hub_room_id, target_id
                );
                return target_id;
            }
            // Target was deleted — fall through to create new
        }

        // Rule 3: Create new topology room.
        let rhythm_room_id = super::canonical::identity::generate_uuid_public();
        let mut room = TopologyRoom::new(rhythm_room_id.clone(), room_name);
        room.upsert_hub_target(HubControlTarget {
            hub_key: hub_key.clone(),
            hub_room_id: hub_room_id.to_string(),
            control_id: control_id.to_string(),
            light_device_ids: light_device_ids.to_vec(),
            topology_aligned: true,
        });

        self.hub_room_index
            .insert(index_key, rhythm_room_id.clone());
        self.rooms.insert(rhythm_room_id.clone(), room);

        debug!(target: "topology",
            "translate_or_create: created room '{}' ({}) for hub room '{}'",
            room_name, rhythm_room_id, hub_room_id
        );
        rhythm_room_id
    }

    /// Build a routing table for the composite controller.
    ///
    /// Maps each Rhythm room ID (topology) to the list of (hub_key, hub_room_id)
    /// pairs. All callers should use topology room IDs — hub-native IDs are
    /// translated at the event/handler boundary before reaching the composite.
    pub fn composite_routing(&self) -> HashMap<String, Vec<(String, String)>> {
        let mut table: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (room_id, room) in &self.rooms {
            let targets: Vec<_> = room
                .hub_targets
                .iter()
                .map(|t| (t.hub_key.to_string(), t.hub_room_id.clone()))
                .collect();
            if !targets.is_empty() {
                table.insert(room_id.clone(), targets);
            }
        }
        table
    }
}

/// What happened during a sync_hub_room call.
#[derive(Debug)]
pub enum SyncAction {
    /// Room already mapped — targets and devices updated.
    Updated { rhythm_room_id: String },
    /// Room bound to existing Rhythm room (previously-approved binding re-applied).
    Bound { rhythm_room_id: String },
    /// New Rhythm room created (no name match found).
    Created { rhythm_room_id: String },
    /// New Rhythm room created, but name-matching candidates exist.
    /// A room binding triage entry should be queued for user approval.
    CreatedWithProposal {
        rhythm_room_id: String,
        /// Best-match existing room (for the proposal).
        proposed_target_id: String,
        proposed_target_name: String,
        /// All name-matching candidate rooms (id, name).
        candidate_rooms: Vec<(String, String)>,
    },
}

impl SyncAction {
    pub fn rhythm_room_id(&self) -> &str {
        match self {
            SyncAction::Updated { rhythm_room_id }
            | SyncAction::Bound { rhythm_room_id }
            | SyncAction::Created { rhythm_room_id }
            | SyncAction::CreatedWithProposal { rhythm_room_id, .. } => rhythm_room_id,
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

    fn make_discovered(hub_room_id: &str, name: &str, control_id: &str) -> DiscoveredTopologyRoom {
        DiscoveredTopologyRoom {
            hub_room_id: hub_room_id.to_string(),
            name: name.to_string(),
            control_id: control_id.to_string(),
            light_device_ids: vec![],
            canonical_device_ids: vec![],
        }
    }

    #[test]
    fn sync_creates_new_room() {
        let mut store = RoomTopologyStore::new();
        let discovered = make_discovered("hue-room-1", "Kitchen", "gl-1");

        let action = store.sync_hub_room(&hue_key(), &discovered);
        assert!(matches!(action, SyncAction::Created { .. }));
        assert_eq!(store.room_count(), 1);

        let room = store.rooms().next().unwrap();
        assert_eq!(room.name, "Kitchen");
        assert_eq!(room.hub_targets.len(), 1);
        assert_eq!(room.hub_targets[0].control_id, "gl-1");
    }

    #[test]
    fn sync_updates_existing_mapped_room() {
        let mut store = RoomTopologyStore::new();
        let discovered1 = make_discovered("hue-room-1", "Kitchen", "gl-1");
        store.sync_hub_room(&hue_key(), &discovered1);

        // Re-sync with updated control_id
        let discovered2 = make_discovered("hue-room-1", "Kitchen", "gl-2");
        let action = store.sync_hub_room(&hue_key(), &discovered2);

        assert!(matches!(action, SyncAction::Updated { .. }));
        assert_eq!(store.room_count(), 1);

        let room = store.rooms().next().unwrap();
        assert_eq!(room.hub_targets[0].control_id, "gl-2");
    }

    #[test]
    fn sync_name_match_creates_proposal_not_bind() {
        let mut store = RoomTopologyStore::new();

        // Hue creates "Kitchen"
        let hue_discovered = make_discovered("hue-room-1", "Kitchen", "gl-1");
        store.sync_hub_room(&hue_key(), &hue_discovered);

        // HA discovers "Kitchen" → creates separate room + proposal (not auto-bind)
        let ha_discovered = make_discovered("ha-area-1", "Kitchen", "ha-area-1");
        let action = store.sync_hub_room(&ha_key(), &ha_discovered);

        assert!(matches!(action, SyncAction::CreatedWithProposal { .. }));
        assert_eq!(store.room_count(), 2); // Two separate rooms

        // Verify proposal contains the right target
        if let SyncAction::CreatedWithProposal {
            proposed_target_name,
            candidate_rooms,
            ..
        } = &action
        {
            assert_eq!(proposed_target_name, "Kitchen");
            assert_eq!(candidate_rooms.len(), 1);
        }
    }

    #[test]
    fn sync_name_match_case_insensitive_creates_proposal() {
        let mut store = RoomTopologyStore::new();

        let hue = make_discovered("hue-room-1", "Living Room", "gl-1");
        store.sync_hub_room(&hue_key(), &hue);

        let ha = make_discovered("ha-area-1", "living room", "ha-area-1");
        let action = store.sync_hub_room(&ha_key(), &ha);

        assert!(matches!(action, SyncAction::CreatedWithProposal { .. }));
        assert_eq!(store.room_count(), 2);
    }

    #[test]
    fn sync_approved_binding_rebinds_silently() {
        let mut store = RoomTopologyStore::new();

        // Hue creates "Kitchen"
        let hue = make_discovered("hue-room-1", "Kitchen", "gl-1");
        let action = store.sync_hub_room(&hue_key(), &hue);
        let hue_room_id = action.rhythm_room_id().to_string();

        // Record an approved binding for HA → Kitchen
        store.approve_binding(ha_key(), "ha-area-1".to_string(), hue_room_id.clone(), 1500);

        // HA discovers "Kitchen" → should re-bind silently (approved)
        let ha = make_discovered("ha-area-1", "Kitchen", "ha-area-1");
        let action = store.sync_hub_room(&ha_key(), &ha);

        assert!(matches!(action, SyncAction::Bound { .. }));
        assert_eq!(store.room_count(), 1); // Still one room
        let room = store.rooms().next().unwrap();
        assert_eq!(room.hub_targets.len(), 2);
    }

    #[test]
    fn sync_no_name_match_creates_new() {
        let mut store = RoomTopologyStore::new();

        let hue = make_discovered("hue-room-1", "Kitchen", "gl-1");
        store.sync_hub_room(&hue_key(), &hue);

        let ha = make_discovered("ha-area-1", "Bedroom", "ha-area-1");
        let action = store.sync_hub_room(&ha_key(), &ha);

        assert!(matches!(action, SyncAction::Created { .. }));
        assert_eq!(store.room_count(), 2);
    }

    #[test]
    fn sync_preserves_user_customized_name() {
        let mut store = RoomTopologyStore::new();
        let discovered = make_discovered("hue-room-1", "Kitchen", "gl-1");
        let action = store.sync_hub_room(&hue_key(), &discovered);
        let room_id = action.rhythm_room_id().to_string();

        // User renames
        store.rename_room(&room_id, "My Kitchen");

        // Re-sync tries to rename back
        let rediscovered = make_discovered("hue-room-1", "Kitchen", "gl-1");
        store.sync_hub_room(&hue_key(), &rediscovered);

        assert_eq!(store.get(&room_id).unwrap().name, "My Kitchen");
    }

    #[test]
    fn sync_preserves_user_override_devices() {
        let mut store = RoomTopologyStore::new();
        let mut discovered = make_discovered("hue-room-1", "Kitchen", "gl-1");
        discovered.canonical_device_ids = vec!["dev-1".to_string(), "dev-2".to_string()];
        let action = store.sync_hub_room(&hue_key(), &discovered);
        let room_id = action.rhythm_room_id().to_string();

        // User moves dev-3 here
        store
            .get_mut(&room_id)
            .unwrap()
            .set_device_user_override("dev-3");

        // Re-sync removes dev-2, adds dev-4
        let mut rediscovered = make_discovered("hue-room-1", "Kitchen", "gl-1");
        rediscovered.canonical_device_ids = vec!["dev-1".to_string(), "dev-4".to_string()];
        store.sync_hub_room(&hue_key(), &rediscovered);

        let room = store.get(&room_id).unwrap();
        let device_ids: Vec<&str> = room.devices.iter().map(|d| d.device_id.as_str()).collect();
        assert!(device_ids.contains(&"dev-1")); // kept
        assert!(!device_ids.contains(&"dev-2")); // removed (HubDefault, not in new list)
        assert!(device_ids.contains(&"dev-3")); // kept (UserOverride)
        assert!(device_ids.contains(&"dev-4")); // added
    }

    #[test]
    fn translate_room_id() {
        let mut store = RoomTopologyStore::new();
        let discovered = make_discovered("hue-room-1", "Kitchen", "gl-1");
        let action = store.sync_hub_room(&hue_key(), &discovered);
        let room_id = action.rhythm_room_id().to_string();

        assert_eq!(
            store.translate_room_id(&hue_key(), "hue-room-1"),
            Some(room_id.as_str())
        );
        assert_eq!(store.translate_room_id(&ha_key(), "ha-area-1"), None);
    }

    #[test]
    fn routing_table_after_approved_binding() {
        let mut store = RoomTopologyStore::new();

        let hue = make_discovered("hue-room-1", "Kitchen", "gl-1");
        let action = store.sync_hub_room(&hue_key(), &hue);
        let room_id = action.rhythm_room_id().to_string();

        // Approve a binding so HA Kitchen binds to the Hue Kitchen room
        store.approve_binding(ha_key(), "ha-area-1".to_string(), room_id.clone(), 1500);

        let ha = make_discovered("ha-area-1", "Kitchen", "ha-area-1");
        store.sync_hub_room(&ha_key(), &ha);

        let table = store.routing_table();
        let targets = table.get(&room_id).unwrap();
        assert_eq!(targets.len(), 2); // Both hubs route to same room
    }

    #[test]
    fn create_room() {
        let mut store = RoomTopologyStore::new();
        let id = store.create_room("Custom Room");

        let room = store.get(&id).unwrap();
        assert_eq!(room.name, "Custom Room");
        assert!(room.user_customized);
    }

    #[test]
    fn rename_room() {
        let mut store = RoomTopologyStore::new();
        let id = store.create_room("Old Name");
        assert!(store.rename_room(&id, "New Name"));

        assert_eq!(store.get(&id).unwrap().name, "New Name");
        assert!(store.get(&id).unwrap().user_customized);
    }

    #[test]
    fn merge_rooms() {
        let mut store = RoomTopologyStore::new();

        // Create two rooms with different hub targets
        let hue = make_discovered("hue-room-1", "Kitchen", "gl-1");
        let action1 = store.sync_hub_room(&hue_key(), &hue);
        let target_id = action1.rhythm_room_id().to_string();

        let ha = make_discovered("ha-area-1", "Kitchen Extension", "ha-area-1");
        let action2 = store.sync_hub_room(&ha_key(), &ha);
        let source_id = action2.rhythm_room_id().to_string();

        assert_eq!(store.room_count(), 2);

        assert!(store.merge_rooms(&target_id, &source_id));

        assert_eq!(store.room_count(), 1);
        let merged = store.get(&target_id).unwrap();
        assert_eq!(merged.hub_targets.len(), 2);
        assert!(merged.user_customized);
    }

    #[test]
    fn move_device() {
        let mut store = RoomTopologyStore::new();
        let id1 = store.create_room("Room A");
        let id2 = store.create_room("Room B");

        store.get_mut(&id1).unwrap().add_device_hub_default("dev-1");
        assert!(store.move_device("dev-1", &id1, &id2));

        assert!(!store
            .get(&id1)
            .unwrap()
            .devices
            .iter()
            .any(|d| d.device_id == "dev-1"));
        let dev = store
            .get(&id2)
            .unwrap()
            .devices
            .iter()
            .find(|d| d.device_id == "dev-1")
            .unwrap();
        assert_eq!(dev.placement, DevicePlacement::UserOverride);
    }

    #[test]
    fn serialization_roundtrip() {
        let mut store = RoomTopologyStore::new();
        let hue = make_discovered("hue-room-1", "Kitchen", "gl-1");
        store.sync_hub_room(&hue_key(), &hue);

        let json = serde_json::to_string(&store).unwrap();
        let mut restored: RoomTopologyStore = serde_json::from_str(&json).unwrap();
        restored.rebuild_indices();

        assert_eq!(restored.room_count(), 1);
        assert!(restored
            .find_by_hub_room(&hue_key(), "hue-room-1")
            .is_some());
    }

    // ---- translate_or_create tests ----

    #[test]
    fn translate_or_create_creates_new_room() {
        let mut store = RoomTopologyStore::new();
        let id = store.translate_or_create(&hue_key(), "hue-room-1", "Kitchen", "gl-1", &[]);

        // Returns a UUID, not the hub-native ID
        assert_ne!(id, "hue-room-1");
        assert_eq!(store.room_count(), 1);

        // Can look up by topology ID
        let room = store.get(&id).unwrap();
        assert_eq!(room.name, "Kitchen");
        assert_eq!(room.hub_targets.len(), 1);
        assert_eq!(room.hub_targets[0].hub_room_id, "hue-room-1");
        assert_eq!(room.hub_targets[0].control_id, "gl-1");
    }

    #[test]
    fn translate_or_create_returns_existing() {
        let mut store = RoomTopologyStore::new();
        let id1 = store.translate_or_create(&hue_key(), "hue-room-1", "Kitchen", "gl-1", &[]);
        let id2 = store.translate_or_create(&hue_key(), "hue-room-1", "Kitchen", "gl-1", &[]);

        assert_eq!(id1, id2, "should return same topology ID for same hub room");
        assert_eq!(store.room_count(), 1, "should not create duplicate");
    }

    #[test]
    fn translate_or_create_reapplies_approved_binding() {
        let mut store = RoomTopologyStore::new();

        // Create an existing room (e.g., from another hub)
        let existing_id = store.create_room("Kitchen");

        // Approve a binding from hue-room-1 to that room
        store.approve_binding(
            hue_key(),
            "hue-room-1".to_string(),
            existing_id.clone(),
            1000,
        );

        // translate_or_create should bind to the existing room
        let id = store.translate_or_create(&hue_key(), "hue-room-1", "Kitchen", "gl-1", &[]);

        assert_eq!(id, existing_id, "should bind to approved target");
        assert_eq!(store.room_count(), 1, "should not create new room");

        // The existing room should now have the hub target
        let room = store.get(&existing_id).unwrap();
        assert_eq!(room.hub_targets.len(), 1);
        assert_eq!(room.hub_targets[0].hub_room_id, "hue-room-1");
    }

    #[test]
    fn translate_or_create_idempotent_with_sync_hub_room() {
        let mut store = RoomTopologyStore::new();

        // First: translate_or_create creates minimal entry
        let topo_id = store.translate_or_create(&hue_key(), "hue-room-1", "Kitchen", "gl-1", &[]);

        // Then: sync_hub_room enriches the same entry (Rule 1: already mapped)
        let discovered = DiscoveredTopologyRoom {
            hub_room_id: "hue-room-1".to_string(),
            name: "Kitchen".to_string(),
            control_id: "gl-1".to_string(),
            light_device_ids: vec!["light-1".to_string()],
            canonical_device_ids: vec!["canonical-1".to_string()],
        };
        let action = store.sync_hub_room(&hue_key(), &discovered);

        // Should update (not create new)
        assert!(matches!(action, SyncAction::Updated { .. }));
        assert_eq!(store.room_count(), 1);

        // Room should have the enriched data
        let room = store.get(&topo_id).unwrap();
        assert_eq!(room.hub_targets[0].light_device_ids, vec!["light-1"]);
        assert_eq!(room.devices.len(), 1);
        assert_eq!(room.devices[0].device_id, "canonical-1");
    }

    #[test]
    fn composite_routing_no_hub_native_aliases() {
        let mut store = RoomTopologyStore::new();
        let topo_id = store.translate_or_create(&hue_key(), "hue-room-1", "Kitchen", "gl-1", &[]);

        let routing = store.composite_routing();

        // Should have entry for topology ID
        assert!(routing.contains_key(&topo_id));
        // Should NOT have alias for hub-native ID
        assert!(!routing.contains_key("hue-room-1"));
    }

    // ---- cross-hub triage detection tests ----

    #[test]
    fn sync_hub_room_detects_name_match_after_translate_or_create() {
        let mut store = RoomTopologyStore::new();

        // HA creates "Office" room first (via translate_or_create in do_room_set)
        let ha_office_id = store.translate_or_create(&ha_key(), "office", "Office", "office", &[]);
        // Simulate Phase 4c completing for HA: add canonical devices
        store
            .get_mut(&ha_office_id)
            .unwrap()
            .add_device_hub_default("canonical-ha-1");

        // Hue connects: translate_or_create creates a separate room for Hue's "Office"
        let hue_office_id =
            store.translate_or_create(&hue_key(), "hue-office-uuid", "Office", "gl-office", &[]);
        assert_ne!(
            ha_office_id, hue_office_id,
            "translate_or_create should create separate room"
        );
        assert_eq!(store.room_count(), 2);

        // Phase 4c for Hue: sync_hub_room should detect the name match
        // because hue_office_id was freshly created (1 target, no devices)
        let discovered = DiscoveredTopologyRoom {
            hub_room_id: "hue-office-uuid".to_string(),
            name: "Office".to_string(),
            control_id: "gl-office".to_string(),
            light_device_ids: vec!["hue-light-1".to_string()],
            canonical_device_ids: vec!["canonical-hue-1".to_string()],
        };
        let action = store.sync_hub_room(&hue_key(), &discovered);

        // Should return a proposal (not Updated)
        assert!(
            matches!(action, SyncAction::CreatedWithProposal { ref proposed_target_id, .. } if *proposed_target_id == ha_office_id),
            "Expected CreatedWithProposal targeting HA's Office room, got {:?}",
            action
        );
    }

    #[test]
    fn sync_hub_room_no_false_proposal_for_established_room() {
        let mut store = RoomTopologyStore::new();

        // HA creates "Office" room
        let ha_office_id = store.translate_or_create(&ha_key(), "office", "Office", "office", &[]);
        store
            .get_mut(&ha_office_id)
            .unwrap()
            .add_device_hub_default("canonical-ha-1");

        // Hue creates "Office" room and fully syncs (has devices)
        let hue_office_id =
            store.translate_or_create(&hue_key(), "hue-office-uuid", "Office", "gl-office", &[]);
        // Simulate a completed Phase 4c for the Hue room (adds devices)
        store
            .get_mut(&hue_office_id)
            .unwrap()
            .add_device_hub_default("canonical-hue-1");

        // On the NEXT sync, sync_hub_room should treat it as a normal update
        // (not generate a duplicate proposal) because the room is established
        let discovered = make_discovered("hue-office-uuid", "Office", "gl-office");
        let action = store.sync_hub_room(&hue_key(), &discovered);

        assert!(
            matches!(action, SyncAction::Updated { .. }),
            "Expected Updated for established room, got {:?}",
            action
        );
    }

    #[test]
    fn translate_or_create_handles_approved_binding_directly() {
        // When an approved binding exists, translate_or_create binds directly —
        // no separate room is created, so sync_hub_room just updates.
        let mut store = RoomTopologyStore::new();

        // HA creates "Office" room
        let ha_office_id = store.translate_or_create(&ha_key(), "office", "Office", "office", &[]);
        store
            .get_mut(&ha_office_id)
            .unwrap()
            .add_device_hub_default("canonical-ha-1");

        // Approve the binding (simulates user accepting triage proposal)
        store.approve_binding(
            hue_key(),
            "hue-office-uuid".to_string(),
            ha_office_id.clone(),
            1000,
        );

        // On next sync, translate_or_create should bind directly
        let hue_office_id =
            store.translate_or_create(&hue_key(), "hue-office-uuid", "Office", "gl-office", &[]);
        assert_eq!(
            ha_office_id, hue_office_id,
            "Approved binding should reuse HA's room"
        );
        assert_eq!(store.room_count(), 1, "No duplicate room should be created");

        // sync_hub_room should see it as already-established (has 2 targets + devices)
        let discovered = DiscoveredTopologyRoom {
            hub_room_id: "hue-office-uuid".to_string(),
            name: "Office".to_string(),
            control_id: "gl-office".to_string(),
            light_device_ids: vec!["hue-light-1".to_string()],
            canonical_device_ids: vec!["canonical-hue-1".to_string()],
        };
        let action = store.sync_hub_room(&hue_key(), &discovered);
        assert!(matches!(action, SyncAction::Updated { .. }));
    }
}
