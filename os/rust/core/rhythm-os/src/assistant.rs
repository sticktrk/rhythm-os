//! Runtime contract and guarded topology operations for AI-assisted setup.
//!
//! The language model is deliberately not an authority. This module exposes a
//! small, self-describing contract backed by the same canonical topology and
//! mutation paths used by ordinary Rhythm clients.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::api_types::TopologyNodeDto;
use crate::commands::{self, TopologyAssignmentPrecondition};
use crate::state::{current_epoch_ms, SharedState};
use crate::topology::DevicePlacement;

pub const LIGHT_ASSISTANT_CONTRACT_PATH: &str = "/api/assistant/contract";
pub const LIGHT_ASSISTANT_TOPOLOGY_PATH: &str = "/api/assistant/topology";
pub const LIGHT_ASSISTANT_MOVE_PLAN_PATH: &str = "/api/assistant/plans/device-room-move";
pub const LIGHT_ASSISTANT_MOVE_APPLY_PATH: &str = "/api/assistant/plans/device-room-move/apply";

pub const LIGHT_ASSISTANT_CONTRACT_SCHEMA_VERSION: u32 = 1;
pub const LIGHT_ASSISTANT_TOPOLOGY_SCHEMA_VERSION: u32 = 1;
pub const LIGHT_ASSISTANT_PLAN_SCHEMA_VERSION: u32 = 1;
pub const LIGHT_ASSISTANT_RECEIPT_SCHEMA_VERSION: u32 = 1;

pub const OP_GET_TOPOLOGY_SNAPSHOT: &str = "get_topology_snapshot";
pub const OP_IDENTIFY_DEVICE: &str = "identify_device";
pub const OP_PLAN_MOVE_DEVICE_ROOM: &str = "plan_move_device_room";
pub const OP_APPLY_MOVE_DEVICE_ROOM_PLAN: &str = "apply_move_device_room_plan";

const IDENTIFY_DEVICE_PATH_TEMPLATE: &str = "/api/devices/canonical/{device_id}/flash";
const MOVE_PLACEMENT: &str = "user_override";
const MAX_CORRELATION_ID_LEN: usize = 128;

