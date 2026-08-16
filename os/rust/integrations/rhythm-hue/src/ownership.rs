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

/// Native Hue targets partitioned by the persisted room-automation owner.
///
/// This value is derived from topology on every reconciliation and is never
/// exposed through diagnostics. The integration expands device ownership to
/// service and scene resources before classifying behavior targets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueAutomationSuppressionScope {
    rhythm_targets: BTreeSet<String>,
    external_targets: BTreeSet<String>,
}

impl HueAutomationSuppressionScope {
    pub fn include_rhythm_room<I, S>(
        &mut self,
        room_id: &str,
        grouped_light_id: &str,
        device_ids: I,
    ) where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.include_room(true, room_id, grouped_light_id, device_ids);
    }

    pub fn include_external_room<I, S>(
        &mut self,
        room_id: &str,
        grouped_light_id: &str,
        device_ids: I,
    ) where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.include_room(false, room_id, grouped_light_id, device_ids);
    }

    fn include_room<I, S>(
        &mut self,
        rhythm_owned: bool,
        room_id: &str,
        grouped_light_id: &str,
        device_ids: I,
    ) where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let targets = if rhythm_owned {
            &mut self.rhythm_targets
        } else {
            &mut self.external_targets
        };
        insert_resource_target(targets, "room", room_id);
        insert_resource_target(targets, "grouped_light", grouped_light_id);
        for device_id in device_ids {
            insert_resource_target(targets, "device", device_id.as_ref());
        }
    }

    pub fn has_rhythm_rooms(&self) -> bool {
        !self.rhythm_targets.is_empty()
    }
}

fn resource_target_key(resource_type: &str, resource_id: &str) -> String {
    format!("{}:{resource_type}{resource_id}", resource_type.len())
}

