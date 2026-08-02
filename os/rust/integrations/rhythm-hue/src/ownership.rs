//! Durable Hue bridge takeover, quiesce, and best-effort restore.
//!
//! Rhythm treats a paired Hue bridge as a Zigbee control plane. Before the
//! first mutation this module captures an immutable bridge-scoped baseline and
//! durably persists it. Every later control-plane mutation is journaled around
//! the remote write so a failed clear or restore remains recoverable.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
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
const CLEAR_DELETE_ORDER: &[&str] = &["smart_scene", "scene", "zone", "room"];
const RESTORE_CREATE_ORDER: &[&str] = &["room", "zone", "scene"];

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
    restored_resource_ids: BTreeMap<String, String>,
    #[serde(default)]
    receipts: BTreeMap<String, HueOwnershipReceipt>,
}

impl HueControllerOwnership {
    fn captured(baseline: HueBridgeOwnershipBaseline) -> Self {
        Self {
            schema_version: HUE_CONTROLLER_OWNERSHIP_SCHEMA_VERSION,
            phase: HueOwnershipPhase::Captured,
            baseline,
            managed_rooms: BTreeMap::new(),
            managed_scenes: BTreeMap::new(),
            restored_resource_ids: BTreeMap::new(),
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

    fn replacement_id(&self, resource_type: &str, original_id: &str) -> Option<&str> {
        self.restored_resource_ids
            .get(&restore_map_key(resource_type, original_id))
            .map(String::as_str)
    }

    fn record_replacement(&mut self, resource_type: &str, original_id: &str, replacement_id: &str) {
        self.restored_resource_ids.insert(
            restore_map_key(resource_type, original_id),
            replacement_id.to_string(),
        );
    }
}

fn managed_scene_key(rhythm_room_id: &str, rhythm_scene_id: &str) -> String {
    format!("{}:{rhythm_room_id}{rhythm_scene_id}", rhythm_room_id.len())
}

/// Serialize every control-plane mutation for one physical bridge, including
/// topology reconciliation, scene projection, and restore. The
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

fn restore_map_key(resource_type: &str, original_id: &str) -> String {
    format!("{resource_type}:{original_id}")
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

/// Persist a state transition while refusing to replace the immutable baseline.
pub fn persist_controller_ownership(
    storage: &dyn Storage,
    state: &HueControllerOwnership,
) -> Result<()> {
    if let Some(existing) = load_controller_ownership(storage, state.baseline.bridge_id())? {
        if existing.baseline != state.baseline {
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
    let mut v2_resources = BTreeMap::new();
    for resource_type in REQUIRED_V2_BASELINE_RESOURCES {
        let payload = transport
            .get_resources(username, resource_type)
            .with_context(|| format!("Failed to capture required Hue V2 {resource_type}"))?;
        data_array(resource_type, &payload)?;
        v2_resources.insert((*resource_type).to_string(), payload);
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

/// Capture and durably persist the immutable bridge baseline, then clear the
/// competing Hue control plane. Incomplete acquisition manifests are resumed.
/// A restored manifest is a durable release fence: stale credentials must not
/// silently reacquire the bridge before local credential teardown finalizes it.
pub fn acquire_authoritative_control<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    transport: &H,
    username: &str,
) -> Result<HueControllerOwnership> {
    let connected_id = connected_hue_bridge_id(transport, username)?;
    let state = match load_controller_ownership(storage, &connected_id)? {
        Some(state) if state.phase == HueOwnershipPhase::Restored => {
            state.ensure_bridge_id(&connected_id)?;
            anyhow::bail!(
                "Hue controller release is awaiting durable local credential finalization"
            );
        }
        Some(state) => state,
        None => capture_and_persist_new_epoch(storage, transport, username, &connected_id)?,
    };
    ensure_baseline_restorable_for_takeover(&state.baseline)?;
    reconcile_authoritative_control(storage, key, transport, username, state)
}

fn next_clear_operation<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
    managed_room_ids: &BTreeSet<String>,
    managed_scene_ids: &BTreeSet<String>,
) -> Result<Option<ControlPlaneOperation>> {
    let behavior_payload = transport.get_resources(username, "behavior_instance")?;
    let mut behavior_ids = data_array("behavior_instance", &behavior_payload)?
        .iter()
        .filter(|resource| resource.get("enabled").and_then(Value::as_bool) == Some(true))
        .filter_map(|resource| resource.get("id").and_then(Value::as_str))
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
        ("v2", "disable" | "restore") => {
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
        ("v2", "create") => {
            let created = transport.create_resource(
                username,
                operation.resource_type,
                operation
                    .body
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Hue create operation has no body"))?,
            )?;
            Ok(Some(created.resource_id))
        }
        ("v1", "disable" | "restore") => {
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
    failure_phase: HueOwnershipPhase,
) -> Result<Option<String>> {
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
            Ok(replacement_id)
        }
        Err(error) => {
            state.phase = failure_phase;
            state.set_receipt(operation, HueOwnershipReceiptStatus::Failed, None);
            if let Err(persist_error) = persist_controller_ownership(storage, state) {
                let _ = persist_error;
                return Err(error).context(
                    "Hue operation failed and its recovery receipt could not be persisted",
                );
            }
            Err(error)
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
    ) {
        anyhow::bail!("Hue bridge release has started; takeover cannot be resumed");
    }
    state.ensure_bridge_id(&connected_hue_bridge_id(transport, username)?)?;
    state.phase = HueOwnershipPhase::Clearing;
    persist_controller_ownership(storage, &state)?;

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
            state.phase = HueOwnershipPhase::ClearIncomplete;
            persist_controller_ownership(storage, &state)?;
            anyhow::bail!("A Hue control-plane mutation failed read-back verification");
        }
        previous_operation_id = Some(operation.operation_id.clone());
        run_journaled_operation(
            storage,
            key,
            &mut state,
            transport,
            username,
            &operation,
            HueOwnershipPhase::ClearIncomplete,
        )?;
    }

    state.phase = HueOwnershipPhase::Active;
    persist_controller_ownership(storage, &state)?;
    Ok(state)
}

fn baseline_items<'a>(
    baseline: &'a HueBridgeOwnershipBaseline,
    resource_type: &str,
) -> Result<&'a [Value]> {
    let payload = baseline
        .v2_resources
        .get(resource_type)
        .ok_or_else(|| anyhow::anyhow!("Hue baseline has no {resource_type} resource"))?;
    data_array(resource_type, payload)
}

fn create_body(resource_type: &str, resource: &Value) -> Result<Value> {
    let keys: &[&str] = match resource_type {
        "room" | "zone" => &["children", "metadata"],
        "scene" => &[
            "actions",
            "metadata",
            "group",
            "palette",
            "speed",
            "auto_dynamic",
        ],
        _ => anyhow::bail!("Hue restore does not support creating {resource_type}"),
    };
    let mut body = serde_json::Map::new();
    for key in keys {
        if let Some(value) = resource.get(*key) {
            body.insert((*key).to_string(), value.clone());
        }
    }
    if body.is_empty() {
        anyhow::bail!("Hue baseline {resource_type} has no restorable fields");
    }
    Ok(Value::Object(body))
}

fn sort_json_array(array: &mut Vec<Value>) {
    array.sort_by_cached_key(|value| serde_json::to_string(value).unwrap_or_default());
}

fn normalize_semantic_arrays(resource_type: &str, body: &mut Value) {
    match resource_type {
        "room" | "zone" => {
            if let Some(children) = body.get_mut("children").and_then(Value::as_array_mut) {
                sort_json_array(children);
            }
        }
        "scene" => {
            if let Some(actions) = body.get_mut("actions").and_then(Value::as_array_mut) {
                sort_json_array(actions);
            }
            if let Some(palette) = body.get_mut("palette").and_then(Value::as_object_mut) {
                for entries in palette.values_mut().filter_map(Value::as_array_mut) {
                    sort_json_array(entries);
                }
            }
        }
        _ => {}
    }
}

fn project_to_expected_shape(actual: &Value, expected: &Value) -> Value {
    match expected {
        Value::Object(expected) => Value::Object(
            expected
                .iter()
                .map(|(key, expected_value)| {
                    let actual_value = actual.get(key).unwrap_or(&Value::Null);
                    (
                        key.clone(),
                        project_to_expected_shape(actual_value, expected_value),
                    )
                })
                .collect(),
        ),
        _ => actual.clone(),
    }
}

fn resource_semantically_matches(
    resource_type: &str,
    expected_body: &Value,
    actual_resource: &Value,
) -> Result<bool> {
    let mut expected = create_body(resource_type, expected_body)?;
    let actual_body = create_body(resource_type, actual_resource)?;
    let mut actual = project_to_expected_shape(&actual_body, &expected);
    normalize_semantic_arrays(resource_type, &mut expected);
    normalize_semantic_arrays(resource_type, &mut actual);
    Ok(expected == actual)
}

fn ensure_baseline_restorable_for_takeover(baseline: &HueBridgeOwnershipBaseline) -> Result<()> {
    let smart_scenes = baseline_items(baseline, "smart_scene")?;
    if !smart_scenes.is_empty() {
        anyhow::bail!(
            "Hue takeover is blocked because smart_scene restore is not supported; no bridge state was changed"
        );
    }

    let mut restorable_groups = BTreeSet::new();
    for resource_type in ["room", "zone"] {
        for resource in baseline_items(baseline, resource_type)? {
            let id = resource
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Hue baseline {resource_type} item has no ID"))?;
            create_body(resource_type, resource)?;
            restorable_groups.insert((resource_type, id));
        }
    }
    for scene in baseline_items(baseline, "scene")? {
        create_body("scene", scene)?;
        if let Some(group) = scene.get("group") {
            let group_id = group.get("rid").and_then(Value::as_str).ok_or_else(|| {
                anyhow::anyhow!("Hue baseline scene has an invalid group identity")
            })?;
            let group_type = group.get("rtype").and_then(Value::as_str).unwrap_or("room");
            if !restorable_groups.contains(&(group_type, group_id)) {
                anyhow::bail!(
                    "Hue takeover is blocked because a scene references a group that cannot be restored; no bridge state was changed"
                );
            }
        }
    }

    fn contains_recreated_v2_reference(value: &Value) -> bool {
        match value {
            Value::Array(values) => values.iter().any(contains_recreated_v2_reference),
            Value::Object(object) => {
                let direct_reference =
                    object
                        .get("rtype")
                        .and_then(Value::as_str)
                        .is_some_and(|resource_type| {
                            matches!(resource_type, "room" | "zone" | "scene" | "smart_scene")
                        })
                        && object.get("rid").and_then(Value::as_str).is_some();
                direct_reference || object.values().any(contains_recreated_v2_reference)
            }
            _ => false,
        }
    }

    fn contains_recreated_v1_reference(value: &Value) -> bool {
        match value {
            Value::String(value) => value.contains("/groups/") || value.contains("/scenes/"),
            Value::Array(values) => values.iter().any(contains_recreated_v1_reference),
            Value::Object(object) => object.values().any(contains_recreated_v1_reference),
            _ => false,
        }
    }

    if baseline_items(baseline, "behavior_instance")?
        .iter()
        .any(contains_recreated_v2_reference)
        || REQUIRED_V1_BASELINE_RESOURCES.iter().any(|resource_type| {
            baseline
                .v1_resources
                .get(*resource_type)
                .is_some_and(contains_recreated_v1_reference)
        })
    {
        anyhow::bail!(
            "Hue takeover is blocked because an automation references a room or scene whose identity cannot yet be remapped safely; no bridge state was changed"
        );
    }
    Ok(())
}

fn rewrite_scene_group(state: &HueControllerOwnership, mut body: Value) -> Result<Value> {
    let Some(group) = body.get_mut("group") else {
        return Ok(body);
    };
    let Some(original_id) = group.get("rid").and_then(Value::as_str) else {
        return Ok(body);
    };
    let resource_type = group.get("rtype").and_then(Value::as_str).unwrap_or("room");
    let replacement = state
        .replacement_id(resource_type, original_id)
        .ok_or_else(|| anyhow::anyhow!("Hue scene references an unrestored group resource"))?
        .to_string();
    group["rid"] = Value::String(replacement);
    Ok(body)
}

fn equivalent_unmapped_resource_id(
    state: &HueControllerOwnership,
    resource_type: &str,
    desired_body: &Value,
    current: &[Value],
) -> Result<Option<String>> {
    let mapped = state
        .restored_resource_ids
        .iter()
        .filter_map(|(key, id)| {
            key.starts_with(&format!("{resource_type}:"))
                .then_some(id.as_str())
        })
        .collect::<BTreeSet<_>>();
    let mut equivalent = current
        .iter()
        .filter_map(|resource| {
            let id = resource.get("id").and_then(Value::as_str)?;
            if mapped.contains(id) {
                return None;
            }
            resource_semantically_matches(resource_type, desired_body, resource)
                .ok()
                .filter(|matches| *matches)
                .map(|_| id.to_string())
        })
        .collect::<Vec<_>>();
    equivalent.sort();
    match equivalent.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(id.clone())),
        _ => anyhow::bail!(
            "Multiple equivalent Hue {resource_type} resources exist; refusing ambiguous restore recovery"
        ),
    }
}

