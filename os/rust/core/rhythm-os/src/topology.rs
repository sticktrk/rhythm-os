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
use rhythm_core::{runtime::hub_registry::DeviceType, ButtonAction, HubDispatchTarget, RhythmMode};
use serde::{Deserialize, Serialize};

use crate::canonical::identity::HubKey;

/// How a device was assigned to a room.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlacement {
    /// Auto-assigned from hub room membership, updated on re-sync.
    HubDefault,
    /// User explicitly placed this device here — survives re-sync. Carries
    /// over even when `parent_id` is `None`, which represents the user
    /// explicitly removing the device from any room.
    UserOverride,
    /// Device exists first-class without a parent room. Reached via
    /// auto-detach (hub stopped reporting the device in its previous room),
    /// not by an explicit user choice — re-attaches if the hub reports the
    /// device in a room again.
    Standalone,
}

/// Resolve the effective placement for a `(parent_id, requested placement)`
/// pair. `UserOverride` is the only placement that survives `parent_id =
/// None` — every other placement collapses to `Standalone` when the device
/// has no parent.
fn effective_placement_for(
    parent_id: Option<&str>,
    placement: &DevicePlacement,
) -> DevicePlacement {
    match (parent_id.is_some(), placement) {
        (true, p) => p.clone(),
        (false, DevicePlacement::UserOverride) => DevicePlacement::UserOverride,
        (false, _) => DevicePlacement::Standalone,
    }
}

/// Resolve the one endpoint that may control a room-bound light. A controller
/// that requires grouped-room control outranks the user's ordinary preferred
/// endpoint while the light is attached to a room. Multiple such controllers
/// are ambiguous and must fail closed.
fn room_bound_light_control_endpoint<'a>(
    device: &'a crate::canonical::identity::CanonicalDevice,
    grouped_room_control_required: &HashSet<String>,
) -> Option<&'a crate::canonical::identity::IntegrationEndpoint> {
    let mut authoritative = device
        .active_endpoints()
        .filter(|endpoint| grouped_room_control_required.contains(&endpoint.hub_key.to_string()))
        .collect::<Vec<_>>();
    authoritative.sort_by(|left, right| {
        left.hub_key
            .to_string()
            .cmp(&right.hub_key.to_string())
            .then_with(|| left.native_id.cmp(&right.native_id))
    });
    match authoritative.as_slice() {
        [endpoint] => Some(*endpoint),
        [] => device.preferred_endpoint(),
        _ => None,
    }
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
            DeviceType::Contact => Some(Self::Switch),
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

/// Built-in input binding preset for cycling between day and sleep modes.
pub const DAY_SLEEP_TOGGLE_INPUT_BINDING_PRESET: &str = "day_sleep_toggle";

/// Server-known preset that expands to a normal trigger/action binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputBindingPreset {
    DaySleepToggle,
}

impl InputBindingPreset {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DaySleepToggle => DAY_SLEEP_TOGGLE_INPUT_BINDING_PRESET,
        }
    }
}

/// Source event kind that can trigger a persisted input binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputTriggerKind {
    Button,
}

/// Event filter for a physical input binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputBindingTrigger {
    pub kind: InputTriggerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button_action: Option<ButtonAction>,
}

impl InputBindingTrigger {
    pub fn button(button_action: Option<ButtonAction>) -> Self {
        Self {
            kind: InputTriggerKind::Button,
            button_action,
        }
    }

    pub fn matches_button(&self, action: ButtonAction) -> bool {
        self.kind == InputTriggerKind::Button
            && self
                .button_action
                .map(|expected| expected == action)
                .unwrap_or(true)
    }
}

/// How a mode-changing action should pick its fade/transition policy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModeTransitionSelection {
    #[default]
    Auto,
    None,
    Exact {
        id: String,
    },
}

/// Reusable high-level action for physical inputs and future automation triggers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationAction {
    ModeCycle {
        modes: Vec<RhythmMode>,
        #[serde(default)]
        transition: ModeTransitionSelection,
    },
    ModeSet {
        mode: RhythmMode,
        #[serde(default)]
        transition: ModeTransitionSelection,
    },
    /// Move one reusable light schedule without changing the global mode.
    LightScheduleModeCycle {
        schedule_id: String,
        modes: Vec<RhythmMode>,
        #[serde(default)]
        transition: ModeTransitionSelection,
    },
    /// Set one reusable light schedule without changing the global mode.
    LightScheduleModeSet {
        schedule_id: String,
        mode: RhythmMode,
        #[serde(default)]
        transition: ModeTransitionSelection,
    },
    /// Backward-compatible two-mode alias; new presets emit `mode_cycle`.
    ModeToggle {
        first_mode: RhythmMode,
        second_mode: RhythmMode,
        #[serde(default)]
        transition: ModeTransitionSelection,
    },
}

/// Persisted binding from one source input node to a high-level action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputBinding {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<InputBindingPreset>,
    pub source_node_id: String,
    pub trigger: InputBindingTrigger,
    pub action: AutomationAction,
    #[serde(default = "default_input_binding_enabled")]
    pub enabled: bool,
}

fn default_input_binding_enabled() -> bool {
    true
}

impl InputBinding {
    pub fn from_preset(
        id: impl Into<String>,
        preset: InputBindingPreset,
        source_node_id: impl Into<String>,
        button_action: Option<ButtonAction>,
    ) -> Self {
        let action = match preset {
            InputBindingPreset::DaySleepToggle => AutomationAction::ModeCycle {
                modes: vec![RhythmMode::Day, RhythmMode::Sleep],
                transition: ModeTransitionSelection::Auto,
            },
        };
        Self {
            id: id.into(),
            preset: Some(preset),
            source_node_id: source_node_id.into(),
            trigger: InputBindingTrigger::button(button_action),
            action,
            enabled: true,
        }
    }

    pub fn preset_binding_id(
        preset: InputBindingPreset,
        source_node_id: &str,
        button_action: Option<ButtonAction>,
    ) -> String {
        let source = input_binding_id_part(source_node_id);
        let action = button_action.map(button_action_id_part).unwrap_or("any");
        format!("{}:{}:{}", preset.as_str(), source, action)
    }

    pub fn day_sleep_toggle(
        source_node_id: impl Into<String>,
        button_action: Option<ButtonAction>,
    ) -> Self {
        let source_node_id = source_node_id.into();
        let id = Self::preset_binding_id(
            InputBindingPreset::DaySleepToggle,
            &source_node_id,
            button_action,
        );
        Self::from_preset(
            id,
            InputBindingPreset::DaySleepToggle,
            source_node_id,
            button_action,
        )
    }

    pub fn matches_button(&self, source_node_id: &str, action: ButtonAction) -> bool {
        self.enabled && self.source_node_id == source_node_id && self.trigger.matches_button(action)
    }
}

fn input_binding_id_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn button_action_id_part(action: ButtonAction) -> &'static str {
    match action {
        ButtonAction::OnPress => "on_press",
        ButtonAction::Toggle => "toggle",
        ButtonAction::OffPress => "off_press",
        ButtonAction::Reset => "reset",
        ButtonAction::ResetToModeDefault => "reset_to_mode_default",
        ButtonAction::UpPress => "up_press",
        ButtonAction::DownPress => "down_press",
        ButtonAction::UpHold => "up_hold",
        ButtonAction::DownHold => "down_hold",
        ButtonAction::Stop => "stop",
        ButtonAction::RhythmOn => "rhythm_on",
        ButtonAction::RhythmOff => "rhythm_off",
        ButtonAction::LightsOff => "lights_off",
        ButtonAction::SleepOn => "sleep_on",
        ButtonAction::SleepOff => "sleep_off",
    }
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Settings node ID whose effective state this target renders.
    ///
    /// Group routes inherit the owning room. Direct device routes use the
    /// device node so per-light profile overrides survive room fan-out.
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
    transition_aliases: Vec<(String, HubKey, HubDispatchTarget)>,
}

const INTERNAL_LIGHT_NODE_PREFIX: &str = "__rhythm_light_node__";

pub fn is_internal_light_node_id(node_id: &str) -> bool {
    node_id.starts_with(INTERNAL_LIGHT_NODE_PREFIX)
}

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

    fn control_light_endpoints_by_hub(
        &self,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
        grouped_room_control_required: &HashSet<String>,
    ) -> HashMap<HubKey, HashMap<String, String>> {
        let mut by_hub: HashMap<HubKey, HashMap<String, String>> = HashMap::new();
        for room_device in &self.devices {
            let Some(device) = canonical_registry.get(&room_device.device_id) else {
                continue;
            };
            if device.device_type != DeviceType::Light {
                continue;
            }
            let Some(endpoint) =
                room_bound_light_control_endpoint(device, grouped_room_control_required)
            else {
                continue;
            };
            if endpoint.native_id.is_empty() {
                continue;
            }
            by_hub
                .entry(endpoint.hub_key.clone())
                .or_default()
                .insert(endpoint.native_id.clone(), room_device.device_id.clone());
        }
        by_hub
    }

    fn exact_native_grouped_dispatch_target_for_hub(
        &self,
        hub_key: &HubKey,
        assigned_native_ids: &HashSet<String>,
    ) -> Option<HubDispatchTarget> {
        if assigned_native_ids.is_empty() {
            return None;
        }
        let binding = self
            .hub_room_bindings
            .iter()
            .filter(|binding| {
                binding.hub_key == *hub_key
                    && !binding.control_id.is_empty()
                    && binding
                        .light_device_ids
                        .iter()
                        .cloned()
                        .collect::<HashSet<_>>()
                        == *assigned_native_ids
            })
            .min_by(|left, right| left.hub_room_id.cmp(&right.hub_room_id))?;
        Some(HubDispatchTarget::Group {
            room_id: binding.hub_room_id.clone(),
            control_id: binding.control_id.clone(),
        })
    }

    fn authoritative_grouped_dispatch_target_for_hub(
        &self,
        hub_key: &HubKey,
        assigned_native_ids: &HashSet<String>,
        rhythm_managed_bindings: &[RhythmManagedHubRoomBinding],
    ) -> Option<HubDispatchTarget> {
        let mut bindings = self
            .hub_room_bindings
            .iter()
            .filter(|binding| binding.hub_key == *hub_key);
        let binding = bindings.next()?;
        if bindings.next().is_some()
            || binding.control_id.is_empty()
            || assigned_native_ids.is_empty()
            || binding
                .light_device_ids
                .iter()
                .cloned()
                .collect::<HashSet<_>>()
                != *assigned_native_ids
            || !rhythm_managed_bindings.iter().any(|managed| {
                managed.rhythm_room_id == self.id
                    && managed.hub_key == *hub_key
                    && managed.hub_room_id == binding.hub_room_id
            })
        {
            return None;
        }

        Some(HubDispatchTarget::Group {
            room_id: binding.hub_room_id.clone(),
            control_id: binding.control_id.clone(),
        })
    }

    fn dispatch_plan(
        &self,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
        grouped_room_control_required: &HashSet<String>,
        rhythm_managed_bindings: &[RhythmManagedHubRoomBinding],
        grouped_dispatch_suspended_hubs: &HashSet<HubKey>,
    ) -> DispatchPlan {
        let mut plan = DispatchPlan::default();
        let preferred_light_endpoints =
            self.control_light_endpoints_by_hub(canonical_registry, grouped_room_control_required);

        if preferred_light_endpoints.is_empty() {
            return plan;
        }

        let mut assigned_by_hub: Vec<_> = preferred_light_endpoints.into_iter().collect();
        assigned_by_hub.sort_by(|left, right| left.0.to_string().cmp(&right.0.to_string()));

        for (hub_key, remaining_ids) in assigned_by_hub {
            let grouped_control_required =
                grouped_room_control_required.contains(&hub_key.to_string());
            if grouped_control_required {
                let assigned_native_ids = remaining_ids.keys().cloned().collect::<HashSet<_>>();
                if let Some(target) = self.authoritative_grouped_dispatch_target_for_hub(
                    &hub_key,
                    &assigned_native_ids,
                    rhythm_managed_bindings,
                ) {
                    plan.room_targets.push((hub_key.clone(), target.clone()));
                    let HubDispatchTarget::Group {
                        room_id: hub_room_id,
                        ..
                    } = &target
                    else {
                        unreachable!("authoritative grouped target must be a group")
                    };
                    plan.node_routes.push(TopologyLightNodeRoute {
                        node: TopologyLightNode {
                            id: group_light_node_id(&self.id, &hub_key, hub_room_id),
                            source_node_id: self.id.clone(),
                            emit_node_id: self.id.clone(),
                        },
                        hub_key,
                        target,
                    });
                }
                continue;
            }

            let assigned_native_ids = remaining_ids.keys().cloned().collect::<HashSet<_>>();
            let exact_grouped_target =
                self.exact_native_grouped_dispatch_target_for_hub(&hub_key, &assigned_native_ids);
            let grouped_dispatch_suspended = grouped_dispatch_suspended_hubs.contains(&hub_key);
            if let Some(target) = exact_grouped_target
                .as_ref()
                .filter(|_| !grouped_dispatch_suspended)
                .cloned()
            {
                let HubDispatchTarget::Group {
                    room_id: hub_room_id,
                    ..
                } = &target
                else {
                    unreachable!("exact native grouped target must be a group")
                };
                plan.room_targets.push((hub_key.clone(), target.clone()));
                plan.node_routes.push(TopologyLightNodeRoute {
                    node: TopologyLightNode {
                        id: group_light_node_id(&self.id, &hub_key, hub_room_id),
                        source_node_id: self.id.clone(),
                        emit_node_id: self.id.clone(),
                    },
                    hub_key,
                    target,
                });
                continue;
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
                if grouped_dispatch_suspended {
                    if let Some(HubDispatchTarget::Group {
                        room_id: hub_room_id,
                        ..
                    }) = exact_grouped_target
                    {
                        // A periodic tick may already be queued under the
                        // synthetic group-node identity when Hue room sync
                        // raises the individual-device fallback fence. Keep
                        // that old identity routable to the safe direct target
                        // until the scheduler naturally replaces it with the
                        // per-device nodes below. This alias is deliberately
                        // omitted from periodic_light_nodes so it cannot
                        // schedule duplicate work or dispatch to the fenced
                        // grouped-light target.
                        plan.transition_aliases.push((
                            group_light_node_id(&self.id, &hub_key, &hub_room_id),
                            hub_key.clone(),
                            HubDispatchTarget::Devices {
                                native_ids: native_ids.clone(),
                            },
                        ));
                    }
                }
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
    /// Whether later source renames may replace the established canonical
    /// Rhythm room name. The source name always bootstraps a new room.
    pub source_name_authoritative: bool,
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

/// Explicit persisted ownership marker for a hub room created or adopted by
/// Rhythm. Kept separate from `HubRoomBinding` so older integrations can keep
/// constructing discovered bindings without claiming ownership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RhythmManagedHubRoomBinding {
    rhythm_room_id: String,
    hub_key: HubKey,
    hub_room_id: String,
}

/// The controller a user explicitly chose for unattended behavior in a room
/// backed by an external automation platform such as Hue.
///
/// Absence of a decision is intentionally different from either variant. An
/// upgraded server must not infer consent from a previously connected bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalRoomAutomationOwner {
    External,
    Rhythm,
}

/// Persisted, address-scoped room consent. Keeping this beside the topology
/// makes room merge/removal and hub-address migration part of the same durable
/// authority transaction as the source-room binding it governs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ExternalRoomAutomationDecision {
    rhythm_room_id: String,
    hub_key: HubKey,
    owner: ExternalRoomAutomationOwner,
}

