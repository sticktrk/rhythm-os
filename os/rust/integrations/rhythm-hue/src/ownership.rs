//! Durable Hue bridge inventory capture, automation suppression, and release.
//!
//! Rhythm treats a paired Hue bridge as a Zigbee control plane. Before the
//! first mutation this module captures a complete Hue V2 inventory plus the V1
//! automation collections and durably persists that bridge-scoped baseline.
//! Releasing authority restores only fields that Rhythm journaled, and only
//! when the live resource still matches either Rhythm's written value or the
//! immutable baseline. Any intervening Hue/user edit fails closed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use log::warn;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::storage::Storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::transport::HueTransport;

pub const HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION: u32 = 1;

const REQUIRED_V2_BASELINE_RESOURCES: &[&str] = &[
    "bridge",
    "device",
    "light",
    "behavior_instance",
    "room",
    "zone",
    "scene",
    "smart_scene",
];
const REQUIRED_V1_BASELINE_RESOURCES: &[&str] = &["rules", "schedules"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HueOwnershipPhase {
    Captured,
    Clearing,
    ClearIncomplete,
    Active,
    Restoring,
    RestoreIncomplete,
    Restored,
    ReleasePending,
    SnapshotRetained,
}

impl HueOwnershipPhase {
    pub fn is_managed(self) -> bool {
        matches!(
            self,
            Self::Clearing
                | Self::ClearIncomplete
                | Self::Active
                | Self::Restoring
                | Self::RestoreIncomplete
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HueBaselineCaptureScope {
    /// Baselines created before complete V2 inventory capture was introduced.
    #[default]
    ControlPlaneSubset,
    /// One complete `/clip/v2/resource` observation grouped by resource type.
    FullV2Inventory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HueOwnershipReceiptStatus {
    Pending,
    Succeeded,
    Failed,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueOwnershipReceipt {
    pub operation_id: String,
    pub api: String,
    pub action: String,
    pub resource_type: String,
    pub original_resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_resource_id: Option<String>,
    pub status: HueOwnershipReceiptStatus,
    pub attempt: u32,
}

/// Explicit mapping proving that a Hue room was created for a Rhythm room.
///
/// Managed status is never inferred from the Hue room name or ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueManagedRoom {
    pub rhythm_room_id: String,
    pub hue_room_id: String,
    pub grouped_light_id: String,
}

/// Explicit ownership receipt for a Rhythm scene projected into one managed
/// Hue room. Native resources are never adopted by display name alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueManagedScene {
    pub rhythm_room_id: String,
    pub rhythm_scene_id: String,
    pub hue_room_id: String,
    pub hue_scene_id: String,
    pub fingerprint: String,
    #[serde(default)]
    pub ephemeral: bool,
}

/// Immutable snapshot captured before Rhythm takes control of a bridge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HueBridgeOwnershipBaseline {
    schema_version: u32,
    capture_id: String,
    bridge_id: String,
    #[serde(default)]
    capture_scope: HueBaselineCaptureScope,
    v2_resources: BTreeMap<String, Value>,
    v1_resources: BTreeMap<String, Value>,
}

impl HueBridgeOwnershipBaseline {
    pub fn capture_id(&self) -> &str {
        &self.capture_id
    }

    pub fn bridge_id(&self) -> &str {
        &self.bridge_id
    }

    pub fn capture_scope(&self) -> HueBaselineCaptureScope {
        self.capture_scope
    }

    pub fn v2_resource(&self, resource_type: &str) -> Option<&Value> {
        self.v2_resources.get(resource_type)
    }

    pub fn v1_resource(&self, resource_type: &str) -> Option<&Value> {
        self.v1_resources.get(resource_type)
    }
}

/// Durable bridge-scoped ownership manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HueControllerOwnership {
    pub schema_version: u32,
    pub phase: HueOwnershipPhase,
    baseline: HueBridgeOwnershipBaseline,
    #[serde(default)]
    managed_rooms: BTreeMap<String, HueManagedRoom>,
    #[serde(default)]
    managed_scenes: BTreeMap<String, HueManagedScene>,
    #[serde(default)]
    receipts: BTreeMap<String, HueOwnershipReceipt>,
}

/// Privacy-bounded ownership summary suitable for support bundles. It never
/// includes bridge/resource identities, room names, addresses, credentials,
/// baseline bodies, or manifest paths.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HueControllerAuthorityDiagnostics {
    pub schema_version: u32,
    pub manifest_count: usize,
    pub baseline_count: usize,
    pub corrupt_manifest_count: usize,
    pub managed_room_count: usize,
    pub managed_scene_count: usize,
    pub captured_v2_resource_type_count: usize,
    pub captured_v2_resource_count: usize,
    pub captured_v1_automation_count: usize,
    pub phases: BTreeMap<String, usize>,
    pub receipt_statuses: BTreeMap<String, usize>,
}

pub fn controller_authority_diagnostics(
    storage: &dyn Storage,
) -> Result<HueControllerAuthorityDiagnostics> {
    let files = storage
        .load_integration_backup_files(true)
        .map_err(|_| anyhow::anyhow!("Failed to inspect Hue ownership diagnostics"))?;
    let mut diagnostics = HueControllerAuthorityDiagnostics {
        schema_version: 1,
        ..HueControllerAuthorityDiagnostics::default()
    };
    for file in files.into_iter().filter(|file| {
        file.path.starts_with("hue/controller-ownership/by-bridge/") && file.path.ends_with(".json")
    }) {
        diagnostics.manifest_count += 1;
        let Ok(ownership) = serde_json::from_str::<HueControllerOwnership>(&file.content) else {
            diagnostics.corrupt_manifest_count += 1;
            continue;
        };
        diagnostics.baseline_count += 1;
        diagnostics.managed_room_count += ownership.managed_rooms.len();
        diagnostics.managed_scene_count += ownership.managed_scenes.len();
        diagnostics.captured_v2_resource_type_count += ownership.baseline.v2_resources.len();
        diagnostics.captured_v2_resource_count += ownership
            .baseline
            .v2_resources
            .values()
            .filter_map(|resource| resource.get("data").and_then(Value::as_array))
            .map(Vec::len)
            .sum::<usize>();
        diagnostics.captured_v1_automation_count += ownership
            .baseline
            .v1_resources
            .values()
            .filter_map(Value::as_object)
            .map(serde_json::Map::len)
            .sum::<usize>();
        let phase = match ownership.phase {
            HueOwnershipPhase::Captured => "captured",
            HueOwnershipPhase::Clearing => "clearing",
            HueOwnershipPhase::ClearIncomplete => "clear_incomplete",
            HueOwnershipPhase::Active => "active",
            HueOwnershipPhase::Restoring => "restoring",
            HueOwnershipPhase::RestoreIncomplete => "restore_incomplete",
            HueOwnershipPhase::Restored => "restored",
            HueOwnershipPhase::ReleasePending => "release_pending",
            HueOwnershipPhase::SnapshotRetained => "snapshot_retained",
        };
        *diagnostics.phases.entry(phase.to_string()).or_default() += 1;
        for receipt in ownership.receipts.values() {
            let status = match receipt.status {
                HueOwnershipReceiptStatus::Pending => "pending",
                HueOwnershipReceiptStatus::Succeeded => "succeeded",
                HueOwnershipReceiptStatus::Failed => "failed",
                HueOwnershipReceiptStatus::Unsupported => "unsupported",
            };
            *diagnostics
                .receipt_statuses
                .entry(status.to_string())
                .or_default() += 1;
        }
    }
    Ok(diagnostics)
}

impl HueControllerOwnership {
    fn captured(baseline: HueBridgeOwnershipBaseline) -> Self {
        Self {
            schema_version: HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
            phase: HueOwnershipPhase::Captured,
            baseline,
            managed_rooms: BTreeMap::new(),
            managed_scenes: BTreeMap::new(),
            receipts: BTreeMap::new(),
        }
    }

    pub fn baseline(&self) -> &HueBridgeOwnershipBaseline {
        &self.baseline
    }

    pub fn managed_rooms(&self) -> &BTreeMap<String, HueManagedRoom> {
        &self.managed_rooms
    }

    pub fn managed_room_ids(&self) -> BTreeSet<String> {
        self.managed_rooms
            .values()
            .map(|room| room.hue_room_id.clone())
            .collect()
    }

    pub fn managed_scenes(&self) -> &BTreeMap<String, HueManagedScene> {
        &self.managed_scenes
    }

    pub fn managed_scene(
        &self,
        rhythm_room_id: &str,
        rhythm_scene_id: &str,
    ) -> Option<&HueManagedScene> {
        self.managed_scenes
            .get(&managed_scene_key(rhythm_room_id, rhythm_scene_id))
    }

    pub fn receipts(&self) -> impl Iterator<Item = &HueOwnershipReceipt> {
        self.receipts.values()
    }

    pub fn record_managed_room(&mut self, room: HueManagedRoom) -> Result<()> {
        if !self.phase.is_managed() || self.phase == HueOwnershipPhase::Restoring {
            anyhow::bail!("Hue managed-room mappings can only change while Rhythm owns the bridge");
        }
        if self.managed_rooms.values().any(|existing| {
            existing.rhythm_room_id != room.rhythm_room_id
                && existing.hue_room_id == room.hue_room_id
        }) {
            anyhow::bail!("A Hue room is already mapped to another Rhythm room");
        }
        self.managed_rooms.insert(room.rhythm_room_id.clone(), room);
        Ok(())
    }

    pub fn remove_managed_room(&mut self, rhythm_room_id: &str) -> Result<()> {
        if !self.phase.is_managed() || self.phase == HueOwnershipPhase::Restoring {
            anyhow::bail!("Hue managed-room mappings can only change while Rhythm owns the bridge");
        }
        if self
            .managed_scenes
            .values()
            .any(|scene| scene.rhythm_room_id == rhythm_room_id)
        {
            anyhow::bail!("Managed Hue scenes must be removed before their room mapping");
        }
        self.managed_rooms.remove(rhythm_room_id);
        Ok(())
    }

    pub fn record_managed_scene(&mut self, scene: HueManagedScene) -> Result<()> {
        if self.phase != HueOwnershipPhase::Active {
            anyhow::bail!("Hue managed-scene mappings can only change while authority is active");
        }
        let room = self
            .managed_rooms
            .get(&scene.rhythm_room_id)
            .ok_or_else(|| anyhow::anyhow!("Managed Hue scene has no managed room"))?;
        if room.hue_room_id != scene.hue_room_id {
            anyhow::bail!("Managed Hue scene room identity is inconsistent");
        }
        if self.managed_scenes.values().any(|existing| {
            existing.hue_scene_id == scene.hue_scene_id
                && (existing.rhythm_room_id != scene.rhythm_room_id
                    || existing.rhythm_scene_id != scene.rhythm_scene_id)
        }) {
            anyhow::bail!("A Hue scene is already owned by another Rhythm projection");
        }
        self.managed_scenes.insert(
            managed_scene_key(&scene.rhythm_room_id, &scene.rhythm_scene_id),
            scene,
        );
        Ok(())
    }

    pub fn remove_managed_scene(
        &mut self,
        rhythm_room_id: &str,
        rhythm_scene_id: &str,
    ) -> Result<Option<HueManagedScene>> {
        if self.phase != HueOwnershipPhase::Active {
            anyhow::bail!("Hue managed-scene mappings can only change while authority is active");
        }
        Ok(self
            .managed_scenes
            .remove(&managed_scene_key(rhythm_room_id, rhythm_scene_id)))
    }

    fn ensure_bridge_id(&self, bridge_id: &str) -> Result<()> {
        if bridge_id != self.baseline.bridge_id {
            anyhow::bail!("Hue bridge identity mismatch with the ownership baseline");
        }
        Ok(())
    }

    fn set_receipt(
        &mut self,
        operation: &ControlPlaneOperation,
        status: HueOwnershipReceiptStatus,
        replacement_resource_id: Option<String>,
    ) {
        let previous_attempt = self
            .receipts
            .get(&operation.operation_id)
            .map(|receipt| receipt.attempt)
            .unwrap_or(0);
        let attempt = if status == HueOwnershipReceiptStatus::Pending {
            previous_attempt.saturating_add(1).max(1)
        } else {
            previous_attempt.max(1)
        };
        self.receipts.insert(
            operation.operation_id.clone(),
            HueOwnershipReceipt {
                operation_id: operation.operation_id.clone(),
                api: operation.api.to_string(),
                action: operation.action.to_string(),
                resource_type: operation.resource_type.to_string(),
                original_resource_id: operation.resource_id.clone(),
                replacement_resource_id,
                status,
                attempt,
            },
        );
    }
}

fn managed_scene_key(rhythm_room_id: &str, rhythm_scene_id: &str) -> String {
    format!("{}:{rhythm_room_id}{rhythm_scene_id}", rhythm_room_id.len())
}

/// Serialize every control-plane mutation for one physical bridge, including
/// topology reconciliation and scene projection. The
/// process-level map intentionally stores only stable bridge identities.
pub fn controller_operation_lock(bridge_id: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<BTreeMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut locks = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks
        .entry(bridge_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[derive(Clone, Debug)]
struct ControlPlaneOperation {
    operation_id: String,
    api: &'static str,
    action: &'static str,
    resource_type: &'static str,
    resource_id: String,
    body: Option<Value>,
}

fn ownership_path(bridge_id: &str) -> Result<String> {
    if bridge_id.trim().is_empty() {
        anyhow::bail!("Hue bridge identity must not be empty");
    }
    let safe_bridge_id = bridge_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    Ok(format!(
        "hue/controller-ownership/by-bridge/{safe_bridge_id}.json"
    ))
}

pub fn hue_controller_ownership_path(bridge_id: &str) -> Result<String> {
    ownership_path(bridge_id)
}

pub fn load_controller_ownership(
    storage: &dyn Storage,
    bridge_id: &str,
) -> Result<Option<HueControllerOwnership>> {
    let path = ownership_path(bridge_id)?;
    let Some(content) = storage
        .load_integration_state_file(&path)
        .map_err(|_| anyhow::anyhow!("Failed to load Hue ownership manifest"))?
    else {
        return Ok(None);
    };
    let state: HueControllerOwnership =
        serde_json::from_str(&content).context("Failed to parse Hue ownership manifest")?;
    if state.schema_version != HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION
        || state.baseline.schema_version != HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION
    {
        anyhow::bail!("Unsupported Hue ownership manifest schema");
    }
    Ok(Some(state))
}

fn baseline_can_be_safely_upgraded(
    existing: &HueControllerOwnership,
    replacement: &HueControllerOwnership,
) -> bool {
    existing.phase == HueOwnershipPhase::Captured
        && replacement.phase == HueOwnershipPhase::Captured
        && existing.baseline.bridge_id == replacement.baseline.bridge_id
        && existing.baseline.capture_scope == HueBaselineCaptureScope::ControlPlaneSubset
        && replacement.baseline.capture_scope == HueBaselineCaptureScope::FullV2Inventory
        && existing.managed_rooms.is_empty()
        && existing.managed_scenes.is_empty()
        && existing.receipts.is_empty()
}

/// Persist a state transition while refusing to replace the immutable baseline.
///
/// The sole exception upgrades an untouched legacy `captured` manifest to a
/// complete V2 inventory. With no mutation receipts or managed resources, the
/// bridge still represents the original pre-takeover state.
pub fn persist_controller_ownership(
    storage: &dyn Storage,
    state: &HueControllerOwnership,
) -> Result<()> {
    if let Some(existing) = load_controller_ownership(storage, state.baseline.bridge_id())? {
        if existing.baseline != state.baseline && !baseline_can_be_safely_upgraded(&existing, state)
        {
            anyhow::bail!("Refusing to replace the immutable Hue ownership baseline");
        }
    }
    let content = serde_json::to_string_pretty(state)?;
    storage
        .save_integration_state_file(&ownership_path(state.baseline.bridge_id())?, &content)
        .map_err(|_| anyhow::anyhow!("Failed to persist Hue ownership manifest"))
}

pub fn persist_managed_room_mapping(
    storage: &dyn Storage,
    _key: &HubKey,
    state: &mut HueControllerOwnership,
    room: HueManagedRoom,
) -> Result<()> {
    state.record_managed_room(room)?;
    persist_controller_ownership(storage, state)
}

pub fn remove_managed_room_mapping(
    storage: &dyn Storage,
    _key: &HubKey,
    state: &mut HueControllerOwnership,
    rhythm_room_id: &str,
) -> Result<()> {
    state.remove_managed_room(rhythm_room_id)?;
    persist_controller_ownership(storage, state)
}

fn data_array<'a>(resource_type: &str, value: &'a Value) -> Result<&'a [Value]> {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow::anyhow!("Hue {resource_type} response has no data array"))
}

fn object_resource<'a>(
    resource_type: &str,
    value: &'a Value,
) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Hue V1 {resource_type} response is not an object"))
}