fn insert_resource_target(targets: &mut BTreeSet<String>, resource_type: &str, resource_id: &str) {
    if !resource_id.trim().is_empty() {
        targets.insert(resource_target_key(resource_type, resource_id));
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct HueTargetOwnership {
    rhythm: bool,
    external: bool,
    unknown: bool,
}

impl HueTargetOwnership {
    fn exclusively_rhythm(self) -> bool {
        self.rhythm && !self.external && !self.unknown
    }
}

#[derive(Clone, Debug)]
struct ResolvedHueAutomationSuppressionScope {
    rhythm_targets: BTreeSet<String>,
    external_targets: BTreeSet<String>,
    expansion_complete: bool,
}

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

/// Durable reason class for a controller release.
///
/// Room-policy releases intentionally keep credentials connected and may
/// resume through ordinary bootstrap after an interruption. Local teardown
/// releases must remain start-fenced until the disconnect/reset transaction
/// finishes removing credentials and recovery material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HueOwnershipReleaseIntent {
    RoomAuthorityChanged,
    LocalTeardown,
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
    /// Exact topology-derived scope that owns this suppression epoch. `None`
    /// is the legacy bridge-wide scope and is also the safe default for
    /// manifests written before room scopes became durable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suppression_scope: Option<HueAutomationSuppressionScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release_intent: Option<HueOwnershipReleaseIntent>,
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
    pub suppressed_behavior_count: usize,
    pub coexistence_gap_count: usize,
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
            if receipt.resource_type == "behavior_instance" {
                match receipt.status {
                    HueOwnershipReceiptStatus::Succeeded => {
                        diagnostics.suppressed_behavior_count += 1;
                    }
                    HueOwnershipReceiptStatus::Unsupported => {
                        diagnostics.coexistence_gap_count += 1;
                    }
                    _ => {}
                }
            }
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
            suppression_scope: None,
            release_intent: None,
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

    pub fn release_intent(&self) -> Option<HueOwnershipReleaseIntent> {
        self.release_intent
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
        if existing.suppression_scope != state.suppression_scope {
            anyhow::bail!("Refusing to replace the immutable Hue suppression scope");
        }
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
    suppression_scope: Option<&HueAutomationSuppressionScope>,
) -> Result<HueControllerOwnership> {
    let baseline = capture_baseline(transport, username, new_capture_id())?;
    if baseline.bridge_id() != connected_id {
        anyhow::bail!("Hue bridge identity changed while capturing ownership baseline");
    }
    let mut state = HueControllerOwnership::captured(baseline);
    state.suppression_scope = suppression_scope.cloned();
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
    let mut replacement =
        HueControllerOwnership::captured(capture_baseline(transport, username, new_capture_id())?);
    replacement.suppression_scope = state.suppression_scope;
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
    acquire_authoritative_control_with_scope(storage, key, transport, username, None)
}

/// Acquire Hue authority for only the native targets belonging to rooms that
/// explicitly chose Rhythm. Hue-owned and unresolved targets remain enabled.
pub fn acquire_authoritative_control_in_scope<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    transport: &H,
    username: &str,
    scope: &HueAutomationSuppressionScope,
) -> Result<HueControllerOwnership> {
    if !scope.has_rhythm_rooms() {
        anyhow::bail!("A scoped Hue takeover requires at least one Rhythm-owned room");
    }
    acquire_authoritative_control_with_scope(storage, key, transport, username, Some(scope))
}

fn acquire_authoritative_control_with_scope<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    transport: &H,
    username: &str,
    scope: Option<&HueAutomationSuppressionScope>,
) -> Result<HueControllerOwnership> {
    let connected_id = connected_hue_bridge_id(transport, username)?;
    let mut existing = load_controller_ownership(storage, &connected_id)?;
    if let Some(state) = existing.as_ref() {
        if matches!(
            state.phase,
            HueOwnershipPhase::Restoring
                | HueOwnershipPhase::RestoreIncomplete
                | HueOwnershipPhase::Restored
                | HueOwnershipPhase::ReleasePending
                | HueOwnershipPhase::SnapshotRetained
        ) {
            state.ensure_bridge_id(&connected_id)?;
            anyhow::bail!(
                "Hue controller release is awaiting durable local credential finalization"
            );
        }
    }

    if existing
        .as_ref()
        .is_some_and(|state| state.suppression_scope.as_ref() != scope)
    {
        // A room-policy write can be interrupted after canonical state is
        // durable but before the previous Hue epoch is restored. The epoch's
        // exact scope is therefore part of the durable recovery contract.
        let restored = release_authoritative_control_with_intent(
            storage,
            key,
            transport,
            username,
            HueOwnershipReleaseIntent::RoomAuthorityChanged,
        )?
        .ok_or_else(|| anyhow::anyhow!("Hue scope changed without an ownership epoch"))?;
        if restored.phase != HueOwnershipPhase::Restored {
            anyhow::bail!("Previous Hue room authority scope was not fully restored");
        }
        finalize_released_control(storage, &connected_id)?;
        existing = None;
    }

    let state = match existing {
        Some(state) => state,
        None => capture_and_persist_new_epoch(storage, transport, username, &connected_id, scope)?,
    };
    let state = upgrade_uncleared_baseline(storage, transport, username, &connected_id, state)?;
    reconcile_authoritative_control_with_scope(storage, key, transport, username, state, scope)
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

impl ResolvedHueAutomationSuppressionScope {
    fn direct(scope: &HueAutomationSuppressionScope) -> Self {
        Self {
            rhythm_targets: scope.rhythm_targets.clone(),
            external_targets: scope.external_targets.clone(),
            expansion_complete: true,
        }
    }

    /// Preserve every behavior when native room/device expansion is partial.
    /// The direct topology still identifies the desired scope for diagnostics
    /// and retry, but it is not sufficient to prove exclusive ownership of a
    /// switch, sensor, or other non-light behavior source.
    fn incomplete(scope: &HueAutomationSuppressionScope) -> Self {
        Self {
            rhythm_targets: scope.rhythm_targets.clone(),
            external_targets: scope.external_targets.clone(),
            expansion_complete: false,
        }
    }

    fn from_transport<H: HueTransport + ?Sized>(
        scope: &HueAutomationSuppressionScope,
        transport: &H,
        username: &str,
    ) -> Result<Self> {
        let mut resolved = Self::direct(scope);

        // Topology bindings deliberately persist only light device IDs. Use
        // the captured native room membership to include switches, sensors,
        // and other source devices when enforcing a Hue-owned room boundary.
        let rooms = transport.get_resources(username, "room")?;
        let expected_room_targets = resolved
            .rhythm_targets
            .iter()
            .chain(resolved.external_targets.iter())
            .filter(|target| target.starts_with("4:room"))
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut observed_room_targets = BTreeSet::new();
        for room in data_array("room", &rooms)? {
            let Some(room_id) = room.get("id").and_then(Value::as_str) else {
                continue;
            };
            let room_key = resource_target_key("room", room_id);
            let rhythm_owned = resolved.rhythm_targets.contains(&room_key);
            let external_owned = resolved.external_targets.contains(&room_key);
            if !rhythm_owned && !external_owned {
                continue;
            }
            observed_room_targets.insert(room_key);
            let children = room
                .get("children")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow::anyhow!("Hue room response has no child array"))?;
            for child in children {
                let Some(resource_type) = child.get("rtype").and_then(Value::as_str) else {
                    continue;
                };
                let Some(resource_id) = child.get("rid").and_then(Value::as_str) else {
                    continue;
                };
                if rhythm_owned {
                    insert_resource_target(
                        &mut resolved.rhythm_targets,
                        resource_type,
                        resource_id,
                    );
                }
                if external_owned {
                    insert_resource_target(
                        &mut resolved.external_targets,
                        resource_type,
                        resource_id,
                    );
                }
            }
        }
        if observed_room_targets != expected_room_targets {
            anyhow::bail!("Hue room response did not cover the complete authority scope");
        }

        let devices = transport.get_resources(username, "device")?;
        for device in data_array("device", &devices)? {
            let Some(device_id) = device.get("id").and_then(Value::as_str) else {
                continue;
            };
            let device_key = resource_target_key("device", device_id);
            let rhythm_owned = resolved.rhythm_targets.contains(&device_key);
            let external_owned = resolved.external_targets.contains(&device_key);
            if !rhythm_owned && !external_owned {
                continue;
            }
            for service in device
                .get("services")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(resource_type) = service.get("rtype").and_then(Value::as_str) else {
                    continue;
                };
                let Some(resource_id) = service.get("rid").and_then(Value::as_str) else {
                    continue;
                };
                if rhythm_owned {
                    insert_resource_target(
                        &mut resolved.rhythm_targets,
                        resource_type,
                        resource_id,
                    );
                }
                if external_owned {
                    insert_resource_target(
                        &mut resolved.external_targets,
                        resource_type,
                        resource_id,
                    );
                }
            }
        }

        for resource_type in ["scene", "smart_scene"] {
            let resources = transport.get_resources(username, resource_type)?;
            for resource in data_array(resource_type, &resources)? {
                let Some(resource_id) = resource.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(group_type) = resource.pointer("/group/rtype").and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(group_id) = resource.pointer("/group/rid").and_then(Value::as_str) else {
                    continue;
                };
                let group_key = resource_target_key(group_type, group_id);
                if resolved.rhythm_targets.contains(&group_key) {
                    insert_resource_target(
                        &mut resolved.rhythm_targets,
                        resource_type,
                        resource_id,
                    );
                }
                if resolved.external_targets.contains(&group_key) {
                    insert_resource_target(
                        &mut resolved.external_targets,
                        resource_type,
                        resource_id,
                    );
                }
            }
        }
        Ok(resolved)
    }

    fn classify_behavior(&self, behavior: &Value) -> HueTargetOwnership {
        let configuration = behavior.get("configuration").unwrap_or(&Value::Null);
        let dependees = behavior.get("dependees").unwrap_or(&Value::Null);
        let source_device_id = accessory_source_device_id(configuration, dependees);
        let mut ownership = HueTargetOwnership {
            unknown: !self.expansion_complete,
            ..HueTargetOwnership::default()
        };
        if let Some(source_device_id) = source_device_id {
            let source_key = resource_target_key("device", source_device_id);
            ownership.rhythm |= self.rhythm_targets.contains(&source_key);
            // A behavior originating in a Hue-owned room is preserved even
            // when it reaches into a Rhythm room. The explicit Hue boundary
            // owns the complete behavior, not only its eventual light target.
            ownership.external |= self.external_targets.contains(&source_key);
        }
        self.classify_value(configuration, source_device_id, &mut ownership);
        self.classify_value(dependees, source_device_id, &mut ownership);
        ownership
    }

    fn classify_value(
        &self,
        value: &Value,
        source_device_id: Option<&str>,
        ownership: &mut HueTargetOwnership,
    ) {
        match value {
            Value::Array(values) => {
                for value in values {
                    self.classify_value(value, source_device_id, ownership);
                }
            }
            Value::Object(object) => {
                if let Some((resource_id, resource_type)) = object
                    .get("rid")
                    .and_then(Value::as_str)
                    .filter(|rid| !rid.trim().is_empty())
                    .zip(object.get("rtype").and_then(Value::as_str))
                {
                    let is_lighting_target = matches!(
                        resource_type,
                        "bridge_home"
                            | "device"
                            | "grouped_light"
                            | "light"
                            | "room"
                            | "scene"
                            | "service_group"
                            | "smart_scene"
                            | "zone"
                    );
                    if is_lighting_target
                        && !(resource_type == "device" && source_device_id == Some(resource_id))
                    {
                        let key = resource_target_key(resource_type, resource_id);
                        let rhythm = self.rhythm_targets.contains(&key);
                        let external = self.external_targets.contains(&key);
                        ownership.rhythm |= rhythm;
                        ownership.external |= external;
                        ownership.unknown |= !rhythm && !external;
                    }
                }
                for value in object.values() {
                    self.classify_value(value, source_device_id, ownership);
                }
            }
            _ => {}
        }
    }
}