fn restore_resources<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
) -> Result<()> {
    for resource_type in RESTORE_CREATE_ORDER {
        let mut items = baseline_items(&state.baseline, resource_type)?.to_vec();
        items.sort_by(|left, right| {
            left.get("id")
                .and_then(Value::as_str)
                .cmp(&right.get("id").and_then(Value::as_str))
        });
        for item in items {
            let original_id = item
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow::anyhow!("Hue baseline {resource_type} has an item without id")
                })?
                .to_string();
            if state.replacement_id(resource_type, &original_id).is_some() {
                continue;
            }
            let mut body = create_body(resource_type, &item)?;
            if *resource_type == "scene" {
                body = rewrite_scene_group(state, body)?;
            }

            let current_payload = transport.get_resources(username, resource_type)?;
            if let Some(recovered_id) = equivalent_unmapped_resource_id(
                state,
                resource_type,
                &body,
                data_array(resource_type, &current_payload)?,
            )? {
                state.record_replacement(resource_type, &original_id, &recovered_id);
                let operation = ControlPlaneOperation {
                    operation_id: format!("restore:v2:{resource_type}:{original_id}"),
                    api: "v2",
                    action: "create",
                    resource_type,
                    resource_id: original_id,
                    body: Some(body),
                };
                state.set_receipt(
                    &operation,
                    HueOwnershipReceiptStatus::Succeeded,
                    Some(recovered_id),
                );
                persist_controller_ownership(storage, state)?;
                continue;
            }

            let operation = ControlPlaneOperation {
                operation_id: format!("restore:v2:{resource_type}:{original_id}"),
                api: "v2",
                action: "create",
                resource_type,
                resource_id: original_id.clone(),
                body: Some(body),
            };
            let replacement_id = run_journaled_operation(
                storage,
                key,
                state,
                transport,
                username,
                &operation,
                HueOwnershipPhase::RestoreIncomplete,
            )?
            .ok_or_else(|| anyhow::anyhow!("Hue create returned no replacement identity"))?;
            state.record_replacement(resource_type, &original_id, &replacement_id);
            persist_controller_ownership(storage, state)?;
        }
    }
    Ok(())
}