fn bridge_id_from_payload(payload: &Value) -> Result<String> {
    let bridges = data_array("bridge", payload)?;
    let ids = bridges
        .iter()
        .filter_map(|bridge| bridge.get("id").and_then(Value::as_str))
        .filter(|id| !id.trim().is_empty())
        .collect::<Vec<_>>();
    match ids.as_slice() {
        [bridge_id] => Ok((*bridge_id).to_string()),
        _ => anyhow::bail!("Hue bridge identity response did not contain exactly one bridge ID"),
    }
}

fn capture_baseline<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
    capture_id: String,
) -> Result<HueBridgeOwnershipBaseline> {
    let inventory = transport
        .get_all_resources(username)
        .context("Failed to capture complete Hue V2 resource inventory")?;
    let inventory = data_array("inventory", &inventory)?;
    let mut v2_resources = BTreeMap::new();
    for resource in inventory {
        let resource_type = resource
            .get("type")
            .and_then(Value::as_str)
            .filter(|resource_type| !resource_type.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Hue V2 inventory item has no resource type"))?;
        v2_resources
            .entry(resource_type.to_string())
            .or_insert_with(|| json!({"data": [], "errors": []}))["data"]
            .as_array_mut()
            .expect("new Hue inventory envelopes have data arrays")
            .push(resource.clone());
    }
    for resource_type in REQUIRED_V2_BASELINE_RESOURCES {
        v2_resources
            .entry((*resource_type).to_string())
            .or_insert_with(|| json!({"data": [], "errors": []}));
    }

    let mut v1_resources = BTreeMap::new();
    for resource_type in REQUIRED_V1_BASELINE_RESOURCES {
        let payload = transport
            .get_v1(username, resource_type)
            .with_context(|| format!("Failed to capture required Hue V1 {resource_type}"))?;
        object_resource(resource_type, &payload)?;
        v1_resources.insert((*resource_type).to_string(), payload);
    }

    let bridge_id = bridge_id_from_payload(
        v2_resources
            .get("bridge")
            .expect("required bridge resource was inserted"),
    )?;
    Ok(HueBridgeOwnershipBaseline {
        schema_version: HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
        capture_id,
        bridge_id,
        capture_scope: HueBaselineCaptureScope::FullV2Inventory,
        v2_resources,
        v1_resources,
    })
}

fn new_capture_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("capture-{millis}")
}