#[derive(Clone, Debug, Serialize)]
pub struct LightAssistantContractDto {
    pub schema_version: u32,
    pub contract_sha256: String,
    pub server_version: String,
    pub topology_snapshot_path: String,
    pub operations: Vec<LightAssistantOperationDto>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightAssistantOperationDto {
    pub id: String,
    pub description: String,
    pub method: String,
    pub path: String,
    pub effect: String,
    pub confirmation: String,
    pub freshness_precondition: String,
    pub verification: String,
    pub physical_confirmation_required: bool,
    pub input_schema: Value,
    pub result_schema: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightAssistantTopologySnapshotDto {
    pub schema_version: u32,
    pub contract_sha256: String,
    pub server_instance_id: String,
    pub server_version: String,
    pub observed_at_epoch_ms: u64,
    pub topology_resource_sha256: String,
    pub nodes: Vec<TopologyNodeDto>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightAssistantMovePlanRequest {
    pub device_id: String,
    pub to_room_id: String,
    pub correlation_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightAssistantNodeRefDto {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightAssistantMovePlanDto {
    pub schema_version: u32,
    pub plan_id: String,
    pub correlation_id: String,
    pub operation: String,
    pub contract_sha256: String,
    pub server_instance_id: String,
    pub topology_resource_sha256: String,
    pub device: LightAssistantNodeRefDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_room: Option<LightAssistantNodeRefDto>,
    pub to_room: LightAssistantNodeRefDto,
    pub resulting_placement: String,
    pub requires_confirmation: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightAssistantMoveApplyRequest {
    pub plan_id: String,
    pub correlation_id: String,
    pub operation: String,
    pub contract_sha256: String,
    pub server_instance_id: String,
    pub topology_resource_sha256: String,
    pub device_id: String,
    #[serde(default)]
    pub from_room_id: Option<String>,
    pub to_room_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightAssistantExecutionReceiptDto {
    pub schema_version: u32,
    pub plan_id: String,
    pub correlation_id: String,
    pub operation: String,
    pub status: String,
    pub contract_sha256: String,
    pub server_instance_id: String,
    pub topology_resource_sha256: String,
    pub device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_parent_id: Option<String>,
    pub current_parent_id: String,
    pub resulting_placement: String,
    pub server_acknowledged: bool,
    pub canonical_readback_verified: bool,
    pub physical_verification: String,
    pub completed_at_epoch_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightAssistantErrorEnvelopeDto {
    pub error: LightAssistantErrorDto,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightAssistantErrorDto {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub mutation_may_have_applied: bool,
}

#[derive(Debug)]
pub struct LightAssistantError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    pub mutation_may_have_applied: bool,
}

impl LightAssistantError {
    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: 400,
            code,
            message: message.into(),
            retryable: false,
            mutation_may_have_applied: false,
        }
    }

    fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: 409,
            code,
            message: message.into(),
            retryable: true,
            mutation_may_have_applied: false,
        }
    }

    fn internal(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: 500,
            code,
            message: message.into(),
            retryable: false,
            mutation_may_have_applied: false,
        }
    }

    fn verification_failed() -> Self {
        Self {
            status: 500,
            code: "canonical_readback_failed",
            message: "The move may have applied, but canonical readback could not verify it; refresh topology before retrying".to_string(),
            retryable: false,
            mutation_may_have_applied: true,
        }
    }

    pub fn envelope(&self) -> LightAssistantErrorEnvelopeDto {
        LightAssistantErrorEnvelopeDto {
            error: LightAssistantErrorDto {
                code: self.code.to_string(),
                message: self.message.clone(),
                retryable: self.retryable,
                mutation_may_have_applied: self.mutation_may_have_applied,
            },
        }
    }
}

fn string_schema(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn object_schema(required: &[&str], properties: Value) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties,
    })
}

fn node_ref_schema() -> Value {
    object_schema(
        &["id", "name"],
        json!({
            "id": {"type": "string"},
            "name": {"type": "string"},
        }),
    )
}

fn topology_control_schema() -> Value {
    object_schema(
        &["kind", "target_id"],
        json!({
            "kind": {"enum": ["motion", "button", "switch"]},
            "target_id": {"type": "string"},
            "inherited": {"type": "boolean", "default": false},
        }),
    )
}

fn hub_room_binding_schema() -> Value {
    object_schema(
        &["hub_key", "hub_room_id", "control_id", "light_device_ids"],
        json!({
            "hub_key": object_schema(
                &["hub_type", "address"],
                json!({
                    "hub_type": {"type": "string"},
                    "address": {"type": "string"},
                }),
            ),
            "hub_room_id": {"type": "string"},
            "control_id": {"type": "string"},
            "light_device_ids": {"type": "array", "items": {"type": "string"}},
        }),
    )
}

fn topology_node_schema() -> Value {
    object_schema(
        &["id", "name", "kind"],
        json!({
            "id": {"type": "string"},
            "name": {"type": "string"},
            "kind": {"enum": ["room", "light_device", "switch_device", "motion_sensor", "sensor", "button", "other_device"]},
            "parent_id": {"type": "string"},
            "placement": {"enum": ["hub_default", "user_override", "standalone"]},
            "controls": {"type": "array", "items": topology_control_schema()},
            "hub_room_bindings": {"type": "array", "items": hub_room_binding_schema()},
            "manufacturer": {"type": "string"},
            "model": {"type": "string"},
            "user_customized": {"type": "boolean"},
            "bootstrap_name": {"type": "string"},
        }),
    )
}

fn operation_registry() -> Vec<LightAssistantOperationDto> {
    vec![
        LightAssistantOperationDto {
            id: OP_GET_TOPOLOGY_SNAPSHOT.to_string(),
            description: "Read the current public room, device, and control topology from the connected Rhythm appliance".to_string(),
            method: "GET".to_string(),
            path: LIGHT_ASSISTANT_TOPOLOGY_PATH.to_string(),
            effect: "read_only".to_string(),
            confirmation: "none".to_string(),
            freshness_precondition: "none".to_string(),
            verification: "resource_sha256".to_string(),
            physical_confirmation_required: false,
            input_schema: object_schema(&[], json!({})),
            result_schema: object_schema(
                &["schema_version", "contract_sha256", "server_instance_id", "topology_resource_sha256", "nodes"],
                json!({
                    "schema_version": {"type": "integer"},
                    "contract_sha256": string_schema("Content hash of this appliance's assistant operation contract"),
                    "server_instance_id": string_schema("Durable identity of the connected Rhythm appliance"),
                    "server_version": {"type": "string"},
                    "observed_at_epoch_ms": {"type": "integer"},
                    "topology_resource_sha256": string_schema("Hash of the returned public topology nodes"),
                    "nodes": {"type": "array", "items": topology_node_schema()},
                }),
            ),
        },
        LightAssistantOperationDto {
            id: OP_IDENTIFY_DEVICE.to_string(),
            description: "Briefly flash one canonical light so the user can confirm which physical device is being discussed".to_string(),
            method: "POST".to_string(),
            path: IDENTIFY_DEVICE_PATH_TEMPLATE.to_string(),
            effect: "temporary_light_output".to_string(),
            confirmation: "session_probe_permission".to_string(),
            freshness_precondition: "canonical_light_device".to_string(),
            verification: "server_acknowledgement_then_user_observation".to_string(),
            physical_confirmation_required: true,
            input_schema: object_schema(
                &["device_id"],
                json!({"device_id": string_schema("Canonical light device ID from the current topology snapshot")}),
            ),
            result_schema: object_schema(&[], json!({})),
        },
        LightAssistantOperationDto {
            id: OP_PLAN_MOVE_DEVICE_ROOM.to_string(),
            description: "Prepare a non-mutating reviewed plan to move one canonical device into a Rhythm room".to_string(),
            method: "POST".to_string(),
            path: LIGHT_ASSISTANT_MOVE_PLAN_PATH.to_string(),
            effect: "proposal_only".to_string(),
            confirmation: "none".to_string(),
            freshness_precondition: "current_topology_snapshot".to_string(),
            verification: "plan_identity_and_resource_sha256".to_string(),
            physical_confirmation_required: false,
            input_schema: object_schema(
                &["device_id", "to_room_id", "correlation_id"],
                json!({
                    "device_id": string_schema("Canonical device ID from the current topology snapshot"),
                    "to_room_id": string_schema("Destination Rhythm room ID from the current topology snapshot"),
                    "correlation_id": {"type": "string", "minLength": 1, "maxLength": MAX_CORRELATION_ID_LEN},
                }),
            ),
            result_schema: object_schema(
                &["plan_id", "contract_sha256", "server_instance_id", "topology_resource_sha256", "device", "to_room", "requires_confirmation"],
                json!({
                    "schema_version": {"type": "integer"},
                    "plan_id": string_schema("Content identity of the reviewed move plan"),
                    "correlation_id": {"type": "string"},
                    "operation": {"const": OP_APPLY_MOVE_DEVICE_ROOM_PLAN},
                    "contract_sha256": {"type": "string"},
                    "server_instance_id": {"type": "string"},
                    "topology_resource_sha256": {"type": "string"},
                    "device": node_ref_schema(),
                    "from_room": {"anyOf": [node_ref_schema(), {"type": "null"}]},
                    "to_room": node_ref_schema(),
                    "resulting_placement": {"const": MOVE_PLACEMENT},
                    "requires_confirmation": {"const": true},
                    "warnings": {"type": "array", "items": {"type": "string"}},
                }),
            ),
        },
        LightAssistantOperationDto {
            id: OP_APPLY_MOVE_DEVICE_ROOM_PLAN.to_string(),
            description: "Apply one explicitly confirmed device-room move while its server, contract, topology, plan, and source-placement preconditions remain current".to_string(),
            method: "POST".to_string(),
            path: LIGHT_ASSISTANT_MOVE_APPLY_PATH.to_string(),
            effect: "persistent_topology_write".to_string(),
            confirmation: "explicit_user_confirmation".to_string(),
            freshness_precondition: "exact_plan_and_topology_match".to_string(),
            verification: "durable_commit_and_canonical_readback".to_string(),
            physical_confirmation_required: false,
            input_schema: object_schema(
                &["plan_id", "correlation_id", "operation", "contract_sha256", "server_instance_id", "topology_resource_sha256", "device_id", "to_room_id"],
                json!({
                    "plan_id": {"type": "string"},
                    "correlation_id": {"type": "string", "minLength": 1, "maxLength": MAX_CORRELATION_ID_LEN},
                    "operation": {"const": OP_APPLY_MOVE_DEVICE_ROOM_PLAN},
                    "contract_sha256": {"type": "string"},
                    "server_instance_id": {"type": "string"},
                    "topology_resource_sha256": {"type": "string"},
                    "device_id": {"type": "string"},
                    "from_room_id": {"type": ["string", "null"]},
                    "to_room_id": {"type": "string"},
                }),
            ),
            result_schema: object_schema(
                &["plan_id", "status", "server_acknowledged", "canonical_readback_verified", "physical_verification"],
                json!({
                    "schema_version": {"type": "integer"},
                    "plan_id": {"type": "string"},
                    "correlation_id": {"type": "string"},
                    "operation": {"const": OP_APPLY_MOVE_DEVICE_ROOM_PLAN},
                    "status": {"const": "applied"},
                    "contract_sha256": {"type": "string"},
                    "server_instance_id": {"type": "string"},
                    "topology_resource_sha256": {"type": "string"},
                    "device_id": {"type": "string"},
                    "previous_parent_id": {"type": ["string", "null"]},
                    "current_parent_id": {"type": "string"},
                    "resulting_placement": {"const": MOVE_PLACEMENT},
                    "server_acknowledged": {"const": true},
                    "canonical_readback_verified": {"const": true},
                    "physical_verification": {"const": "not_required"},
                    "completed_at_epoch_ms": {"type": "integer"},
                }),
            ),
        },
    ]
}

fn canonical_json_sha256(value: &Value) -> String {
    fn canonicalize(value: &Value) -> Value {
        match value {
            Value::Object(values) => {
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), canonicalize(&values[key])))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
            _ => value.clone(),
        }
    }

    let canonical = serde_json::to_vec(&canonicalize(value)).unwrap_or_default();
    Sha256::digest(canonical)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn light_assistant_contract_sha256() -> String {
    canonical_json_sha256(&json!({
        "schema_version": LIGHT_ASSISTANT_CONTRACT_SCHEMA_VERSION,
        "topology_snapshot_path": LIGHT_ASSISTANT_TOPOLOGY_PATH,
        "operations": operation_registry(),
    }))
}