fn restore_automation_statuses<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
) -> Result<()> {
    for item in baseline_items(&state.baseline, "behavior_instance")?.to_vec() {
        let Some(resource_id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let body = if let Some(enabled) = item.get("enabled").and_then(Value::as_bool) {
            json!({"enabled": enabled})
        } else if let Some(status) = item.get("status").and_then(Value::as_str) {
            json!({"status": status})
        } else {
            continue;
        };
        let operation = ControlPlaneOperation {
            operation_id: format!("restore:v2:behavior_instance:{resource_id}"),
            api: "v2",
            action: "restore",
            resource_type: "behavior_instance",
            resource_id: resource_id.to_string(),
            body: Some(body),
        };
        run_journaled_operation(
            storage,
            key,
            state,
            transport,
            username,
            &operation,
            HueOwnershipPhase::RestoreIncomplete,
        )?;
    }

    for resource_type in REQUIRED_V1_BASELINE_RESOURCES {
        let resources = state
            .baseline
            .v1_resources
            .get(*resource_type)
            .ok_or_else(|| anyhow::anyhow!("Hue baseline has no V1 {resource_type}"))?
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Hue baseline V1 {resource_type} is not an object"))?
            .clone();
        for (resource_id, resource) in resources {
            let Some(status) = resource.get("status").and_then(Value::as_str) else {
                continue;
            };
            let operation = ControlPlaneOperation {
                operation_id: format!("restore:v1:{resource_type}:{resource_id}"),
                api: "v1",
                action: "restore",
                resource_type,
                resource_id,
                body: Some(json!({"status": status})),
            };
            run_journaled_operation(
                storage,
                key,
                state,
                transport,
                username,
                &operation,
                HueOwnershipPhase::RestoreIncomplete,
            )?;
        }
    }
    Ok(())
}

fn mark_unsupported_smart_scenes(
    storage: &dyn Storage,
    _key: &HubKey,
    state: &mut HueControllerOwnership,
) -> Result<bool> {
    let mut unsupported = false;
    for item in baseline_items(&state.baseline, "smart_scene")?.to_vec() {
        let original_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("<missing-id>")
            .to_string();
        let operation = ControlPlaneOperation {
            operation_id: format!("restore:v2:smart_scene:{original_id}"),
            api: "v2",
            action: "create",
            resource_type: "smart_scene",
            resource_id: original_id,
            body: None,
        };
        state.set_receipt(&operation, HueOwnershipReceiptStatus::Unsupported, None);
        unsupported = true;
    }
    if unsupported {
        state.phase = HueOwnershipPhase::RestoreIncomplete;
        persist_controller_ownership(storage, state)?;
    }
    Ok(unsupported)
}

fn clear_all_restorable_resources<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
) -> Result<()> {
    for resource_type in CLEAR_DELETE_ORDER {
        let mut attempted_resource_ids = BTreeSet::new();
        loop {
            let payload = transport.get_resources(username, resource_type)?;
            let mut ids = data_array(resource_type, &payload)?
                .iter()
                .filter_map(|resource| resource.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>();
            ids.sort();
            let Some(resource_id) = ids.into_iter().next() else {
                break;
            };
            if !attempted_resource_ids.insert(resource_id.clone()) {
                anyhow::bail!("A Hue release-clear mutation failed read-back verification");
            }
            let operation = ControlPlaneOperation {
                operation_id: format!("release-clear:v2:{resource_type}:{resource_id}"),
                api: "v2",
                action: "delete",
                resource_type,
                resource_id,
                body: None,
            };
            run_journaled_operation(
                storage,
                key,
                state,
                transport,
                username,
                &operation,
                HueOwnershipPhase::RestoreIncomplete,
            )?;
        }
    }
    state.managed_scenes.clear();
    state.managed_rooms.clear();
    persist_controller_ownership(storage, state)
}

fn verify_restored<H: HueTransport + ?Sized>(
    state: &HueControllerOwnership,
    transport: &H,
    username: &str,
) -> Result<()> {
    for resource_type in RESTORE_CREATE_ORDER {
        let payload = transport.get_resources(username, resource_type)?;
        let current = data_array(resource_type, &payload)?;
        for original in baseline_items(&state.baseline, resource_type)? {
            let original_id = original
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Hue baseline {resource_type} item has no ID"))?;
            let replacement_id = state
                .replacement_id(resource_type, original_id)
                .ok_or_else(|| anyhow::anyhow!("A Hue {resource_type} was not restored"))?;
            let restored = current
                .iter()
                .find(|resource| resource.get("id").and_then(Value::as_str) == Some(replacement_id))
                .ok_or_else(|| {
                    anyhow::anyhow!("A restored Hue {resource_type} is missing from the bridge")
                })?;
            let mut expected_body = create_body(resource_type, original)?;
            if *resource_type == "scene" {
                expected_body = rewrite_scene_group(state, expected_body)?;
            }
            if !resource_semantically_matches(resource_type, &expected_body, restored)? {
                anyhow::bail!(
                    "A restored Hue {resource_type} does not match its ownership baseline"
                );
            }
        }
    }

    let behavior_payload = transport.get_resources(username, "behavior_instance")?;
    let behaviors = data_array("behavior_instance", &behavior_payload)?;
    for original in baseline_items(&state.baseline, "behavior_instance")? {
        let Some(id) = original.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(current) = behaviors
            .iter()
            .find(|resource| resource.get("id").and_then(Value::as_str) == Some(id))
        else {
            anyhow::bail!("A Hue behavior instance disappeared during restore");
        };
        for field in ["enabled", "status"] {
            if original.get(field).is_some() && original.get(field) != current.get(field) {
                anyhow::bail!("A Hue behavior instance did not restore its {field} field");
            }
        }
    }

    for resource_type in REQUIRED_V1_BASELINE_RESOURCES {
        let current = transport.get_v1(username, resource_type)?;
        let current = object_resource(resource_type, &current)?;
        let original = state
            .baseline
            .v1_resources
            .get(*resource_type)
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("Hue baseline V1 {resource_type} is invalid"))?;
        for (id, original) in original {
            let expected_status = original.get("status");
            if expected_status.is_some()
                && current.get(id).and_then(|value| value.get("status")) != expected_status
            {
                anyhow::bail!("A Hue V1 {resource_type} status was not restored");
            }
        }
    }
    Ok(())
}

