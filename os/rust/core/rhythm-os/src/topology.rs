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
//! - `turn_on(room_id, cmd)` → look up topology-derived dispatch routes
//!   rebuilt from source room bindings + canonical room membership
//! - `any_lights_on(room_id)` → OR across all hub controllers
//! - Source room bindings remain authoritative for sync/triage/translation
//! - Dispatch targets are explicit: grouped hub control or direct device lists
//! - Room IDs are server-assigned (topology room IDs), not hub-native

use std::collections::{HashMap, HashSet};

use log::{debug, info};
use rhythm_core::{runtime::hub_registry::DeviceType, HubDispatchTarget};
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
    /// Device exists first-class without a parent room.
    Standalone,
}

/// Generic node-to-node control relationships.
///
/// This is the topology-level control graph used to resolve automation and
/// input behavior without assuming that every target is a room.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeControlKind {
    Motion,
    Button,
    Switch,
}

impl NodeControlKind {
    pub const fn default_for_device_type(device_type: &DeviceType) -> Option<Self> {
        match device_type {
            DeviceType::Motion => Some(Self::Motion),
            DeviceType::Button => Some(Self::Button),
            DeviceType::Light => None,
        }
    }
}

/// Persisted explicit control override from one topology node to another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeControlLink {
    pub source_id: String,
    pub target_id: String,
    pub kind: NodeControlKind,
}

/// A device assigned to a Rhythm room.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomDevice {
    /// Canonical device ID.
    pub device_id: String,
    /// How this device ended up in this room.
    pub placement: DevicePlacement,
}

/// A first-class device node in the topology graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopologyDeviceNode {
    /// Stable topology node ID. Uses the canonical device ID.
    pub id: String,
    /// Canonical device ID.
    pub canonical_device_id: String,
    /// Optional parent room ID when attached into a room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// How this device ended up where it is.
    pub placement: DevicePlacement,
}

/// A discovered source room bound into a topology room.
///
/// This is the authoritative source-side mapping used for room sync, triage,
/// reverse lookups, and topology persistence. It is not itself the dispatch
/// model; dispatch targets are rebuilt from canonical room membership.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubRoomBinding {
    /// Which hub instance this source room belongs to.
    pub hub_key: HubKey,
    /// Hub-native room ID (Hue room UUID, HA area_id).
    pub hub_room_id: String,
    /// Hub-native grouped control target (Hue grouped_light, HA area_id).
    pub control_id: String,
    /// Hub-native light device IDs currently reported in this source room.
    pub light_device_ids: Vec<String>,
}

/// A Rhythm room — independent of any hub's room hierarchy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopologyRoom {
    /// Stable Rhythm room UUID.
    pub id: String,
    /// User-overridable display name.
    pub name: String,
    /// Bound source rooms from connected hubs.
    pub hub_room_bindings: Vec<HubRoomBinding>,
    /// Canonical devices assigned to this room.
    pub devices: Vec<RoomDevice>,
    /// Whether the user has customized this room (rename, merge, split, move devices).
    #[serde(default)]
    pub user_customized: bool,
    /// Original hub name, used for cross-hub name matching during sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_name: Option<String>,
}

/// A schedulable light-control node derived from topology membership.
///
/// Public APIs and persistence remain room-centric. These nodes are internal
/// runtime addresses used for periodic scheduling and typed composite routing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyLightNode {
    /// Stable internal routing/scheduler identifier.
    pub id: String,
    /// Owning settings node ID whose state this target inherits.
    pub source_node_id: String,
    /// Public node ID whose state updates should be emitted after the tick.
    pub emit_node_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TopologyLightNodeRoute {
    node: TopologyLightNode,
    hub_key: HubKey,
    target: HubDispatchTarget,
}

#[derive(Clone, Debug, Default)]
struct DispatchPlan {
    room_targets: Vec<(HubKey, HubDispatchTarget)>,
    node_routes: Vec<TopologyLightNodeRoute>,
}

const INTERNAL_LIGHT_NODE_PREFIX: &str = "__rhythm_light_node__";

fn group_light_node_id(room_id: &str, hub_key: &HubKey, hub_room_id: &str) -> String {
    format!(
        "{INTERNAL_LIGHT_NODE_PREFIX}|room={room_id}|kind=group|hub={hub_key}|source={hub_room_id}"
    )
}