pub fn connected_hue_bridge_id<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
) -> Result<String> {
    bridge_id_from_payload(&transport.get_resources(username, "bridge")?)
}

pub fn load_connected_controller_ownership<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    transport: &H,
    username: &str,
) -> Result<Option<HueControllerOwnership>> {
    let bridge_id = connected_hue_bridge_id(transport, username)?;
    load_controller_ownership(storage, &bridge_id)
}

fn capture_and_persist_new_epoch<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    transport: &H,
    username: &str,
    connected_id: &str,
) -> Result<HueControllerOwnership> {
    let baseline = capture_baseline(transport, username, new_capture_id())?;
    if baseline.bridge_id() != connected_id {
        anyhow::bail!("Hue bridge identity changed while capturing ownership baseline");
    }
    let state = HueControllerOwnership::captured(baseline);
    // This is the required durability barrier before the first bridge write.
    persist_controller_ownership(storage, &state)?;
    let persisted = load_controller_ownership(storage, connected_id)?.ok_or_else(|| {
        anyhow::anyhow!("Hue ownership baseline was not durable after persistence")
    })?;
    if persisted != state {
        anyhow::bail!("Hue ownership baseline failed durable read-back verification");
    }
    Ok(state)
}

fn upgrade_uncleared_baseline<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    transport: &H,
    username: &str,
    connected_id: &str,
    state: HueControllerOwnership,
) -> Result<HueControllerOwnership> {
    if state.phase != HueOwnershipPhase::Captured
        || state.baseline.capture_scope != HueBaselineCaptureScope::ControlPlaneSubset
        || !state.managed_rooms.is_empty()
        || !state.managed_scenes.is_empty()
        || !state.receipts.is_empty()
    {
        return Ok(state);
    }
    let replacement =
        HueControllerOwnership::captured(capture_baseline(transport, username, new_capture_id())?);
    replacement.ensure_bridge_id(connected_id)?;
    persist_controller_ownership(storage, &replacement)?;
    let persisted = load_controller_ownership(storage, connected_id)?.ok_or_else(|| {
        anyhow::anyhow!("Complete Hue ownership baseline was not durable after persistence")
    })?;
    if persisted != replacement {
        anyhow::bail!("Complete Hue ownership baseline failed durable read-back verification");
    }
    Ok(replacement)
}

/// Capture and durably persist the immutable bridge inventory, then suppress
/// Hue automations that can compete with Rhythm light control. Incomplete
/// acquisition manifests are resumed.
/// Untouched subset captures from older builds are upgraded before any write.
pub fn acquire_authoritative_control<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    transport: &H,
    username: &str,
) -> Result<HueControllerOwnership> {
    let connected_id = connected_hue_bridge_id(transport, username)?;
    let state = match load_controller_ownership(storage, &connected_id)? {
        Some(state)
            if matches!(
                state.phase,
                HueOwnershipPhase::Restoring
                    | HueOwnershipPhase::RestoreIncomplete
                    | HueOwnershipPhase::Restored
                    | HueOwnershipPhase::ReleasePending
                    | HueOwnershipPhase::SnapshotRetained
            ) =>
        {
            state.ensure_bridge_id(&connected_id)?;
            anyhow::bail!(
                "Hue controller release is awaiting durable local credential finalization"
            );
        }
        Some(state) => state,
        None => capture_and_persist_new_epoch(storage, transport, username, &connected_id)?,
    };
    let state = upgrade_uncleared_baseline(storage, transport, username, &connected_id, state)?;
    reconcile_authoritative_control(storage, key, transport, username, state)
}

fn references_lighting_target(value: &Value, source_device_id: Option<&str>) -> bool {
    match value {
        Value::Array(values) => values
            .iter()
            .any(|value| references_lighting_target(value, source_device_id)),
        Value::Object(object) => {
            let resource_target = object
                .get("rid")
                .and_then(Value::as_str)
                .filter(|rid| !rid.trim().is_empty())
                .zip(object.get("rtype").and_then(Value::as_str))
                .is_some_and(|(rid, resource_type)| match resource_type {
                    "bridge_home" | "grouped_light" | "light" | "room" | "scene"
                    | "service_group" | "smart_scene" | "zone" => true,
                    "device" => source_device_id != Some(rid),
                    _ => false,
                });
            resource_target
                || object
                    .values()
                    .any(|value| references_lighting_target(value, source_device_id))
        }
        _ => false,
    }
}

fn accessory_source_device_id<'a>(
    configuration: &'a Value,
    dependees: &'a Value,
) -> Option<&'a str> {
    configuration
        .pointer("/device/rid")
        .and_then(Value::as_str)
        .filter(|rid| !rid.trim().is_empty())
        .or_else(|| {
            dependees.as_array()?.iter().find_map(|dependee| {
                let target = dependee.get("target")?;
                if target.get("rtype").and_then(Value::as_str) != Some("device") {
                    return None;
                }
                target
                    .get("rid")
                    .and_then(Value::as_str)
                    .filter(|rid| !rid.trim().is_empty())
            })
        })
}

fn accessory_behavior_targets_lighting(behavior: &Value, resource_id: &str) -> Result<bool> {
    let configuration = behavior
        .get("configuration")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Enabled Hue accessory behavior instance {resource_id} has no object configuration"
            )
        })?;
    let dependees = behavior
        .get("dependees")
        .filter(|value| value.is_array())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Enabled Hue accessory behavior instance {resource_id} has no dependee array"
            )
        })?;
    let source_device_id = accessory_source_device_id(configuration, dependees);
    Ok(references_lighting_target(configuration, source_device_id)
        || references_lighting_target(dependees, source_device_id))
}