pub fn build_light_assistant_contract(
    state: &SharedState,
) -> Result<LightAssistantContractDto, LightAssistantError> {
    let server_version = state
        .lock()
        .map_err(|_| {
            LightAssistantError::internal("state_unavailable", "Rhythm state is unavailable")
        })?
        .firmware_version
        .to_string();
    Ok(LightAssistantContractDto {
        schema_version: LIGHT_ASSISTANT_CONTRACT_SCHEMA_VERSION,
        contract_sha256: light_assistant_contract_sha256(),
        server_version,
        topology_snapshot_path: LIGHT_ASSISTANT_TOPOLOGY_PATH.to_string(),
        operations: operation_registry(),
    })
}

pub fn build_light_assistant_topology_snapshot(
    state: &SharedState,
) -> Result<LightAssistantTopologySnapshotDto, LightAssistantError> {
    let s = state.lock().map_err(|_| {
        LightAssistantError::internal("state_unavailable", "Rhythm state is unavailable")
    })?;
    let nodes = commands::build_topology_node_dtos(&s);
    let topology_resource_sha256 =
        commands::topology_node_dtos_resource_sha256(&nodes).map_err(|_| {
            LightAssistantError::internal(
                "snapshot_failed",
                "Rhythm could not build a topology snapshot",
            )
        })?;
    Ok(LightAssistantTopologySnapshotDto {
        schema_version: LIGHT_ASSISTANT_TOPOLOGY_SCHEMA_VERSION,
        contract_sha256: light_assistant_contract_sha256(),
        server_instance_id: s.server_instance_id.clone(),
        server_version: s.firmware_version.to_string(),
        observed_at_epoch_ms: current_epoch_ms(),
        topology_resource_sha256,
        nodes,
    })
}