/// Summary of topology repairs applied while loading older persisted state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopologyMigrationReport {
    pub filtered_light_device_ids: usize,
    pub moved_light_devices: usize,
    pub grandfathered_hue_rooms: usize,
    pub external_automation_policy_version_advanced: bool,
}

impl TopologyMigrationReport {
    pub fn changed(&self) -> bool {
        self.filtered_light_device_ids > 0
            || self.moved_light_devices > 0
            || self.grandfathered_hue_rooms > 0
            || self.external_automation_policy_version_advanced
    }
}

/// The room topology store — Rhythm's authoritative room registry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomTopologyStore {
    /// All Rhythm rooms, keyed by Rhythm room UUID.
    rooms: HashMap<String, TopologyRoom>,
    /// First-class device nodes keyed by canonical/topology device ID.
    #[serde(default)]
    device_nodes: HashMap<String, TopologyDeviceNode>,
    /// Explicit node-to-node control overrides.
    #[serde(default)]
    control_links: Vec<NodeControlLink>,
    /// Persisted physical input bindings to high-level actions.
    #[serde(default)]
    input_bindings: Vec<InputBinding>,
    /// Approved cross-hub room bindings (survives re-sync).
    #[serde(default)]
    approved_bindings: Vec<RoomBindingRecord>,
    /// Hub-native rooms that Rhythm may authoritatively update/delete.
    /// Ownership is explicit and is never inferred from IDs or names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rhythm_managed_bindings: Vec<RhythmManagedHubRoomBinding>,
    /// Explicit per-room automation ownership decisions. Missing means
    /// unreviewed and therefore external-controller owned (fail closed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    external_room_automation_decisions: Vec<ExternalRoomAutomationDecision>,
    /// Hue hubs configured before explicit authority review was introduced.
    /// Their existing behavior remains approved even when room bindings are
    /// discovered after startup. Disconnecting the hub clears this marker.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    external_room_automation_grandfathered_hubs: HashSet<HubKey>,
    /// Version 0 predates explicit Hue authority review. Loading that version
    /// grants existing Hue-backed rooms the behavior they had before review
    /// was introduced, then advances to version 1. New stores begin at 1, so
    /// hubs added after the migration remain unreviewed.
    #[serde(default)]
    external_room_automation_policy_version: u8,
    /// Hue bridges whose light membership should mirror Rhythm's canonical
    /// room assignments. This is deliberately independent from automation
    /// suppression consent and defaults off for mixed-version safety.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    external_room_topology_sync_hubs: HashSet<HubKey>,
    /// Bridges whose grouped-light readback is not currently authoritative.
    /// While fenced, routing fans out to individual devices until a complete
    /// reconciliation publishes exact room membership.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    external_grouped_dispatch_suspended_hubs: HashSet<HubKey>,
    /// Bridges whose last topology projection failed or found ambiguous
    /// native state. This is a privacy-bounded status marker; detailed errors
    /// remain in server logs while the app can explain the safe fallback.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    external_room_topology_sync_attention_hubs: HashSet<HubKey>,
    /// Index: (hub_key display, hub_room_id) → Rhythm room ID.
    #[serde(skip)]
    hub_room_index: HashMap<(String, String), String>,
    /// Connected hub instances whose integration requires attached lights to
    /// use a native grouped-room target. Rehydrated from integration metadata
    /// at runtime rather than persisted as user topology.
    #[serde(skip)]
    grouped_room_control_required: HashSet<String>,
}

impl Default for RoomTopologyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RoomTopologyStore {
    pub fn new() -> Self {
        Self {
            rooms: HashMap::new(),
            device_nodes: HashMap::new(),
            control_links: Vec::new(),
            input_bindings: Vec::new(),
            approved_bindings: Vec::new(),
            rhythm_managed_bindings: Vec::new(),
            external_room_automation_decisions: Vec::new(),
            external_room_automation_grandfathered_hubs: HashSet::new(),
            external_room_automation_policy_version: 1,
            external_room_topology_sync_hubs: HashSet::new(),
            external_grouped_dispatch_suspended_hubs: HashSet::new(),
            external_room_topology_sync_attention_hubs: HashSet::new(),
            hub_room_index: HashMap::new(),
            grouped_room_control_required: HashSet::new(),
        }
    }

    /// Construct an empty topology that came from a pre-policy persistence
    /// source. Fresh installations must use [`Self::new`]; legacy loaders use
    /// this value so configured Hue bridges still cross the one-time authority
    /// migration even when the older installation had no topology half.
    pub fn legacy_empty() -> Self {
        let mut store = Self::new();
        store.external_room_automation_policy_version = 0;
        store
    }

    /// Migrate pre-consent topology exactly once. Explicit choices are never
    /// overwritten; only rooms that previously relied on Rhythm's implicit
    /// Hue authority are grandfathered.
    pub fn migrate_legacy_external_room_automation_policy(
        &mut self,
        configured_hub_keys: &[HubKey],
    ) -> TopologyMigrationReport {
        let mut report = TopologyMigrationReport::default();
        if self.external_room_automation_policy_version >= 1 {
            return report;
        }

        self.external_room_automation_grandfathered_hubs.extend(
            configured_hub_keys
                .iter()
                .filter(|key| key.hub_type.as_str() == crate::hub::HubType::HUE)
                .cloned(),
        );
        self.external_room_automation_grandfathered_hubs.extend(
            self.rooms
                .values()
                .flat_map(|room| room.hub_room_bindings.iter())
                .filter(|binding| binding.hub_key.hub_type.as_str() == crate::hub::HubType::HUE)
                .map(|binding| binding.hub_key.clone()),
        );
        report.grandfathered_hue_rooms = self
            .rooms
            .values()
            .filter(|room| {
                room.hub_room_bindings.iter().any(|binding| {
                    self.external_room_automation_grandfathered_hubs
                        .contains(&binding.hub_key)
                        && self
                            .external_room_automation_decisions
                            .iter()
                            .all(|decision| {
                                decision.rhythm_room_id != room.id
                                    || decision.hub_key != binding.hub_key
                            })
                })
            })
            .count();
        self.external_room_automation_policy_version = 1;
        report.external_automation_policy_version_advanced = true;
        report
    }

    /// Declare whether attached lights on this hub may fall back to direct
    /// device routing when their grouped-room binding is missing.
    pub fn set_grouped_room_control_required(&mut self, hub_key: &HubKey, required: bool) {
        let key = hub_key.to_string();
        if required {
            self.grouped_room_control_required.insert(key);
        } else {
            self.grouped_room_control_required.remove(&key);
        }
    }

    pub fn grouped_room_control_is_required(&self, hub_key: &HubKey) -> bool {
        self.grouped_room_control_required
            .contains(&hub_key.to_string())
    }

    /// Enable or disable canonical Rhythm room membership projection for one
    /// external hub. Enabling immediately fences grouped dispatch until the
    /// integration completes an exact read-back reconciliation.
    pub fn set_external_room_topology_sync_enabled(
        &mut self,
        hub_key: &HubKey,
        enabled: bool,
    ) -> bool {
        if enabled {
            let enabled_changed = self
                .external_room_topology_sync_hubs
                .insert(hub_key.clone());
            let fence_changed = self
                .external_grouped_dispatch_suspended_hubs
                .insert(hub_key.clone());
            let attention_changed = self
                .external_room_topology_sync_attention_hubs
                .remove(hub_key);
            enabled_changed || fence_changed || attention_changed
        } else {
            let enabled_changed = self.external_room_topology_sync_hubs.remove(hub_key);
            // Never lower a grouped-dispatch fence merely because the user
            // disabled future reconciliation. A failed/partial bridge write
            // may have made the last local binding stale; only exact readback
            // or hub removal may prove it safe again.
            let attention_changed = self
                .external_room_topology_sync_attention_hubs
                .remove(hub_key);
            enabled_changed || attention_changed
        }
    }

    pub fn external_room_topology_sync_is_enabled(&self, hub_key: &HubKey) -> bool {
        self.external_room_topology_sync_hubs.contains(hub_key)
    }

    pub fn set_external_grouped_dispatch_suspended(
        &mut self,
        hub_key: &HubKey,
        suspended: bool,
    ) -> bool {
        if suspended {
            self.external_grouped_dispatch_suspended_hubs
                .insert(hub_key.clone())
        } else {
            self.external_grouped_dispatch_suspended_hubs
                .remove(hub_key)
        }
    }

    pub fn external_grouped_dispatch_is_suspended(&self, hub_key: &HubKey) -> bool {
        self.external_grouped_dispatch_suspended_hubs
            .contains(hub_key)
    }

    pub fn set_external_room_topology_sync_attention(
        &mut self,
        hub_key: &HubKey,
        attention: bool,
    ) -> bool {
        if attention {
            self.external_room_topology_sync_attention_hubs
                .insert(hub_key.clone())
        } else {
            self.external_room_topology_sync_attention_hubs
                .remove(hub_key)
        }
    }

    pub fn external_room_topology_sync_needs_attention(&self, hub_key: &HubKey) -> bool {
        self.external_room_topology_sync_attention_hubs
            .contains(hub_key)
    }

    /// Return whether the topology graph or live grouped-routing policy names
    /// this address-scoped hub key. A migration-only grandfather marker is not
    /// structural identity: pending Hue address migration may legitimately
    /// carry the marker at both its old and new address until remap commits.
    pub fn structurally_references_hub_key(&self, hub_key: &HubKey) -> bool {
        self.rooms.values().any(|room| {
            room.hub_room_bindings
                .iter()
                .any(|binding| binding.hub_key == *hub_key)
        }) || self
            .approved_bindings
            .iter()
            .any(|binding| binding.hub_key == *hub_key)
            || self
                .rhythm_managed_bindings
                .iter()
                .any(|binding| binding.hub_key == *hub_key)
            || self
                .external_room_automation_decisions
                .iter()
                .any(|decision| decision.hub_key == *hub_key)
            || self.external_room_topology_sync_hubs.contains(hub_key)
            || self
                .external_grouped_dispatch_suspended_hubs
                .contains(hub_key)
            || self
                .external_room_topology_sync_attention_hubs
                .contains(hub_key)
            || self.grouped_room_control_is_required(hub_key)
    }

    /// Return whether persisted topology, a migration policy marker, or the
    /// live grouping policy still names this address-scoped hub key.
    pub fn references_hub_key(&self, hub_key: &HubKey) -> bool {
        self.structurally_references_hub_key(hub_key)
            || self
                .external_room_automation_grandfathered_hubs
                .contains(hub_key)
    }

    /// Return every structured hub key retained by room topology.
    pub fn referenced_hub_keys(&self) -> HashSet<HubKey> {
        let mut keys = HashSet::new();
        for room in self.rooms.values() {
            keys.extend(
                room.hub_room_bindings
                    .iter()
                    .map(|binding| binding.hub_key.clone()),
            );
        }
        keys.extend(
            self.approved_bindings
                .iter()
                .map(|binding| binding.hub_key.clone()),
        );
        keys.extend(
            self.external_room_automation_decisions
                .iter()
                .map(|decision| decision.hub_key.clone()),
        );
        keys.extend(
            self.external_room_automation_grandfathered_hubs
                .iter()
                .cloned(),
        );
        keys.extend(
            self.rhythm_managed_bindings
                .iter()
                .map(|binding| binding.hub_key.clone()),
        );
        keys.extend(self.external_room_topology_sync_hubs.iter().cloned());
        keys.extend(
            self.external_grouped_dispatch_suspended_hubs
                .iter()
                .cloned(),
        );
        keys.extend(
            self.external_room_topology_sync_attention_hubs
                .iter()
                .cloned(),
        );
        keys
    }