fn next_clear_operation<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
    skipped_operation_ids: &BTreeSet<String>,
) -> Result<Option<ControlPlaneOperation>> {
    let behavior_payload = transport.get_resources(username, "behavior_instance")?;
    let script_payload = transport.get_resources(username, "behavior_script")?;
    let behaviors = data_array("behavior_instance", &behavior_payload)?;
    let scripts = data_array("behavior_script", &script_payload)?;
    let mut behavior_ids = Vec::new();
    // Validate the complete live enabled set before selecting the first write.
    // This prevents a malformed or future Hue behavior from being discovered
    // only after Rhythm has partially suppressed otherwise-known automations.
    for behavior in behaviors {
        let enabled = behavior
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow::anyhow!("Hue behavior instance has no boolean enabled state"))?;
        if !enabled {
            continue;
        }
        let resource_id = behavior
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Enabled Hue behavior instance has no ID"))?;
        let script_id = behavior
            .get("script_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Enabled Hue behavior instance has no script ID"))?;
        let matching_scripts = scripts
            .iter()
            .filter(|script| script.get("id").and_then(Value::as_str) == Some(script_id))
            .collect::<Vec<_>>();
        let [script] = matching_scripts.as_slice() else {
            anyhow::bail!(
                "Enabled Hue behavior instance {resource_id} did not resolve to exactly one script"
            );
        };
        let category = script
            .pointer("/metadata/category")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!("Hue behavior script {script_id} has no string metadata category")
            })?;
        match category {
            "automation"
                if !skipped_operation_ids
                    .contains(&format!("clear:v2:behavior_instance:{resource_id}")) =>
            {
                behavior_ids.push(resource_id.to_string());
            }
            "automation" => {}
            // Accessory scripts include both source-only device plumbing and
            // button/sensor programs that target lights or groups. Suppress
            // only the latter. A rejected target suppression fences the
            // complete bridge takeover.
            "accessory"
                if accessory_behavior_targets_lighting(behavior, resource_id)?
                    && !skipped_operation_ids
                        .contains(&format!("clear:v2:behavior_instance:{resource_id}")) =>
            {
                behavior_ids.push(resource_id.to_string());
            }
            // Entertainment playback and source-only/internal behavior
            // instances are not unattended lighting automations.
            "accessory" | "entertainment" | "other" => {}
            _ => anyhow::bail!("Hue behavior script {script_id} has unknown category '{category}'"),
        }
    }
    behavior_ids.sort();
    if let Some(resource_id) = behavior_ids.into_iter().next() {
        return Ok(Some(ControlPlaneOperation {
            operation_id: format!("clear:v2:behavior_instance:{resource_id}"),
            api: "v2",
            action: "disable",
            resource_type: "behavior_instance",
            resource_id,
            body: Some(json!({"enabled": false})),
        }));
    }

    for resource_type in REQUIRED_V1_BASELINE_RESOURCES {
        let payload = transport.get_v1(username, resource_type)?;
        let mut ids = object_resource(resource_type, &payload)?
            .iter()
            .filter(|(_, resource)| {
                resource.get("status").and_then(Value::as_str) != Some("disabled")
            })
            .filter(|(resource_id, _)| {
                !skipped_operation_ids.contains(&format!("clear:v1:{resource_type}:{resource_id}"))
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        if let Some(resource_id) = ids.into_iter().next() {
            return Ok(Some(ControlPlaneOperation {
                operation_id: format!("clear:v1:{resource_type}:{resource_id}"),
                api: "v1",
                action: "disable",
                resource_type,
                resource_id,
                body: Some(json!({"status": "disabled"})),
            }));
        }
    }

    Ok(None)
}

fn validate_v1_update_receipt(operation: &ControlPlaneOperation, receipt: &Value) -> Result<()> {
    let body = operation
        .body
        .as_ref()
        .and_then(Value::as_object)
        .filter(|body| !body.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Hue V1 update operation has no valid body"))?;
    let entries = receipt
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Hue V1 update returned an invalid receipt"))?;
    let success_addresses = entries
        .iter()
        .filter_map(|entry| entry.get("success").and_then(Value::as_object))
        .flat_map(|success| success.keys())
        .collect::<BTreeSet<_>>();
    let confirmed = body.keys().all(|field| {
        let expected = format!(
            "/{}/{}/{}",
            operation.resource_type, operation.resource_id, field
        );
        success_addresses.contains(&expected)
    });
    if confirmed {
        Ok(())
    } else {
        anyhow::bail!("Hue V1 update did not confirm every expected success address")
    }
}

fn execute_operation<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
    operation: &ControlPlaneOperation,
) -> Result<Option<String>> {
    match (operation.api, operation.action) {
        ("v2", "disable") => {
            transport.update_resource(
                username,
                operation.resource_type,
                &operation.resource_id,
                operation
                    .body
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Hue update operation has no body"))?,
            )?;
            Ok(None)
        }
        ("v1", "disable") => {
            let receipt = transport.put_v1(
                username,
                &format!("{}/{}", operation.resource_type, operation.resource_id),
                operation
                    .body
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Hue V1 update operation has no body"))?,
            )?;
            validate_v1_update_receipt(operation, &receipt)?;
            Ok(None)
        }
        ("v2", "restore") => {
            transport.update_resource(
                username,
                operation.resource_type,
                &operation.resource_id,
                operation
                    .body
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Hue restore operation has no body"))?,
            )?;
            Ok(None)
        }
        ("v1", "restore") => {
            let receipt = transport.put_v1(
                username,
                &format!("{}/{}", operation.resource_type, operation.resource_id),
                operation
                    .body
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Hue restore operation has no body"))?,
            )?;
            validate_v1_update_receipt(operation, &receipt)?;
            Ok(None)
        }
        _ => anyhow::bail!(
            "Unsupported Hue ownership operation {} {}",
            operation.api,
            operation.action
        ),
    }
}

fn verify_operation<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
    operation: &ControlPlaneOperation,
) -> Result<()> {
    match (operation.api, operation.action) {
        ("v2", "disable") => {
            let payload = transport.get_resources(username, operation.resource_type)?;
            let resource = data_array(operation.resource_type, &payload)?
                .iter()
                .find(|resource| {
                    resource.get("id").and_then(Value::as_str)
                        == Some(operation.resource_id.as_str())
                });
            match resource {
                None => Ok(()),
                Some(resource)
                    if resource.get("enabled").and_then(Value::as_bool) == Some(false) =>
                {
                    Ok(())
                }
                Some(_) => anyhow::bail!(
                    "Hue V2 {} {} was not disabled by read-back",
                    operation.resource_type,
                    operation.resource_id
                ),
            }
        }
        ("v1", "disable") => {
            let payload = transport.get_v1(username, operation.resource_type)?;
            let resource =
                object_resource(operation.resource_type, &payload)?.get(&operation.resource_id);
            match resource {
                None => Ok(()),
                Some(resource)
                    if resource.get("status").and_then(Value::as_str) == Some("disabled") =>
                {
                    Ok(())
                }
                Some(_) => anyhow::bail!(
                    "Hue V1 {} {} was not disabled by read-back",
                    operation.resource_type,
                    operation.resource_id
                ),
            }
        }
        ("v2", "restore") => {
            let payload = transport.get_resources(username, operation.resource_type)?;
            let resource = data_array(operation.resource_type, &payload)?
                .iter()
                .find(|resource| {
                    resource.get("id").and_then(Value::as_str)
                        == Some(operation.resource_id.as_str())
                });
            match resource {
                Some(resource)
                    if resource.get("enabled")
                        == operation.body.as_ref().and_then(|body| body.get("enabled")) =>
                {
                    Ok(())
                }
                _ => anyhow::bail!(
                    "Hue V2 {} was not restored by read-back",
                    operation.resource_type
                ),
            }
        }
        ("v1", "restore") => {
            let payload = transport.get_v1(username, operation.resource_type)?;
            let resource =
                object_resource(operation.resource_type, &payload)?.get(&operation.resource_id);
            match resource {
                Some(resource)
                    if resource.get("status")
                        == operation.body.as_ref().and_then(|body| body.get("status")) =>
                {
                    Ok(())
                }
                _ => anyhow::bail!(
                    "Hue V1 {} was not restored by read-back",
                    operation.resource_type
                ),
            }
        }
        _ => anyhow::bail!(
            "Unsupported Hue ownership verification {} {}",
            operation.api,
            operation.action
        ),
    }
}

fn baseline_resource_for_operation<'a>(
    state: &'a HueControllerOwnership,
    operation: &ControlPlaneOperation,
) -> Result<&'a Value> {
    match operation.api {
        "v2" => data_array(
            operation.resource_type,
            state
                .baseline
                .v2_resource(operation.resource_type)
                .ok_or_else(|| anyhow::anyhow!("Hue V2 restore baseline is incomplete"))?,
        )?
        .iter()
        .find(|resource| {
            resource.get("id").and_then(Value::as_str) == Some(operation.resource_id.as_str())
        })
        .ok_or_else(|| anyhow::anyhow!("Hue V2 restore baseline resource is missing")),
        "v1" => object_resource(
            operation.resource_type,
            state
                .baseline
                .v1_resource(operation.resource_type)
                .ok_or_else(|| anyhow::anyhow!("Hue V1 restore baseline is incomplete"))?,
        )?
        .get(&operation.resource_id)
        .ok_or_else(|| anyhow::anyhow!("Hue V1 restore baseline resource is missing")),
        _ => anyhow::bail!("Unsupported Hue restore API"),
    }
}