fn validated_correlation_id(value: &str) -> Result<String, LightAssistantError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_CORRELATION_ID_LEN
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(LightAssistantError::bad_request(
            "invalid_correlation_id",
            "correlation_id must contain 1-128 ASCII letters, numbers, '.', ':', '_' or '-'",
        ));
    }
    Ok(value.to_string())
}

fn validate_id(value: &str, field: &'static str) -> Result<String, LightAssistantError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 256 {
        return Err(LightAssistantError::bad_request(
            "invalid_target",
            format!("{field} must contain 1-256 characters"),
        ));
    }
    Ok(value.to_string())
}

fn move_plan_id(
    contract_sha256: &str,
    server_instance_id: &str,
    topology_resource_sha256: &str,
    correlation_id: &str,
    device_id: &str,
    from_room_id: Option<&str>,
    to_room_id: &str,
) -> String {
    format!(
        "move-{}",
        canonical_json_sha256(&json!({
            "contract_sha256": contract_sha256,
            "server_instance_id": server_instance_id,
            "topology_resource_sha256": topology_resource_sha256,
            "correlation_id": correlation_id,
            "device_id": device_id,
            "from_room_id": from_room_id,
            "to_room_id": to_room_id,
        }))
    )
}

pub fn plan_light_assistant_device_room_move(
    state: &SharedState,
    request: LightAssistantMovePlanRequest,
) -> Result<LightAssistantMovePlanDto, LightAssistantError> {
    let correlation_id = validated_correlation_id(&request.correlation_id)?;
    let device_id = validate_id(&request.device_id, "device_id")?;
    let to_room_id = validate_id(&request.to_room_id, "to_room_id")?;
    let contract_sha256 = light_assistant_contract_sha256();

    let s = state.lock().map_err(|_| {
        LightAssistantError::internal("state_unavailable", "Rhythm state is unavailable")
    })?;
    let node = s.topology.get_device_node(&device_id).ok_or_else(|| {
        LightAssistantError::bad_request(
            "device_not_found",
            "The selected canonical device does not exist",
        )
    })?;
    let canonical = s.canonical_registry.get(&device_id).ok_or_else(|| {
        LightAssistantError::bad_request(
            "device_not_found",
            "The selected canonical device does not exist",
        )
    })?;
    let to_room = s.topology.get(&to_room_id).ok_or_else(|| {
        LightAssistantError::bad_request(
            "room_not_found",
            "The destination Rhythm room does not exist",
        )
    })?;
    if node.parent_id.as_deref() == Some(to_room_id.as_str()) {
        return Err(LightAssistantError::bad_request(
            "no_change",
            "The selected device is already assigned to the destination room",
        ));
    }

    let from_room = node
        .parent_id
        .as_deref()
        .and_then(|room_id| s.topology.get(room_id))
        .map(|room| LightAssistantNodeRefDto {
            id: room.id.clone(),
            name: room.name.clone(),
        });
    let nodes = commands::build_topology_node_dtos(&s);
    let topology_resource_sha256 =
        commands::topology_node_dtos_resource_sha256(&nodes).map_err(|_| {
            LightAssistantError::internal(
                "snapshot_failed",
                "Rhythm could not build a topology snapshot",
            )
        })?;
    let server_instance_id = s.server_instance_id.clone();
    let from_room_id = node.parent_id.clone();
    let warnings = if matches!(node.placement, DevicePlacement::UserOverride) {
        Vec::new()
    } else {
        vec!["move_creates_user_override".to_string()]
    };
    let plan_id = move_plan_id(
        &contract_sha256,
        &server_instance_id,
        &topology_resource_sha256,
        &correlation_id,
        &device_id,
        from_room_id.as_deref(),
        &to_room_id,
    );

    Ok(LightAssistantMovePlanDto {
        schema_version: LIGHT_ASSISTANT_PLAN_SCHEMA_VERSION,
        plan_id,
        correlation_id,
        operation: OP_APPLY_MOVE_DEVICE_ROOM_PLAN.to_string(),
        contract_sha256,
        server_instance_id,
        topology_resource_sha256,
        device: LightAssistantNodeRefDto {
            id: device_id,
            name: canonical.name.clone(),
        },
        from_room,
        to_room: LightAssistantNodeRefDto {
            id: to_room.id.clone(),
            name: to_room.name.clone(),
        },
        resulting_placement: MOVE_PLACEMENT.to_string(),
        requires_confirmation: true,
        warnings,
    })
}