/// Restore the pre-takeover bridge state to the supported extent.
///
/// The manifest is deliberately retained in `restored` or
/// `restore_incomplete`; callers must invoke [`finalize_released_control`] only
/// after a verified successful restore.
pub fn release_authoritative_control<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    transport: &H,
    username: &str,
) -> Result<Option<HueControllerOwnership>> {
    let connected_id = connected_hue_bridge_id(transport, username)?;
    let Some(mut state) = load_controller_ownership(storage, &connected_id)? else {
        return Ok(None);
    };
    if state.phase == HueOwnershipPhase::Restored {
        state.ensure_bridge_id(&connected_id)?;
        if verify_restored(&state, transport, username).is_ok() {
            return Ok(Some(state));
        }
        // Finalization may have been interrupted long enough for the bridge to
        // drift. Retain recovery state and rebuild from the immutable baseline
        // rather than trusting a stale phase marker.
        state.phase = HueOwnershipPhase::RestoreIncomplete;
        persist_controller_ownership(storage, &state)?;
    }
    state.ensure_bridge_id(&connected_id)?;
    if state.phase == HueOwnershipPhase::Captured {
        // Capture is the pre-mutation durability barrier. A manifest that never
        // advanced past it has nothing to undo (including a preflight-blocked
        // smart-scene baseline).
        state.phase = HueOwnershipPhase::Restored;
        persist_controller_ownership(storage, &state)?;
        return Ok(Some(state));
    }
    ensure_baseline_restorable_for_takeover(&state.baseline)?;
    if matches!(
        state.phase,
        HueOwnershipPhase::Restoring | HueOwnershipPhase::RestoreIncomplete
    ) {
        // A retry starts from a freshly cleared bridge. Any replacement IDs
        // from the partial attempt are about to be deleted and must not cause
        // restore_resources to skip their recreation.
        state.restored_resource_ids.clear();
        persist_controller_ownership(storage, &state)?;
    }
    state.phase = HueOwnershipPhase::Restoring;
    persist_controller_ownership(storage, &state)?;

    if let Err(error) =
        clear_all_restorable_resources(storage, key, &mut state, transport, username)
    {
        state.phase = HueOwnershipPhase::RestoreIncomplete;
        persist_controller_ownership(storage, &state)?;
        return Err(error.context("Failed to clear Rhythm-managed Hue resources before restore"));
    }
    if let Err(error) = restore_resources(storage, key, &mut state, transport, username) {
        state.phase = HueOwnershipPhase::RestoreIncomplete;
        persist_controller_ownership(storage, &state)?;
        return Err(error.context("Failed to recreate Hue baseline resources"));
    }
    if let Err(error) = restore_automation_statuses(storage, key, &mut state, transport, username) {
        state.phase = HueOwnershipPhase::RestoreIncomplete;
        persist_controller_ownership(storage, &state)?;
        return Err(error.context("Failed to restore Hue automation statuses"));
    }
    if mark_unsupported_smart_scenes(storage, key, &mut state)? {
        anyhow::bail!(
            "Hue smart_scene recreation is not supported; the ownership manifest was retained for recovery"
        );
    }
    if let Err(error) = verify_restored(&state, transport, username) {
        state.phase = HueOwnershipPhase::RestoreIncomplete;
        persist_controller_ownership(storage, &state)?;
        return Err(error.context("Hue bridge restore verification failed"));
    }

    state.phase = HueOwnershipPhase::Restored;
    persist_controller_ownership(storage, &state)?;
    Ok(Some(state))
}