fn next_clear_operation<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
    skipped_operation_ids: &BTreeSet<String>,
    scope: Option<&ResolvedHueAutomationSuppressionScope>,
) -> Result<Option<ControlPlaneOperation>> {
    let behavior_payload = transport.get_resources(username, "behavior_instance")?;
    let script_payload = transport.get_resources(username, "behavior_script")?;
    let behaviors = data_array("behavior_instance", &behavior_payload)?;
    let scripts = data_array("behavior_script", &script_payload)?;
    let mut behavior_ids = Vec::new();
    let coexistence_allowed = scope.is_some();
    for behavior in behaviors {
        let enabled = match behavior.get("enabled").and_then(Value::as_bool) {
            Some(enabled) => enabled,
            None if coexistence_allowed => {
                warn!(
                    target: "hue_authority",
                    "Preserving a Hue behavior with no boolean enabled state during forced per-room coexistence"
                );
                continue;
            }
            None => anyhow::bail!("Hue behavior instance has no boolean enabled state"),
        };
        if !enabled {
            continue;
        }
        let resource_id = match behavior
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            Some(resource_id) => resource_id,
            None if coexistence_allowed => {
                warn!(
                    target: "hue_authority",
                    "Preserving an enabled Hue behavior with no identity during forced per-room coexistence"
                );
                continue;
            }
            None => anyhow::bail!("Enabled Hue behavior instance has no ID"),
        };
        let script_id = match behavior
            .get("script_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            Some(script_id) => script_id,
            None if coexistence_allowed => {
                warn!(
                    target: "hue_authority",
                    "Preserving an enabled Hue behavior with no script identity during forced per-room coexistence"
                );
                continue;
            }
            None => anyhow::bail!("Enabled Hue behavior instance has no script ID"),
        };
        let matching_scripts = scripts
            .iter()
            .filter(|script| script.get("id").and_then(Value::as_str) == Some(script_id))
            .collect::<Vec<_>>();
        let [script] = matching_scripts.as_slice() else {
            if coexistence_allowed {
                warn!(
                    target: "hue_authority",
                    "Preserving a Hue behavior whose script could not be resolved during forced per-room coexistence"
                );
                continue;
            }
            anyhow::bail!(
                "Enabled Hue behavior instance {resource_id} did not resolve to exactly one script"
            );
        };
        let category = match script.pointer("/metadata/category").and_then(Value::as_str) {
            Some(category) => category,
            None if coexistence_allowed => {
                warn!(
                    target: "hue_authority",
                    "Preserving a Hue behavior with no recognized script category during forced per-room coexistence"
                );
                continue;
            }
            None => {
                anyhow::bail!("Hue behavior script {script_id} has no string metadata category")
            }
        };
        let target_is_in_scope = scope
            .map(|scope| scope.classify_behavior(behavior).exclusively_rhythm())
            .unwrap_or(true);
        match category {
            "automation"
                if target_is_in_scope
                    && !skipped_operation_ids
                        .contains(&format!("clear:v2:behavior_instance:{resource_id}")) =>
            {
                behavior_ids.push(resource_id.to_string());
            }
            "automation" => {}
            // Accessory scripts include both source-only device plumbing and
            // button/sensor programs that target lights or groups. Suppress
            // only the latter. Forced per-room authority preserves malformed
            // or rejected accessory behavior and allows it to coexist.
            "accessory" => {
                let targets_lighting = match accessory_behavior_targets_lighting(
                    behavior,
                    resource_id,
                ) {
                    Ok(targets_lighting) => targets_lighting,
                    Err(_) if coexistence_allowed => {
                        warn!(
                            target: "hue_authority",
                            "Preserving an unclassified Hue accessory behavior during forced per-room coexistence"
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if targets_lighting
                    && target_is_in_scope
                    && !skipped_operation_ids
                        .contains(&format!("clear:v2:behavior_instance:{resource_id}"))
                {
                    behavior_ids.push(resource_id.to_string());
                }
            }
            // Entertainment playback and source-only/internal behavior
            // instances are not unattended lighting automations.
            "entertainment" | "other" => {}
            _ if coexistence_allowed => {
                warn!(
                    target: "hue_authority",
                    "Preserving a Hue behavior with an unknown script category during forced per-room coexistence"
                );
            }
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

    // Legacy V1 rules and schedules have no complete room-target graph. A
    // room-bounded takeover preserves them and relies on explicit coexistence
    // consent instead of risking a bridge-wide mutation.
    if scope.is_none() {
        for resource_type in REQUIRED_V1_BASELINE_RESOURCES {
            let payload = transport.get_v1(username, resource_type)?;
            let mut ids = object_resource(resource_type, &payload)?
                .iter()
                .filter(|(_, resource)| {
                    resource.get("status").and_then(Value::as_str) != Some("disabled")
                })
                .filter(|(resource_id, _)| {
                    !skipped_operation_ids
                        .contains(&format!("clear:v1:{resource_type}:{resource_id}"))
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
    /// The write returned an error and an authoritative read proved the
    /// complete resource still matches the immutable pre-write baseline.
    Rejected,
    /// The write may have reached Hue, but its effect could not be proven.
    /// This must remain fenced and restorable rather than becoming a durable
    /// coexistence exception.
    Indeterminate,
}

fn failed_suppression_is_confirmed_unchanged<H: HueTransport + ?Sized>(
    state: &HueControllerOwnership,
    transport: &H,
    username: &str,
    operation: &ControlPlaneOperation,
) -> Result<bool> {
    let baseline = baseline_resource_for_operation(state, operation)?;
    let live = live_resource_for_operation(transport, username, operation)?;
    Ok(restore_resources_equivalent(operation.api, &live, baseline))
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
    match execute_operation(transport, username, operation) {
        Ok(replacement_id) => match verify_operation(transport, username, operation) {
            Ok(()) => {
                state.set_receipt(
                    operation,
                    HueOwnershipReceiptStatus::Succeeded,
                    replacement_id,
                );
                persist_controller_ownership(storage, state)?;
                Ok(JournaledOperationOutcome::Succeeded)
            }
            Err(error) => {
                // Hue acknowledged the mutation, so a failed read-back is an
                // indeterminate physical outcome. Keep the failed receipt: a
                // subsequent release will compare-and-set it back to baseline
                // if the disable actually landed.
                state.set_receipt(operation, HueOwnershipReceiptStatus::Failed, None);
                persist_controller_ownership(storage, state)
                    .context("Indeterminate Hue suppression receipt was not durable")?;
                warn!(
                    target: "hue_authority",
                    "Hue {} {} acknowledged suppression but read-back was indeterminate: {}",
                    operation.api,
                    operation.resource_type,
                    error
                );
                Ok(JournaledOperationOutcome::Indeterminate)
            }
        },
        Err(error) => {
            let confirmed_unchanged =
                failed_suppression_is_confirmed_unchanged(state, transport, username, operation);
            state.set_receipt(operation, HueOwnershipReceiptStatus::Failed, None);
            persist_controller_ownership(storage, state)
                .context("Failed Hue suppression receipt was not durable")?;
            match confirmed_unchanged {
                Ok(true) => {
                    warn!(
                        target: "hue_authority",
                        "Hue {} {} rejected suppression and remained unchanged: {}",
                        operation.api,
                        operation.resource_type,
                        error
                    );
                    Ok(JournaledOperationOutcome::Rejected)
                }
                Ok(false) => {
                    warn!(
                        target: "hue_authority",
                        "Hue {} {} suppression failed with an indeterminate observed state: {}",
                        operation.api,
                        operation.resource_type,
                        error
                    );
                    Ok(JournaledOperationOutcome::Indeterminate)
                }
                Err(read_error) => {
                    warn!(
                        target: "hue_authority",
                        "Hue {} {} suppression failed and authoritative read-back was unavailable: {}; {}",
                        operation.api,
                        operation.resource_type,
                        error,
                        read_error
                    );
                    Ok(JournaledOperationOutcome::Indeterminate)
                }
            }
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
    state: HueControllerOwnership,
) -> Result<HueControllerOwnership> {
    reconcile_authoritative_control_with_scope(storage, key, transport, username, state, None)
}

fn reconcile_authoritative_control_with_scope<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    transport: &H,
    username: &str,
    mut state: HueControllerOwnership,
    scope: Option<&HueAutomationSuppressionScope>,
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
    let resolved_scope = scope.map(|scope| {
        ResolvedHueAutomationSuppressionScope::from_transport(scope, transport, username)
            .unwrap_or_else(|_| {
                warn!(
                    target: "hue_authority",
                    "Hue target expansion was incomplete; preserving unresolved behavior during forced per-room coexistence"
                );
                ResolvedHueAutomationSuppressionScope::incomplete(scope)
            })
    });
    state.phase = HueOwnershipPhase::Clearing;
    persist_controller_ownership(storage, &state)?;

    let mut skipped_operation_ids = if resolved_scope.is_some() {
        state
            .receipts
            .values()
            .filter(|receipt| receipt.status == HueOwnershipReceiptStatus::Unsupported)
            .map(|receipt| receipt.operation_id.clone())
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    loop {
        let operation = match next_clear_operation(
            transport,
            username,
            &skipped_operation_ids,
            resolved_scope.as_ref(),
        ) {
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
        match run_journaled_operation(storage, key, &mut state, transport, username, &operation)? {
            JournaledOperationOutcome::Succeeded => {}
            JournaledOperationOutcome::Rejected if resolved_scope.is_some() => {
                state.set_receipt(&operation, HueOwnershipReceiptStatus::Unsupported, None);
                persist_controller_ownership(storage, &state)?;
                warn!(
                    target: "hue_authority",
                    "Hue behavior suppression was rejected; preserving the behavior and continuing with explicit per-room coexistence"
                );
                skipped_operation_ids.insert(operation.operation_id);
                continue;
            }
            JournaledOperationOutcome::Rejected => {
                state.phase = HueOwnershipPhase::ClearIncomplete;
                persist_controller_ownership(storage, &state)?;
                anyhow::bail!(
                    "Hue automation suppression was incomplete; controller takeover refused"
                );
            }
            JournaledOperationOutcome::Indeterminate => {
                state.phase = HueOwnershipPhase::ClearIncomplete;
                persist_controller_ownership(storage, &state)?;
                anyhow::bail!(
                    "Hue automation suppression outcome was indeterminate; controller takeover refused"
                );
            }
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
    key: &HubKey,
    transport: &H,
    username: &str,
) -> Result<Option<HueControllerOwnership>> {
    release_authoritative_control_with_intent(
        storage,
        key,
        transport,
        username,
        HueOwnershipReleaseIntent::LocalTeardown,
    )
}

/// Release controller authority while durably recording whether startup may
/// resume the release with the existing credentials.
pub fn release_authoritative_control_with_intent<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    transport: &H,
    username: &str,
    requested_intent: HueOwnershipReleaseIntent,
) -> Result<Option<HueControllerOwnership>> {
    let connected_id = connected_hue_bridge_id(transport, username)?;
    let Some(mut state) = load_controller_ownership(storage, &connected_id)? else {
        return Ok(None);
    };
    state.ensure_bridge_id(&connected_id)?;
    let release_intent = match (state.release_intent, requested_intent) {
        (Some(HueOwnershipReleaseIntent::LocalTeardown), _)
        | (_, HueOwnershipReleaseIntent::LocalTeardown) => HueOwnershipReleaseIntent::LocalTeardown,
        _ => HueOwnershipReleaseIntent::RoomAuthorityChanged,
    };
    let intent_changed = state.release_intent != Some(release_intent);
    state.release_intent = Some(release_intent);
    if state.phase == HueOwnershipPhase::Restored {
        if intent_changed {
            persist_controller_ownership(storage, &state)?;
        }
        return Ok(Some(state));
    }
    state.phase = HueOwnershipPhase::Restoring;
    persist_controller_ownership(storage, &state)?;
    while let Some(operation) = next_restore_operation(&state)? {
        run_restore_operation(storage, key, &mut state, transport, username, &operation)?;
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

    fn seed_scoped_bridge(spy: &SpyHueTransport) {
        seed_bridge(spy, json!([]));
        spy.set_resource_response(
            "device",
            json!({"data": [
                {"id": "rhythm-device", "services": [
                    {"rid": "rhythm-light", "rtype": "light"}
                ]},
                {"id": "external-device", "services": [
                    {"rid": "external-light", "rtype": "light"}
                ]},
                {"id": "power-device", "services": [
                    {"rid": "power-service", "rtype": "device_power"}
                ]}
            ], "errors": []}),
        );
        spy.set_resource_response(
            "light",
            json!({"data": [
                {"id": "rhythm-light"},
                {"id": "external-light"}
            ], "errors": []}),
        );
        spy.set_resource_response(
            "room",
            json!({"data": [
                {"id": "rhythm-room", "children": [
                    {"rid": "rhythm-device", "rtype": "device"}
                ], "services": [
                    {"rid": "rhythm-group", "rtype": "grouped_light"}
                ]},
                {"id": "external-room", "children": [
                    {"rid": "external-device", "rtype": "device"},
                    {"rid": "power-device", "rtype": "device"}
                ], "services": [
                    {"rid": "external-group", "rtype": "grouped_light"}
                ]}
            ], "errors": []}),
        );
        spy.set_resource_response("scene", json!({"data": [], "errors": []}));
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [
                {
                    "id": "rhythm-behavior",
                    "script_id": "automation-script-1",
                    "enabled": true,
                    "configuration": {
                        "target": {"rid": "rhythm-light", "rtype": "light"}
                    },
                    "dependees": []
                },
                {
                    "id": "external-behavior",
                    "script_id": "automation-script-1",
                    "enabled": true,
                    "configuration": {
                        "target": {"rid": "external-device", "rtype": "device"}
                    },
                    "dependees": []
                },
                {
                    "id": "external-source-behavior",
                    "script_id": "automation-script-1",
                    "enabled": true,
                    "configuration": {
                        "device": {"rid": "external-device", "rtype": "device"},
                        "target": {"rid": "rhythm-group", "rtype": "grouped_light"}
                    },
                    "dependees": []
                },
                {
                    "id": "power-source-behavior",
                    "script_id": "automation-script-1",
                    "enabled": true,
                    "configuration": {
                        "device": {"rid": "power-device", "rtype": "device"},
                        "target": {"rid": "rhythm-group", "rtype": "grouped_light"}
                    },
                    "dependees": []
                },
                {
                    "id": "cross-room-behavior",
                    "script_id": "automation-script-1",
                    "enabled": true,
                    "configuration": {
                        "targets": [
                            {"rid": "rhythm-group", "rtype": "grouped_light"},
                            {"rid": "external-group", "rtype": "grouped_light"}
                        ]
                    },
                    "dependees": []
                }
            ], "errors": []}),
        );
    }

    fn mixed_scope() -> HueAutomationSuppressionScope {
        let mut scope = HueAutomationSuppressionScope::default();
        scope.include_rhythm_room("rhythm-room", "rhythm-group", ["rhythm-device"]);
        scope.include_external_room("external-room", "external-group", ["external-device"]);
        scope
    }

    #[test]
    fn scoped_takeover_suppresses_only_exclusive_rhythm_room_behavior() {
        let temp = TempStorage::new("room-scope");
        let spy = SpyHueTransport::new();
        seed_scoped_bridge(&spy);

        let active = acquire_authoritative_control_in_scope(
            &temp.storage,
            &key(),
            &spy,
            "user",
            &mixed_scope(),
        )
        .unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        let behaviors = spy.get_resources("user", "behavior_instance").unwrap();
        let enabled = |id: &str| {
            behaviors["data"]
                .as_array()
                .unwrap()
                .iter()
                .find(|behavior| behavior["id"] == id)
                .unwrap()["enabled"]
                .as_bool()
                .unwrap()
        };
        assert!(!enabled("rhythm-behavior"));
        assert!(enabled("external-behavior"));
        assert!(enabled("external-source-behavior"));
        assert!(enabled("power-source-behavior"));
        assert!(enabled("cross-room-behavior"));
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
    fn scoped_takeover_forces_rhythm_after_hue_rejects_suppression() {
        let temp = TempStorage::new("room-scope-rejection");
        let spy = SpyHueTransport::new();
        seed_scoped_bridge(&spy);
        spy.set_fail_resource_update("behavior_instance", "rhythm-behavior");

        let active = acquire_authoritative_control_in_scope(
            &temp.storage,
            &key(),
            &spy,
            "user",
            &mixed_scope(),
        )
        .unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert!(active.receipts().any(|receipt| {
            receipt.original_resource_id == "rhythm-behavior"
                && receipt.status == HueOwnershipReceiptStatus::Unsupported
        }));
        assert_eq!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"]
                .as_array()
                .unwrap()
                .iter()
                .find(|behavior| behavior["id"] == "rhythm-behavior")
                .unwrap()["enabled"],
            true
        );
    }

    #[test]
    fn scoped_takeover_forces_rhythm_past_unclassified_hue_behavior() {
        let temp = TempStorage::new("room-scope-unclassified");
        let spy = SpyHueTransport::new();
        seed_scoped_bridge(&spy);
        spy.set_resource_response(
            "behavior_instance",
            json!({"data": [{
                "id": "future-behavior",
                "enabled": true,
                "configuration": {
                    "target": {"rid": "rhythm-group", "rtype": "grouped_light"}
                }
            }], "errors": []}),
        );

        let active = acquire_authoritative_control_in_scope(
            &temp.storage,
            &key(),
            &spy,
            "user",
            &mixed_scope(),
        )
        .unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource {
                resource_type,
                ..
            } if resource_type == "behavior_instance"
        )));
    }

    #[test]
    fn scoped_policy_change_restores_old_room_before_suppressing_new_room() {
        let temp = TempStorage::new("room-scope-change");
        let spy = SpyHueTransport::new();
        seed_scoped_bridge(&spy);

        acquire_authoritative_control_in_scope(&temp.storage, &key(), &spy, "user", &mixed_scope())
            .unwrap();
        let restored = release_authoritative_control_with_intent(
            &temp.storage,
            &key(),
            &spy,
            "user",
            HueOwnershipReleaseIntent::RoomAuthorityChanged,
        )
        .unwrap()
        .unwrap();
        assert_eq!(restored.phase, HueOwnershipPhase::Restored);
        finalize_released_control(&temp.storage, "bridge-1").unwrap();

        let mut reversed = HueAutomationSuppressionScope::default();
        reversed.include_external_room("rhythm-room", "rhythm-group", ["rhythm-device"]);
        reversed.include_rhythm_room("external-room", "external-group", ["external-device"]);
        acquire_authoritative_control_in_scope(&temp.storage, &key(), &spy, "user", &reversed)
            .unwrap();

        let behaviors = spy.get_resources("user", "behavior_instance").unwrap();
        let enabled = |id: &str| {
            behaviors["data"]
                .as_array()
                .unwrap()
                .iter()
                .find(|behavior| behavior["id"] == id)
                .unwrap()["enabled"]
                .as_bool()
                .unwrap()
        };
        assert!(enabled("rhythm-behavior"));
        assert!(!enabled("external-behavior"));
        assert!(enabled("external-source-behavior"));
        assert!(enabled("power-source-behavior"));
        assert!(enabled("cross-room-behavior"));
    }

    #[test]
    fn incomplete_scope_expansion_preserves_hue_owned_non_light_behavior() {
        let temp = TempStorage::new("room-scope-incomplete-expansion");
        let spy = SpyHueTransport::new();
        seed_scoped_bridge(&spy);
        // The aggregate capture remains available, but the typed device read
        // cannot prove ownership of the Hue-owned power device.
        spy.set_resource_response("device", json!({"data": {}}));

        let active = acquire_authoritative_control_in_scope(
            &temp.storage,
            &key(),
            &spy,
            "user",
            &mixed_scope(),
        )
        .unwrap();

        assert_eq!(active.phase, HueOwnershipPhase::Active);
        let behaviors = spy.get_resources("user", "behavior_instance").unwrap();
        assert!(behaviors["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|behavior| { behavior.get("enabled").and_then(Value::as_bool) == Some(true) }));
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource {
                resource_type,
                ..
            } if resource_type == "behavior_instance"
        )));
    }

    #[test]
    fn scoped_restart_restores_changed_epoch_before_new_suppression() {
        let temp = TempStorage::new("room-scope-restart-change");
        let spy = SpyHueTransport::new();
        seed_scoped_bridge(&spy);

        acquire_authoritative_control_in_scope(&temp.storage, &key(), &spy, "user", &mixed_scope())
            .unwrap();

        // Simulate restart after a new canonical room policy became durable
        // but before the command path released the old Hue epoch.
        let mut reversed = HueAutomationSuppressionScope::default();
        reversed.include_external_room("rhythm-room", "rhythm-group", ["rhythm-device"]);
        reversed.include_rhythm_room("external-room", "external-group", ["external-device"]);
        let second =
            acquire_authoritative_control_in_scope(&temp.storage, &key(), &spy, "user", &reversed)
                .unwrap();

        assert_eq!(second.suppression_scope.as_ref(), Some(&reversed));
        let behaviors = spy.get_resources("user", "behavior_instance").unwrap();
        let enabled = |id: &str| {
            behaviors["data"]
                .as_array()
                .unwrap()
                .iter()
                .find(|behavior| behavior["id"] == id)
                .unwrap()["enabled"]
                .as_bool()
                .unwrap()
        };
        assert!(enabled("rhythm-behavior"));
        assert!(!enabled("external-behavior"));
        assert!(enabled("power-source-behavior"));
    }

    #[test]
    fn acknowledged_suppression_with_lost_readback_remains_restorable() {
        let temp = TempStorage::new("room-scope-indeterminate-write");
        let spy = SpyHueTransport::new();
        seed_scoped_bridge(&spy);
        spy.set_fail_next_resource_read_after_update(true);

        let error = acquire_authoritative_control_in_scope(
            &temp.storage,
            &key(),
            &spy,
            "user",
            &mixed_scope(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("outcome was indeterminate"));

        let incomplete = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(incomplete.phase, HueOwnershipPhase::ClearIncomplete);
        assert!(incomplete.receipts().any(|receipt| {
            receipt.original_resource_id == "rhythm-behavior"
                && receipt.status == HueOwnershipReceiptStatus::Failed
        }));
        assert_eq!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"]
                .as_array()
                .unwrap()
                .iter()
                .find(|behavior| behavior["id"] == "rhythm-behavior")
                .unwrap()["enabled"],
            false
        );

        let restored = release_authoritative_control_with_intent(
            &temp.storage,
            &key(),
            &spy,
            "user",
            HueOwnershipReleaseIntent::RoomAuthorityChanged,
        )
        .unwrap()
        .unwrap();
        assert_eq!(restored.phase, HueOwnershipPhase::Restored);
        assert_eq!(
            spy.get_resources("user", "behavior_instance").unwrap()["data"]
                .as_array()
                .unwrap()
                .iter()
                .find(|behavior| behavior["id"] == "rhythm-behavior")
                .unwrap()["enabled"],
            true
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