fn map_guarded_move_error(error: anyhow::Error) -> LightAssistantError {
    if commands::native_room_assignment_rollback_failed(&error) {
        return LightAssistantError {
            status: 500,
            code: "apply_failed",
            message: "Rhythm could not confirm the reviewed move outcome; refresh topology before taking another action".to_string(),
            retryable: false,
            mutation_may_have_applied: true,
        };
    }
    let message = error.to_string();
    if message.starts_with("assistant topology precondition failed: ") {
        return if message.contains("server_instance_changed") {
            LightAssistantError::conflict(
                "server_instance_changed",
                "The connected Rhythm appliance changed; refresh and create a new plan",
            )
        } else if message.contains("topology_changed") {
            LightAssistantError::conflict(
                "topology_changed",
                "Rhythm topology changed; refresh and create a new plan",
            )
        } else if message.contains("source_placement_changed") {
            LightAssistantError::conflict(
                "source_placement_changed",
                "The device placement changed; refresh and create a new plan",
            )
        } else {
            LightAssistantError::conflict(
                "plan_stale",
                "The reviewed plan is stale; refresh and create a new plan",
            )
        };
    }
    LightAssistantError {
        status: 500,
        code: "apply_failed",
        message: "Rhythm could not confirm the reviewed move outcome; refresh topology before taking another action".to_string(),
        retryable: false,
        mutation_may_have_applied: true,
    }
}