    /// Rewrite an address-scoped hub key after an integration has proven both
    /// addresses identify the same physical controller.
    ///
    /// Callers must reject a topology that already references both keys before
    /// invoking this method. The rebuild refreshes every derived index and
    /// preserves explicit Rhythm-managed room ownership.
    pub fn remap_hub_key(&mut self, old_key: &HubKey, new_key: &HubKey) -> bool {
        if old_key == new_key {
            return false;
        }
        let mut changed = false;
        for room in self.rooms.values_mut() {
            for binding in &mut room.hub_room_bindings {
                if binding.hub_key == *old_key {
                    binding.hub_key = new_key.clone();
                    changed = true;
                }
            }
        }
        for binding in &mut self.approved_bindings {
            if binding.hub_key == *old_key {
                binding.hub_key = new_key.clone();
                changed = true;
            }
        }
        for binding in &mut self.rhythm_managed_bindings {
            if binding.hub_key == *old_key {
                binding.hub_key = new_key.clone();
                changed = true;
            }
        }
        for decision in &mut self.external_room_automation_decisions {
            if decision.hub_key == *old_key {
                decision.hub_key = new_key.clone();
                changed = true;
            }
        }
        if self
            .external_room_automation_grandfathered_hubs
            .remove(old_key)
        {
            self.external_room_automation_grandfathered_hubs
                .insert(new_key.clone());
            changed = true;
        }
        if self.external_room_topology_sync_hubs.remove(old_key) {
            self.external_room_topology_sync_hubs
                .insert(new_key.clone());
            changed = true;
        }
        if self
            .external_grouped_dispatch_suspended_hubs
            .remove(old_key)
        {
            self.external_grouped_dispatch_suspended_hubs
                .insert(new_key.clone());
            changed = true;
        }
        if self
            .external_room_topology_sync_attention_hubs
            .remove(old_key)
        {
            self.external_room_topology_sync_attention_hubs
                .insert(new_key.clone());
            changed = true;
        }
        let old_policy = self
            .grouped_room_control_required
            .remove(&old_key.to_string());
        if old_policy {
            self.grouped_room_control_required
                .insert(new_key.to_string());
            changed = true;
        }
        if changed {
            self.rebuild_indices();
        }
        changed
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
        self.rhythm_managed_bindings.retain(|managed| {
            self.rooms.get(&managed.rhythm_room_id).is_some_and(|room| {
                room.hub_room_bindings.iter().any(|binding| {
                    binding.hub_key == managed.hub_key && binding.hub_room_id == managed.hub_room_id
                })
            })
        });
        self.rhythm_managed_bindings.sort_by(|left, right| {
            left.rhythm_room_id
                .cmp(&right.rhythm_room_id)
                .then_with(|| left.hub_key.to_string().cmp(&right.hub_key.to_string()))
                .then_with(|| left.hub_room_id.cmp(&right.hub_room_id))
        });
        self.rhythm_managed_bindings.dedup();
        self.external_room_automation_decisions.retain(|decision| {
            self.rooms
                .get(&decision.rhythm_room_id)
                .is_some_and(|room| {
                    room.hub_room_bindings
                        .iter()
                        .any(|binding| binding.hub_key == decision.hub_key)
                })
        });
        let mut consolidated_decisions =
            HashMap::<(HubKey, String), ExternalRoomAutomationOwner>::new();
        for decision in self.external_room_automation_decisions.drain(..) {
            consolidated_decisions
                .entry((decision.hub_key, decision.rhythm_room_id))
                .and_modify(|owner| {
                    // A room merge can collapse previously different choices.
                    // External ownership wins that ambiguity; merge must never
                    // manufacture destructive consent.
                    if decision.owner == ExternalRoomAutomationOwner::External {
                        *owner = ExternalRoomAutomationOwner::External;
                    }
                })
                .or_insert(decision.owner);
        }
        self.external_room_automation_decisions = consolidated_decisions
            .into_iter()
            .map(
                |((hub_key, rhythm_room_id), owner)| ExternalRoomAutomationDecision {
                    rhythm_room_id,
                    hub_key,
                    owner,
                },
            )
            .collect();
        self.external_room_automation_decisions
            .sort_by(|left, right| {
                left.hub_key
                    .to_string()
                    .cmp(&right.hub_key.to_string())
                    .then_with(|| left.rhythm_room_id.cmp(&right.rhythm_room_id))
            });
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

    /// Repair persisted state from versions that copied every hub room child
    /// into `light_device_ids`.
    ///
    /// A `UserOverride` is durable user intent, even when the integration's
    /// source-room binding points somewhere else. Legacy state cannot be
    /// distinguished safely from a legitimate move once it carries that
    /// marker, so this migration may only realign `HubDefault` devices.
    pub fn migrate_legacy_light_room_bindings(
        &mut self,
        canonical_registry: &mut crate::canonical::registry::CanonicalRegistry,
    ) -> TopologyMigrationReport {
        let mut report = TopologyMigrationReport::default();
        let mut light_native_ids_by_hub: HashMap<String, HashSet<String>> = HashMap::new();

        for device in canonical_registry.devices() {
            if device.device_type != DeviceType::Light {
                continue;
            }
            for endpoint in device.active_endpoints() {
                light_native_ids_by_hub
                    .entry(endpoint.hub_key.to_string())
                    .or_default()
                    .insert(endpoint.native_id.clone());
            }
        }

        if light_native_ids_by_hub.is_empty() {
            return report;
        }

        for room in self.rooms.values_mut() {
            for binding in &mut room.hub_room_bindings {
                let hub_key = binding.hub_key.to_string();
                let Some(light_native_ids) = light_native_ids_by_hub.get(&hub_key) else {
                    continue;
                };

                let before = binding.light_device_ids.len();
                binding
                    .light_device_ids
                    .retain(|native_id| light_native_ids.contains(native_id));
                binding.light_device_ids.sort();
                binding.light_device_ids.dedup();
                report.filtered_light_device_ids +=
                    before.saturating_sub(binding.light_device_ids.len());
            }
        }

        let mut binding_targets: HashMap<(String, String), Option<String>> = HashMap::new();
        for (room_id, room) in &self.rooms {
            for binding in &room.hub_room_bindings {
                let hub_key = binding.hub_key.to_string();
                for native_id in &binding.light_device_ids {
                    let key = (hub_key.clone(), native_id.clone());
                    binding_targets
                        .entry(key)
                        .and_modify(|existing| {
                            if existing.as_deref() != Some(room_id.as_str()) {
                                *existing = None;
                            }
                        })
                        .or_insert_with(|| Some(room_id.clone()));
                }
            }
        }

        let binding_targets: HashMap<(String, String), String> = binding_targets
            .into_iter()
            .filter_map(|(key, room_id)| room_id.map(|room_id| (key, room_id)))
            .collect();

        let mut repairs = Vec::new();
        let mut node_ids: Vec<_> = self.device_nodes.keys().cloned().collect();
        node_ids.sort();

        for node_id in node_ids {
            let Some(node) = self.device_nodes.get(&node_id) else {
                continue;
            };
            if node.placement != DevicePlacement::HubDefault {
                continue;
            }
            let Some(current_parent_id) = node.parent_id.as_deref() else {
                continue;
            };
            let Some(device) = canonical_registry.get(&node.canonical_device_id) else {
                continue;
            };
            if device.device_type != DeviceType::Light || device.is_removed() {
                continue;
            }

            let mut target_room_ids = Vec::new();
            for endpoint in device.active_endpoints() {
                let key = (endpoint.hub_key.to_string(), endpoint.native_id.clone());
                if let Some(target_room_id) = binding_targets.get(&key) {
                    target_room_ids.push(target_room_id.clone());
                }
            }
            target_room_ids.sort();
            target_room_ids.dedup();

            let [target_room_id] = target_room_ids.as_slice() else {
                continue;
            };
            if target_room_id == current_parent_id {
                continue;
            }

            repairs.push((node.canonical_device_id.clone(), target_room_id.clone()));
        }

        for (canonical_device_id, target_room_id) in repairs {
            if let Some(node) = self.device_nodes.get_mut(&canonical_device_id) {
                node.parent_id = Some(target_room_id.clone());
                node.placement = DevicePlacement::HubDefault;
                report.moved_light_devices += 1;
            }
            canonical_registry.assign_room(&canonical_device_id, Some(&target_room_id));
        }

        if report.moved_light_devices > 0 {
            self.rebuild_room_device_projections();
        }

        report
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
            if !self.rooms.contains_key(room_id) {
                return false;
            }
        }

        let existing_parent_id = match self.device_nodes.get(canonical_device_id) {
            Some(node) => node.parent_id.clone(),
            None if create_if_missing => {
                self.upsert_device_node(
                    canonical_device_id,
                    parent_id.map(str::to_string),
                    effective_placement_for(parent_id, &placement),
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

        let effective_placement = effective_placement_for(parent_id, &placement);
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
            Some(node) if node.placement == DevicePlacement::UserOverride => {}
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
        hub_key: &HubKey,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
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
            let Some(device) = canonical_registry.get(&node.canonical_device_id) else {
                continue;
            };
            let (has_current_hub_endpoint, has_other_hub_endpoint) = device
                .active_endpoints()
                .fold((false, false), |(current, other), endpoint| {
                    if &endpoint.hub_key == hub_key {
                        (true, other)
                    } else {
                        (current, true)
                    }
                });
            if !has_current_hub_endpoint || has_other_hub_endpoint {
                continue;
            }
            node.parent_id = None;
            node.placement = DevicePlacement::Standalone;
        }
    }

    pub fn sync_hub_room(
        &mut self,
        hub_key: &HubKey,
        discovered: &DiscoveredTopologyRoom,
    ) -> SyncAction {
        let empty_registry = crate::canonical::registry::CanonicalRegistry::new();
        self.sync_hub_room_with_registry(hub_key, discovered, &empty_registry)
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
    pub fn sync_hub_room_with_registry(
        &mut self,
        hub_key: &HubKey,
        discovered: &DiscoveredTopologyRoom,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
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
                            self.detach_hub_default_devices_for_room(
                                &target_id,
                                hub_key,
                                canonical_registry,
                                &keep_device_ids,
                            );
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
                    self.detach_hub_default_devices_for_room(
                        &rhythm_room_id,
                        hub_key,
                        canonical_registry,
                        &keep_device_ids,
                    );
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

                if !room.user_customized && discovered.source_name_authoritative {
                    room.name = discovered.name.clone();
                }
            }
            let keep_device_ids: HashSet<String> =
                discovered.canonical_device_ids.iter().cloned().collect();
            self.detach_hub_default_devices_for_room(
                &rhythm_room_id,
                hub_key,
                canonical_registry,
                &keep_device_ids,
            );
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
                    self.detach_hub_default_devices_for_room(
                        &target_id,
                        hub_key,
                        canonical_registry,
                        &keep_device_ids,
                    );
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
        self.rhythm_managed_bindings.retain(|managed| {
            managed.hub_key != *hub_key || current_set_owned.contains(&managed.hub_room_id)
        });
        self.external_room_automation_decisions.retain(|decision| {
            decision.hub_key != *hub_key
                || self
                    .rooms
                    .get(&decision.rhythm_room_id)
                    .is_some_and(|room| {
                        room.hub_room_bindings
                            .iter()
                            .any(|binding| binding.hub_key == *hub_key)
                    })
        });

        affected
    }

    /// Insert or replace a source room binding on an existing Rhythm room.
    pub fn upsert_room_binding(&mut self, rhythm_room_id: &str, binding: HubRoomBinding) -> bool {
        let Some(room) = self.rooms.get_mut(rhythm_room_id) else {
            return false;
        };

        let changed =
            room.binding_for_hub_room(&binding.hub_key, &binding.hub_room_id) != Some(&binding);
        if changed {
            room.upsert_hub_room_binding(binding.clone());
        }

        self.hub_room_index.insert(
            (binding.hub_key.to_string(), binding.hub_room_id.clone()),
            rhythm_room_id.to_string(),
        );
        changed
    }

    /// Insert the one explicitly Rhythm-managed grouped binding for a room
    /// and hub. This is the restart-reconciliation API for authoritative
    /// integrations; ordinary discovery must use [`Self::upsert_room_binding`]
    /// and never gains ownership implicitly.
    pub fn upsert_managed_room_binding(
        &mut self,
        rhythm_room_id: &str,
        mut binding: HubRoomBinding,
    ) -> bool {
        if binding.hub_room_id.is_empty()
            || binding.control_id.is_empty()
            || !self.rooms.contains_key(rhythm_room_id)
        {
            return false;
        }

        binding.light_device_ids.sort();
        binding.light_device_ids.dedup();
        let hub_key = binding.hub_key.clone();
        let hub_room_id = binding.hub_room_id.clone();

        for (room_id, room) in &mut self.rooms {
            room.hub_room_bindings.retain(|existing| {
                if room_id == rhythm_room_id {
                    existing.hub_key != hub_key
                } else {
                    !(existing.hub_key == hub_key && existing.hub_room_id == hub_room_id)
                }
            });
        }
        let Some(room) = self.rooms.get_mut(rhythm_room_id) else {
            return false;
        };
        room.upsert_hub_room_binding(binding);

        self.rhythm_managed_bindings.retain(|managed| {
            !(managed.hub_key == hub_key
                && (managed.rhythm_room_id == rhythm_room_id || managed.hub_room_id == hub_room_id))
        });
        self.rhythm_managed_bindings
            .push(RhythmManagedHubRoomBinding {
                rhythm_room_id: rhythm_room_id.to_string(),
                hub_key,
                hub_room_id,
            });
        self.rebuild_indices();
        true
    }

    /// Return whether this exact room binding carries explicit Rhythm
    /// ownership metadata.
    pub fn room_binding_is_managed(
        &self,
        rhythm_room_id: &str,
        hub_key: &HubKey,
        hub_room_id: &str,
    ) -> bool {
        self.rhythm_managed_bindings.iter().any(|managed| {
            managed.rhythm_room_id == rhythm_room_id
                && managed.hub_key == *hub_key
                && managed.hub_room_id == hub_room_id
        })
    }

    /// Resolve the exact, explicitly owned grouped binding used by an
    /// authoritative controller for one Rhythm room. The returned membership
    /// is validated against the controller endpoints Rhythm actually routes,
    /// so scene projection and ordinary lighting share the same fail-closed
    /// authority proof.
    pub fn exact_managed_group_binding(
        &self,
        rhythm_room_id: &str,
        hub_key: &HubKey,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> Option<HubRoomBinding> {
        if !self.grouped_room_control_is_required(hub_key) {
            return None;
        }
        let room = self.rooms.get(rhythm_room_id)?;
        let assigned_native_ids = room
            .control_light_endpoints_by_hub(canonical_registry, &self.grouped_room_control_required)
            .remove(hub_key)?
            .into_keys()
            .collect::<HashSet<_>>();
        let target = room.authoritative_grouped_dispatch_target_for_hub(
            hub_key,
            &assigned_native_ids,
            &self.rhythm_managed_bindings,
        )?;
        let HubDispatchTarget::Group {
            room_id,
            control_id,
        } = target
        else {
            return None;
        };
        room.hub_room_bindings
            .iter()
            .find(|binding| {
                binding.hub_key == *hub_key
                    && binding.hub_room_id == room_id
                    && binding.control_id == control_id
            })
            .cloned()
    }

    /// Remove source room bindings for a hub that match an integration-specific predicate.
    pub fn remove_room_bindings_for_hub_where<F>(
        &mut self,
        hub_key: &HubKey,
        predicate: F,
    ) -> Vec<String>
    where
        F: Fn(&HubRoomBinding) -> bool,
    {
        let mut affected = Vec::new();

        for (room_id, room) in &mut self.rooms {
            let before = room.hub_room_bindings.len();
            room.hub_room_bindings
                .retain(|binding| binding.hub_key != *hub_key || !predicate(binding));
            if room.hub_room_bindings.len() < before {
                affected.push(room_id.clone());
            }
        }

        if !affected.is_empty() {
            self.rebuild_indices();
        }

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
        self.rhythm_managed_bindings
            .retain(|managed| managed.hub_key != *hub_key || managed.hub_room_id != hub_room_id);

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

    /// Return the effective automation owner for one room/hub pair. Explicit
    /// choices take precedence over the one-time legacy-hub approval. `None`
    /// means a newly configured hub has not been reviewed, which callers must
    /// treat as external-controller owned.
    pub fn external_room_automation_owner(
        &self,
        rhythm_room_id: &str,
        hub_key: &HubKey,
    ) -> Option<ExternalRoomAutomationOwner> {
        self.external_room_automation_decisions
            .iter()
            .find(|decision| {
                decision.rhythm_room_id == rhythm_room_id && decision.hub_key == *hub_key
            })
            .map(|decision| decision.owner)
            .or_else(|| {
                self.external_room_automation_grandfathered_hubs
                    .contains(hub_key)
                    .then_some(ExternalRoomAutomationOwner::Rhythm)
            })
    }

    /// Finish the one-time legacy bridge bootstrap after complete room
    /// discovery. Current undecided rooms receive durable explicit Rhythm
    /// ownership, while every prior explicit choice is preserved. Removing
    /// the bridge marker makes rooms discovered in later epochs unreviewed.
    pub fn materialize_grandfathered_external_room_automation_decisions(
        &mut self,
        hub_key: &HubKey,
    ) -> bool {
        if !self
            .external_room_automation_grandfathered_hubs
            .contains(hub_key)
        {
            return false;
        }

        let explicitly_decided = self
            .external_room_automation_decisions
            .iter()
            .filter(|decision| decision.hub_key == *hub_key)
            .map(|decision| decision.rhythm_room_id.clone())
            .collect::<HashSet<_>>();
        let mut undecided_room_ids = self
            .rooms
            .values()
            .filter(|room| {
                room.hub_room_bindings
                    .iter()
                    .any(|binding| binding.hub_key == *hub_key)
                    && !explicitly_decided.contains(&room.id)
            })
            .map(|room| room.id.clone())
            .collect::<Vec<_>>();
        undecided_room_ids.sort();

        self.external_room_automation_decisions
            .extend(undecided_room_ids.into_iter().map(|rhythm_room_id| {
                ExternalRoomAutomationDecision {
                    rhythm_room_id,
                    hub_key: hub_key.clone(),
                    owner: ExternalRoomAutomationOwner::Rhythm,
                }
            }));
        self.external_room_automation_decisions
            .sort_by(|left, right| {
                left.hub_key
                    .to_string()
                    .cmp(&right.hub_key.to_string())
                    .then_with(|| left.rhythm_room_id.cmp(&right.rhythm_room_id))
            });
        self.external_room_automation_grandfathered_hubs
            .remove(hub_key);
        true
    }

    /// Forget all retained authority for a hub that the user disconnected.
    /// Pairing the same address again is a new hub review journey.
    pub fn forget_external_room_automation_policy_for_hub(&mut self, hub_key: &HubKey) -> bool {
        let before = self.external_room_automation_decisions.len();
        self.external_room_automation_decisions
            .retain(|decision| decision.hub_key != *hub_key);
        let grandfathered = self
            .external_room_automation_grandfathered_hubs
            .remove(hub_key);
        let topology_sync = self.external_room_topology_sync_hubs.remove(hub_key);
        let grouped_fence = self
            .external_grouped_dispatch_suspended_hubs
            .remove(hub_key);
        let topology_attention = self
            .external_room_topology_sync_attention_hubs
            .remove(hub_key);
        grandfathered
            || topology_sync
            || grouped_fence
            || topology_attention
            || self.external_room_automation_decisions.len() != before
    }

    /// Snapshot every Rhythm room backed by a specific external hub. Results
    /// are stable-sorted for API revisions and deterministic review screens.
    pub fn external_automation_rooms_for_hub(
        &self,
        hub_key: &HubKey,
    ) -> Vec<(String, String, Option<ExternalRoomAutomationOwner>)> {
        let mut rooms = self
            .rooms
            .values()
            .filter(|room| {
                room.hub_room_bindings
                    .iter()
                    .any(|binding| binding.hub_key == *hub_key)
            })
            .map(|room| {
                (
                    room.id.clone(),
                    room.name.clone(),
                    self.external_room_automation_owner(&room.id, hub_key),
                )
            })
            .collect::<Vec<_>>();
        rooms.sort_by(|left, right| left.0.cmp(&right.0));
        rooms
    }

    /// Opaque optimistic-concurrency revision for one hub's review surface.
    /// The hash deliberately contains no native Hue identifiers or secrets.
    pub fn external_automation_revision(&self, hub_key: &HubKey) -> String {
        // Stable FNV-1a is sufficient here: this is a stale-write guard, not a
        // security boundary. The full exact room set is validated on write.
        let mut hash = 0xcbf29ce484222325_u64;
        let mut absorb = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        };
        absorb(hub_key.to_string().as_bytes());
        for (room_id, name, owner) in self.external_automation_rooms_for_hub(hub_key) {
            absorb(room_id.as_bytes());
            absorb(&[0]);
            absorb(name.as_bytes());
            absorb(&[match owner {
                None => 0,
                Some(ExternalRoomAutomationOwner::External) => 1,
                Some(ExternalRoomAutomationOwner::Rhythm) => 2,
            }]);
        }
        format!("{hash:016x}")
    }

    /// Atomically replace the complete room decision set for one hub.
    /// Partial or duplicate submissions are rejected so a concurrent room
    /// discovery cannot silently inherit a destructive bridge-wide choice.
    pub fn replace_external_room_automation_decisions(
        &mut self,
        hub_key: &HubKey,
        decisions: &[(String, ExternalRoomAutomationOwner)],
    ) -> Result<bool, String> {
        let expected = self
            .external_automation_rooms_for_hub(hub_key)
            .into_iter()
            .map(|(room_id, _, _)| room_id)
            .collect::<HashSet<_>>();
        if expected.is_empty() {
            return Err("No rooms are bound to this hub".to_string());
        }
        let requested = decisions
            .iter()
            .map(|(room_id, _)| room_id.clone())
            .collect::<HashSet<_>>();
        if requested.len() != decisions.len() || requested != expected {
            return Err("The submitted room set is stale or incomplete".to_string());
        }

        let before = self
            .external_room_automation_decisions
            .iter()
            .filter(|decision| decision.hub_key == *hub_key)
            .cloned()
            .collect::<Vec<_>>();
        self.external_room_automation_decisions
            .retain(|decision| decision.hub_key != *hub_key);
        self.external_room_automation_decisions
            .extend(
                decisions
                    .iter()
                    .map(|(room_id, owner)| ExternalRoomAutomationDecision {
                        rhythm_room_id: room_id.clone(),
                        hub_key: hub_key.clone(),
                        owner: *owner,
                    }),
            );
        self.external_room_automation_decisions
            .sort_by(|left, right| left.rhythm_room_id.cmp(&right.rhythm_room_id));

        // A complete explicit review supersedes the migration-only bridge
        // fallback. Future rooms must be reviewed instead of inheriting the
        // pre-policy approval.
        let grandfathering_cleared = self
            .external_room_automation_grandfathered_hubs
            .remove(hub_key);

        let after = self
            .external_room_automation_decisions
            .iter()
            .filter(|decision| decision.hub_key == *hub_key)
            .cloned()
            .collect::<Vec<_>>();
        Ok(before != after || grandfathering_cleared)
    }

    /// Bridge-wide Hue automation suppression is permitted only after every
    /// currently bound room made an explicit Rhythm choice.
    pub fn external_hub_has_full_rhythm_consent(&self, hub_key: &HubKey) -> bool {
        let rooms = self.external_automation_rooms_for_hub(hub_key);
        !rooms.is_empty()
            && rooms
                .iter()
                .all(|(_, _, owner)| *owner == Some(ExternalRoomAutomationOwner::Rhythm))
    }

    /// Whether at least one room on an external controller explicitly chose
    /// Rhythm automation. Unlike full consent, this is sufficient for a
    /// controller that can reconcile a selective, room-bounded suppression
    /// scope while preserving every externally owned room.
    pub fn external_hub_has_rhythm_consent(&self, hub_key: &HubKey) -> bool {
        self.external_automation_rooms_for_hub(hub_key)
            .iter()
            .any(|(_, _, owner)| *owner == Some(ExternalRoomAutomationOwner::Rhythm))
    }

    /// Whether unattended Rhythm behavior may target this public topology
    /// node. Hue-backed rooms default to false until explicitly reviewed.
    ///
    /// Hue behavior suppression is reconciled independently from this policy.
    /// Cross-room or unsupported Hue behavior may intentionally coexist with
    /// Rhythm; the persisted room decision is the admission boundary.
    pub fn rhythm_automation_allowed_for_node(&self, node_id: &str) -> bool {
        let room_id = if self.rooms.contains_key(node_id) {
            Some(node_id)
        } else {
            self.device_parent_room_id(node_id)
        };
        let Some(room_id) = room_id else {
            return true;
        };
        let Some(room) = self.rooms.get(room_id) else {
            return true;
        };
        room.hub_room_bindings
            .iter()
            .filter(|binding| binding.hub_key.hub_type.as_str() == crate::hub::HubType::HUE)
            .all(|binding| {
                self.external_room_automation_owner(room_id, &binding.hub_key)
                    == Some(ExternalRoomAutomationOwner::Rhythm)
            })
    }

    /// Get all persisted explicit control links.
    pub fn control_links(&self) -> &[NodeControlLink] {
        &self.control_links
    }

    /// Get all persisted physical input bindings.
    pub fn input_bindings(&self) -> &[InputBinding] {
        &self.input_bindings
    }

    /// Find a persisted physical input binding by ID.
    pub fn input_binding(&self, id: &str) -> Option<&InputBinding> {
        self.input_bindings.iter().find(|binding| binding.id == id)
    }

    /// Find the first enabled input binding matching a button event.
    pub fn matching_button_input_binding(
        &self,
        source_node_id: &str,
        action: ButtonAction,
    ) -> Option<&InputBinding> {
        self.input_bindings
            .iter()
            .find(|binding| binding.matches_button(source_node_id, action))
    }

    /// Add or replace a persisted physical input binding.
    pub fn set_input_binding(&mut self, binding: InputBinding) -> bool {
        if let Some(existing) = self
            .input_bindings
            .iter_mut()
            .find(|existing| existing.id == binding.id)
        {
            let changed = *existing != binding;
            *existing = binding;
            changed
        } else {
            self.input_bindings.push(binding);
            true
        }
    }

    /// Remove a persisted physical input binding.
    pub fn remove_input_binding(&mut self, id: &str) -> bool {
        let before = self.input_bindings.len();
        self.input_bindings.retain(|binding| binding.id != id);
        self.input_bindings.len() != before
    }

    /// Remove every persisted automation whose source is a device/node that
    /// is being omitted or deleted.
    pub fn remove_input_bindings_for_source(&mut self, source_node_id: &str) -> usize {
        let before = self.input_bindings.len();
        self.input_bindings
            .retain(|binding| binding.source_node_id != source_node_id);
        before - self.input_bindings.len()
    }

    /// Find an explicit control target for a source node and control kind.
    pub fn explicit_control_target(&self, source_id: &str, kind: &NodeControlKind) -> Option<&str> {
        self.control_links
            .iter()
            .find(|link| link.source_id == source_id && &link.kind == kind)
            .map(|link| link.target_id.as_str())
    }

    /// Find all explicit control targets for a source node and control kind.
    pub fn explicit_control_targets(&self, source_id: &str, kind: &NodeControlKind) -> Vec<&str> {
        self.control_links
            .iter()
            .filter(|link| link.source_id == source_id && &link.kind == kind)
            .map(|link| link.target_id.as_str())
            .collect()
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

    /// Resolve every effective target for a source node and control kind.
    ///
    /// Explicit topology links win. If no override exists, device nodes inherit
    /// their parent room as the single default control target.
    pub fn effective_control_targets(
        &self,
        source_id: &str,
        kind: &NodeControlKind,
    ) -> Vec<String> {
        let explicit_targets = self.explicit_control_targets(source_id, kind);
        if !explicit_targets.is_empty() {
            let mut targets: Vec<String> =
                explicit_targets.into_iter().map(str::to_string).collect();
            targets.sort();
            targets.dedup();
            return targets;
        }

        self.device_nodes
            .get(source_id)
            .and_then(|node| node.parent_id.clone())
            .into_iter()
            .collect()
    }

    /// Set or clear an explicit control target override.
    pub fn set_control_target(
        &mut self,
        source_id: &str,
        kind: NodeControlKind,
        target_id: Option<&str>,
    ) -> bool {
        let targets: Vec<&str> = target_id.into_iter().collect();
        self.set_control_targets(source_id, kind, &targets)
    }

    /// Set or clear explicit control target overrides.
    pub fn set_control_targets(
        &mut self,
        source_id: &str,
        kind: NodeControlKind,
        target_ids: &[&str],
    ) -> bool {
        if !self.has_public_node(source_id) {
            return false;
        }
        for target_id in target_ids {
            if !self.has_public_node(target_id) {
                return false;
            }
        }

        self.control_links
            .retain(|link| !(link.source_id == source_id && link.kind == kind));

        let mut target_ids = target_ids.to_vec();
        target_ids.sort();
        target_ids.dedup();

        for target_id in target_ids {
            self.control_links.push(NodeControlLink {
                source_id: source_id.to_string(),
                target_id: target_id.to_string(),
                kind: kind.clone(),
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

    /// Resolve any room-like identifier to the owning topology room ID.
    ///
    /// Accepts:
    /// - topology room IDs
    /// - topology device node IDs
    /// - bound hub-native room IDs when the caller supplies a hub key
    pub fn resolve_room_alias(&self, hub_key: Option<&HubKey>, room_id: &str) -> Option<String> {
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
        None
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
            for target_id in self.effective_control_targets(source_id, &kind) {
                controls.push((kind.clone(), target_id, !explicit));
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

            for target_id in self.effective_control_targets(&node.id, kind) {
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

    /// Mirror a confirmed hub-side light move into the persisted source-room
    /// bindings immediately, without waiting for the next discovery sync.
    pub fn reassign_hub_light_device(
        &mut self,
        hub_key: &HubKey,
        native_device_id: &str,
        target_rhythm_room_id: Option<&str>,
        target_hub_room_id: Option<&str>,
    ) -> bool {
        let target = match (target_rhythm_room_id, target_hub_room_id) {
            (Some(room_id), Some(hub_room_id)) => {
                let valid = self.rooms.get(room_id).is_some_and(|room| {
                    room.hub_room_bindings.iter().any(|binding| {
                        binding.hub_key == *hub_key && binding.hub_room_id == hub_room_id
                    })
                });
                if !valid {
                    return false;
                }
                Some((room_id.to_string(), Some(hub_room_id.to_string())))
            }
            (Some(room_id), None) => {
                let valid = self.rooms.get(room_id).is_some_and(|room| {
                    room.hub_room_bindings
                        .iter()
                        .all(|binding| binding.hub_key != *hub_key)
                });
                if !valid {
                    return false;
                }
                Some((room_id.to_string(), None))
            }
            (None, None) => None,
            _ => return false,
        };

        for room in self.rooms.values_mut() {
            for binding in &mut room.hub_room_bindings {
                if binding.hub_key != *hub_key {
                    continue;
                }
                binding
                    .light_device_ids
                    .retain(|device_id| device_id != native_device_id);
            }
        }

        if let Some((room_id, Some(hub_room_id))) = target {
            let Some(binding) = self.rooms.get_mut(&room_id).and_then(|room| {
                room.hub_room_bindings.iter_mut().find(|binding| {
                    binding.hub_key == *hub_key && binding.hub_room_id == hub_room_id
                })
            }) else {
                return false;
            };
            if !binding
                .light_device_ids
                .iter()
                .any(|device_id| device_id == native_device_id)
            {
                binding.light_device_ids.push(native_device_id.to_string());
                binding.light_device_ids.sort();
                binding.light_device_ids.dedup();
            }
        }

        true
    }

    /// Mirror a confirmed authoritative native-room assignment, including a
    /// room that the integration created during the prepare phase.
    ///
    /// The returned binding is the complete membership receipt for one
    /// Rhythm room and hub. It replaces any prior binding for that hub in the
    /// target room and removes those exact members from other rooms on the
    /// same hub, keeping grouped dispatch unambiguous.
    pub fn apply_authoritative_hub_light_binding(
        &mut self,
        hub_key: &HubKey,
        native_device_id: &str,
        target_rhythm_room_id: Option<&str>,
        target_binding: Option<HubRoomBinding>,
        managed_by_rhythm: bool,
    ) -> bool {
        if !managed_by_rhythm {
            return false;
        }
        let mut target_binding = match (target_rhythm_room_id, target_binding) {
            (Some(room_id), Some(binding))
                if self.rooms.contains_key(room_id)
                    && binding.hub_key == *hub_key
                    && !binding.hub_room_id.is_empty()
                    && !binding.control_id.is_empty()
                    && binding
                        .light_device_ids
                        .iter()
                        .any(|device_id| device_id == native_device_id) =>
            {
                Some((room_id.to_string(), binding))
            }
            (None, None) => None,
            _ => return false,
        };

        let exact_target_members: HashSet<String> = target_binding
            .as_ref()
            .map(|(_, binding)| binding.light_device_ids.iter().cloned().collect())
            .unwrap_or_else(|| HashSet::from([native_device_id.to_string()]));

        for room in self.rooms.values_mut() {
            for binding in &mut room.hub_room_bindings {
                if binding.hub_key == *hub_key {
                    binding
                        .light_device_ids
                        .retain(|device_id| !exact_target_members.contains(device_id));
                }
            }
        }

        if let Some((room_id, binding)) = target_binding.take() {
            return self.upsert_managed_room_binding(&room_id, binding);
        }

        self.rebuild_indices();
        true
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

        for managed in &mut self.rhythm_managed_bindings {
            if managed.rhythm_room_id == source_id {
                managed.rhythm_room_id = target_id.to_string();
            }
        }
        for decision in &mut self.external_room_automation_decisions {
            if decision.rhythm_room_id == source_id {
                decision.rhythm_room_id = target_id.to_string();
            }
        }

        self.rebuild_indices();
        true
    }

    /// Remove a room and detach its devices into standalone nodes.
    pub fn remove_room(&mut self, room_id: &str) -> Option<Vec<String>> {
        let room = self.rooms.remove(room_id)?;

        for binding in &room.hub_room_bindings {
            self.hub_room_index
                .remove(&(binding.hub_key.to_string(), binding.hub_room_id.clone()));
        }

        self.approved_bindings
            .retain(|binding| binding.rhythm_room_id != room_id);
        self.rhythm_managed_bindings
            .retain(|binding| binding.rhythm_room_id != room_id);
        self.external_room_automation_decisions
            .retain(|decision| decision.rhythm_room_id != room_id);

        let mut detached_device_ids = Vec::new();
        for node in self.device_nodes.values_mut() {
            if node.parent_id.as_deref() != Some(room_id) {
                continue;
            }
            node.parent_id = None;
            node.placement = DevicePlacement::Standalone;
            detached_device_ids.push(node.canonical_device_id.clone());
        }

        detached_device_ids.sort();
        self.rebuild_room_device_projections();
        Some(detached_device_ids)
    }

    /// Move a device from one room to another.
    pub fn move_device(&mut self, device_id: &str, from_room: &str, to_room: &str) -> bool {
        if !self.rooms.contains_key(from_room) {
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
        self.translate_or_create_with_status(
            hub_key,
            hub_room_id,
            room_name,
            control_id,
            light_device_ids,
        )
        .0
    }

    /// Like [`translate_or_create`], but reports whether a new Rhythm room was
    /// created.
    pub fn translate_or_create_with_status(
        &mut self,
        hub_key: &HubKey,
        hub_room_id: &str,
        room_name: &str,
        control_id: &str,
        light_device_ids: &[String],
    ) -> (String, bool) {
        let index_key = (hub_key.to_string(), hub_room_id.to_string());

        // Rule 1: Already mapped — return existing topology room ID.
        if let Some(rhythm_room_id) = self.hub_room_index.get(&index_key) {
            return (rhythm_room_id.clone(), false);
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
                return (target_id, false);
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
        (rhythm_room_id, true)
    }

    fn light_device_endpoint_route(
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

    fn device_dispatch_route(
        &self,
        node: &TopologyDeviceNode,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> Option<(HubKey, HubDispatchTarget)> {
        self.light_device_endpoint_route(node, canonical_registry)
    }

    fn attached_light_dispatch_route(
        &self,
        node: &TopologyDeviceNode,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> Option<(HubKey, HubDispatchTarget)> {
        let parent_id = node.parent_id.as_deref()?;
        let device = canonical_registry.get(&node.canonical_device_id)?;
        if device.device_type != DeviceType::Light {
            return None;
        }
        let endpoint =
            room_bound_light_control_endpoint(device, &self.grouped_room_control_required)?;
        if endpoint.native_id.is_empty() {
            return None;
        }
        let hub_key = endpoint.hub_key.clone();
        let device_target = HubDispatchTarget::Devices {
            native_ids: vec![endpoint.native_id.clone()],
        };
        let room = self.rooms.get(parent_id)?;
        let assigned_native_ids = room
            .control_light_endpoints_by_hub(canonical_registry, &self.grouped_room_control_required)
            .remove(&hub_key)
            .unwrap_or_default()
            .into_keys()
            .collect::<HashSet<_>>();
        let grouped_target = if self.grouped_room_control_is_required(&hub_key) {
            room.authoritative_grouped_dispatch_target_for_hub(
                &hub_key,
                &assigned_native_ids,
                &self.rhythm_managed_bindings,
            )
        } else if self.external_grouped_dispatch_is_suspended(&hub_key) {
            None
        } else {
            room.exact_native_grouped_dispatch_target_for_hub(&hub_key, &assigned_native_ids)
        };
        let target = match grouped_target {
            Some(group_target) => group_target,
            None if self.grouped_room_control_is_required(&hub_key) => return None,
            None => device_target,
        };
        Some((hub_key, target))
    }

    pub fn attached_light_uses_parent_dispatch(
        &self,
        node_id: &str,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> bool {
        matches!(
            self.device_nodes
                .get(node_id)
                .and_then(|node| self.attached_light_dispatch_route(node, canonical_registry)),
            Some((_, HubDispatchTarget::Group { .. }))
        )
    }

    /// Return true when this light node's normal topology route targets the
    /// underlying device instead of collapsing to a hub-native group.
    pub fn light_node_uses_device_dispatch(
        &self,
        node_id: &str,
        canonical_registry: &crate::canonical::registry::CanonicalRegistry,
    ) -> bool {
        let Some(node) = self.device_nodes.get(node_id) else {
            return false;
        };
        let route = if node.parent_id.is_some() {
            self.attached_light_dispatch_route(node, canonical_registry)
        } else {
            self.device_dispatch_route(node, canonical_registry)
        };
        matches!(route, Some((_, HubDispatchTarget::Devices { .. })))
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
            let plan = room.dispatch_plan(
                canonical_registry,
                &self.grouped_room_control_required,
                &self.rhythm_managed_bindings,
                &self.external_grouped_dispatch_suspended_hubs,
            );
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
            for (node_id, hub_key, target) in plan.transition_aliases {
                table.insert(node_id, vec![(hub_key.to_string(), target)]);
            }
        }
        let mut attached_light_node_ids: Vec<_> = self
            .device_nodes
            .values()
            .filter(|node| node.parent_id.is_some())
            .map(|node| node.id.clone())
            .collect();
        attached_light_node_ids.sort();
        for node_id in attached_light_node_ids {
            let node = self.device_nodes.get(&node_id).unwrap();
            let Some(device) = canonical_registry.get(&node.canonical_device_id) else {
                continue;
            };
            if device.device_type != DeviceType::Light {
                continue;
            }
            if let Some((hub_key, target)) =
                self.attached_light_dispatch_route(node, canonical_registry)
            {
                table.insert(node.id.clone(), vec![(hub_key.to_string(), target)]);
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
                .dispatch_plan(
                    canonical_registry,
                    &self.grouped_room_control_required,
                    &self.rhythm_managed_bindings,
                    &self.external_grouped_dispatch_suspended_hubs,
                )
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

    fn matter_key() -> HubKey {
        HubKey::new(HubType::new("matter"), "local")
    }

    fn make_discovered(hub_room_id: &str, name: &str, control_id: &str) -> DiscoveredTopologyRoom {
        DiscoveredTopologyRoom {
            hub_room_id: hub_room_id.to_string(),
            name: name.to_string(),
            source_name_authoritative: true,
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
            room_id: Some(room_id.to_string()),
            room_name: Some(room_name.to_string()),
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
    fn non_authoritative_source_name_cannot_replace_a_cross_hub_canonical_name() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.translate_or_create(&ha_key(), "kitchen", "Kitchen", "kitchen", &[]);
        store.approve_binding(hue_key(), "hue-room-1".to_string(), room_id.clone(), 1000);
        assert_eq!(
            store.translate_or_create(&hue_key(), "hue-room-1", "Rhythm · Kitchen", "gl-1", &[],),
            room_id
        );

        let mut ha_rename = make_discovered("kitchen", "Galley", "kitchen");
        ha_rename.source_name_authoritative = true;
        store.sync_hub_room(&ha_key(), &ha_rename);

        let mut hue_rediscovery = make_discovered("hue-room-1", "Rhythm · Kitchen", "gl-2");
        hue_rediscovery.source_name_authoritative = false;
        hue_rediscovery.light_device_ids = vec!["hue-light-1".to_string()];
        store.sync_hub_room(&hue_key(), &hue_rediscovery);

        let room = store.get(&room_id).unwrap();
        assert_eq!(room.name, "Galley");
        let hue_binding = room.binding_for_hub(&hue_key()).unwrap();
        assert_eq!(hue_binding.control_id, "gl-2");
        assert_eq!(hue_binding.light_device_ids, vec!["hue-light-1"]);
    }

    #[test]
    fn legacy_light_binding_migration_repairs_only_hub_default_drift() {
        let mut store = RoomTopologyStore::new();
        let mut registry = CanonicalRegistry::new();
        let light_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-light-1",
                "hue-room-1",
                "Kitchen",
                "Lamp",
                DeviceType::Light,
            ),
        );
        let mut discovered = make_discovered("hue-room-1", "Kitchen", "gl-1");
        discovered.light_device_ids = vec!["hue-light-1".to_string()];
        discovered.canonical_device_ids = vec![light_id.clone()];
        let source_room_id = store
            .sync_hub_room_with_registry(&hue_key(), &discovered, &registry)
            .rhythm_room_id()
            .to_string();
        registry.assign_room(&light_id, Some(&source_room_id));

        let drifted_room_id = store.create_room("Drifted room");
        assert!(store.assign_device(
            &light_id,
            Some(&drifted_room_id),
            DevicePlacement::HubDefault,
        ));
        registry.assign_room(&light_id, Some(&drifted_room_id));

        let report = store.migrate_legacy_light_room_bindings(&mut registry);

        assert_eq!(report.moved_light_devices, 1);
        let repaired = store.get_device_node(&light_id).unwrap();
        assert_eq!(repaired.parent_id.as_deref(), Some(source_room_id.as_str()));
        assert_eq!(repaired.placement, DevicePlacement::HubDefault);
        assert_eq!(
            registry.get(&light_id).unwrap().room_id.as_deref(),
            Some(source_room_id.as_str())
        );
    }

    #[test]
    fn sync_preserves_user_override_devices() {
        let mut store = RoomTopologyStore::new();
        let mut registry = CanonicalRegistry::new();
        let dev1 = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-dev-1",
                "hue-room-1",
                "Kitchen",
                "Lamp 1",
                DeviceType::Light,
            ),
        );
        let dev2 = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-dev-2",
                "hue-room-1",
                "Kitchen",
                "Lamp 2",
                DeviceType::Light,
            ),
        );
        let dev4 = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-dev-4",
                "hue-room-1",
                "Kitchen",
                "Lamp 4",
                DeviceType::Light,
            ),
        );
        let mut discovered = make_discovered("hue-room-1", "Kitchen", "gl-1");
        discovered.canonical_device_ids = vec![dev1.clone(), dev2.clone()];
        let action = store.sync_hub_room_with_registry(&hue_key(), &discovered, &registry);
        let room_id = action.rhythm_room_id().to_string();

        // User moves dev-3 here
        assert!(store.attach_device_user_override(&room_id, "dev-3"));

        // Re-sync removes dev-2, adds dev-4
        let mut rediscovered = make_discovered("hue-room-1", "Kitchen", "gl-1");
        rediscovered.canonical_device_ids = vec![dev1.clone(), dev4.clone()];
        store.sync_hub_room_with_registry(&hue_key(), &rediscovered, &registry);

        let room = store.get(&room_id).unwrap();
        let device_ids: Vec<&str> = room.devices.iter().map(|d| d.device_id.as_str()).collect();
        assert!(device_ids.contains(&dev1.as_str())); // kept
        assert!(!device_ids.contains(&dev2.as_str())); // removed (HubDefault, not in new list)
        assert!(device_ids.contains(&"dev-3")); // kept (UserOverride)
        assert!(device_ids.contains(&dev4.as_str())); // added
    }

    #[test]
    fn sync_preserves_device_moved_to_different_room() {
        // Issue #43: motion sensor moved out of its hub-default room kept
        // reverting to the hub room on the next sync.
        let mut store = RoomTopologyStore::new();
        let mut registry = CanonicalRegistry::new();

        // Hue reports a motion sensor in "Balcony"
        let sensor_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-motion-1",
                "balcony-hue-id",
                "Balcony",
                "Master",
                DeviceType::Motion,
            ),
        );

        // Initial sync: motion sensor lands in Balcony as HubDefault
        let mut discovered = make_discovered("balcony-hue-id", "Balcony", "balcony-gl");
        discovered.canonical_device_ids = vec![sensor_id.clone()];
        let action = store.sync_hub_room_with_registry(&hue_key(), &discovered, &registry);
        let balcony_id = action.rhythm_room_id().to_string();

        let initial = store.get_device_node(&sensor_id).unwrap();
        assert_eq!(initial.parent_id.as_deref(), Some(balcony_id.as_str()));
        assert_eq!(initial.placement, DevicePlacement::HubDefault);

        // User creates a separate Rhythm room and moves the sensor into it.
        let master_room_id = store.create_room("Master Room");
        assert!(store.move_device(&sensor_id, &balcony_id, &master_room_id));
        let after_move = store.get_device_node(&sensor_id).unwrap();
        assert_eq!(
            after_move.parent_id.as_deref(),
            Some(master_room_id.as_str())
        );
        assert_eq!(after_move.placement, DevicePlacement::UserOverride);

        // Hue still reports the sensor in Balcony (the user only moved it in
        // Rhythm). A re-sync of Balcony must not pull the device back.
        let mut rediscovered = make_discovered("balcony-hue-id", "Balcony", "balcony-gl");
        rediscovered.canonical_device_ids = vec![sensor_id.clone()];
        store.sync_hub_room_with_registry(&hue_key(), &rediscovered, &registry);

        let after_resync = store.get_device_node(&sensor_id).unwrap();
        assert_eq!(
            after_resync.parent_id.as_deref(),
            Some(master_room_id.as_str()),
            "sensor must stay in the user's Rhythm room"
        );
        assert_eq!(after_resync.placement, DevicePlacement::UserOverride);
    }

    #[test]
    fn sync_preserves_user_move_when_origin_room_becomes_empty() {
        // Variant of #43 covering the edge case where the device the user
        // moved out was the room's *only* canonical device. After the move
        // the source room's projection is empty, which historically tripped
        // the freshly_created branch of sync_hub_room_with_registry.
        let mut store = RoomTopologyStore::new();
        let mut registry = CanonicalRegistry::new();

        let sensor_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-motion-only",
                "balcony-hue-id",
                "Balcony",
                "Master",
                DeviceType::Motion,
            ),
        );

        let mut discovered = make_discovered("balcony-hue-id", "Balcony", "balcony-gl");
        discovered.canonical_device_ids = vec![sensor_id.clone()];
        let action = store.sync_hub_room_with_registry(&hue_key(), &discovered, &registry);
        let balcony_id = action.rhythm_room_id().to_string();

        let master_room_id = store.create_room("Master Room");
        assert!(store.move_device(&sensor_id, &balcony_id, &master_room_id));
        assert!(store.get(&balcony_id).unwrap().devices.is_empty());

        let mut rediscovered = make_discovered("balcony-hue-id", "Balcony", "balcony-gl");
        rediscovered.canonical_device_ids = vec![sensor_id.clone()];
        store.sync_hub_room_with_registry(&hue_key(), &rediscovered, &registry);

        let after_resync = store.get_device_node(&sensor_id).unwrap();
        assert_eq!(
            after_resync.parent_id.as_deref(),
            Some(master_room_id.as_str())
        );
        assert_eq!(after_resync.placement, DevicePlacement::UserOverride);
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
    fn remove_room_detaches_devices_and_cleans_links() {
        let mut store = RoomTopologyStore::new();
        let discovered = make_discovered("ha-room-1", "Office", "ha-control-1");
        let room_id = store
            .sync_hub_room(&ha_key(), &discovered)
            .rhythm_room_id()
            .to_string();

        assert!(store.attach_device_user_override(&room_id, "dev-1"));
        assert!(store.set_control_target("dev-1", NodeControlKind::Motion, Some(&room_id)));
        store.approve_binding(hue_key(), "hue-room-1".to_string(), room_id.clone(), 42);
        assert!(store.upsert_managed_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: ha_key(),
                hub_room_id: "ha-room-1".to_string(),
                control_id: "ha-control-1".to_string(),
                light_device_ids: Vec::new(),
            },
        ));

        let detached = store.remove_room(&room_id).unwrap();

        assert_eq!(detached, vec!["dev-1".to_string()]);
        assert!(store.get(&room_id).is_none());
        assert!(store.translate_room_id(&ha_key(), "ha-room-1").is_none());
        assert!(store.approved_bindings.is_empty());
        assert!(store.rhythm_managed_bindings.is_empty());
        assert!(store.control_links().is_empty());

        let node = store.get_device_node("dev-1").unwrap();
        assert_eq!(node.parent_id, None);
        assert_eq!(node.placement, DevicePlacement::Standalone);
    }

    #[test]
    fn serialization_roundtrip() {
        let mut store = RoomTopologyStore::new();
        let hue = make_discovered("hue-room-1", "Kitchen", "gl-1");
        let room_id = store
            .sync_hub_room(&hue_key(), &hue)
            .rhythm_room_id()
            .to_string();
        assert!(store.upsert_managed_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: hue_key(),
                hub_room_id: "hue-room-1".to_string(),
                control_id: "gl-1".to_string(),
                light_device_ids: Vec::new(),
            },
        ));

        let json = serde_json::to_string(&store).unwrap();
        assert!(json.contains("rhythm_managed_bindings"));
        let mut restored: RoomTopologyStore = serde_json::from_str(&json).unwrap();
        restored.rebuild_indices();

        assert_eq!(restored.room_count(), 1);
        assert!(restored
            .find_by_hub_room(&hue_key(), "hue-room-1")
            .is_some());
        assert!(restored.room_binding_is_managed(&room_id, &hue_key(), "hue-room-1"));
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
            source_name_authoritative: true,
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
    fn native_binding_without_assigned_lights_is_inert() {
        let mut store = RoomTopologyStore::new();
        let light_ids = vec!["light-1".to_string()];
        let topo_id =
            store.translate_or_create(&hue_key(), "hue-room-1", "Kitchen", "gl-1", &light_ids);

        let canonical_registry = crate::canonical::registry::CanonicalRegistry::new();
        let routing = store.composite_routing(&canonical_registry);

        assert!(!routing.contains_key(&topo_id));
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
        assert!(store.light_node_uses_device_dispatch(&light_id, &registry));
    }

    #[test]
    fn composite_routing_falls_back_to_devices_when_native_membership_is_stale() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.translate_or_create(
            &hue_key(),
            "hue-room-1",
            "Kitchen",
            "gl-kitchen",
            &["hue-light-1".to_string(), "hue-light-2".to_string()],
        );

        let mut registry = CanonicalRegistry::new();
        let light_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-light-1",
                "hue-room-1",
                "Kitchen",
                "Counter Light",
                DeviceType::Light,
            ),
        );

        assert!(store.attach_device_user_override(&room_id, &light_id));

        let routing = store.composite_routing(&registry);
        assert_eq!(
            routing.get(&room_id),
            Some(&vec![(
                hue_key().to_string(),
                HubDispatchTarget::Devices {
                    native_ids: vec!["hue-light-1".to_string()],
                },
            )])
        );

        let nodes = store.periodic_light_nodes(&registry);
        assert_eq!(
            nodes,
            vec![TopologyLightNode {
                id: light_id.clone(),
                source_node_id: light_id,
                emit_node_id: room_id.clone(),
            }]
        );
    }

    #[test]
    fn topology_sync_fence_persists_and_forces_individual_hue_dispatch() {
        let key = hue_key();
        let mut store = RoomTopologyStore::new();
        let room_id = store.translate_or_create(
            &key,
            "hue-room-1",
            "Kitchen",
            "gl-kitchen",
            &["hue-light-1".to_string()],
        );
        let mut registry = CanonicalRegistry::new();
        let light_id = register_identity(
            &mut registry,
            &key,
            make_identity(
                "hue-light-1",
                "hue-room-1",
                "Kitchen",
                "Counter Light",
                DeviceType::Light,
            ),
        );
        assert!(store.attach_device_user_override(&room_id, &light_id));
        assert!(matches!(
            &store.composite_routing(&registry)[&room_id][0].1,
            HubDispatchTarget::Group { .. }
        ));
        let scheduled_group_node_id = store
            .periodic_light_nodes(&registry)
            .into_iter()
            .find(|node| node.source_node_id == room_id)
            .expect("grouped Hue room should schedule one synthetic node")
            .id;

        assert!(store.set_external_room_topology_sync_enabled(&key, true));
        assert!(store.set_external_room_topology_sync_attention(&key, true));
        let direct_target = HubDispatchTarget::Devices {
            native_ids: vec!["hue-light-1".to_string()],
        };
        let suspended_routing = store.composite_routing(&registry);
        assert_eq!(
            suspended_routing.get(&room_id),
            Some(&vec![(key.to_string(), direct_target.clone())])
        );
        assert_eq!(
            suspended_routing.get(&scheduled_group_node_id),
            Some(&vec![(key.to_string(), direct_target)]),
            "a queued group-node tick must remain routable through the direct fallback"
        );
        assert!(
            store
                .periodic_light_nodes(&registry)
                .iter()
                .all(|node| node.id != scheduled_group_node_id),
            "the compatibility alias must not schedule duplicate periodic work"
        );

        let mut restored: RoomTopologyStore =
            serde_json::from_value(serde_json::to_value(&store).unwrap()).unwrap();
        restored.rebuild_indices();
        assert!(restored.external_room_topology_sync_is_enabled(&key));
        assert!(restored.external_grouped_dispatch_is_suspended(&key));
        assert!(restored.external_room_topology_sync_needs_attention(&key));
        assert!(matches!(
            &restored.composite_routing(&registry)[&room_id][0].1,
            HubDispatchTarget::Devices { .. }
        ));

        assert!(restored.set_external_room_topology_sync_enabled(&key, false));
        assert!(restored.external_grouped_dispatch_is_suspended(&key));
        assert!(matches!(
            &restored.composite_routing(&registry)[&room_id][0].1,
            HubDispatchTarget::Devices { .. }
        ));
        assert!(restored.set_external_room_topology_sync_enabled(&key, true));

        assert!(restored.set_external_grouped_dispatch_suspended(&key, false));
        assert!(matches!(
            &restored.composite_routing(&registry)[&room_id][0].1,
            HubDispatchTarget::Group { .. }
        ));
    }

    #[test]
    fn attached_light_nodes_use_parent_group_dispatch_when_available() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.translate_or_create(
            &hue_key(),
            "hue-room-1",
            "Kitchen",
            "gl-kitchen",
            &["hue-light-1".to_string()],
        );

        let mut registry = CanonicalRegistry::new();
        let light_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-light-1",
                "hue-room-1",
                "Kitchen",
                "Counter Light",
                DeviceType::Light,
            ),
        );

        assert!(store.attach_device_user_override(&room_id, &light_id));

        let routing = store.composite_routing(&registry);
        assert_eq!(
            routing.get(&light_id),
            Some(&vec![(
                hue_key().to_string(),
                HubDispatchTarget::Group {
                    room_id: "hue-room-1".to_string(),
                    control_id: "gl-kitchen".to_string(),
                },
            )])
        );
        assert!(store.attached_light_uses_parent_dispatch(&light_id, &registry));
        assert!(!store.light_node_uses_device_dispatch(&light_id, &registry));
    }