/// Delete a verified restored manifest. This is intentionally separate from
/// release so credential teardown can fail safely without losing recovery data.
pub fn finalize_released_control(storage: &dyn Storage, bridge_id: &str) -> Result<()> {
    let state = load_controller_ownership(storage, bridge_id)?
        .ok_or_else(|| anyhow::anyhow!("No Hue ownership manifest exists for the bridge"))?;
    if state.phase != HueOwnershipPhase::Restored {
        anyhow::bail!(
            "Refusing to delete Hue ownership manifest in {:?} phase",
            state.phase
        );
    }
    storage
        .delete_integration_state_file(&ownership_path(bridge_id)?)
        .map_err(|_| anyhow::anyhow!("Failed to delete Hue ownership manifest"))
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

        let state = acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        assert_eq!(state.phase, HueOwnershipPhase::Active);
        assert_eq!(state.baseline.bridge_id(), "bridge-1");
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
    fn takeover_fails_boundedly_when_a_confirmed_clear_is_not_observed() {
        let temp = TempStorage::new("clear-readback");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        spy.set_ignore_resource_mutations(true);

        let error = acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("an unobserved write must not loop or become active");

        assert!(error.to_string().contains("read-back verification"));
        let persisted = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(persisted.phase, HueOwnershipPhase::ClearIncomplete);
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
    fn release_fails_boundedly_when_a_confirmed_delete_is_not_observed() {
        let temp = TempStorage::new("release-clear-readback");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        spy.set_resource_response(
            "room",
            json!({"data": [{
                "id": "unobserved-room",
                "children": [{"rid": "device-1", "rtype": "device"}],
                "metadata": {"name": "Unobserved", "archetype": "living_room"}
            }], "errors": []}),
        );
        spy.set_ignore_resource_mutations(true);
        spy.reset();

        let error = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("an unobserved release delete must not loop forever");

        assert!(format!("{error:#}").contains("read-back verification"));
        let persisted = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(persisted.phase, HueOwnershipPhase::RestoreIncomplete);
        assert_eq!(
            spy.calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    HueTransportCall::DeleteResource {
                        resource_type,
                        resource_id,
                    } if resource_type == "room" && resource_id == "unobserved-room"
                ))
                .count(),
            1
        );
    }

    #[test]
    fn restore_recreates_rooms_before_scenes_and_keeps_manifest_until_finalize() {
        let temp = TempStorage::new("restore");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        spy.reset();

        let restored = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        assert_eq!(restored.phase, HueOwnershipPhase::Restored);
        let creates = spy
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                HueTransportCall::CreateResource {
                    resource_type,
                    body,
                } => Some((resource_type, body)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(creates[0].0, "room");
        assert_eq!(creates[1].0, "scene");
        assert_ne!(creates[1].1.pointer("/group/rid"), Some(&json!("old-room")));
        assert!(load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .is_some());
        finalize_released_control(&temp.storage, "bridge-1").unwrap();
        assert!(load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .is_none());
    }

    #[test]
    fn acquisition_never_reclaims_a_bridge_behind_a_restored_release_fence() {
        let temp = TempStorage::new("restored-reacquire");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        let restored = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        assert_eq!(restored.phase, HueOwnershipPhase::Restored);
        spy.reset();

        let error = acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("stale credentials must not reverse a completed release");

        assert!(error.to_string().contains("credential finalization"));
        assert_eq!(
            load_controller_ownership(&temp.storage, "bridge-1")
                .unwrap()
                .unwrap()
                .phase,
            HueOwnershipPhase::Restored
        );
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::CreateResource { .. }
                | HueTransportCall::UpdateResource { .. }
                | HueTransportCall::DeleteResource { .. }
        )));
    }

    #[test]
    fn semantic_restore_comparison_normalizes_unordered_hue_collections() {
        let expected_room = json!({
            "children": [
                {"rid": "device-a", "rtype": "device"},
                {"rid": "device-b", "rtype": "device"}
            ],
            "metadata": {"name": "Office", "archetype": "office"}
        });
        let actual_room = json!({
            "id": "replacement-room",
            "children": [
                {"rid": "device-b", "rtype": "device"},
                {"rid": "device-a", "rtype": "device"}
            ],
            "metadata": {
                "name": "Office",
                "archetype": "office",
                "bridge_generated": true
            }
        });
        assert!(resource_semantically_matches("room", &expected_room, &actual_room).unwrap());
        assert!(resource_semantically_matches("zone", &expected_room, &actual_room).unwrap());

        let expected_scene = json!({
            "group": {"rid": "room-a", "rtype": "room"},
            "actions": [
                {"target": {"rid": "light-a", "rtype": "light"}, "action": {"on": {"on": true}}},
                {"target": {"rid": "light-b", "rtype": "light"}, "action": {"dimming": {"brightness": 42.0}}}
            ],
            "palette": {
                "dimming": [{"brightness": 75.0}, {"brightness": 25.0}],
                "color_temperature": [{"mirek": 250}, {"mirek": 400}]
            }
        });
        let actual_scene = json!({
            "id": "replacement-scene",
            "group": {"rid": "room-a", "rtype": "room"},
            "actions": [
                {"target": {"rid": "light-b", "rtype": "light"}, "action": {"dimming": {"brightness": 42.0}}},
                {"target": {"rid": "light-a", "rtype": "light"}, "action": {"on": {"on": true}}}
            ],
            "palette": {
                "dimming": [{"brightness": 25.0}, {"brightness": 75.0}],
                "color_temperature": [{"mirek": 400}, {"mirek": 250}]
            }
        });
        assert!(resource_semantically_matches("scene", &expected_scene, &actual_scene).unwrap());

        let mut wrong_scene = actual_scene;
        wrong_scene["actions"][0]["action"]["dimming"]["brightness"] = json!(99.0);
        assert!(!resource_semantically_matches("scene", &expected_scene, &wrong_scene).unwrap());
    }

    #[test]
    fn restore_verification_rejects_semantic_drift_without_leaking_resource_data() {
        let temp = TempStorage::new("semantic-restore");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        let restored = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        let replacement_room = restored.replacement_id("room", "old-room").unwrap();
        let private_name = "private-customer-bedroom";
        let private_device = "private-device-identifier";
        spy.set_resource_response(
            "room",
            json!({"data": [{
                "id": replacement_room,
                "children": [{"rid": private_device, "rtype": "device"}],
                "metadata": {"name": private_name, "archetype": "living_room"}
            }], "errors": []}),
        );

        let error = verify_restored(&restored, &spy, "user").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("does not match its ownership baseline"));
        assert!(!message.contains(replacement_room));
        assert!(!message.contains(private_name));
        assert!(!message.contains(private_device));
    }

    #[test]
    fn malformed_v1_receipt_is_journaled_as_failed_not_succeeded() {
        let temp = TempStorage::new("v1-receipt");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        let private_id = "private-unrelated-rule";
        spy.set_v1_write_response_override(Some(json!([{
            "success": {format!("/rules/{private_id}/status"): "disabled"}
        }])));

        let error = acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("a mismatched success address must stop takeover");
        assert!(!error.to_string().contains(private_id));
        let state = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(state.phase, HueOwnershipPhase::ClearIncomplete);
        let v1_receipts = state
            .receipts()
            .filter(|receipt| receipt.api == "v1")
            .collect::<Vec<_>>();
        assert_eq!(v1_receipts.len(), 1);
        assert_eq!(v1_receipts[0].status, HueOwnershipReceiptStatus::Failed);
    }

    #[test]
    fn unsupported_smart_scene_blocks_takeover_before_any_bridge_mutation() {
        let temp = TempStorage::new("smart-scene");
        let spy = SpyHueTransport::new();
        seed_bridge(
            &spy,
            json!([{"id": "smart-1", "metadata": {"name": "Wake"}}]),
        );
        let error = acquire_authoritative_control(&temp.storage, &key(), &spy, "user")
            .expect_err("smart scene must block takeover before clear");
        assert!(error.to_string().contains("smart_scene"));
        let state = load_controller_ownership(&temp.storage, "bridge-1")
            .unwrap()
            .unwrap();
        assert_eq!(state.phase, HueOwnershipPhase::Captured);
        assert!(state.receipts().next().is_none());
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { .. }
                | HueTransportCall::DeleteResource { .. }
                | HueTransportCall::PutV1 { .. }
        )));
        spy.reset();
        let released = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        assert_eq!(released.phase, HueOwnershipPhase::Restored);
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { .. }
                | HueTransportCall::DeleteResource { .. }
                | HueTransportCall::PutV1 { .. }
        )));
        finalize_released_control(&temp.storage, "bridge-1").unwrap();
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

    #[test]
    fn restore_retry_recreates_ids_deleted_from_a_partial_attempt() {
        let temp = TempStorage::new("restore-retry");
        let spy = SpyHueTransport::new();
        seed_bridge(&spy, json!([]));
        acquire_authoritative_control(&temp.storage, &key(), &spy, "user").unwrap();
        let mut first = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        let first_room = first
            .replacement_id("room", "old-room")
            .unwrap()
            .to_string();
        // Simulate a crash after replacement IDs were persisted but before the
        // restore phase could advance. The next invocation must recover in one
        // pass rather than deleting those IDs and then skipping recreation.
        first.phase = HueOwnershipPhase::Restoring;
        persist_controller_ownership(&temp.storage, &first).unwrap();

        let retried = release_authoritative_control(&temp.storage, &key(), &spy, "user")
            .unwrap()
            .unwrap();
        let retried_room = retried.replacement_id("room", "old-room").unwrap();
        assert_ne!(retried_room, first_room);
        assert_eq!(retried.phase, HueOwnershipPhase::Restored);
    }
}