pub fn apply_light_assistant_device_room_move(
    state: &SharedState,
    request: LightAssistantMoveApplyRequest,
) -> Result<LightAssistantExecutionReceiptDto, LightAssistantError> {
    let correlation_id = validated_correlation_id(&request.correlation_id)?;
    let device_id = validate_id(&request.device_id, "device_id")?;
    let to_room_id = validate_id(&request.to_room_id, "to_room_id")?;
    let from_room_id = request
        .from_room_id
        .as_deref()
        .map(|value| validate_id(value, "from_room_id"))
        .transpose()?;
    let current_contract_sha256 = light_assistant_contract_sha256();
    if request.operation != OP_APPLY_MOVE_DEVICE_ROOM_PLAN {
        return Err(LightAssistantError::bad_request(
            "invalid_operation",
            "The reviewed plan operation is not supported by this endpoint",
        ));
    }
    if request.contract_sha256 != current_contract_sha256 {
        return Err(LightAssistantError::conflict(
            "contract_changed",
            "The Rhythm assistant contract changed; refresh and create a new plan",
        ));
    }
    let expected_plan_id = move_plan_id(
        &request.contract_sha256,
        &request.server_instance_id,
        &request.topology_resource_sha256,
        &correlation_id,
        &device_id,
        from_room_id.as_deref(),
        &to_room_id,
    );
    if request.plan_id != expected_plan_id {
        return Err(LightAssistantError::bad_request(
            "plan_identity_mismatch",
            "The apply request does not match the reviewed plan",
        ));
    }

    let precondition = TopologyAssignmentPrecondition {
        expected_server_instance_id: request.server_instance_id.clone(),
        expected_resource_sha256: request.topology_resource_sha256.clone(),
        expected_parent_id: from_room_id.clone(),
    };
    commands::do_canonical_assign_room_with_precondition(
        state,
        &device_id,
        Some(&to_room_id),
        Some(precondition),
    )
    .map_err(|error| {
        let mapped = map_guarded_move_error(error);
        log::warn!(
            target: "assistant",
            "Assistant plan apply failed plan={} correlation={} operation={} code={}",
            request.plan_id,
            correlation_id,
            OP_APPLY_MOVE_DEVICE_ROOM_PLAN,
            mapped.code,
        );
        mapped
    })?;

    let snapshot = build_light_assistant_topology_snapshot(state)
        .map_err(|_| LightAssistantError::verification_failed())?;
    let readback = snapshot.nodes.iter().find(|node| node.id == device_id);
    if !readback.is_some_and(|node| {
        node.parent_id.as_deref() == Some(to_room_id.as_str())
            && matches!(node.placement, Some(DevicePlacement::UserOverride))
    }) {
        return Err(LightAssistantError::verification_failed());
    }

    let mut activity =
        crate::activity::LightActivityRecord::app(&device_id, "assistant_move_device_room");
    activity.correlation_id = Some(correlation_id.clone());
    activity.payload = Some(json!({
        "status": "applied",
        "placement": MOVE_PLACEMENT,
        "canonical_readback_verified": true,
    }));
    crate::activity::record_light_activity(state, activity);

    log::info!(
        target: "assistant",
        "Applied assistant plan {} correlation={} operation={} canonical_readback_verified=true",
        request.plan_id,
        correlation_id,
        OP_APPLY_MOVE_DEVICE_ROOM_PLAN,
    );

    Ok(LightAssistantExecutionReceiptDto {
        schema_version: LIGHT_ASSISTANT_RECEIPT_SCHEMA_VERSION,
        plan_id: request.plan_id,
        correlation_id,
        operation: OP_APPLY_MOVE_DEVICE_ROOM_PLAN.to_string(),
        status: "applied".to_string(),
        contract_sha256: current_contract_sha256,
        server_instance_id: snapshot.server_instance_id,
        topology_resource_sha256: snapshot.topology_resource_sha256,
        device_id,
        previous_parent_id: from_room_id,
        current_parent_id: to_room_id,
        resulting_placement: MOVE_PLACEMENT.to_string(),
        server_acknowledged: true,
        canonical_readback_verified: true,
        physical_verification: "not_required".to_string(),
        completed_at_epoch_ms: current_epoch_ms(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_types::TopologyNodeControlDto;
    use crate::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
    use crate::canonical::registry::ResolveResult;
    use crate::hub::HubType;
    use crate::state::AppState;
    use crate::topology::{NodeControlKind, TopologyRoom};
    use rhythm_core::runtime::hub_registry::DeviceType;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn assistant_test_state() -> (SharedState, String) {
        let mut app = AppState::default();
        app.server_instance_id = "srv-assistant-test".to_string();
        app.topology
            .insert_room(TopologyRoom::new("bathroom", "Bathroom"));
        app.topology
            .insert_room(TopologyRoom::new("foyer", "Foyer"));

        let hub_key = HubKey::new(HubType::new(HubType::MATTER), "fabric-1");
        let identity = DiscoveredIdentity {
            native_id: "matter-1".to_string(),
            room_id: Some("bathroom-native".to_string()),
            room_name: Some("Bathroom".to_string()),
            name: "Ceiling Light".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("matter-1")],
            manufacturer: None,
            model: None,
        };
        let device_id = match app.canonical_registry.resolve(&identity, &hub_key, 1000) {
            ResolveResult::Created { canonical_id }
            | ResolveResult::AlreadyKnown { canonical_id } => canonical_id,
            other => panic!("unexpected canonical resolve result: {other:?}"),
        };
        app.topology.ensure_standalone_device(&device_id);
        app.topology
            .assign_device(&device_id, Some("bathroom"), DevicePlacement::HubDefault);
        app.canonical_registry
            .assign_room(&device_id, Some("bathroom"));

        (Arc::new(Mutex::new(app)), device_id)
    }

    #[test]
    fn contract_is_content_hashed_and_marks_identify_as_physical() {
        let (state, _) = assistant_test_state();
        let contract = build_light_assistant_contract(&state).unwrap();
        assert_eq!(contract.schema_version, 1);
        assert_eq!(contract.contract_sha256.len(), 64);
        assert_eq!(contract.operations.len(), 4);
        let identify = contract
            .operations
            .iter()
            .find(|operation| operation.id == OP_IDENTIFY_DEVICE)
            .unwrap();
        assert!(identify.physical_confirmation_required);
        assert_eq!(identify.effect, "temporary_light_output");
        assert_eq!(
            identify.verification,
            "server_acknowledgement_then_user_observation"
        );
        let snapshot = contract
            .operations
            .iter()
            .find(|operation| operation.id == OP_GET_TOPOLOGY_SNAPSHOT)
            .unwrap();
        assert_eq!(
            snapshot.result_schema["properties"]["nodes"]["items"]["additionalProperties"],
            false
        );
    }

    #[test]
    fn topology_control_schema_matches_omitted_false_wire_default() {
        let schema = topology_control_schema();
        assert_eq!(schema["required"], json!(["kind", "target_id"]));
        assert_eq!(schema["properties"]["inherited"]["type"], "boolean");
        assert_eq!(schema["properties"]["inherited"]["default"], false);

        let explicit = serde_json::to_value(TopologyNodeControlDto {
            kind: NodeControlKind::Motion,
            target_id: "room-explicit".to_string(),
            inherited: false,
        })
        .unwrap();
        assert_eq!(
            explicit,
            json!({"kind": "motion", "target_id": "room-explicit"})
        );

        let inherited = serde_json::to_value(TopologyNodeControlDto {
            kind: NodeControlKind::Button,
            target_id: "room-inherited".to_string(),
            inherited: true,
        })
        .unwrap();
        assert_eq!(
            inherited,
            json!({
                "kind": "button",
                "target_id": "room-inherited",
                "inherited": true,
            })
        );

        for control in [explicit, inherited] {
            let object = control.as_object().unwrap();
            for required in schema["required"].as_array().unwrap() {
                assert!(object.contains_key(required.as_str().unwrap()));
            }
            assert!(object
                .keys()
                .all(|key| schema["properties"].get(key).is_some()));
            if let Some(inherited) = object.get("inherited") {
                assert!(inherited.is_boolean());
            } else {
                assert_eq!(schema["properties"]["inherited"]["default"], false);
            }
        }
    }

    #[test]
    fn assistant_write_requests_reject_unknown_fields() {
        let request = json!({
            "device_id": "canonical-device",
            "to_room_id": "bathroom",
            "correlation_id": "setup-unknown-field",
            "prompt": "move the ceiling light",
        });

        assert!(serde_json::from_value::<LightAssistantMovePlanRequest>(request).is_err());
    }

    #[test]
    fn unclassified_apply_failure_requires_refresh_instead_of_retry() {
        let error = map_guarded_move_error(anyhow::anyhow!("runtime reconciliation failed"));

        assert_eq!(error.status, 500);
        assert_eq!(error.code, "apply_failed");
        assert!(!error.retryable);
        assert!(error.mutation_may_have_applied);
    }

    #[test]
    fn failed_native_rollback_is_uncertain_and_retains_recovery_fence() {
        let (state, device_id) = assistant_test_state();
        let plan = plan_light_assistant_device_room_move(
            &state,
            LightAssistantMovePlanRequest {
                device_id: device_id.clone(),
                to_room_id: "foyer".to_string(),
                correlation_id: "setup-rollback-failure".to_string(),
            },
        )
        .unwrap();
        let hub_key = HubKey::new(HubType::new(HubType::MATTER), "fabric-1");
        let recovery_requests = Arc::new(AtomicUsize::new(0));
        {
            let recovery_requests = recovery_requests.clone();
            let mut app = state.lock().unwrap();
            app.topology
                .set_grouped_room_control_required(&hub_key, true);
            app.request_hub_bootstrap_fn = Some(Arc::new(move |state| {
                assert!(state.try_lock().is_ok());
                recovery_requests.fetch_add(1, Ordering::SeqCst);
            }));
        }
        state.lock().unwrap().prepare_hub_device_room_assignment_fn =
            Some(Arc::new(move |state, assignment| {
                state
                    .lock()
                    .unwrap()
                    .topology
                    .get_mut("foyer")
                    .unwrap()
                    .name = "Foyer changed concurrently".to_string();
                Ok(
                    crate::hub::HubDeviceRoomAssignmentOutcome::ReassignedWithBinding {
                        target_binding: Some(crate::topology::HubRoomBinding {
                            hub_key: assignment.hub_key.clone(),
                            hub_room_id: "managed-foyer".to_string(),
                            control_id: "grouped-managed-foyer".to_string(),
                            light_device_ids: vec![assignment.native_device_id.clone()],
                        }),
                        managed_by_rhythm: true,
                        rollback: Box::new(|| {
                            anyhow::bail!("simulated native compensation failure")
                        }),
                    },
                )
            }));

        let error = apply_light_assistant_device_room_move(
            &state,
            LightAssistantMoveApplyRequest {
                plan_id: plan.plan_id,
                correlation_id: plan.correlation_id,
                operation: plan.operation,
                contract_sha256: plan.contract_sha256,
                server_instance_id: plan.server_instance_id,
                topology_resource_sha256: plan.topology_resource_sha256,
                device_id: plan.device.id,
                from_room_id: plan.from_room.map(|room| room.id),
                to_room_id: plan.to_room.id,
            },
        )
        .unwrap_err();

        assert_eq!(error.status, 500);
        assert_eq!(error.code, "apply_failed");
        assert!(!error.retryable);
        assert!(error.mutation_may_have_applied);
        assert_eq!(recovery_requests.load(Ordering::SeqCst), 1);
        let app = state.lock().unwrap();
        assert_eq!(
            app.topology.device_parent_room_id(&device_id),
            Some("bathroom")
        );
        assert!(app.external_controller_authority_pending.contains(&hub_key));
        assert!(!app.external_controller_authority_is_ready(&hub_key));
    }

    #[test]
    fn plan_and_apply_move_return_guarded_verified_receipt() {
        let (state, device_id) = assistant_test_state();
        let plan = plan_light_assistant_device_room_move(
            &state,
            LightAssistantMovePlanRequest {
                device_id: device_id.clone(),
                to_room_id: "foyer".to_string(),
                correlation_id: "setup-move-123".to_string(),
            },
        )
        .unwrap();
        assert_eq!(plan.from_room.as_ref().unwrap().id, "bathroom");
        assert_eq!(plan.to_room.id, "foyer");
        assert_eq!(plan.warnings, vec!["move_creates_user_override"]);

        let receipt = apply_light_assistant_device_room_move(
            &state,
            LightAssistantMoveApplyRequest {
                plan_id: plan.plan_id,
                correlation_id: plan.correlation_id,
                operation: plan.operation,
                contract_sha256: plan.contract_sha256,
                server_instance_id: plan.server_instance_id,
                topology_resource_sha256: plan.topology_resource_sha256,
                device_id: plan.device.id,
                from_room_id: plan.from_room.map(|room| room.id),
                to_room_id: plan.to_room.id,
            },
        )
        .unwrap();

        assert_eq!(receipt.status, "applied");
        assert!(receipt.server_acknowledged);
        assert!(receipt.canonical_readback_verified);
        assert_eq!(receipt.physical_verification, "not_required");
        let locked = state.lock().unwrap();
        assert_eq!(
            locked.topology.device_parent_room_id(&device_id),
            Some("foyer")
        );
        let activity = locked.light_activity.first().unwrap();
        assert_eq!(activity.action_id, "assistant_move_device_room");
        assert_eq!(activity.correlation_id.as_deref(), Some("setup-move-123"));
        let serialized = serde_json::to_string(activity).unwrap();
        assert!(!serialized.contains("Bathroom"));
        assert!(!serialized.contains("Foyer"));
        assert!(!serialized.contains(&receipt.topology_resource_sha256));
    }

    #[test]
    fn stale_topology_rejects_apply_without_a_move_or_activity() {
        let (state, device_id) = assistant_test_state();
        let plan = plan_light_assistant_device_room_move(
            &state,
            LightAssistantMovePlanRequest {
                device_id: device_id.clone(),
                to_room_id: "foyer".to_string(),
                correlation_id: "setup-move-stale".to_string(),
            },
        )
        .unwrap();
        state.lock().unwrap().topology.rename_room("foyer", "Entry");

        let error = apply_light_assistant_device_room_move(
            &state,
            LightAssistantMoveApplyRequest {
                plan_id: plan.plan_id,
                correlation_id: plan.correlation_id,
                operation: plan.operation,
                contract_sha256: plan.contract_sha256,
                server_instance_id: plan.server_instance_id,
                topology_resource_sha256: plan.topology_resource_sha256,
                device_id: plan.device.id,
                from_room_id: plan.from_room.map(|room| room.id),
                to_room_id: plan.to_room.id,
            },
        )
        .unwrap_err();

        assert_eq!(error.status, 409);
        assert_eq!(error.code, "topology_changed");
        let locked = state.lock().unwrap();
        assert_eq!(
            locked.topology.device_parent_room_id(&device_id),
            Some("bathroom")
        );
        assert!(locked.light_activity.is_empty());
    }

    #[test]
    fn changed_server_and_source_placement_have_distinct_stale_codes() {
        let cases = [
            ("server", "server_instance_changed"),
            ("source", "source_placement_changed"),
        ];
        for (change, expected_code) in cases {
            let (state, device_id) = assistant_test_state();
            let plan = plan_light_assistant_device_room_move(
                &state,
                LightAssistantMovePlanRequest {
                    device_id: device_id.clone(),
                    to_room_id: "foyer".to_string(),
                    correlation_id: format!("setup-{change}-stale"),
                },
            )
            .unwrap();
            {
                let mut locked = state.lock().unwrap();
                if change == "server" {
                    locked.server_instance_id = "srv-replaced".to_string();
                } else {
                    locked.canonical_registry.assign_room(&device_id, None);
                    locked
                        .topology
                        .assign_device(&device_id, None, DevicePlacement::UserOverride);
                }
            }

            let error = apply_light_assistant_device_room_move(
                &state,
                LightAssistantMoveApplyRequest {
                    plan_id: plan.plan_id,
                    correlation_id: plan.correlation_id,
                    operation: plan.operation,
                    contract_sha256: plan.contract_sha256,
                    server_instance_id: plan.server_instance_id,
                    topology_resource_sha256: plan.topology_resource_sha256,
                    device_id: plan.device.id,
                    from_room_id: plan.from_room.map(|room| room.id),
                    to_room_id: plan.to_room.id,
                },
            )
            .unwrap_err();

            assert_eq!(error.status, 409);
            assert_eq!(error.code, expected_code);
            assert!(state.lock().unwrap().light_activity.is_empty());
        }
    }

    #[test]
    fn changed_contract_and_plan_identity_are_rejected_before_mutation() {
        let cases = [
            ("contract", "contract_changed", 409),
            ("plan", "plan_identity_mismatch", 400),
        ];
        for (change, expected_code, expected_status) in cases {
            let (state, device_id) = assistant_test_state();
            let plan = plan_light_assistant_device_room_move(
                &state,
                LightAssistantMovePlanRequest {
                    device_id: device_id.clone(),
                    to_room_id: "foyer".to_string(),
                    correlation_id: format!("setup-{change}-invalid"),
                },
            )
            .unwrap();
            let mut request = LightAssistantMoveApplyRequest {
                plan_id: plan.plan_id,
                correlation_id: plan.correlation_id,
                operation: plan.operation,
                contract_sha256: plan.contract_sha256,
                server_instance_id: plan.server_instance_id,
                topology_resource_sha256: plan.topology_resource_sha256,
                device_id: plan.device.id,
                from_room_id: plan.from_room.map(|room| room.id),
                to_room_id: plan.to_room.id,
            };
            if change == "contract" {
                request.contract_sha256 = "0".repeat(64);
            } else {
                request.plan_id = "move-not-the-reviewed-plan".to_string();
            }

            let error = apply_light_assistant_device_room_move(&state, request).unwrap_err();
            assert_eq!(error.status, expected_status);
            assert_eq!(error.code, expected_code);
            assert_eq!(
                state
                    .lock()
                    .unwrap()
                    .topology
                    .device_parent_room_id(&device_id),
                Some("bathroom")
            );
        }
    }

    #[test]
    fn unassigned_device_can_be_planned_and_applied_without_a_source_room() {
        let (state, device_id) = assistant_test_state();
        {
            let mut locked = state.lock().unwrap();
            locked.canonical_registry.assign_room(&device_id, None);
            locked
                .topology
                .assign_device(&device_id, None, DevicePlacement::Standalone);
        }
        let plan = plan_light_assistant_device_room_move(
            &state,
            LightAssistantMovePlanRequest {
                device_id: device_id.clone(),
                to_room_id: "foyer".to_string(),
                correlation_id: "setup-unassigned-1".to_string(),
            },
        )
        .unwrap();
        assert!(plan.from_room.is_none());

        let receipt = apply_light_assistant_device_room_move(
            &state,
            LightAssistantMoveApplyRequest {
                plan_id: plan.plan_id,
                correlation_id: plan.correlation_id,
                operation: plan.operation,
                contract_sha256: plan.contract_sha256,
                server_instance_id: plan.server_instance_id,
                topology_resource_sha256: plan.topology_resource_sha256,
                device_id: plan.device.id,
                from_room_id: None,
                to_room_id: plan.to_room.id,
            },
        )
        .unwrap();

        assert!(receipt.previous_parent_id.is_none());
        assert_eq!(receipt.current_parent_id, "foyer");
        assert_eq!(
            state
                .lock()
                .unwrap()
                .topology
                .device_parent_room_id(&device_id),
            Some("foyer")
        );
    }
}