    #[test]
    fn attached_multi_endpoint_light_uses_sole_required_group_controller() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Kitchen");
        let hub_key = hue_key();

        let mut registry = CanonicalRegistry::new();
        let light_id = register_identity(
            &mut registry,
            &hub_key,
            make_identity("hue-light-1", "", "", "Counter Light", DeviceType::Light),
        );
        registry.get_mut(&light_id).unwrap().upsert_endpoint(
            ha_key(),
            "light.counter".to_string(),
            2000,
            None,
        );
        registry.set_preferred_endpoint(&light_id, &ha_key(), "light.counter");

        assert!(store.attach_device_user_override(&room_id, &light_id));
        store.set_grouped_room_control_required(&hub_key, true);
        assert!(store.upsert_managed_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: "hue-kitchen".to_string(),
                control_id: "grouped-kitchen".to_string(),
                light_device_ids: vec!["hue-light-1".to_string()],
            },
        ));

        let expected = vec![(
            hub_key.to_string(),
            HubDispatchTarget::Group {
                room_id: "hue-kitchen".to_string(),
                control_id: "grouped-kitchen".to_string(),
            },
        )];
        let routing = store.composite_routing(&registry);
        assert_eq!(routing.get(&room_id), Some(&expected));
        assert_eq!(routing.get(&light_id), Some(&expected));
        assert!(routing
            .get(&light_id)
            .unwrap()
            .iter()
            .all(|(key, _)| key != &ha_key().to_string()));
        assert!(store.attached_light_uses_parent_dispatch(&light_id, &registry));
        assert!(!store.light_node_uses_device_dispatch(&light_id, &registry));
    }

    #[test]
    fn attached_light_nodes_fall_back_to_device_dispatch_without_group_target() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Office");

        let mut registry = CanonicalRegistry::new();
        let light_one_id = register_identity(
            &mut registry,
            &matter_key(),
            make_identity("matter-light-1", "", "", "Desk Lamp", DeviceType::Light),
        );
        let light_two_id = register_identity(
            &mut registry,
            &matter_key(),
            make_identity("matter-light-2", "", "", "Floor Lamp", DeviceType::Light),
        );

        assert!(store.attach_device_user_override(&room_id, &light_one_id));
        assert!(store.attach_device_user_override(&room_id, &light_two_id));

        let routing = store.composite_routing(&registry);
        assert_eq!(
            routing.get(&room_id),
            Some(&vec![(
                matter_key().to_string(),
                HubDispatchTarget::Devices {
                    native_ids: vec!["matter-light-1".to_string(), "matter-light-2".to_string(),],
                },
            )])
        );
        assert_eq!(
            routing.get(&light_one_id),
            Some(&vec![(
                matter_key().to_string(),
                HubDispatchTarget::Devices {
                    native_ids: vec!["matter-light-1".to_string()],
                },
            )])
        );
        let mut expected_nodes = vec![
            TopologyLightNode {
                id: light_one_id.clone(),
                source_node_id: light_one_id.clone(),
                emit_node_id: room_id.clone(),
            },
            TopologyLightNode {
                id: light_two_id.clone(),
                source_node_id: light_two_id.clone(),
                emit_node_id: room_id.clone(),
            },
        ];
        expected_nodes.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(store.periodic_light_nodes(&registry), expected_nodes);
        assert!(!store.attached_light_uses_parent_dispatch(&light_one_id, &registry));
        assert!(store.light_node_uses_device_dispatch(&light_one_id, &registry));
    }

    #[test]
    fn required_group_control_fails_closed_until_binding_is_managed_exact_and_unambiguous() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Office");
        let hub_key = hue_key();

        let mut registry = CanonicalRegistry::new();
        let light_one_id = register_identity(
            &mut registry,
            &hub_key,
            make_identity("hue-light-1", "", "", "Desk Lamp", DeviceType::Light),
        );
        let light_two_id = register_identity(
            &mut registry,
            &hub_key,
            make_identity("hue-light-2", "", "", "Floor Lamp", DeviceType::Light),
        );
        assert!(store.attach_device_user_override(&room_id, &light_one_id));
        assert!(store.attach_device_user_override(&room_id, &light_two_id));
        store.set_grouped_room_control_required(&hub_key, true);

        assert!(store.composite_routing(&registry).get(&room_id).is_none());
        assert!(store.periodic_light_nodes(&registry).is_empty());

        // An exact discovered binding is still not owned by Rhythm.
        assert!(store.upsert_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: "hue-office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: vec!["hue-light-1".to_string(), "hue-light-2".to_string()],
            },
        ));
        assert!(store.composite_routing(&registry).get(&room_id).is_none());

        assert!(store.upsert_managed_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: "hue-office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: vec!["hue-light-2".to_string(), "hue-light-1".to_string()],
            },
        ));
        assert_eq!(
            store.composite_routing(&registry).get(&room_id),
            Some(&vec![(
                hub_key.to_string(),
                HubDispatchTarget::Group {
                    room_id: "hue-office".to_string(),
                    control_id: "grouped-office".to_string(),
                },
            )])
        );

        // Stale membership fails closed.
        store.get_mut(&room_id).unwrap().hub_room_bindings[0]
            .light_device_ids
            .pop();
        assert!(store.composite_routing(&registry).get(&room_id).is_none());

        // Even with exact membership, a second binding is ambiguous.
        store.get_mut(&room_id).unwrap().hub_room_bindings[0]
            .light_device_ids
            .push("hue-light-2".to_string());
        store
            .get_mut(&room_id)
            .unwrap()
            .upsert_hub_room_binding(HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: "hue-office-duplicate".to_string(),
                control_id: "grouped-office-duplicate".to_string(),
                light_device_ids: vec!["hue-light-1".to_string(), "hue-light-2".to_string()],
            });
        assert!(store.composite_routing(&registry).get(&room_id).is_none());
        assert!(store.periodic_light_nodes(&registry).is_empty());

        // Truly standalone Hue remains directly routable under the same hub
        // policy because no room-group invariant applies.
        assert!(store.assign_device(&light_one_id, None, DevicePlacement::UserOverride,));
        let routing = store.composite_routing(&registry);
        assert_eq!(
            routing.get(&light_one_id),
            Some(&vec![(
                hub_key.to_string(),
                HubDispatchTarget::Devices {
                    native_ids: vec!["hue-light-1".to_string()],
                },
            )])
        );
    }

    #[test]
    fn required_group_discovery_binding_without_assigned_lights_stays_inert() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Observed only");
        let hub_key = hue_key();
        assert!(store.upsert_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: "foreign-hue-room".to_string(),
                control_id: "foreign-grouped-light".to_string(),
                light_device_ids: vec!["unassigned-native-light".to_string()],
            },
        ));
        store.set_grouped_room_control_required(&hub_key, true);

        assert!(store
            .composite_routing(&CanonicalRegistry::new())
            .get(&room_id)
            .is_none());
        assert!(store
            .periodic_light_nodes(&CanonicalRegistry::new())
            .is_empty());
    }

    #[test]
    fn standalone_matter_light_remains_periodic_routable() {
        let mut store = RoomTopologyStore::new();
        let mut registry = CanonicalRegistry::new();
        let light_id = register_identity(
            &mut registry,
            &matter_key(),
            make_identity(
                "matter-light-1",
                "",
                "",
                "Standalone Lamp",
                DeviceType::Light,
            ),
        );
        store.ensure_standalone_device(&light_id);

        assert_eq!(
            store.composite_routing(&registry).get(&light_id),
            Some(&vec![(
                matter_key().to_string(),
                HubDispatchTarget::Devices {
                    native_ids: vec!["matter-light-1".to_string()],
                },
            )])
        );
        assert_eq!(
            store.periodic_light_nodes(&registry),
            vec![TopologyLightNode {
                id: light_id.clone(),
                source_node_id: light_id.clone(),
                emit_node_id: light_id.clone(),
            }]
        );
        assert!(store.light_node_uses_device_dispatch(&light_id, &registry));
    }

    #[test]
    fn standalone_hue_light_without_room_is_periodic_routable() {
        let mut store = RoomTopologyStore::new();
        let mut registry = CanonicalRegistry::new();
        let light_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "5761f9a0-bf64-4aac-aa92-ad6335c2451f",
                "",
                "",
                "Hue color lamp 32",
                DeviceType::Light,
            ),
        );
        store.ensure_standalone_device(&light_id);

        assert_eq!(
            store.composite_routing(&registry).get(&light_id),
            Some(&vec![(
                hue_key().to_string(),
                HubDispatchTarget::Devices {
                    native_ids: vec!["5761f9a0-bf64-4aac-aa92-ad6335c2451f".to_string()],
                },
            )])
        );
        assert_eq!(
            store.periodic_light_nodes(&registry),
            vec![TopologyLightNode {
                id: light_id.clone(),
                source_node_id: light_id.clone(),
                emit_node_id: light_id.clone(),
            }]
        );
        assert!(store.light_node_uses_device_dispatch(&light_id, &registry));
    }

    #[test]
    fn attached_hue_light_without_group_dispatch_uses_individual_routing() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Office");

        let mut registry = CanonicalRegistry::new();
        let light_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-light-1",
                "",
                "",
                "Assigned Hue Lamp",
                DeviceType::Light,
            ),
        );

        assert!(store.attach_device_user_override(&room_id, &light_id));

        let routing = store.composite_routing(&registry);
        assert_eq!(
            routing.get(&room_id),
            Some(&vec![(
                hue_key().to_string(),
                HubDispatchTarget::Devices {
                    native_ids: vec!["hue-light-1".to_string()],
                },
            )])
        );
        assert_eq!(
            routing.get(&light_id),
            Some(&vec![(
                hue_key().to_string(),
                HubDispatchTarget::Devices {
                    native_ids: vec!["hue-light-1".to_string()],
                },
            )])
        );
        assert_eq!(
            store.periodic_light_nodes(&registry),
            vec![TopologyLightNode {
                id: light_id.clone(),
                source_node_id: light_id.clone(),
                emit_node_id: room_id,
            }]
        );
        assert!(!store.attached_light_uses_parent_dispatch(&light_id, &registry));
        assert!(store.light_node_uses_device_dispatch(&light_id, &registry));
    }

    #[test]
    fn non_light_devices_do_not_activate_an_unproven_group_binding() {
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

        assert!(store.composite_routing(&registry).get(&room_id).is_none());
        assert!(store.periodic_light_nodes(&registry).is_empty());
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

    #[test]
    fn motion_device_controls_can_target_multiple_rooms() {
        let mut store = RoomTopologyStore::new();
        let source_room_id = store.create_room("Office");
        let hall_id = store.create_room("Hall");
        let stairs_id = store.create_room("Stairs");
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
        assert!(store.set_control_targets(
            &sensor_id,
            NodeControlKind::Motion,
            &[hall_id.as_str(), stairs_id.as_str()],
        ));

        let mut controls = store.effective_node_controls(&sensor_id, &registry);
        controls.sort_by(|left, right| left.1.cmp(&right.1));
        let mut expected_controls = vec![
            (NodeControlKind::Motion, hall_id.clone(), false),
            (NodeControlKind::Motion, stairs_id.clone(), false),
        ];
        expected_controls.sort_by(|left, right| left.1.cmp(&right.1));
        assert_eq!(controls, expected_controls);
        assert_eq!(
            store
                .effective_control_targets_for_kind(&NodeControlKind::Motion, &registry)
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            [hall_id, stairs_id].into_iter().collect()
        );
    }

    #[test]
    fn button_multi_room_controls_survive_roundtrip_and_clear_to_parent() {
        let mut store = RoomTopologyStore::new();
        let source_room_id = store.create_room("Office");
        let hall_id = store.create_room("Hall");
        let stairs_id = store.create_room("Stairs");
        let mut registry = CanonicalRegistry::new();

        let button_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "button-1",
                "office-native",
                "Office",
                "Office Button",
                DeviceType::Button,
            ),
        );

        assert!(store.attach_device_user_override(&source_room_id, &button_id));
        assert!(store.set_control_targets(
            &button_id,
            NodeControlKind::Button,
            &[hall_id.as_str(), stairs_id.as_str()],
        ));

        let serialized = serde_json::to_value(&store).expect("serialize topology");
        let mut restored: RoomTopologyStore =
            serde_json::from_value(serialized).expect("restore topology");
        let mut expected_targets = vec![hall_id, stairs_id];
        expected_targets.sort();
        assert_eq!(
            restored.effective_control_targets(&button_id, &NodeControlKind::Button),
            expected_targets
        );

        assert!(restored.set_control_targets(&button_id, NodeControlKind::Button, &[],));
        assert_eq!(
            restored.effective_control_targets(&button_id, &NodeControlKind::Button),
            vec![source_room_id]
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
            source_name_authoritative: true,
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
            source_name_authoritative: true,
            control_id: "gl-office".to_string(),
            light_device_ids: vec!["hue-light-1".to_string()],
            canonical_device_ids: vec!["canonical-hue-1".to_string()],
        };
        let action = store.sync_hub_room(&hue_key(), &discovered);
        assert!(matches!(action, SyncAction::Updated { .. }));
    }

    #[test]
    fn sync_hub_room_with_registry_preserves_other_hub_defaults_in_bound_room() {
        let mut store = RoomTopologyStore::new();
        let mut registry = CanonicalRegistry::new();

        let office_id = store.translate_or_create(&ha_key(), "office", "Office", "office", &[]);
        let ha_light_id = register_identity(
            &mut registry,
            &ha_key(),
            make_identity(
                "light.office_bloom",
                "office",
                "Office",
                "Bloom",
                DeviceType::Light,
            ),
        );
        let hue_light_id = register_identity(
            &mut registry,
            &hue_key(),
            make_identity(
                "hue-office-1",
                "hue-office",
                "Office",
                "Hue Lamp",
                DeviceType::Light,
            ),
        );
        assert!(store.attach_device_hub_default(&office_id, &ha_light_id));
        assert!(store.attach_device_hub_default(&office_id, &hue_light_id));

        store.approve_binding(hue_key(), "hue-office".to_string(), office_id.clone(), 1000);
        let bound_id =
            store.translate_or_create(&hue_key(), "hue-office", "Office", "gl-office", &[]);
        assert_eq!(bound_id, office_id);

        let hue_discovered = DiscoveredTopologyRoom {
            hub_room_id: "hue-office".to_string(),
            name: "Office".to_string(),
            source_name_authoritative: true,
            control_id: "gl-office".to_string(),
            light_device_ids: vec!["hue-office-1".to_string()],
            canonical_device_ids: vec![hue_light_id.clone()],
        };
        store.sync_hub_room_with_registry(&hue_key(), &hue_discovered, &registry);

        assert_eq!(
            store.device_parent_room_id(&ha_light_id),
            Some(office_id.as_str()),
            "syncing Hue should not detach HA defaults from a bound room"
        );
        assert_eq!(
            store.device_parent_room_id(&hue_light_id),
            Some(office_id.as_str()),
        );

        let ha_discovered = DiscoveredTopologyRoom {
            hub_room_id: "office".to_string(),
            name: "Office".to_string(),
            source_name_authoritative: true,
            control_id: "office".to_string(),
            light_device_ids: vec!["light.office_bloom".to_string()],
            canonical_device_ids: vec![ha_light_id.clone()],
        };
        store.sync_hub_room_with_registry(&ha_key(), &ha_discovered, &registry);

        assert_eq!(
            store.device_parent_room_id(&ha_light_id),
            Some(office_id.as_str()),
            "syncing HA should keep its own defaults attached"
        );
        assert_eq!(
            store.device_parent_room_id(&hue_light_id),
            Some(office_id.as_str()),
            "syncing HA should not strip Hue defaults from a bound room"
        );
    }

    #[test]
    fn input_binding_matches_button_source_and_action() {
        let mut store = RoomTopologyStore::new();
        let binding = InputBinding::day_sleep_toggle("button-1", Some(ButtonAction::OnPress));
        let binding_id = binding.id.clone();

        assert!(store.set_input_binding(binding.clone()));
        assert_eq!(store.input_binding(&binding_id), Some(&binding));
        assert!(store
            .matching_button_input_binding("button-1", ButtonAction::OnPress)
            .is_some());
        assert!(store
            .matching_button_input_binding("button-1", ButtonAction::OffPress)
            .is_none());
        assert!(store
            .matching_button_input_binding("button-2", ButtonAction::OnPress)
            .is_none());
    }

    #[test]
    fn input_binding_roundtrips_with_topology() {
        let mut store = RoomTopologyStore::new();
        let binding = InputBinding::day_sleep_toggle("button-1", None);
        let binding_id = binding.id.clone();
        store.set_input_binding(binding);

        let json = serde_json::to_value(&store).unwrap();
        let mut loaded: RoomTopologyStore = serde_json::from_value(json).unwrap();
        loaded.rebuild_indices();

        let binding = loaded
            .input_binding(&binding_id)
            .expect("binding should roundtrip");
        assert_eq!(binding.source_node_id, "button-1");
        assert_eq!(binding.preset, Some(InputBindingPreset::DaySleepToggle));
        assert!(binding.matches_button("button-1", ButtonAction::DownHold));
    }

    #[test]
    fn hue_rooms_apply_explicit_authority_per_room() {
        let mut store = RoomTopologyStore::new();
        let office_id = store.create_room("Office");
        let bedroom_id = store.create_room("Bedroom");
        let key = hue_key();
        for (room_id, native_id) in [(&office_id, "hue-office"), (&bedroom_id, "hue-bedroom")] {
            assert!(store.upsert_room_binding(
                room_id,
                HubRoomBinding {
                    hub_key: key.clone(),
                    hub_room_id: native_id.to_string(),
                    control_id: format!("grouped-{native_id}"),
                    light_device_ids: Vec::new(),
                },
            ));
        }

        assert!(!store.rhythm_automation_allowed_for_node(&office_id));
        assert!(!store.external_hub_has_full_rhythm_consent(&key));
        assert!(!store.external_hub_has_rhythm_consent(&key));
        let before_revision = store.external_automation_revision(&key);

        assert!(store
            .replace_external_room_automation_decisions(
                &key,
                &[
                    (office_id.clone(), ExternalRoomAutomationOwner::Rhythm),
                    (bedroom_id.clone(), ExternalRoomAutomationOwner::External,),
                ],
            )
            .unwrap());
        assert!(store.rhythm_automation_allowed_for_node(&office_id));
        assert!(!store.rhythm_automation_allowed_for_node(&bedroom_id));
        assert!(store.external_hub_has_rhythm_consent(&key));
        assert!(!store.external_hub_has_full_rhythm_consent(&key));
        assert_ne!(store.external_automation_revision(&key), before_revision);

        let restored: RoomTopologyStore =
            serde_json::from_value(serde_json::to_value(&store).unwrap()).unwrap();
        assert!(restored.rhythm_automation_allowed_for_node(&office_id));
        assert!(!restored.rhythm_automation_allowed_for_node(&bedroom_id));
        assert!(restored.external_hub_has_rhythm_consent(&key));
        assert!(!restored.external_hub_has_full_rhythm_consent(&key));

        assert!(store
            .replace_external_room_automation_decisions(
                &key,
                &[
                    (office_id.clone(), ExternalRoomAutomationOwner::Rhythm),
                    (bedroom_id, ExternalRoomAutomationOwner::Rhythm),
                ],
            )
            .unwrap());
        assert!(store.external_hub_has_full_rhythm_consent(&key));
    }

    #[test]
    fn legacy_hue_rooms_are_grandfathered_once_without_overwriting_choices() {
        let mut store = RoomTopologyStore::new();
        let office_id = store.create_room("Office");
        let bedroom_id = store.create_room("Bedroom");
        let key = hue_key();
        for (room_id, native_id) in [(&office_id, "hue-office"), (&bedroom_id, "hue-bedroom")] {
            assert!(store.upsert_room_binding(
                room_id,
                HubRoomBinding {
                    hub_key: key.clone(),
                    hub_room_id: native_id.to_string(),
                    control_id: format!("grouped-{native_id}"),
                    light_device_ids: Vec::new(),
                },
            ));
        }
        store
            .replace_external_room_automation_decisions(
                &key,
                &[
                    (office_id.clone(), ExternalRoomAutomationOwner::External),
                    (bedroom_id.clone(), ExternalRoomAutomationOwner::Rhythm),
                ],
            )
            .unwrap();

        let mut legacy_json = serde_json::to_value(&store).unwrap();
        legacy_json
            .as_object_mut()
            .unwrap()
            .remove("external_room_automation_policy_version");
        // Simulate an older snapshot with one explicit choice and one room
        // that still relied on Rhythm's pre-review implicit authority.
        legacy_json["external_room_automation_decisions"]
            .as_array_mut()
            .unwrap()
            .retain(|decision| decision["rhythm_room_id"] == office_id);
        let mut restored: RoomTopologyStore = serde_json::from_value(legacy_json).unwrap();
        restored.rebuild_indices();

        let migration =
            restored.migrate_legacy_external_room_automation_policy(std::slice::from_ref(&key));
        assert!(migration.changed());
        assert_eq!(migration.grandfathered_hue_rooms, 1);
        assert_eq!(
            restored.external_room_automation_owner(&office_id, &key),
            Some(ExternalRoomAutomationOwner::External)
        );
        assert_eq!(
            restored.external_room_automation_owner(&bedroom_id, &key),
            Some(ExternalRoomAutomationOwner::Rhythm)
        );
        assert!(restored.materialize_grandfathered_external_room_automation_decisions(&key));
        assert_eq!(
            restored.external_room_automation_owner(&office_id, &key),
            Some(ExternalRoomAutomationOwner::External),
            "materialization must preserve an explicit Hue-owned choice"
        );
        assert_eq!(
            restored.external_room_automation_owner(&bedroom_id, &key),
            Some(ExternalRoomAutomationOwner::Rhythm)
        );
        assert!(!restored
            .migrate_legacy_external_room_automation_policy(std::slice::from_ref(&key))
            .changed());
    }

    #[test]
    fn newly_created_hue_rooms_are_not_grandfathered() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Office");
        let key = hue_key();
        assert!(store.upsert_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: key.clone(),
                hub_room_id: "hue-office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: Vec::new(),
            },
        ));

        assert!(!store
            .migrate_legacy_external_room_automation_policy(&[])
            .changed());
        assert_eq!(store.external_room_automation_owner(&room_id, &key), None);
        assert!(!store.rhythm_automation_allowed_for_node(&room_id));
    }

    #[test]
    fn legacy_configured_hue_bridge_is_grandfathered_before_room_discovery() {
        let key = hue_key();
        let mut legacy_json = serde_json::to_value(RoomTopologyStore::new()).unwrap();
        legacy_json
            .as_object_mut()
            .unwrap()
            .remove("external_room_automation_policy_version");
        let mut restored: RoomTopologyStore = serde_json::from_value(legacy_json).unwrap();

        let migration =
            restored.migrate_legacy_external_room_automation_policy(std::slice::from_ref(&key));
        assert!(migration.changed());
        assert_eq!(migration.grandfathered_hue_rooms, 0);
        let mut restored: RoomTopologyStore =
            serde_json::from_value(serde_json::to_value(restored).unwrap()).unwrap();
        restored.rebuild_indices();

        let room_id = restored.create_room("Office");
        assert!(restored.upsert_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: key.clone(),
                hub_room_id: "hue-office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: Vec::new(),
            },
        ));
        assert_eq!(
            restored.external_room_automation_owner(&room_id, &key),
            Some(ExternalRoomAutomationOwner::Rhythm)
        );
        assert!(restored.rhythm_automation_allowed_for_node(&room_id));

        assert!(restored.forget_external_room_automation_policy_for_hub(&key));
        assert_eq!(
            restored.external_room_automation_owner(&room_id, &key),
            None
        );
    }

    #[test]
    fn legacy_first_discovery_materializes_decisions_and_retires_hub_fallback() {
        let key = hue_key();
        let mut store = RoomTopologyStore::legacy_empty();
        assert!(store
            .migrate_legacy_external_room_automation_policy(std::slice::from_ref(&key))
            .changed());

        let first_action = store.sync_hub_room(
            &key,
            &DiscoveredTopologyRoom {
                hub_room_id: "hue-office".to_string(),
                name: "Office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: Vec::new(),
                canonical_device_ids: Vec::new(),
                source_name_authoritative: true,
            },
        );
        let first_room_id = first_action.rhythm_room_id().to_string();
        assert_eq!(
            store.external_room_automation_owner(&first_room_id, &key),
            Some(ExternalRoomAutomationOwner::Rhythm)
        );

        assert!(store.materialize_grandfathered_external_room_automation_decisions(&key));
        assert!(!store
            .external_room_automation_grandfathered_hubs
            .contains(&key));
        assert_eq!(
            store.external_room_automation_owner(&first_room_id, &key),
            Some(ExternalRoomAutomationOwner::Rhythm),
            "the current legacy room should retain explicit Rhythm ownership"
        );

        let later_action = store.sync_hub_room(
            &key,
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
            store.external_room_automation_owner(later_action.rhythm_room_id(), &key),
            None,
            "rooms discovered after the bootstrap epoch must be reviewed"
        );
        assert!(!store.materialize_grandfathered_external_room_automation_decisions(&key));
    }

    #[test]
    fn explicit_hue_review_clears_fallback_and_rediscovery_stays_unreviewed() {
        let key = hue_key();
        let mut store = RoomTopologyStore::legacy_empty();
        store.migrate_legacy_external_room_automation_policy(std::slice::from_ref(&key));
        let action = store.sync_hub_room(
            &key,
            &DiscoveredTopologyRoom {
                hub_room_id: "hue-office".to_string(),
                name: "Office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: Vec::new(),
                canonical_device_ids: Vec::new(),
                source_name_authoritative: true,
            },
        );
        let room_id = action.rhythm_room_id().to_string();

        assert!(store
            .replace_external_room_automation_decisions(
                &key,
                &[(room_id, ExternalRoomAutomationOwner::External)],
            )
            .unwrap());
        assert!(!store
            .external_room_automation_grandfathered_hubs
            .contains(&key));

        assert_eq!(store.remove_stale_bindings(&key, &[]).len(), 1);
        let rediscovered = store.sync_hub_room(
            &key,
            &DiscoveredTopologyRoom {
                hub_room_id: "hue-office".to_string(),
                name: "Office".to_string(),
                control_id: "grouped-office-new".to_string(),
                light_device_ids: Vec::new(),
                canonical_device_ids: Vec::new(),
                source_name_authoritative: true,
            },
        );
        assert_eq!(
            store.external_room_automation_owner(rediscovered.rhythm_room_id(), &key),
            None,
            "a stale Hue-owned room must never return as implicitly Rhythm-owned"
        );
        assert!(!store.external_hub_has_full_rhythm_consent(&key));
    }

    #[test]
    fn repeated_full_review_reports_grandfather_marker_clear() {
        let key = hue_key();
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Office");
        assert!(store.upsert_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: key.clone(),
                hub_room_id: "hue-office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: Vec::new(),
            },
        ));
        let decisions = vec![(room_id, ExternalRoomAutomationOwner::External)];
        assert!(store
            .replace_external_room_automation_decisions(&key, &decisions)
            .unwrap());

        let mut legacy_json = serde_json::to_value(store).unwrap();
        legacy_json
            .as_object_mut()
            .unwrap()
            .remove("external_room_automation_policy_version");
        let mut restored: RoomTopologyStore = serde_json::from_value(legacy_json).unwrap();
        assert!(restored
            .migrate_legacy_external_room_automation_policy(std::slice::from_ref(&key))
            .changed());
        assert!(restored
            .external_room_automation_grandfathered_hubs
            .contains(&key));

        assert!(restored
            .replace_external_room_automation_decisions(&key, &decisions)
            .unwrap());
        assert!(!restored
            .external_room_automation_grandfathered_hubs
            .contains(&key));
    }

    #[test]
    fn grandfather_marker_is_not_a_structural_hub_reference() {
        let key = hue_key();
        let mut store = RoomTopologyStore::legacy_empty();
        store.migrate_legacy_external_room_automation_policy(std::slice::from_ref(&key));

        assert!(store.references_hub_key(&key));
        assert!(!store.structurally_references_hub_key(&key));

        let room_id = store.create_room("Office");
        assert!(store.upsert_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: key.clone(),
                hub_room_id: "hue-office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: Vec::new(),
            },
        ));
        assert!(store.structurally_references_hub_key(&key));
    }

    #[test]
    fn hue_room_decisions_require_the_complete_current_room_set_and_roundtrip() {
        let mut store = RoomTopologyStore::new();
        let room_id = store.create_room("Office");
        let key = hue_key();
        assert!(store.upsert_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key: key.clone(),
                hub_room_id: "hue-office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: Vec::new(),
            },
        ));
        assert!(store
            .replace_external_room_automation_decisions(&key, &[])
            .is_err());
        store
            .replace_external_room_automation_decisions(
                &key,
                &[(room_id.clone(), ExternalRoomAutomationOwner::External)],
            )
            .unwrap();

        let json = serde_json::to_value(&store).unwrap();
        let mut restored: RoomTopologyStore = serde_json::from_value(json).unwrap();
        restored.rebuild_indices();
        assert_eq!(
            restored.external_room_automation_owner(&room_id, &key),
            Some(ExternalRoomAutomationOwner::External)
        );
    }

    #[test]
    fn merging_rooms_never_turns_conflicting_hue_choices_into_consent() {
        let mut store = RoomTopologyStore::new();
        let office_id = store.create_room("Office");
        let hall_id = store.create_room("Hall");
        let key = hue_key();
        for (room_id, native_id) in [(&office_id, "hue-office"), (&hall_id, "hue-hall")] {
            assert!(store.upsert_room_binding(
                room_id,
                HubRoomBinding {
                    hub_key: key.clone(),
                    hub_room_id: native_id.to_string(),
                    control_id: format!("grouped-{native_id}"),
                    light_device_ids: Vec::new(),
                },
            ));
        }
        store
            .replace_external_room_automation_decisions(
                &key,
                &[
                    (office_id.clone(), ExternalRoomAutomationOwner::Rhythm),
                    (hall_id.clone(), ExternalRoomAutomationOwner::External),
                ],
            )
            .unwrap();

        assert!(store.merge_rooms(&office_id, &hall_id));

        assert_eq!(
            store.external_room_automation_owner(&office_id, &key),
            Some(ExternalRoomAutomationOwner::External)
        );
        assert!(!store.external_hub_has_full_rhythm_consent(&key));
    }
}