fn live_resource_for_operation<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
    operation: &ControlPlaneOperation,
) -> Result<Value> {
    match operation.api {
        "v2" => {
            let payload = transport.get_resources(username, operation.resource_type)?;
            data_array(operation.resource_type, &payload)?
                .iter()
                .find(|resource| {
                    resource.get("id").and_then(Value::as_str)
                        == Some(operation.resource_id.as_str())
                })
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Hue V2 restore target is missing"))
        }
        "v1" => {
            let payload = transport.get_v1(username, operation.resource_type)?;
            object_resource(operation.resource_type, &payload)?
                .get(&operation.resource_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Hue V1 restore target is missing"))
        }
        _ => anyhow::bail!("Unsupported Hue restore API"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestorePrecondition {
    AlreadyRestored,
    WriteRequired,
}

fn restore_resources_equivalent(api: &str, left: &Value, right: &Value) -> bool {
    if api != "v2" {
        return left == right;
    }
    let mut left = left.clone();
    let mut right = right.clone();
    // A typed V2 collection read may omit the redundant `type` field that is
    // present in the complete-inventory capture. The endpoint and receipt
    // already bind the resource type, so this is not a mutable user field.
    left.as_object_mut().map(|object| object.remove("type"));
    right.as_object_mut().map(|object| object.remove("type"));
    left == right
}

/// Compare the complete live resource with the immutable baseline. The only
/// accepted pre-write delta is the single field Rhythm previously changed.
/// This prevents release from overwriting later edits made in Hue.
fn validate_restore_precondition<H: HueTransport + ?Sized>(
    state: &HueControllerOwnership,
    transport: &H,
    username: &str,
    operation: &ControlPlaneOperation,
) -> Result<RestorePrecondition> {
    let baseline = baseline_resource_for_operation(state, operation)?;
    let live = live_resource_for_operation(transport, username, operation)?;
    if restore_resources_equivalent(operation.api, &live, baseline) {
        return Ok(RestorePrecondition::AlreadyRestored);
    }

    let mut rhythm_written = baseline.clone();
    match operation.api {
        "v2" => rhythm_written["enabled"] = Value::Bool(false),
        "v1" => rhythm_written["status"] = Value::String("disabled".to_string()),
        _ => anyhow::bail!("Unsupported Hue restore API"),
    }
    if restore_resources_equivalent(operation.api, &live, &rhythm_written) {
        Ok(RestorePrecondition::WriteRequired)
    } else {
        anyhow::bail!("Hue automation changed after Rhythm suppression; automatic restore refused")
    }
}

fn restore_operation_for_receipt(
    state: &HueControllerOwnership,
    receipt: &HueOwnershipReceipt,
) -> Result<Option<ControlPlaneOperation>> {
    if receipt.action != "disable" || receipt.status == HueOwnershipReceiptStatus::Unsupported {
        return Ok(None);
    }
    let operation_id = format!(
        "restore:{}:{}:{}",
        receipt.api, receipt.resource_type, receipt.original_resource_id
    );
    if state
        .receipts
        .get(&operation_id)
        .is_some_and(|receipt| receipt.status == HueOwnershipReceiptStatus::Succeeded)
    {
        return Ok(None);
    }

    let baseline_operation = ControlPlaneOperation {
        operation_id: operation_id.clone(),
        api: if receipt.api == "v2" { "v2" } else { "v1" },
        action: "restore",
        resource_type: match receipt.resource_type.as_str() {
            "behavior_instance" => "behavior_instance",
            "rules" => "rules",
            "schedules" => "schedules",
            _ => anyhow::bail!("Unsupported Hue restore resource type"),
        },
        resource_id: receipt.original_resource_id.clone(),
        body: None,
    };
    let baseline = baseline_resource_for_operation(state, &baseline_operation)?;
    let body = match baseline_operation.api {
        "v2" if baseline.get("enabled").and_then(Value::as_bool) == Some(true) => {
            json!({"enabled": true})
        }
        "v1" if baseline.get("status").and_then(Value::as_str) != Some("disabled") => {
            json!({"status": baseline.get("status").cloned().unwrap_or_else(|| json!("enabled"))})
        }
        // Rhythm did not change an originally-disabled resource.
        "v2" | "v1" => return Ok(None),
        _ => anyhow::bail!("Unsupported Hue restore API"),
    };
    Ok(Some(ControlPlaneOperation {
        body: Some(body),
        ..baseline_operation
    }))
}

fn next_restore_operation(state: &HueControllerOwnership) -> Result<Option<ControlPlaneOperation>> {
    let mut suppression_receipts = state
        .receipts
        .values()
        .filter(|receipt| receipt.action == "disable")
        .cloned()
        .collect::<Vec<_>>();
    suppression_receipts.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
    for receipt in suppression_receipts {
        if let Some(operation) = restore_operation_for_receipt(state, &receipt)? {
            return Ok(Some(operation));
        }
    }
    Ok(None)
}

fn run_restore_operation<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
    operation: &ControlPlaneOperation,
) -> Result<()> {
    state.set_receipt(operation, HueOwnershipReceiptStatus::Pending, None);
    persist_controller_ownership(storage, state)?;
    let result = (|| {
        if validate_restore_precondition(state, transport, username, operation)?
            == RestorePrecondition::WriteRequired
        {
            execute_operation(transport, username, operation)?;
        }
        verify_operation(transport, username, operation)?;
        let baseline = baseline_resource_for_operation(state, operation)?;
        let live = live_resource_for_operation(transport, username, operation)?;
        if !restore_resources_equivalent(operation.api, &live, baseline) {
            anyhow::bail!("Hue automation restore did not reproduce the captured baseline");
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            state.set_receipt(operation, HueOwnershipReceiptStatus::Succeeded, None);
            persist_controller_ownership(storage, state)
        }
        Err(error) => {
            state.set_receipt(operation, HueOwnershipReceiptStatus::Failed, None);
            state.phase = HueOwnershipPhase::RestoreIncomplete;
            persist_controller_ownership(storage, state)
                .context("Hue restore failed and its recovery receipt was not durable")?;
            let _ = key;
            Err(error).context("Hue automation could not be safely restored")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JournaledOperationOutcome {
    Succeeded,
    Failed,
}

fn run_journaled_operation<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    _key: &HubKey,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
    operation: &ControlPlaneOperation,
) -> Result<JournaledOperationOutcome> {
    state.set_receipt(operation, HueOwnershipReceiptStatus::Pending, None);
    persist_controller_ownership(storage, state)?;
    match execute_operation(transport, username, operation).and_then(|replacement_id| {
        verify_operation(transport, username, operation)?;
        Ok(replacement_id)
    }) {
        Ok(replacement_id) => {
            state.set_receipt(
                operation,
                HueOwnershipReceiptStatus::Succeeded,
                replacement_id.clone(),
            );
            persist_controller_ownership(storage, state)?;
            Ok(JournaledOperationOutcome::Succeeded)
        }
        Err(error) => {
            state.set_receipt(operation, HueOwnershipReceiptStatus::Failed, None);
            if let Err(persist_error) = persist_controller_ownership(storage, state) {
                let _ = persist_error;
                return Err(error)
                    .context("Hue automation suppression failed and its receipt was not durable");
            }
            warn!(
                target: "hue_authority",
                "Hue {} {} could not be suppressed; controller takeover remains fenced: {}",
                operation.api,
                operation.resource_type,
                error
            );
            Ok(JournaledOperationOutcome::Failed)
        }
    }
}

/// Reassert Hue automation suppression. This is safe to call after interruption
/// and writes only when an automation is observed enabled.
pub fn reconcile_authoritative_control<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    transport: &H,
    username: &str,
    mut state: HueControllerOwnership,
) -> Result<HueControllerOwnership> {
    if matches!(
        state.phase,
        HueOwnershipPhase::Restoring
            | HueOwnershipPhase::RestoreIncomplete
            | HueOwnershipPhase::Restored
            | HueOwnershipPhase::ReleasePending
    ) {
        anyhow::bail!("Hue bridge release has started; takeover cannot be resumed");
    }
    state.ensure_bridge_id(&connected_hue_bridge_id(transport, username)?)?;
    state.phase = HueOwnershipPhase::Clearing;
    persist_controller_ownership(storage, &state)?;

    let skipped_operation_ids = BTreeSet::new();
    loop {
        let operation = match next_clear_operation(transport, username, &skipped_operation_ids) {
            Ok(operation) => operation,
            Err(error) => {
                state.phase = HueOwnershipPhase::ClearIncomplete;
                persist_controller_ownership(storage, &state)?;
                return Err(error).context(
                    "Hue automation inventory was incomplete; controller takeover refused",
                );
            }
        };
        let Some(operation) = operation else {
            break;
        };
        if run_journaled_operation(storage, key, &mut state, transport, username, &operation)?
            == JournaledOperationOutcome::Failed
        {
            state.phase = HueOwnershipPhase::ClearIncomplete;
            persist_controller_ownership(storage, &state)?;
            anyhow::bail!("Hue automation suppression was incomplete; controller takeover refused");
        }
    }

    state.phase = HueOwnershipPhase::Active;
    persist_controller_ownership(storage, &state)?;
    Ok(state)
}

/// Release controller authority by restoring only journaled automation fields
/// whose complete live resource still matches Rhythm's write. Restoration is
/// resumable and refuses to overwrite later Hue/user edits.
pub fn release_authoritative_control<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    _key: &HubKey,
    transport: &H,
    username: &str,
) -> Result<Option<HueControllerOwnership>> {
    let connected_id = connected_hue_bridge_id(transport, username)?;
    let Some(mut state) = load_controller_ownership(storage, &connected_id)? else {
        return Ok(None);
    };
    state.ensure_bridge_id(&connected_id)?;
    if state.phase == HueOwnershipPhase::Restored {
        return Ok(Some(state));
    }
    state.phase = HueOwnershipPhase::Restoring;
    persist_controller_ownership(storage, &state)?;
    while let Some(operation) = next_restore_operation(&state)? {
        run_restore_operation(storage, _key, &mut state, transport, username, &operation)?;
    }
    state.phase = HueOwnershipPhase::Restored;
    persist_controller_ownership(storage, &state)?;
    let persisted = load_controller_ownership(storage, &connected_id)?.ok_or_else(|| {
        anyhow::anyhow!("Restored Hue ownership receipt was not durable after release")
    })?;
    if persisted != state {
        anyhow::bail!("Restored Hue ownership receipt failed durable read-back verification");
    }
    Ok(Some(persisted))
}

/// Delete a verified-restored ownership epoch. Callers use this after the
/// credential-removal durability barrier, or immediately for a room-policy
/// release where credentials intentionally remain connected.
pub fn finalize_released_control(storage: &dyn Storage, bridge_id: &str) -> Result<()> {
    let state = load_controller_ownership(storage, bridge_id)?
        .ok_or_else(|| anyhow::anyhow!("No Hue ownership manifest exists for the bridge"))?;
    if state.phase != HueOwnershipPhase::Restored {
        anyhow::bail!(
            "Refusing to finalize Hue ownership recovery in {:?} phase",
            state.phase
        );
    }
    storage
        .delete_integration_state_file(&ownership_path(bridge_id)?)
        .map_err(|_| anyhow::anyhow!("Failed to delete restored Hue ownership recovery"))?;
    if load_controller_ownership(storage, bridge_id)?.is_some() {
        anyhow::bail!("Restored Hue ownership recovery was not durably deleted");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{HueTransportCall, SpyHueTransport};
    use rhythm_os::hub::{HubCredentials, HubType};
    use rhythm_os::storage::{FileStorage, StoredLightProfiles, StoredLocation, StoredSettings};

    struct AckWithoutPersistStorage;

    impl Storage for AckWithoutPersistStorage {
        fn load_rooms(&self) -> Result<rhythm_core::room::RoomManager> {
            unreachable!()
        }

        fn save_rooms(&self, _rooms: &rhythm_core::room::RoomManager) -> Result<()> {
            unreachable!()
        }

        fn load_light_profiles(&self) -> Result<StoredLightProfiles> {
            unreachable!()
        }

        fn save_light_profiles(&self, _config: &StoredLightProfiles) -> Result<()> {
            unreachable!()
        }

        fn load_location(&self) -> Result<StoredLocation> {
            unreachable!()
        }

        fn save_location(&self, _location: &StoredLocation) -> Result<()> {
            unreachable!()
        }

        fn load_settings(&self) -> Result<StoredSettings> {
            unreachable!()
        }

        fn save_settings(&self, _settings: &StoredSettings) -> Result<()> {
            unreachable!()
        }

        fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>> {
            unreachable!()
        }

        fn save_all_hub_credentials(&self, _credentials: &[HubCredentials]) -> Result<()> {
            unreachable!()
        }

        fn load_hub_registry_for(&self, _key: &HubKey) -> Result<Option<Value>> {
            unreachable!()
        }

        fn save_hub_registry_for(&self, _key: &HubKey, _data: &Value) -> Result<()> {
            unreachable!()
        }
    }

    struct TempStorage {
        path: std::path::PathBuf,
        storage: FileStorage,
    }

    impl TempStorage {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "rhythm-hue-ownership-{name}-{}-{}",
                std::process::id(),
                new_capture_id()
            ));
            let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
            Self { path, storage }
        }
    }

    impl Drop for TempStorage {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn key() -> HubKey {
        HubKey::new(HubType::new(HubType::HUE), "192.0.2.10")
    }

    fn seed_bridge(spy: &SpyHueTransport, smart_scenes: Value) {
        spy.set_resource_response(
            "bridge",
            json!({"data": [{"id": "bridge-1"}], "errors": []}),
        );
        spy.set_resource_response(
            "device",
            json!({"data": [{"id": "device-1"}], "errors": []}),
        );
        spy.set_resource_response("light", json!({"data": [{"id": "light-1"}], "errors": []}));
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [{
                "id": "behavior-1",
                "script_id": "automation-script-1",
                "enabled": true
            }], "errors": []}),
        );
        spy.set_resource_response(
            "behavior_script",
            json!({"data": [{
                "id": "automation-script-1",
                "metadata": {"name": "Automation", "category": "automation"}
            }], "errors": []}),
        );
        spy.set_resource_response(
            "room",
            json!({"data": [{
                "id": "old-room",
                "children": [{"rid": "device-1", "rtype": "device"}],
                "metadata": {"name": "Old room", "archetype": "living_room"}
            }], "errors": []}),
        );
        spy.set_resource_response("zone", json!({"data": [], "errors": []}));
        spy.set_resource_response(
            "scene",
            json!({"data": [{
                "id": "old-scene",
                "metadata": {"name": "Old scene"},
                "group": {"rid": "old-room", "rtype": "room"},
                "actions": []
            }], "errors": []}),
        );
        spy.set_resource_response("smart_scene", json!({"data": smart_scenes, "errors": []}));
        spy.set_v1_response("rules", json!({"1": {"name": "rule", "status": "enabled"}}));
        spy.set_v1_response(
            "schedules",
            json!({"2": {"name": "schedule", "status": "enabled"}}),
        );
    }

    #[test]
    fn capture_is_durable_before_clear_and_baseline_cannot_be_replaced() {
        let temp = TempStorage::new("capture");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_resource_response(
            "button",
            json!({"data": [{"id": "button-1", "metadata": {"name": "Dimmer"}}]}),
        );

        let state = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        assert_eq!(state.phase, HueOwnershipPhase::Active);
        assert_eq!(state.baseline.bridge_id(), "bridge-1");
        assert_eq!(
            state.baseline.capture_scope(),
            HueBaselineCaptureScope::FullV2Inventory
        );
        assert_eq!(
            state.baseline.v2_resource("button").unwrap()["data"][0]["id"],
            "button-1"
        );
        assert!(load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .is_some());

        let calls = spy.calls();
        assert!(calls.iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource {
                resource_type,
                resource_id,
                body,
            } if resource_type == "behavior_instance"
                && resource_id == "behavior-1"
                && body == &json!({"enabled": false})
        )));
        assert!(!calls
            .iter()
            .any(|call| matches!(call, HueTransportCall::DeleteResource { .. })));
        assert_eq!(
            spy.get_resources("user", "room").unwrap()["data"][0]["id"],
            "old-room"
        );

        let mut altered = state.clone();
        altered.baseline.capture_id = "replacement".to_string();
        assert!(persist_controller_ownership(&temp.storage, &altered)
            .unwrap_err()
            .to_string()
            .contains("immutable"));
    }

    #[test]
    fn untouched_subset_capture_is_upgraded_before_first_bridge_write() {
        let temp = TempStorage::new("upgrade-subset");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_resource_response(
            "button",
            json!({"data": [{"id": "button-1", "metadata": {"name": "Dimmer"}}]}),
        );

        let mut legacy = capture_baseline(&spy, "user", "legacy-capture".to_string()).unwrap();
        legacy.capture_scope = HueBaselineCaptureScope::ControlPlaneSubset;
        legacy.v2_resources.remove("button");
        persist_controller_ownership(&temp.storage, &HueControllerOwnership::captured(legacy))
            .unwrap();
        spy.reset();

        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert_eq!(
            active.baseline.capture_scope(),
            HueBaselineCaptureScope::FullV2Inventory
        );
        assert_ne!(active.baseline.capture_id(), "legacy-capture");
        assert_eq!(
            active.baseline.v2_resource("button").unwrap()["data"][0]["id"],
            "button-1"
        );
        let first_mutation = spy.calls().iter().position(|call| {
            matches!(
                call,
                HueTransportCall::UpdateResource { .. }
                    | HueTransportCall::DeleteResource { .. }
                    | HueTransportCall::PutV1 { .. }
            )
        });
        let inventory_capture = spy.calls().iter().position(|call| {
            matches!(
                call,
                HueTransportCall::GetResources { resource_type } if resource_type == "*"
            )
        });
        assert!(inventory_capture.unwrap() < first_mutation.unwrap());
    }

    #[test]
    fn capture_requires_exact_durable_readback_before_any_bridge_mutation() {
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));

        let error = acquire_authoritative_control(&AckWithoutPersistStorage, &key(), &spy, "user")
            .expect_err("an acknowledged but missing baseline must block takeover");

        assert!(error.to_string().contains("not durable"));
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { .. }
                | HueTransportCall::DeleteResource { .. }
                | HueTransportCall::PutV1 { .. }
        )));
    }

    #[test]
    fn takeover_fails_closed_when_suppression_readback_is_not_observed() {
        let temp = TempStorage::new("clear-readback");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_ignore_resource_mutations(true);

        acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("unverified suppression must not grant authority");

        let persisted = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(persisted.phase, HueOwnershipPhase::ClearIncomplete);
        assert!(persisted
            .receipts()
            .any(|receipt| { receipt.status == HueOwnershipReceiptStatus::Failed }));
        assert_eq!(
            spy.calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    HueTransportCall::UpdateResource {
                        resource_type,
                        ..
                    } if resource_type == "behavior_instance"
                ))
                .count(),
            1
        );
    }

    #[test]
    fn takeover_stops_when_a_v2_disable_is_rejected() {
        let temp = TempStorage::new("skip-rejected-mutation");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_fail_resource_update("behavior_instance", "behavior-1");

        acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("a rejected mutation must not grant authority");

        let state = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(state.phase, HueOwnershipPhase::ClearIncomplete);
        assert!(state.receipts().any(|receipt| {
            receipt.resource_type == "behavior_instance"
                && receipt.status == HueOwnershipReceiptStatus::Failed
        }));
        assert!(!spy
            .calls()
            .iter()
            .any(|call| matches!(call, HueTransportCall::DeleteResource { .. })));
        assert_eq!(
            spy.get_v1("user", "rules").unwrap()["1"]["status"],
            "enabled"
        );
        assert_eq!(
            spy.get_v1("user", "schedules").unwrap()["2"]["status"],
            "enabled"
        );
        assert_eq!(
            spy.calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    HueTransportCall::UpdateResource { resource_type, .. }
                        if resource_type == "behavior_instance"
                ))
                .count(),
            1
        );
    }

    #[test]
    fn takeover_preserves_source_only_and_known_non_automatic_behaviors() {
        let temp = TempStorage::new("accessory-behavior");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [
                {
                    "id": "accessory-behavior-1",
                    "script_id": "accessory-script-1",
                    "enabled": true,
                    "configuration": {
                        "device": {"rid": "button-1", "rtype": "device"}
                    },
                    "dependees": [{
                        "target": {"rid": "button-1", "rtype": "device"}
                    }]
                },
                {
                    "id": "entertainment-behavior-1",
                    "script_id": "entertainment-script-1",
                    "enabled": true
                },
                {
                    "id": "other-behavior-1",
                    "script_id": "other-script-1",
                    "enabled": true
                }
            ], "errors": []}),
        );
        spy.set_resource_response(
            "behavior_script",
            json!({"data": [
                {
                    "id": "accessory-script-1",
                    "metadata": {"name": "Dimmer", "category": "accessory"}
                },
                {
                    "id": "entertainment-script-1",
                    "metadata": {"name": "Entertainment", "category": "entertainment"}
                },
                {
                    "id": "other-script-1",
                    "metadata": {"name": "Internal", "category": "other"}
                }
            ], "errors": []}),
        );
        spy.set_fail_resource_update("behavior_instance", "accessory-behavior-1");

        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource {
                resource_type,
                resource_id,
                ..
            } if resource_type == "behavior_instance"
                && resource_id == "accessory-behavior-1"
        )));
        assert!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"]
                .as_array()
                .unwrap()
                .iter()
                .all(|behavior| behavior["enabled"] == true)
        );
    }

    #[test]
    fn takeover_preserves_dependee_only_accessory_source() {
        let temp = TempStorage::new("dependee-only-accessory-source");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [{
                "id": "accessory-behavior-1",
                "script_id": "accessory-script-1",
                "enabled": true,
                "configuration": {},
                "dependees": [{
                    "target": {"rid": "button-1", "rtype": "device"}
                }]
            }], "errors": []}),
        );
        spy.set_resource_response(
            "behavior_script",
            json!({"data": [{
                "id": "accessory-script-1",
                "metadata": {"name": "Dimmer", "category": "accessory"}
            }], "errors": []}),
        );
        spy.set_fail_resource_update("behavior_instance", "accessory-behavior-1");

        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert_eq!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"][0]["enabled"],
            true
        );
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource {
                resource_type,
                resource_id,
                ..
            } if resource_type == "behavior_instance"
                && resource_id == "accessory-behavior-1"
        )));
    }

    #[test]
    fn takeover_stops_after_rejected_accessory_suppression() {
        let temp = TempStorage::new("effectful-accessory-behavior");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [{
                "id": "accessory-behavior-1",
                "script_id": "accessory-script-1",
                "enabled": true,
                "configuration": {
                    "device": {"rid": "button-1", "rtype": "device"},
                    "buttons": {
                        "button-1": {
                            "where": [{
                                "group": {"rid": "room-1", "rtype": "room"}
                            }]
                        }
                    }
                },
                "dependees": [
                    {"target": {"rid": "button-1", "rtype": "device"}},
                    {"target": {"rid": "room-1", "rtype": "room"}}
                ]
            }], "errors": []}),
        );
        spy.set_resource_response(
            "behavior_script",
            json!({"data": [{
                "id": "accessory-script-1",
                "metadata": {"name": "Dimmer", "category": "accessory"}
            }], "errors": []}),
        );
        spy.set_fail_resource_update("behavior_instance", "accessory-behavior-1");

        acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("a rejected accessory mutation must not grant authority");

        assert_eq!(
            load_controller_ownership(&temp.storage, "bridge-1")
                .unwrap()
                .unwrap()
                .phase,
            HueOwnershipPhase::ClearIncomplete
        );
        assert_eq!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"][0]["enabled"],
            true
        );
        assert!(spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource {
                resource_type,
                resource_id,
                ..
            } if resource_type == "behavior_instance" && resource_id == "accessory-behavior-1"
        )));
        assert_eq!(
            spy.get_v1("user", "rules").unwrap()["1"]["status"],
            "enabled"
        );
        assert_eq!(
            spy.get_v1("user", "schedules").unwrap()["2"]["status"],
            "enabled"
        );
    }

    #[test]
    fn unclassified_enabled_behavior_fences_takeover_without_mutation() {
        let temp = TempStorage::new("unclassified-behavior");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [
                {
                    "id": "known-automation",
                    "script_id": "automation-script-1",
                    "enabled": true
                },
                {
                    "id": "unclassified-behavior",
                    "script_id": "missing-script",
                    "enabled": true
                }
            ], "errors": []}),
        );

        acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("an unresolved enabled behavior must fence takeover");
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { .. } | HueTransportCall::PutV1 { .. }
        )));
        assert_eq!(
            load_controller_ownership(&temp.storage, "bridge-1")
                .unwrap()
                .unwrap()
                .phase,
            HueOwnershipPhase::ClearIncomplete
        );
    }

    #[test]
    fn malformed_accessory_payload_fences_takeover_without_mutation() {
        let temp = TempStorage::new("malformed-accessory-behavior");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [
                {
                    "id": "known-automation",
                    "script_id": "automation-script-1",
                    "enabled": true
                },
                {
                    "id": "malformed-accessory",
                    "script_id": "accessory-script-1",
                    "enabled": true
                }
            ], "errors": []}),
        );
        spy.set_resource_response(
            "behavior_script",
            json!({"data": [
                {
                    "id": "automation-script-1",
                    "metadata": {"name": "Automation", "category": "automation"}
                },
                {
                    "id": "accessory-script-1",
                    "metadata": {"name": "Accessory", "category": "accessory"}
                }
            ], "errors": []}),
        );

        acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("a malformed enabled accessory must fence takeover");
        let behaviors = spy.get_resources("user", "behavior_instance").unwrap();
        assert_eq!(behaviors["data"][0]["enabled"], true);
        assert_eq!(behaviors["data"][1]["enabled"], true);
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { .. } | HueTransportCall::PutV1 { .. }
        )));
    }

    #[test]
    fn unknown_behavior_category_fences_takeover_without_mutation() {
        let temp = TempStorage::new("unknown-behavior-category");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_resource_response(
            "behavior_script",
            json!({"data": [{
                "id": "automation-script-1",
                "metadata": {"name": "Future", "category": "future_category"}
            }], "errors": []}),
        );

        acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("an unknown enabled category must fence takeover");
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { .. } | HueTransportCall::PutV1 { .. }
        )));
    }

    #[test]
    fn disabled_unclassified_behavior_does_not_block_known_automation_suppression() {
        let temp = TempStorage::new("disabled-unclassified-behavior");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        let mut behaviors = spy.get_resources("user", "behavior_instance").unwrap()["data"]
            .as_array()
            .unwrap()
            .clone();
        behaviors.push(json!({
            "id": "disabled-unclassified",
            "script_id": "missing-script",
            "enabled": false
        }));
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": behaviors, "errors": []}),
        );

        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert!(spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource {
                resource_type,
                resource_id,
                ..
            } if resource_type == "behavior_instance" && resource_id == "behavior-1"
        )));
    }

    #[test]
    fn takeover_retries_an_unsupported_receipt_persisted_by_an_older_build() {
        let temp = TempStorage::new("resume-old-failure");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        let mut interrupted = HueControllerOwnership::captured(
            capture_baseline(&spy, "user", "old-build-capture".to_string()).unwrap(),
        );
        interrupted.phase = HueOwnershipPhase::ClearIncomplete;
        interrupted.set_receipt(
            &ControlPlaneOperation {
                operation_id: "clear:v2:behavior_instance:behavior-1".to_string(),
                api: "v2",
                action: "disable",
                resource_type: "behavior_instance",
                resource_id: "behavior-1".to_string(),
                body: Some(json!({"enabled": false})),
            },
            HueOwnershipReceiptStatus::Unsupported,
            None,
        );
        persist_controller_ownership(&temp.storage, &interrupted).unwrap();
        spy.reset();

        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert!(active.receipts().any(|receipt| {
            receipt.resource_type == "behavior_instance"
                && receipt.status == HueOwnershipReceiptStatus::Succeeded
        }));
        assert!(spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { resource_type, .. }
                if resource_type == "behavior_instance"
        )));
        assert!(!spy
            .calls()
            .iter()
            .any(|call| matches!(call, HueTransportCall::DeleteResource { .. })));
    }

    #[test]
    fn takeover_retires_an_old_unsupported_accessory_receipt_without_retrying_it() {
        let temp = TempStorage::new("retire-old-accessory-failure");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [{
                "id": "behavior-1",
                "script_id": "accessory-script-1",
                "enabled": true,
                "configuration": {
                    "device": {"rid": "button-1", "rtype": "device"}
                },
                "dependees": [{
                    "target": {"rid": "button-1", "rtype": "device"}
                }]
            }], "errors": []}),
        );
        spy.set_resource_response(
            "behavior_script",
            json!({"data": [{
                "id": "accessory-script-1",
                "metadata": {"name": "Dimmer", "category": "accessory"}
            }], "errors": []}),
        );
        let mut interrupted = HueControllerOwnership::captured(
            capture_baseline(&spy, "user", "old-build-capture".to_string()).unwrap(),
        );
        interrupted.phase = HueOwnershipPhase::ClearIncomplete;
        interrupted.set_receipt(
            &ControlPlaneOperation {
                operation_id: "clear:v2:behavior_instance:behavior-1".to_string(),
                api: "v2",
                action: "disable",
                resource_type: "behavior_instance",
                resource_id: "behavior-1".to_string(),
                body: Some(json!({"enabled": false})),
            },
            HueOwnershipReceiptStatus::Unsupported,
            None,
        );
        persist_controller_ownership(&temp.storage, &interrupted).unwrap();
        spy.reset();

        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert!(active.receipts().any(|receipt| {
            receipt.original_resource_id == "behavior-1"
                && receipt.status == HueOwnershipReceiptStatus::Unsupported
        }));
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource {
                resource_type,
                resource_id,
                ..
            } if resource_type == "behavior_instance" && resource_id == "behavior-1"
        )));
    }

    #[test]
    fn v1_disable_requires_disabled_readback() {
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        let operation = ControlPlaneOperation {
            operation_id: "clear:v1:rules:1".to_string(),
            api: "v1",
            action: "disable",
            resource_type: "rules",
            resource_id: "1".to_string(),
            body: Some(json!({"status": "disabled"})),
        };

        let error = verify_operation(&spy, "user", &operation)
            .expect_err("an enabled read-back cannot confirm suppression");
        assert!(error.to_string().contains("was not disabled by read-back"));

        spy.set_v1_response("rules", json!({"1": {"status": "disabled"}}));
        verify_operation(&spy, "user", &operation).unwrap();
    }

    #[test]
    fn release_restores_journaled_fields_before_starting_a_fresh_epoch() {
        let temp = TempStorage::new("retained-reacquire");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        spy.reset();
        let restored = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        assert_eq!(restored.phase, HueOwnershipPhase::Restored);
        assert_eq!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"][0]["enabled"],
            true
        );
        assert_eq!(
            spy.get_v1("user", "rules").unwrap()["1"]["status"],
            "enabled"
        );
        assert_eq!(
            spy.get_v1("user", "schedules").unwrap()["2"]["status"],
            "enabled"
        );
        spy.reset();

        let error = acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("stale credentials must not reverse a pending release");

        assert!(error.to_string().contains("credential finalization"));
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::CreateResource { .. }
                | HueTransportCall::UpdateResource { .. }
                | HueTransportCall::DeleteResource { .. }
        )));

        finalize_released_control(&temp.storage, "bridge-1").unwrap();
        assert!(load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .is_none());

        let reacquired =
            acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        assert_eq!(reacquired.phase, HueOwnershipPhase::Active);
        assert!(!reacquired.baseline.capture_id().is_empty());
    }

    #[test]
    fn release_refuses_to_overwrite_a_later_hue_edit() {
        let temp = TempStorage::new("restore-compare-and-set");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        spy.update_resource(
            "user",
            "behavior_instance",
            "behavior-1",
            &json!({"metadata": {"name": "Edited in Hue"}}),
        )
        .unwrap();
        spy.reset();

        let error = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("a later Hue edit must block automatic restoration");

        assert!(error.to_string().contains("could not be safely restored"));
        assert_eq!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"][0]["enabled"],
            false
        );
        assert_eq!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"][0]["metadata"]["name"],
            "Edited in Hue"
        );
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource {
                resource_type,
                body,
                ..
            } if resource_type == "behavior_instance" && body == &json!({"enabled": true})
        )));
        assert_eq!(
            load_controller_ownership(&temp.storage, "bridge-1")
                .unwrap()
                .unwrap()
                .phase,
            HueOwnershipPhase::RestoreIncomplete
        );
    }

    #[test]
    fn malformed_v1_receipts_fence_takeover() {
        let temp = TempStorage::new("v1-receipt");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        let private_id = "private-unrelated-rule";
        spy.set_v1_write_response_override(Some(json!([{
            "success": {format!("/rules/{private_id}/status"): "disabled"}
        }])));

        acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("an unverified V1 receipt must fence takeover");
        let state = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(state.phase, HueOwnershipPhase::ClearIncomplete);
        let v1_receipts = state
            .receipts()
            .filter(|receipt| receipt.api == "v1")
            .collect::<Vec<_>>();
        assert_eq!(v1_receipts.len(), 1);
        assert!(v1_receipts
            .iter()
            .all(|receipt| receipt.status == HueOwnershipReceiptStatus::Failed));
        assert!(v1_receipts
            .iter()
            .all(|receipt| !receipt.operation_id.contains(private_id)));
    }

    #[test]
    fn automations_are_disabled_while_hue_resources_are_preserved() {
        let temp = TempStorage::new("smart-scene");
        let spy = SpyHueTransport::new();
        let smart_scenes = json!([{
            "id": "smart-1",
            "metadata": {"name": "Wake"},
            "group": {"rid": "old-room", "rtype": "room"}
        }]);
        seed_bridge(&spy, smart_scenes.clone());
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [{
                "id": "behavior-1",
                "script_id": "automation-script-1",
                "enabled": true,
                "configuration": {
                    "where": {"rid": "old-room", "rtype": "room"},
                    "actions": [
                        {"rid": "old-scene", "rtype": "scene"},
                        {"rid": "smart-1", "rtype": "smart_scene"}
                    ]
                }
            }], "errors": []}),
        );
        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert_eq!(
            active.baseline.v2_resource("smart_scene").unwrap()["data"][0]["id"],
            "smart-1"
        );
        assert!(
            active.baseline.v2_resource("behavior_instance").unwrap()["data"][0]["configuration"]
                .is_object()
        );
        assert_eq!(
            spy.get_resources("user", "smart_scene").unwrap()["data"],
            smart_scenes
        );
        assert_eq!(
            spy.get_resources("user", "scene").unwrap()["data"][0]["id"],
            "old-scene"
        );
        assert_eq!(
            spy.get_resources("user", "room").unwrap()["data"][0]["id"],
            "old-room"
        );
        assert_eq!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"][0]["enabled"],
            false
        );
        assert_eq!(
            spy.get_v1("user", "rules").unwrap()["1"]["status"],
            "disabled"
        );
        assert_eq!(
            spy.get_v1("user", "schedules").unwrap()["2"]["status"],
            "disabled"
        );
        assert!(!spy
            .calls()
            .iter()
            .any(|call| matches!(call, HueTransportCall::DeleteResource { .. })));

        spy.reset();
        let released = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        assert_eq!(released.phase, HueOwnershipPhase::Restored);
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::CreateResource { resource_type, .. }
                | HueTransportCall::UpdateResource { resource_type, .. }
                | HueTransportCall::DeleteResource { resource_type, .. }
                if resource_type == "smart_scene"
        )));
        finalize_released_control(&temp.storage, "bridge-1").unwrap();
        assert!(load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .is_none());
    }

    #[test]
    fn authoritative_reconcile_preserves_all_hue_resource_definitions() {
        let temp = TempStorage::new("native-managed-scenes");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        let mut active =
            acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        active
            .record_managed_room(HueManagedRoom {
                rhythm_room_id: "rhythm-room".to_string(),
                hue_room_id: "managed-room".to_string(),
                grouped_light_id: "managed-group".to_string(),
            })
            .unwrap();
        active
            .record_managed_scene(HueManagedScene {
                rhythm_room_id: "rhythm-room".to_string(),
                rhythm_scene_id: "rhythm-scene".to_string(),
                hue_room_id: "managed-room".to_string(),
                hue_scene_id: "managed-projection".to_string(),
                fingerprint: "fingerprint".to_string(),
                ephemeral: false,
            })
            .unwrap();
        persist_controller_ownership(&temp.storage, &active).unwrap();
        spy.set_resource_response(
            "room",
            json!({"data": [{"id": "managed-room"}], "errors": []}),
        );
        spy.set_resource_response(
            "scene",
            json!({"data": [
                {
                    "id": "native-managed",
                    "group": {"rtype": "room", "rid": "managed-room"}
                },
                {
                    "id": "managed-projection",
                    "group": {"rtype": "room", "rid": "managed-room"}
                },
                {
                    "id": "native-outside",
                    "group": {"rtype": "room", "rid": "outside-room"}
                }
            ], "errors": []}),
        );
        spy.set_resource_response(
            "smart_scene",
            json!({"data": [{
                "id": "smart-managed",
                "group": {"rtype": "room", "rid": "managed-room"}
            }], "errors": []}),
        );
        spy.reset();

        reconcile_authoritative_control(&temp.storage, &key(), &spy, "user", active).unwrap();

        let scene_resources = spy.get_resources("user", "scene").unwrap();
        let scene_ids = scene_resources["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|scene| scene.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            scene_ids,
            BTreeSet::from([
                "managed-projection".to_string(),
                "native-managed".to_string(),
                "native-outside".to_string(),
            ])
        );
        assert_eq!(
            spy.get_resources("user", "smart_scene").unwrap()["data"][0]["id"],
            "smart-managed"
        );
        assert!(!spy
            .calls()
            .iter()
            .any(|call| matches!(call, HueTransportCall::DeleteResource { .. })));
    }

    #[test]
    fn bridge_identity_mismatch_blocks_mutation() {
        let temp = TempStorage::new("identity");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        spy.reset();
        spy.set_resource_response(
            "bridge",
            json!({"data": [{"id": "other-bridge"}], "errors": []}),
        );

        assert!(reconcile_authoritative_control(
            &temp.storage,
            &key(),
            &spy,
            "user",
            load_controller_ownership(&temp.storage, "bridge-1")
                .unwrap()
                .unwrap(),
        )
        .unwrap_err()
        .to_string()
        .contains("identity mismatch"));
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { .. } | HueTransportCall::DeleteResource { .. }
        )));
    }

    #[test]
    fn ownership_manifest_survives_bridge_address_change() {
        let temp = TempStorage::new("address-change");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        let old_key = key();
        acquire_authoritative_control(&temp.storage, &old_key, &spy, "user").unwrap();

        let new_key = HubKey::new(HubType::new(HubType::HUE), "192.0.2.99");
        let resumed = reconcile_authoritative_control(
            &temp.storage,
            &new_key,
            &spy,
            "user",
            load_controller_ownership(&temp.storage, "bridge-1")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(resumed.phase, HueOwnershipPhase::Active);
        assert_eq!(
            hue_controller_ownership_path("bridge-1").unwrap(),
            "hue/controller-ownership/by-bridge/bridge-1.json"
        );
        assert!(!hue_controller_ownership_path("bridge-1")
            .unwrap()
            .contains("192.0.2"));
    }

    #[test]
    fn releasing_an_unowned_bridge_is_idempotent() {
        let temp = TempStorage::new("unowned-release");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));

        assert!(
            release_authoritative_control(&temp.storage, &key(), &spy, "user")
                .unwrap()
                .is_none()
        );
        assert!(
            release_authoritative_control(&temp.storage, &key(), &spy, "user")
                .unwrap()
                .is_none()
        );
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { .. }
                | HueTransportCall::DeleteResource { .. }
                | HueTransportCall::PutV1 { .. }
        )));
    }
}