impl TopologyRoom {
    /// Create a new empty room.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: id.into(),
            name: name.clone(),
            hub_room_bindings: Vec::new(),
            devices: Vec::new(),
            user_customized: false,
            bootstrap_name: Some(name),
        }
    }

    /// Find a source room binding by hub key.
    pub fn binding_for_hub(&self, hub_key: &HubKey) -> Option<&HubRoomBinding> {
        self.hub_room_bindings
            .iter()
            .find(|t| &t.hub_key == hub_key)
    }

    /// Find a source room binding by hub room ID.
    pub fn binding_for_hub_room(
        &self,
        hub_key: &HubKey,
        hub_room_id: &str,
    ) -> Option<&HubRoomBinding> {
        self.hub_room_bindings
            .iter()
            .find(|t| &t.hub_key == hub_key && t.hub_room_id == hub_room_id)
    }

    /// Add or update a source room binding.
    pub fn upsert_hub_room_binding(&mut self, binding: HubRoomBinding) {
        if let Some(existing) = self
            .hub_room_bindings
            .iter_mut()
            .find(|t| t.hub_key == binding.hub_key && t.hub_room_id == binding.hub_room_id)
        {
            existing.control_id = binding.control_id;
            existing.light_device_ids = binding.light_device_ids;
        } else {
            self.hub_room_bindings.push(binding);
        }
    }

    /// Remove a source room binding.
    pub fn remove_hub_room_binding(&mut self, hub_key: &HubKey, hub_room_id: &str) {
        self.hub_room_bindings
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

    /// Check if this room has any source room bindings.
    pub fn has_bindings(&self) -> bool {
        !self.hub_room_bindings.is_empty()
    }

    /// Check if this room has devices.
    pub fn has_devices(&self) -> bool {
        !self.devices.is_empty()
    }

    fn preferred_light_endpoints_by_hub(
        &self,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> HashMap<HubKey, HashMap<String, String>> {
        let mut by_hub: HashMap<HubKey, HashMap<String, String>> = HashMap::new();
        for room_device in &self.devices {
            let Some(device) = canonical_registry.get(&room_device.device_id) else {
                continue;
            };
            if device.device_type != DeviceType::Light {
                continue;
            }
            let Some(endpoint) = device.preferred_endpoint() else {
                continue;
            };
            by_hub
                .entry(endpoint.hub_key.clone())
                .or_default()
                .insert(endpoint.native_id.clone(), room_device.device_id.clone());
        }
        by_hub
    }

    fn dispatch_plan(
        &self,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> DispatchPlan {
        let mut plan = DispatchPlan::default();
        let preferred_light_endpoints = self.preferred_light_endpoints_by_hub(canonical_registry);

        if preferred_light_endpoints.is_empty() {
            let mut bindings = self.hub_room_bindings.clone();
            bindings.sort_by(|left, right| {
                left.hub_key
                    .to_string()
                    .cmp(&right.hub_key.to_string())
                    .then_with(|| left.hub_room_id.cmp(&right.hub_room_id))
            });
            for binding in bindings {
                if binding.light_device_ids.is_empty() {
                    continue;
                }
                let target = HubDispatchTarget::Group {
                    room_id: binding.hub_room_id.clone(),
                    control_id: binding.control_id.clone(),
                };
                plan.room_targets
                    .push((binding.hub_key.clone(), target.clone()));
                plan.node_routes.push(TopologyLightNodeRoute {
                    node: TopologyLightNode {
                        id: group_light_node_id(&self.id, &binding.hub_key, &binding.hub_room_id),
                        source_node_id: self.id.clone(),
                        emit_node_id: self.id.clone(),
                    },
                    hub_key: binding.hub_key,
                    target,
                });
            }
            return plan;
        }

        let mut assigned_by_hub: Vec<_> = preferred_light_endpoints.into_iter().collect();
        assigned_by_hub.sort_by(|left, right| left.0.to_string().cmp(&right.0.to_string()));

        for (hub_key, mut remaining_ids) in assigned_by_hub {
            let mut bindings: Vec<_> = self
                .hub_room_bindings
                .iter()
                .filter(|binding| binding.hub_key == hub_key)
                .collect();
            bindings.sort_by(|left, right| left.hub_room_id.cmp(&right.hub_room_id));

            for binding in bindings {
                if binding.light_device_ids.is_empty() {
                    continue;
                }
                if binding
                    .light_device_ids
                    .iter()
                    .all(|native_id| remaining_ids.contains_key(native_id))
                {
                    let target = HubDispatchTarget::Group {
                        room_id: binding.hub_room_id.clone(),
                        control_id: binding.control_id.clone(),
                    };
                    plan.room_targets.push((hub_key.clone(), target.clone()));
                    plan.node_routes.push(TopologyLightNodeRoute {
                        node: TopologyLightNode {
                            id: group_light_node_id(&self.id, &hub_key, &binding.hub_room_id),
                            source_node_id: self.id.clone(),
                            emit_node_id: self.id.clone(),
                        },
                        hub_key: hub_key.clone(),
                        target,
                    });
                    for native_id in &binding.light_device_ids {
                        remaining_ids.remove(native_id);
                    }
                }
            }

            if !remaining_ids.is_empty() {
                let mut device_targets: Vec<_> = remaining_ids.into_iter().collect();
                device_targets
                    .sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));

                let mut native_ids: Vec<_> = device_targets
                    .iter()
                    .map(|(native_id, _)| native_id.clone())
                    .collect();
                native_ids.sort();
                plan.room_targets.push((
                    hub_key.clone(),
                    HubDispatchTarget::Devices {
                        native_ids: native_ids.clone(),
                    },
                ));

                for (native_id, device_id) in device_targets {
                    plan.node_routes.push(TopologyLightNodeRoute {
                        node: TopologyLightNode {
                            id: device_id.clone(),
                            source_node_id: device_id.clone(),
                            emit_node_id: self.id.clone(),
                        },
                        hub_key: hub_key.clone(),
                        target: HubDispatchTarget::Devices {
                            native_ids: vec![native_id],
                        },
                    });
                }
            }
        }

        plan
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
    /// First-class device nodes keyed by canonical/topology device ID.
    #[serde(default)]
    device_nodes: HashMap<String, TopologyDeviceNode>,
    /// Explicit node-to-node control overrides.
    #[serde(default)]
    control_links: Vec<NodeControlLink>,
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
            device_nodes: HashMap::new(),
            control_links: Vec::new(),
            approved_bindings: Vec::new(),
            hub_room_index: HashMap::new(),
        }
    }

    /// Rebuild indices from the room map. Call after deserialization.
    pub fn rebuild_indices(&mut self) {
        self.hub_room_index.clear();
        for (room_id, room) in &self.rooms {
            for binding in &room.hub_room_bindings {
                self.hub_room_index.insert(
                    (binding.hub_key.to_string(), binding.hub_room_id.clone()),
                    room_id.clone(),
                );
            }
        }
        self.rebuild_room_device_projections();
    }

    fn rebuild_room_device_projections(&mut self) {
        for room in self.rooms.values_mut() {
            room.devices.clear();
        }

        let mut device_ids: Vec<_> = self.device_nodes.keys().cloned().collect();
        device_ids.sort();
        for device_id in device_ids {
            let Some(node) = self.device_nodes.get(&device_id) else {
                continue;
            };
            let Some(parent_id) = node.parent_id.as_deref() else {
                continue;
            };
            let Some(room) = self.rooms.get_mut(parent_id) else {
                continue;
            };
            room.devices.push(RoomDevice {
                device_id: node.canonical_device_id.clone(),
                placement: node.placement.clone(),
            });
        }

        let valid_public_nodes: std::collections::HashSet<String> = self
            .rooms
            .keys()
            .cloned()
            .chain(self.device_nodes.keys().cloned())
            .collect();
        self.control_links.retain(|link| {
            valid_public_nodes.contains(&link.source_id)
                && valid_public_nodes.contains(&link.target_id)
        });
    }

    pub fn has_public_node(&self, node_id: &str) -> bool {
        self.rooms.contains_key(node_id) || self.device_nodes.contains_key(node_id)
    }

    fn upsert_device_node(
        &mut self,
        canonical_device_id: &str,
        parent_id: Option<String>,
        placement: DevicePlacement,
    ) {
        self.device_nodes.insert(
            canonical_device_id.to_string(),
            TopologyDeviceNode {
                id: canonical_device_id.to_string(),
                canonical_device_id: canonical_device_id.to_string(),
                parent_id,
                placement,
            },
        );
    }

    fn set_device_node_placement(
        &mut self,
        canonical_device_id: &str,
        parent_id: Option<&str>,
        placement: DevicePlacement,
        create_if_missing: bool,
    ) -> bool {
        if let Some(room_id) = parent_id {
            if self.rooms.get(room_id).is_none() {
                return false;
            }
        }

        let existing_parent_id = match self.device_nodes.get(canonical_device_id) {
            Some(node) => node.parent_id.clone(),
            None if create_if_missing => {
                self.upsert_device_node(
                    canonical_device_id,
                    parent_id.map(str::to_string),
                    if parent_id.is_some() {
                        placement.clone()
                    } else {
                        DevicePlacement::Standalone
                    },
                );
                if placement == DevicePlacement::UserOverride {
                    if let Some(room_id) = parent_id {
                        if let Some(room) = self.rooms.get_mut(room_id) {
                            room.user_customized = true;
                        }
                    }
                }
                self.rebuild_room_device_projections();
                return true;
            }
            None => return false,
        };

        let effective_placement = if parent_id.is_some() {
            placement.clone()
        } else {
            DevicePlacement::Standalone
        };
        if let Some(node) = self.device_nodes.get_mut(canonical_device_id) {
            node.parent_id = parent_id.map(str::to_string);
            node.placement = effective_placement;
        }

        if placement == DevicePlacement::UserOverride {
            if let Some(room_id) = existing_parent_id.as_deref() {
                if let Some(room) = self.rooms.get_mut(room_id) {
                    room.user_customized = true;
                }
            }
            if let Some(room_id) = parent_id {
                if let Some(room) = self.rooms.get_mut(room_id) {
                    room.user_customized = true;
                }
            }
        }

        self.rebuild_room_device_projections();
        true
    }

    fn sync_hub_default_device_for_room(&mut self, room_id: &str, canonical_device_id: &str) {
        match self.device_nodes.get_mut(canonical_device_id) {
            Some(node) if node.placement != DevicePlacement::HubDefault => {}
            Some(node) => {
                node.parent_id = Some(room_id.to_string());
                node.placement = DevicePlacement::HubDefault;
            }
            None => {
                self.upsert_device_node(
                    canonical_device_id,
                    Some(room_id.to_string()),
                    DevicePlacement::HubDefault,
                );
            }
        }
    }

    fn detach_hub_default_devices_for_room(
        &mut self,
        room_id: &str,
        keep_device_ids: &HashSet<String>,
    ) {
        for node in self.device_nodes.values_mut() {
            if node.parent_id.as_deref() != Some(room_id) {
                continue;
            }
            if node.placement != DevicePlacement::HubDefault {
                continue;
            }
            if keep_device_ids.contains(&node.canonical_device_id) {
                continue;
            }
            node.parent_id = None;
            node.placement = DevicePlacement::Standalone;
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
    /// 4. (Stale removal done separately via `remove_stale_bindings`)
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
                .map(|room| room.hub_room_bindings.len() == 1 && room.devices.is_empty())
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
                            && room.binding_for_hub(hub_key).is_none()
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
                            target_room.upsert_hub_room_binding(HubRoomBinding {
                                hub_key: hub_key.clone(),
                                hub_room_id: discovered.hub_room_id.clone(),
                                control_id: discovered.control_id.clone(),
                                light_device_ids: discovered.light_device_ids.clone(),
                            });
                            let keep_device_ids: HashSet<String> =
                                discovered.canonical_device_ids.iter().cloned().collect();
                            self.detach_hub_default_devices_for_room(&target_id, &keep_device_ids);
                            for dev_id in &discovered.canonical_device_ids {
                                self.sync_hub_default_device_for_room(&target_id, dev_id);
                            }
                            // Remove the duplicate room created by translate_or_create
                            self.rooms.remove(&rhythm_room_id);
                            self.hub_room_index.insert(index_key, target_id.clone());
                            self.rebuild_room_device_projections();
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
                        room.upsert_hub_room_binding(HubRoomBinding {
                            hub_key: hub_key.clone(),
                            hub_room_id: discovered.hub_room_id.clone(),
                            control_id: discovered.control_id.clone(),
                            light_device_ids: discovered.light_device_ids.clone(),
                        });
                        if !room.user_customized {
                            room.name = discovered.name.clone();
                        }
                    }
                    let keep_device_ids: HashSet<String> =
                        discovered.canonical_device_ids.iter().cloned().collect();
                    self.detach_hub_default_devices_for_room(&rhythm_room_id, &keep_device_ids);
                    for dev_id in &discovered.canonical_device_ids {
                        self.sync_hub_default_device_for_room(&rhythm_room_id, dev_id);
                    }
                    self.rebuild_room_device_projections();

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
                room.upsert_hub_room_binding(HubRoomBinding {
                    hub_key: hub_key.clone(),
                    hub_room_id: discovered.hub_room_id.clone(),
                    control_id: discovered.control_id.clone(),
                    light_device_ids: discovered.light_device_ids.clone(),
                });

                if !room.user_customized {
                    room.name = discovered.name.clone();
                }
            }
            let keep_device_ids: HashSet<String> =
                discovered.canonical_device_ids.iter().cloned().collect();
            self.detach_hub_default_devices_for_room(&rhythm_room_id, &keep_device_ids);
            for dev_id in &discovered.canonical_device_ids {
                self.sync_hub_default_device_for_room(&rhythm_room_id, dev_id);
            }
            self.rebuild_room_device_projections();
            if let Some(room) = self.rooms.get(&rhythm_room_id) {
                debug!(target: "topology", "Updated room '{}' ({})", room.name, rhythm_room_id);
            }
            return SyncAction::Updated { rhythm_room_id };
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
                (matches_bootstrap || matches_name) && room.binding_for_hub(hub_key).is_none()
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
                    let room_name = room.name.clone();
                    room.upsert_hub_room_binding(HubRoomBinding {
                        hub_key: hub_key.clone(),
                        hub_room_id: discovered.hub_room_id.clone(),
                        control_id: discovered.control_id.clone(),
                        light_device_ids: discovered.light_device_ids.clone(),
                    });
                    let _ = room;
                    let keep_device_ids: HashSet<String> =
                        discovered.canonical_device_ids.iter().cloned().collect();
                    self.detach_hub_default_devices_for_room(&target_id, &keep_device_ids);
                    for dev_id in &discovered.canonical_device_ids {
                        self.sync_hub_default_device_for_room(&target_id, dev_id);
                    }
                    self.hub_room_index.insert(index_key, target_id.clone());
                    self.rebuild_room_device_projections();

                    info!(target: "topology",
                        "Re-applied approved binding: hub room '{}' → Rhythm room '{}' ({})",
                        discovered.name, room_name, target_id
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
            room.upsert_hub_room_binding(HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: discovered.hub_room_id.clone(),
                control_id: discovered.control_id.clone(),
                light_device_ids: discovered.light_device_ids.clone(),
            });

            self.hub_room_index
                .insert(index_key, rhythm_room_id.clone());
            self.rooms.insert(rhythm_room_id.clone(), room);
            for dev_id in &discovered.canonical_device_ids {
                self.sync_hub_default_device_for_room(&rhythm_room_id, dev_id);
            }
            self.rebuild_room_device_projections();

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
        room.upsert_hub_room_binding(HubRoomBinding {
            hub_key: hub_key.clone(),
            hub_room_id: discovered.hub_room_id.clone(),
            control_id: discovered.control_id.clone(),
            light_device_ids: discovered.light_device_ids.clone(),
        });

        self.hub_room_index
            .insert(index_key, rhythm_room_id.clone());
        self.rooms.insert(rhythm_room_id.clone(), room);
        for dev_id in &discovered.canonical_device_ids {
            self.sync_hub_default_device_for_room(&rhythm_room_id, dev_id);
        }
        self.rebuild_room_device_projections();

        info!(target: "topology", "Created Rhythm room '{}' ({})", discovered.name, rhythm_room_id);
        SyncAction::Created { rhythm_room_id }
    }

    /// Remove stale source room bindings for a hub that no longer reports certain rooms.
    ///
    /// Rule 4: Never delete Rhythm rooms — only remove source bindings.
    pub fn remove_stale_bindings(
        &mut self,
        hub_key: &HubKey,
        current_hub_room_ids: &[String],
    ) -> Vec<String> {
        let current_set: HashSet<&String> = current_hub_room_ids.iter().collect();
        let mut affected = Vec::new();

        for (room_id, room) in &mut self.rooms {
            let had_target = room.hub_room_bindings.len();
            room.hub_room_bindings
                .retain(|t| &t.hub_key != hub_key || current_set.contains(&t.hub_room_id));

            if room.hub_room_bindings.len() < had_target {
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

    /// Remove a canonical device from every topology room.
    pub fn remove_device_everywhere(&mut self, device_id: &str) -> Vec<String> {
        let mut affected = Vec::new();
        if let Some(node) = self.device_nodes.remove(device_id) {
            if let Some(parent_id) = node.parent_id {
                affected.push(parent_id);
            }
        }
        self.rebuild_room_device_projections();
        affected
    }

    /// Remove a source room binding from every topology room and clear the room index.
    pub fn remove_hub_room_binding_everywhere(
        &mut self,
        hub_key: &HubKey,
        hub_room_id: &str,
    ) -> Vec<String> {
        let mut affected = Vec::new();

        for (room_id, room) in &mut self.rooms {
            let had_targets = room.hub_room_bindings.len();
            room.remove_hub_room_binding(hub_key, hub_room_id);
            if room.hub_room_bindings.len() < had_targets {
                affected.push(room_id.clone());
            }
        }

        self.hub_room_index
            .remove(&(hub_key.to_string(), hub_room_id.to_string()));

        affected
    }

    // ---- Query methods ----

    /// Get all rooms.
    pub fn rooms(&self) -> impl Iterator<Item = &TopologyRoom> {
        self.rooms.values()
    }

    /// Get all first-class device nodes.
    pub fn device_nodes(&self) -> impl Iterator<Item = &TopologyDeviceNode> {
        self.device_nodes.values()
    }

    /// Get a room by Rhythm room ID.
    pub fn get(&self, room_id: &str) -> Option<&TopologyRoom> {
        self.rooms.get(room_id)
    }

    /// Get a room by Rhythm room ID (mutable).
    pub fn get_mut(&mut self, room_id: &str) -> Option<&mut TopologyRoom> {
        self.rooms.get_mut(room_id)
    }

    /// Get a device node by topology/canonical device ID.
    pub fn get_device_node(&self, node_id: &str) -> Option<&TopologyDeviceNode> {
        self.device_nodes.get(node_id)
    }

    /// Get the parent room ID for a device node, if attached.
    pub fn device_parent_room_id(&self, device_id: &str) -> Option<&str> {
        self.device_nodes
            .get(device_id)
            .and_then(|node| node.parent_id.as_deref())
    }

    /// Get all persisted explicit control links.
    pub fn control_links(&self) -> &[NodeControlLink] {
        &self.control_links
    }

    /// Find an explicit control target for a source node and control kind.
    pub fn explicit_control_target(&self, source_id: &str, kind: &NodeControlKind) -> Option<&str> {
        self.control_links
            .iter()
            .find(|link| link.source_id == source_id && &link.kind == kind)
            .map(|link| link.target_id.as_str())
    }

    /// Resolve the effective target for a source node and control kind.
    ///
    /// Explicit topology links win. If no override exists, device nodes inherit
    /// their parent room as the default control target.
    pub fn effective_control_target(
        &self,
        source_id: &str,
        kind: &NodeControlKind,
    ) -> Option<String> {
        if let Some(target_id) = self.explicit_control_target(source_id, kind) {
            return Some(target_id.to_string());
        }

        self.device_nodes
            .get(source_id)
            .and_then(|node| node.parent_id.clone())
    }

    /// Set or clear an explicit control target override.
    pub fn set_control_target(
        &mut self,
        source_id: &str,
        kind: NodeControlKind,
        target_id: Option<&str>,
    ) -> bool {
        if !self.has_public_node(source_id) {
            return false;
        }
        if let Some(target_id) = target_id {
            if !self.has_public_node(target_id) {
                return false;
            }
        }

        self.control_links
            .retain(|link| !(link.source_id == source_id && link.kind == kind));

        if let Some(target_id) = target_id {
            self.control_links.push(NodeControlLink {
                source_id: source_id.to_string(),
                target_id: target_id.to_string(),
                kind,
            });
        }

        true
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

    /// Resolve any room-like identifier to the owning topology room ID.
    ///
    /// Accepts:
    /// - topology room IDs
    /// - bound hub-native room IDs
    /// - direct hub-native device IDs assigned into a topology room
    pub fn resolve_room_alias(
        &self,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
        hub_key: Option<&HubKey>,
        room_id: &str,
    ) -> Option<String> {
        if self.rooms.contains_key(room_id) {
            return Some(room_id.to_string());
        }

        if self.device_nodes.contains_key(room_id) {
            return Some(room_id.to_string());
        }

        if let Some(hub_key) = hub_key {
            if let Some(topology_id) = self.translate_room_id(hub_key, room_id) {
                return Some(topology_id.to_string());
            }
        }

        if let Some(topology_id) = self.translate_room_id_any_hub(room_id) {
            return Some(topology_id.to_string());
        }

        self.device_nodes.values().find_map(|node| {
            canonical_registry
                .get(&node.canonical_device_id)
                .is_some_and(|device| {
                    device.endpoints.iter().any(|endpoint| {
                        endpoint.native_id == room_id
                            && hub_key.is_none_or(|expected| &endpoint.hub_key == expected)
                    })
                })
                .then(|| node.id.clone())
        })
    }

    /// Derive the effective control relationships exposed by a public source node.
    pub fn effective_node_controls(
        &self,
        source_id: &str,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> Vec<(NodeControlKind, String, bool)> {
        let mut kinds = Vec::new();

        if let Some(node) = self.device_nodes.get(source_id) {
            if let Some(device) = canonical_registry.get(&node.canonical_device_id) {
                if let Some(kind) = NodeControlKind::default_for_device_type(&device.device_type) {
                    kinds.push(kind);
                }
            }
        }

        for link in &self.control_links {
            if link.source_id == source_id && !kinds.contains(&link.kind) {
                kinds.push(link.kind.clone());
            }
        }

        let mut controls = Vec::new();
        for kind in kinds {
            let explicit = self.explicit_control_target(source_id, &kind).is_some();
            if let Some(target_id) = self.effective_control_target(source_id, &kind) {
                controls.push((kind, target_id, !explicit));
            }
        }

        controls.sort_by(|left, right| {
            format!("{:?}", left.0)
                .cmp(&format!("{:?}", right.0))
                .then_with(|| left.1.cmp(&right.1))
        });
        controls
    }

    /// Collect effective control targets for all source nodes of a given kind.
    pub fn effective_control_targets_for_kind(
        &self,
        kind: &NodeControlKind,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> HashSet<String> {
        let mut targets = HashSet::new();
        for node in self.device_nodes.values() {
            let Some(device) = canonical_registry.get(&node.canonical_device_id) else {
                continue;
            };
            if NodeControlKind::default_for_device_type(&device.device_type).as_ref() != Some(kind)
                && self.explicit_control_target(&node.id, kind).is_none()
            {
                continue;
            }

            if let Some(target_id) = self.effective_control_target(&node.id, kind) {
                targets.insert(target_id);
            }
        }
        targets
    }

    /// Get the grouped-binding routing table: rhythm_room_id → Vec<(hub_key, control_id)>.
    ///
    /// This only reflects persisted source room bindings. Composite dispatch
    /// should use [`Self::composite_routing`] to include direct-device routes.
    pub fn routing_table(&self) -> HashMap<String, Vec<(HubKey, String)>> {
        let mut table = HashMap::new();
        for (room_id, room) in &self.rooms {
            let targets: Vec<_> = room
                .hub_room_bindings
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
        self.rebuild_room_device_projections();
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

        {
            let target = match self.rooms.get_mut(target_id) {
                Some(t) => t,
                None => {
                    // Put source back
                    self.rooms.insert(source_id.to_string(), source);
                    return false;
                }
            };

            // Move hub targets
            for source_target in source.hub_room_bindings {
                // Update index
                let key = (
                    source_target.hub_key.to_string(),
                    source_target.hub_room_id.clone(),
                );
                self.hub_room_index.insert(key, target_id.to_string());
                target.upsert_hub_room_binding(source_target);
            }

            target.user_customized = true;
        }

        // Move devices (preserving placement)
        for node in self.device_nodes.values_mut() {
            if node.parent_id.as_deref() == Some(source_id) {
                node.parent_id = Some(target_id.to_string());
            }
        }

        self.rebuild_room_device_projections();
        true
    }

    /// Move a device from one room to another.
    pub fn move_device(&mut self, device_id: &str, from_room: &str, to_room: &str) -> bool {
        if self.rooms.get(from_room).is_none() {
            return false;
        }
        if self.device_parent_room_id(device_id) != Some(from_room) {
            false
        } else {
            self.set_device_node_placement(
                device_id,
                Some(to_room),
                DevicePlacement::UserOverride,
                false,
            )
        }
    }

    /// Place a device into a room or detach it as a standalone first-class node.
    pub fn assign_device(
        &mut self,
        device_id: &str,
        room_id: Option<&str>,
        placement: DevicePlacement,
    ) -> bool {
        self.set_device_node_placement(device_id, room_id, placement, false)
    }

    /// Attach a device to a room as a hub-default member, creating the device
    /// node if needed.
    pub fn attach_device_hub_default(&mut self, room_id: &str, device_id: &str) -> bool {
        self.set_device_node_placement(device_id, Some(room_id), DevicePlacement::HubDefault, true)
    }

    /// Attach a device to a room as a user override, creating the device node
    /// if needed.
    pub fn attach_device_user_override(&mut self, room_id: &str, device_id: &str) -> bool {
        self.set_device_node_placement(
            device_id,
            Some(room_id),
            DevicePlacement::UserOverride,
            true,
        )
    }

    /// Insert or refresh a standalone device node.
    pub fn ensure_standalone_device(&mut self, device_id: &str) {
        let _ = self.set_device_node_placement(device_id, None, DevicePlacement::Standalone, true);
    }

    /// Insert a room directly (used for deserialization / testing).
    pub fn insert_room(&mut self, mut room: TopologyRoom) {
        for target in &room.hub_room_bindings {
            self.hub_room_index.insert(
                (target.hub_key.to_string(), target.hub_room_id.clone()),
                room.id.clone(),
            );
        }
        for device in &room.devices {
            self.device_nodes.insert(
                device.device_id.clone(),
                TopologyDeviceNode {
                    id: device.device_id.clone(),
                    canonical_device_id: device.device_id.clone(),
                    parent_id: Some(room.id.clone()),
                    placement: device.placement.clone(),
                },
            );
        }
        room.devices.clear();
        self.rooms.insert(room.id.clone(), room);
        self.rebuild_room_device_projections();
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
                room.upsert_hub_room_binding(HubRoomBinding {
                    hub_key: hub_key.clone(),
                    hub_room_id: hub_room_id.to_string(),
                    control_id: control_id.to_string(),
                    light_device_ids: light_device_ids.to_vec(),
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
        room.upsert_hub_room_binding(HubRoomBinding {
            hub_key: hub_key.clone(),
            hub_room_id: hub_room_id.to_string(),
            control_id: control_id.to_string(),
            light_device_ids: light_device_ids.to_vec(),
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

    fn device_dispatch_route(
        &self,
        node: &TopologyDeviceNode,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> Option<(HubKey, HubDispatchTarget)> {
        let device = canonical_registry.get(&node.canonical_device_id)?;
        if device.device_type != DeviceType::Light {
            return None;
        }
        let endpoint = device.preferred_endpoint()?;
        Some((
            endpoint.hub_key.clone(),
            HubDispatchTarget::Devices {
                native_ids: vec![endpoint.native_id.clone()],
            },
        ))
    }

    /// Build a routing table for the composite controller.
    ///
    /// Maps each Rhythm room ID to the typed dispatch routes required by the
    /// composite controller. All callers should use topology room IDs —
    /// hub-native IDs are translated at the event/handler boundary before
    /// reaching the composite.
    pub fn composite_routing(
        &self,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> HashMap<String, Vec<(String, HubDispatchTarget)>> {
        let mut table: HashMap<String, Vec<(String, HubDispatchTarget)>> = HashMap::new();
        let mut room_ids: Vec<_> = self.rooms.keys().cloned().collect();
        room_ids.sort();
        for room_id in room_ids {
            let room = self.rooms.get(&room_id).unwrap();
            let plan = room.dispatch_plan(canonical_registry);
            let targets: Vec<_> = plan
                .room_targets
                .into_iter()
                .map(|(hub_key, target)| (hub_key.to_string(), target))
                .collect();
            if !targets.is_empty() {
                table.insert(room_id.clone(), targets);
            }
            for node_route in plan.node_routes {
                table.insert(
                    node_route.node.id,
                    vec![(node_route.hub_key.to_string(), node_route.target)],
                );
            }
        }
        let mut standalone_node_ids: Vec<_> = self
            .device_nodes
            .values()
            .filter(|node| node.parent_id.is_none())
            .map(|node| node.id.clone())
            .collect();
        standalone_node_ids.sort();
        for node_id in standalone_node_ids {
            let node = self.device_nodes.get(&node_id).unwrap();
            if let Some((hub_key, target)) = self.device_dispatch_route(node, canonical_registry) {
                table.insert(node.id.clone(), vec![(hub_key.to_string(), target)]);
            }
        }
        table
    }

    /// Build the derived schedulable light nodes for periodic scheduling.
    pub fn periodic_light_nodes(
        &self,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> Vec<TopologyLightNode> {
        let mut nodes = Vec::new();
        let mut room_ids: Vec<_> = self.rooms.keys().cloned().collect();
        room_ids.sort();
        for room_id in room_ids {
            let room = self.rooms.get(&room_id).unwrap();
            let mut room_nodes: Vec<_> = room
                .dispatch_plan(canonical_registry)
                .node_routes
                .into_iter()
                .map(|route| route.node)
                .collect();
            room_nodes.sort_by(|left, right| left.id.cmp(&right.id));
            nodes.extend(room_nodes);
        }

        let mut standalone_node_ids: Vec<_> = self
            .device_nodes
            .values()
            .filter(|node| node.parent_id.is_none())
            .map(|node| node.id.clone())
            .collect();
        standalone_node_ids.sort();
        for node_id in standalone_node_ids {
            let node = self.device_nodes.get(&node_id).unwrap();
            if self
                .device_dispatch_route(node, canonical_registry)
                .is_some()
            {
                nodes.push(TopologyLightNode {
                    id: node.id.clone(),
                    source_node_id: node.id.clone(),
                    emit_node_id: node.id.clone(),
                });
            }
        }

        nodes
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
    use crate::canonical::identity::DiscoveredIdentity;
    use crate::canonical::registry::{CanonicalRegistry, ResolveResult};
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

    fn make_identity(
        native_id: &str,
        room_id: &str,
        room_name: &str,
        name: &str,
        device_type: DeviceType,
    ) -> DiscoveredIdentity {
        DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: room_id.to_string(),
            room_name: room_name.to_string(),
            name: name.to_string(),
            device_type,
            hardware_ids: vec![],
            manufacturer: None,
            model: None,
        }
    }

    fn register_identity(
        registry: &mut CanonicalRegistry,
        hub_key: &HubKey,
        identity: DiscoveredIdentity,
    ) -> String {
        match registry.resolve(&identity, hub_key, 1000) {
            ResolveResult::Created { canonical_id }
            | ResolveResult::AlreadyKnown { canonical_id }
            | ResolveResult::ReApproved { canonical_id } => canonical_id,
            ResolveResult::Queued { .. } => panic!("unexpected triage for test identity"),
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
        assert_eq!(room.hub_room_bindings.len(), 1);
        assert_eq!(room.hub_room_bindings[0].control_id, "gl-1");
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
        assert_eq!(room.hub_room_bindings[0].control_id, "gl-2");
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
        assert_eq!(room.hub_room_bindings.len(), 2);
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
        assert!(store.attach_device_user_override(&room_id, "dev-3"));

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
        assert_eq!(merged.hub_room_bindings.len(), 2);
        assert!(merged.user_customized);
    }

    #[test]
    fn move_device() {
        let mut store = RoomTopologyStore::new();
        let id1 = store.create_room("Room A");
        let id2 = store.create_room("Room B");

        assert!(store.attach_device_hub_default(&id1, "dev-1"));
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
        assert_eq!(room.hub_room_bindings.len(), 1);
        assert_eq!(room.hub_room_bindings[0].hub_room_id, "hue-room-1");
        assert_eq!(room.hub_room_bindings[0].control_id, "gl-1");
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
        assert_eq!(room.hub_room_bindings.len(), 1);
        assert_eq!(room.hub_room_bindings[0].hub_room_id, "hue-room-1");
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
        assert_eq!(room.hub_room_bindings[0].light_device_ids, vec!["light-1"]);
        assert_eq!(room.devices.len(), 1);
        assert_eq!(room.devices[0].device_id, "canonical-1");
    }

    #[test]
    fn composite_routing_no_hub_native_aliases() {
        let mut store = RoomTopologyStore::new();
        let light_ids = vec!["light-1".to_string()];
        let topo_id =
            store.translate_or_create(&hue_key(), "hue-room-1", "Kitchen", "gl-1", &light_ids);

        let canonical_registry = crate::canonical::registry::CanonicalRegistry::new();
        let routing = store.composite_routing(&canonical_registry);

        // Should have entry for topology ID
        assert!(routing.contains_key(&topo_id));
        // Should NOT have alias for hub-native ID
        assert!(!routing.contains_key("hue-room-1"));
    }

    #[test]
    fn composite_routing_uses_preferred_light_endpoints_only() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Studio");

        let mut registry = CanonicalRegistry::new();
        let light_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-light-1",
                "hue-room-1",
                "Studio",
                "Lamp",
                DeviceType::Light,
            ),
        );
        registry.get_mut(&light_id).unwrap().upsert_endpoint(
            ha_key(),
            "light.studio_lamp".to_string(),
            2000,
            None,
        );
        registry.set_preferred_endpoint(&light_id, &ha_key(), "light.studio_lamp");

        let button_id = register_identity(
            &mut registry,
            &ha_key(),
            make_identity(
                "switch.remote_1",
                "studio",
                "Studio",
                "Remote",
                DeviceType::Button,
            ),
        );

        assert!(store.attach_device_user_override(&room_id, &light_id));
        assert!(store.attach_device_user_override(&room_id, &button_id));

        let routing = store.composite_routing(&registry);
        assert_eq!(
            routing.get(&room_id),
            Some(&vec![(
                ha_key().to_string(),
                HubDispatchTarget::Devices {
                    native_ids: vec!["light.studio_lamp".to_string()],
                },
            )])
        );
        assert!(routing
            .values()
            .flatten()
            .all(|(_, target)| target.label() != "switch.remote_1"));

        let nodes = store.periodic_light_nodes(&registry);
        assert_eq!(
            nodes,
            vec![TopologyLightNode {
                id: light_id.clone(),
                source_node_id: light_id.clone(),
                emit_node_id: room_id.clone(),
            }]
        );
    }

    #[test]
    fn non_light_devices_do_not_block_group_dispatch_nodes() {
        let mut store = RoomTopologyStore::new();
        let room_id =
            store.translate_or_create(&hue_key(), "hue-room-1", "Kitchen", "gl-kitchen", &[]);

        let mut registry = CanonicalRegistry::new();
        let button_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "button-1",
                "hue-room-1",
                "Kitchen",
                "Wall Button",
                DeviceType::Button,
            ),
        );

        let room = store.get_mut(&room_id).unwrap();
        room.hub_room_bindings[0].light_device_ids = vec!["hue-light-1".to_string()];
        let _ = room;
        assert!(store.attach_device_user_override(&room_id, &button_id));

        let routing = store.composite_routing(&registry);
        assert_eq!(
            routing.get(&room_id),
            Some(&vec![(
                hue_key().to_string(),
                HubDispatchTarget::Group {
                    room_id: "hue-room-1".to_string(),
                    control_id: "gl-kitchen".to_string(),
                },
            )])
        );

        let nodes = store.periodic_light_nodes(&registry);
        assert_eq!(
            nodes,
            vec![TopologyLightNode {
                id: group_light_node_id(&room_id, &hue_key(), "hue-room-1"),
                source_node_id: room_id.clone(),
                emit_node_id: room_id.clone(),
            }]
        );
    }

    #[test]
    fn non_light_only_rooms_do_not_emit_dispatch_or_periodic_nodes() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Sensors");
        let mut registry = CanonicalRegistry::new();

        let button_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "button-2",
                "sensor-room",
                "Sensors",
                "Scene Button",
                DeviceType::Button,
            ),
        );

        let room = store.get_mut(&room_id).unwrap();
        room.upsert_hub_room_binding(HubRoomBinding {
            hub_key: hue_key(),
            hub_room_id: "sensor-room".to_string(),
            control_id: "sensor-room".to_string(),
            light_device_ids: vec![],
        });
        let _ = room;
        assert!(store.attach_device_user_override(&room_id, &button_id));

        let routing = store.composite_routing(&registry);
        assert!(!routing.contains_key(&room_id));
        assert!(store.periodic_light_nodes(&registry).is_empty());
    }

    #[test]
    fn motion_device_controls_inherit_parent_until_overridden() {
        let mut store = RoomTopologyStore::new();
        let source_room_id = store.create_room("Office");
        let target_room_id = store.create_room("Hall");
        let mut registry = CanonicalRegistry::new();

        let sensor_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "motion-1",
                "office-native",
                "Office",
                "Office Motion",
                DeviceType::Motion,
            ),
        );

        assert!(store.attach_device_user_override(&source_room_id, &sensor_id));
        assert_eq!(
            store.effective_node_controls(&sensor_id, &registry),
            vec![(NodeControlKind::Motion, source_room_id.clone(), true)]
        );

        assert!(store.set_control_target(
            &sensor_id,
            NodeControlKind::Motion,
            Some(&target_room_id),
        ));
        assert_eq!(
            store.effective_node_controls(&sensor_id, &registry),
            vec![(NodeControlKind::Motion, target_room_id.clone(), false)]
        );
    }

    // ---- cross-hub triage detection tests ----

    #[test]
    fn sync_hub_room_detects_name_match_after_translate_or_create() {
        let mut store = RoomTopologyStore::new();

        // HA creates "Office" room first (via translate_or_create in do_room_set)
        let ha_office_id = store.translate_or_create(&ha_key(), "office", "Office", "office", &[]);
        // Simulate Phase 4c completing for HA: add canonical devices
        store.attach_device_hub_default(&ha_office_id, "canonical-ha-1");

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
        store.attach_device_hub_default(&ha_office_id, "canonical-ha-1");

        // Hue creates "Office" room and fully syncs (has devices)
        let hue_office_id =
            store.translate_or_create(&hue_key(), "hue-office-uuid", "Office", "gl-office", &[]);
        // Simulate a completed Phase 4c for the Hue room (adds devices)
        store.attach_device_hub_default(&hue_office_id, "canonical-hue-1");

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
        store.attach_device_hub_default(&ha_office_id, "canonical-ha-1");

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
