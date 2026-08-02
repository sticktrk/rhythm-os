//! Durable Hue bridge inventory capture and authoritative takeover.
//!
//! Rhythm treats a paired Hue bridge as a Zigbee control plane. Before the
//! first mutation this module captures a complete Hue V2 inventory plus the V1
//! automation collections and durably persists that bridge-scoped baseline.
//! Automated restoration is intentionally deferred; releasing authority keeps
//! the snapshot instead of writing it back to the bridge.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::storage::Storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::warn;

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
const CLEAR_DELETE_ORDER: &[&str] = &["smart_scene", "scene", "zone", "room"];

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

/// Capture and durably persist the immutable bridge inventory, then clear the
/// competing Hue control plane. Incomplete acquisition manifests are resumed.
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
                HueOwnershipPhase::Restored | HueOwnershipPhase::ReleasePending
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

fn next_clear_operation<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
    managed_room_ids: &BTreeSet<String>,
    managed_scene_ids: &BTreeSet<String>,
    skipped_operation_ids: &BTreeSet<String>,
) -> Result<Option<ControlPlaneOperation>> {
    let behavior_payload = transport.get_resources(username, "behavior_instance")?;
    let mut behavior_ids = data_array("behavior_instance", &behavior_payload)?
        .iter()
        .filter(|resource| resource.get("enabled").and_then(Value::as_bool) == Some(true))
        .filter_map(|resource| resource.get("id").and_then(Value::as_str))
        .filter(|resource_id| {
            !skipped_operation_ids.contains(&format!("clear:v2:behavior_instance:{resource_id}"))
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
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

    for resource_type in CLEAR_DELETE_ORDER {
        let payload = transport.get_resources(username, resource_type)?;
        let mut ids = data_array(resource_type, &payload)?
            .iter()
            .filter_map(|resource| resource.get("id").and_then(Value::as_str))
            .filter(|resource_id| {
                !skipped_operation_ids.contains(&format!("clear:v2:{resource_type}:{resource_id}"))
            })
            .filter(|resource_id| {
                !((*resource_type == "room" && managed_room_ids.contains(*resource_id))
                    || (*resource_type == "scene" && managed_scene_ids.contains(*resource_id)))
            })
            .map(str::to_string)
            .collect::<Vec<_>>();
        ids.sort();
        if let Some(resource_id) = ids.into_iter().next() {
            return Ok(Some(ControlPlaneOperation {
                operation_id: format!("clear:v2:{resource_type}:{resource_id}"),
                api: "v2",
                action: "delete",
                resource_type,
                resource_id,
                body: None,
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
        ("v2", "delete") => {
            transport.delete_resource(username, operation.resource_type, &operation.resource_id)?;
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
        _ => anyhow::bail!(
            "Unsupported Hue ownership operation {} {}",
            operation.api,
            operation.action
        ),
    }
}

fn run_journaled_operation<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    _key: &HubKey,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
    operation: &ControlPlaneOperation,
) -> Result<bool> {
    state.set_receipt(operation, HueOwnershipReceiptStatus::Pending, None);
    persist_controller_ownership(storage, state)?;
    match execute_operation(transport, username, operation) {
        Ok(replacement_id) => {
            state.set_receipt(
                operation,
                HueOwnershipReceiptStatus::Succeeded,
                replacement_id.clone(),
            );
            persist_controller_ownership(storage, state)?;
            Ok(true)
        }
        Err(error) => {
            state.set_receipt(operation, HueOwnershipReceiptStatus::Unsupported, None);
            if let Err(persist_error) = persist_controller_ownership(storage, state) {
                let _ = persist_error;
                return Err(error)
                    .context("Hue operation was skipped but its receipt could not be persisted");
            }
            warn!(
                target: "hue_authority",
                api = operation.api,
                action = operation.action,
                resource_type = operation.resource_type,
                error = %error,
                "Skipping unsupported Hue control-plane mutation and continuing authority reconciliation"
            );
            Ok(false)
        }
    }
}

/// Reassert Rhythm ownership. This is safe to call after interruption and also
/// clears control-plane resources created out of band while ownership is active.
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
    // Older builds stopped authority reconciliation after a single remote
    // mutation failure. Treat those durable failure receipts as already
    // skipped so an upgrade can resume the rest of the control-plane pass.
    for receipt in state.receipts.values_mut() {
        if receipt.status == HueOwnershipReceiptStatus::Failed {
            receipt.status = HueOwnershipReceiptStatus::Unsupported;
        }
    }
    state.phase = HueOwnershipPhase::Clearing;
    persist_controller_ownership(storage, &state)?;

    let mut skipped_operation_ids = state
        .receipts
        .values()
        .filter(|receipt| receipt.status == HueOwnershipReceiptStatus::Unsupported)
        .map(|receipt| receipt.operation_id.clone())
        .collect::<BTreeSet<_>>();
    let mut previous_operation_id: Option<String> = None;
    loop {
        let managed_room_ids = state.managed_room_ids();
        let managed_scene_ids = state
            .managed_scenes()
            .values()
            .map(|scene| scene.hue_scene_id.clone())
            .collect::<BTreeSet<_>>();
        let operation = match next_clear_operation(
            transport,
            username,
            &managed_room_ids,
            &managed_scene_ids,
            &skipped_operation_ids,
        ) {
            Ok(operation) => operation,
            Err(error) => {
                state.phase = HueOwnershipPhase::ClearIncomplete;
                persist_controller_ownership(storage, &state)?;
                return Err(error.context("Failed to inspect the Hue control plane while clearing"));
            }
        };
        let Some(operation) = operation else {
            break;
        };
        if previous_operation_id.as_deref() == Some(operation.operation_id.as_str()) {
            state.set_receipt(&operation, HueOwnershipReceiptStatus::Unsupported, None);
            persist_controller_ownership(storage, &state)?;
            warn!(
                target: "hue_authority",
                api = operation.api,
                action = operation.action,
                resource_type = operation.resource_type,
                "Skipping Hue control-plane mutation whose acknowledged effect was not observed"
            );
            skipped_operation_ids.insert(operation.operation_id.clone());
            previous_operation_id = None;
            continue;
        }
        previous_operation_id = Some(operation.operation_id.clone());
        if !run_journaled_operation(storage, key, &mut state, transport, username, &operation)? {
            skipped_operation_ids.insert(operation.operation_id.clone());
            previous_operation_id = None;
        }
    }

    state.phase = HueOwnershipPhase::Active;
    persist_controller_ownership(storage, &state)?;
    Ok(state)
}

/// Release controller authority without restoring the captured bridge state.
///
/// This transition performs no Hue control-plane writes. The complete
/// pre-takeover snapshot, clear receipts, and managed-resource mappings remain
/// durable for a future explicit recovery implementation.
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
    if state.phase != HueOwnershipPhase::ReleasePending {
        state.phase = HueOwnershipPhase::ReleasePending;
        persist_controller_ownership(storage, &state)?;
    }
    let persisted = load_controller_ownership(storage, &connected_id)?.ok_or_else(|| {
        anyhow::anyhow!("Retained Hue ownership snapshot was not durable after release")
    })?;
    if persisted != state {
        anyhow::bail!("Retained Hue ownership snapshot failed durable read-back verification");
    }
    Ok(Some(persisted))
}

/// Finalize local credential teardown while deliberately retaining the Hue
/// snapshot. Legacy `restored` manifests are migrated to the retained phase.
pub fn finalize_released_control(storage: &dyn Storage, bridge_id: &str) -> Result<()> {
    let mut state = load_controller_ownership(storage, bridge_id)?
        .ok_or_else(|| anyhow::anyhow!("No Hue ownership manifest exists for the bridge"))?;
    if matches!(
        state.phase,
        HueOwnershipPhase::Restored | HueOwnershipPhase::ReleasePending
    ) {
        state.phase = HueOwnershipPhase::SnapshotRetained;
        persist_controller_ownership(storage, &state)?;
    }
    if state.phase != HueOwnershipPhase::SnapshotRetained {
        anyhow::bail!(
            "Refusing to finalize Hue ownership snapshot in {:?} phase",
            state.phase
        );
    }
    let persisted = load_controller_ownership(storage, bridge_id)?.ok_or_else(|| {
        anyhow::anyhow!("Retained Hue ownership snapshot disappeared during finalization")
    })?;
    if persisted.phase != HueOwnershipPhase::SnapshotRetained {
        anyhow::bail!("Hue ownership snapshot was not durably retained");
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
            json!({"data": [{"id": "behavior-1", "enabled": true}], "errors": []}),
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
        assert!(calls.iter().any(|call| matches!(
            call,
            HueTransportCall::DeleteResource { resource_type, resource_id }
                if resource_type == "room" && resource_id == "old-room"
        )));

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
    fn takeover_skips_a_confirmed_clear_that_is_not_observed() {
        let temp = TempStorage::new("clear-readback");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_ignore_resource_mutations(true);

        let state = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();

        assert_eq!(state.phase, HueOwnershipPhase::Active);
        let persisted = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(persisted.phase, HueOwnershipPhase::Active);
        assert!(persisted.receipts().any(|receipt| {
            receipt.resource_type == "behavior_instance"
                && receipt.status == HueOwnershipReceiptStatus::Unsupported
        }));
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
    fn takeover_skips_one_rejected_mutation_and_continues_clearing() {
        let temp = TempStorage::new("skip-rejected-mutation");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_fail_resource_update("behavior_instance", "behavior-1");

        let state = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();

        assert_eq!(state.phase, HueOwnershipPhase::Active);
        assert!(state.receipts().any(|receipt| {
            receipt.resource_type == "behavior_instance"
                && receipt.status == HueOwnershipReceiptStatus::Unsupported
        }));
        assert!(spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::DeleteResource { resource_type, resource_id }
                if resource_type == "room" && resource_id == "old-room"
        )));
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
    fn takeover_resumes_past_a_failure_persisted_by_an_older_build() {
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
            HueOwnershipReceiptStatus::Failed,
            None,
        );
        persist_controller_ownership(&temp.storage, &interrupted).unwrap();
        spy.reset();

        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert!(active.receipts().any(|receipt| {
            receipt.resource_type == "behavior_instance"
                && receipt.status == HueOwnershipReceiptStatus::Unsupported
        }));
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { resource_type, .. }
                if resource_type == "behavior_instance"
        )));
        assert!(spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::DeleteResource { resource_type, resource_id }
                if resource_type == "room" && resource_id == "old-room"
        )));
    }

    #[test]
    fn release_fences_stale_credentials_then_retains_snapshot_for_repair() {
        let temp = TempStorage::new("retained-reacquire");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        let capture_id = active.baseline.capture_id().to_string();
        spy.reset();
        let pending = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        assert_eq!(pending.phase, HueOwnershipPhase::ReleasePending);
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::CreateResource { .. }
                | HueTransportCall::UpdateResource { .. }
                | HueTransportCall::DeleteResource { .. }
                | HueTransportCall::PutV1 { .. }
        )));
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
        let retained = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(retained.phase, HueOwnershipPhase::SnapshotRetained);
        assert_eq!(retained.baseline.capture_id(), capture_id);

        let reacquired =
            acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        assert_eq!(reacquired.phase, HueOwnershipPhase::Active);
        assert_eq!(reacquired.baseline.capture_id(), capture_id);
    }

    #[test]
    fn malformed_v1_receipt_is_skipped_and_journaled_as_unsupported() {
        let temp = TempStorage::new("v1-receipt");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        let private_id = "private-unrelated-rule";
        spy.set_v1_write_response_override(Some(json!([{
            "success": {format!("/rules/{private_id}/status"): "disabled"}
        }])));

        let active = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        assert_eq!(active.phase, HueOwnershipPhase::Active);
        let state = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(state.phase, HueOwnershipPhase::Active);
        let v1_receipts = state
            .receipts()
            .filter(|receipt| receipt.api == "v1")
            .collect::<Vec<_>>();
        assert_eq!(v1_receipts.len(), 2);
        assert!(v1_receipts
            .iter()
            .all(|receipt| receipt.status == HueOwnershipReceiptStatus::Unsupported));
        assert!(v1_receipts
            .iter()
            .all(|receipt| !receipt.operation_id.contains(private_id)));
    }

    #[test]
    fn automations_and_smart_scenes_are_captured_then_cleared_without_restore() {
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
        assert!(spy.get_resources("user", "smart_scene").unwrap()["data"]
            .as_array()
            .unwrap()
            .is_empty());

        spy.reset();
        let released = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        assert_eq!(released.phase, HueOwnershipPhase::ReleasePending);
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::CreateResource { resource_type, .. }
                | HueTransportCall::UpdateResource { resource_type, .. }
                | HueTransportCall::DeleteResource { resource_type, .. }
                if resource_type == "smart_scene"
        )));
        finalize_released_control(&temp.storage, "bridge-1").unwrap();
        let retained = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(retained.phase, HueOwnershipPhase::SnapshotRetained);
        assert_eq!(
            retained.baseline.v2_resource("smart_scene").unwrap()["data"][0]["id"],
            "smart-1"
        );
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
