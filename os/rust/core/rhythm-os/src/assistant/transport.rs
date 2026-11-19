//! Stateless MCP Streamable HTTP and typed REST projections of one registry.
use super::*;
use crate::auth::{ApiAuthRequestInfo, ApiTokenRole};
use axum::{
    extract::{Extension, Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

const VERSION: &str = "2025-06-18";
const CONTRACT_URI: &str = "rhythm://hub/assistant/contract";

pub(crate) fn agent_route(method: &Method, path: &str) -> bool {
    (method == Method::GET && path == LIGHT_ASSISTANT_CONTRACT_PATH)
        || (method == Method::POST
            && (path == MCP_PATH
                || operation_registry()
                    .iter()
                    .any(|op| op.invocation_path == path && op.tier != OperationTier::OwnerOnly)))
}

fn invalid_args() -> LightAssistantError {
    LightAssistantError::bad_request(
        "invalid_arguments",
        "Use the fields advertised by this operation",
    )
}

async fn execute(
    state: SharedState,
    name: &str,
    args: Value,
) -> Result<Value, LightAssistantError> {
    let op = operation_registry()
        .iter()
        .find(|op| op.id == name)
        .ok_or_else(|| LightAssistantError {
            status: 404,
            code: "unknown_operation",
            message: "This hub does not advertise that operation".into(),
            retryable: false,
            mutation_may_have_applied: false,
            trial_id: None,
        })?;
    let fields = args.as_object().ok_or_else(invalid_args)?;
    if op.tier == OperationTier::OwnerOnly {
        return Err(LightAssistantError {
            status: 403,
            code: "owner_required",
            message: "Hand this operation to the owner".into(),
            retryable: false,
            mutation_may_have_applied: false,
            trial_id: None,
        });
    }
    if op.tier == OperationTier::PersistentChange
        && args.get("confirmed") != Some(&Value::Bool(true))
    {
        return Err(LightAssistantError::bad_request(
            "confirmation_required",
            "Review the plan and obtain explicit confirmation before applying it",
        ));
    }
    if op.input_schema["additionalProperties"] == false
        && fields
            .keys()
            .any(|k| op.input_schema["properties"].get(k).is_none())
    {
        return Err(invalid_args());
    }
    if op.input_schema["required"]
        .as_array()
        .is_some_and(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .any(|k| !fields.contains_key(k))
        })
    {
        return Err(invalid_args());
    }
    // Existing appliance handlers perform synchronous persistence and native
    // device I/O. Never stall the Pi's single async worker while they run.
    let runtime = tokio::runtime::Handle::current();
    let mut result = tokio::task::spawn_blocking(move || match op.handler {
        Handler::Lighting => runtime.block_on(lighting::call(&state, &op.id, args)),
        Handler::Motion => runtime.block_on(motion::call(&state, &op.id, args)),
        Handler::Automation => automation::call(&state, &op.id, args),
        Handler::Devices => runtime.block_on(devices::call(&state, &op.id, args)),
        Handler::Portable => portable::call(&state, &op.id, args),
        Handler::Information => information::call(&state, &op.id, args),
        Handler::Legacy => legacy(&state, &op.id, args),
    })
    .await
    .map_err(|_| LightAssistantError {
        status: 500,
        code: "operation_interrupted",
        message: "Reread the affected resource before trying again".into(),
        retryable: false,
        mutation_may_have_applied: op.tier != OperationTier::Read,
        trial_id: None,
    })??;
    if op.tier == OperationTier::Read && result["plan_id"].is_string() {
        let target = match op.id.as_str() {
            OP_PLAN_MOVE_DEVICE_ROOM => OP_APPLY_MOVE_DEVICE_ROOM_PLAN,
            "plan_motion_target" => "apply_motion_target",
            _ => result["operation"].as_str().unwrap_or_default(),
        };
        if let Some(apply) = operation_registry()
            .iter()
            .find(|entry| entry.id == target && entry.tier == OperationTier::PersistentChange)
        {
            let mut arguments = serde_json::Map::new();
            if let Some(fields) = apply.input_schema["properties"].as_object() {
                for field in fields.keys().filter(|f| f.as_str() != "confirmed") {
                    let value = match (target, field.as_str()) {
                        (OP_APPLY_MOVE_DEVICE_ROOM_PLAN, "device_id") => result["device"].get("id"),
                        (OP_APPLY_MOVE_DEVICE_ROOM_PLAN, "from_room_id") => {
                            result["from_room"].get("id")
                        }
                        (OP_APPLY_MOVE_DEVICE_ROOM_PLAN, "to_room_id") => {
                            result["to_room"].get("id")
                        }
                        _ => result.get(field),
                    };
                    if let Some(value) = value {
                        arguments.insert(field.clone(), value.clone());
                    }
                }
            }
            result["operation"] = json!(apply.id);
            result["apply_arguments"] = Value::Object(arguments);
        }
    }
    Ok(result)
}

fn identify_error(error: anyhow::Error) -> LightAssistantError {
    use rhythm_core::controller::LightControlError;
    match error.downcast_ref::<LightControlError>() {
        None | Some(LightControlError::NotDispatched(_)) => LightAssistantError::bad_request(
            "identify_unavailable",
            "This light could not start Identify. Check its connection, light selection and Identify support.",
        ),
        Some(_) => LightAssistantError {
            status: 503,
            code: "identify_uncertain",
            message: "The hub could not verify the Identify response. Ask the resident whether the light flashed before trying again.".into(),
            retryable: false,
            mutation_may_have_applied: true,
            trial_id: None,
        },
    }
}

fn legacy(state: &SharedState, id: &str, mut args: Value) -> Result<Value, LightAssistantError> {
    if matches!(
        id,
        OP_PLAN_MOVE_DEVICE_ROOM | OP_APPLY_MOVE_DEVICE_ROOM_PLAN
    ) {
        let s = state
            .lock()
            .map_err(|_| LightAssistantError::internal("state_unavailable", "Hub unavailable"))?;
        if s.platform_context == "ha_addon" {
            return Ok(
                json!({"status":"handoff","actor":"ha_administrator","surface":"home_assistant","server_instance_id":s.server_instance_id,"instructions":"Move the device to its area in Home Assistant, then refresh this hub."}),
            );
        }
    }
    match id {
        OP_GET_TOPOLOGY_SNAPSHOT => Ok(json!(build_light_assistant_topology_snapshot(state)?)),
        OP_PLAN_MOVE_DEVICE_ROOM => {
            let plan = plan_light_assistant_device_room_move(
                state,
                serde_json::from_value(args).map_err(|_| invalid_args())?,
            )?;
            let mut s = state.lock().map_err(|_| {
                LightAssistantError::internal("state_unavailable", "Hub unavailable")
            })?;
            s.assistant_move_reviews
                .retain(|(id, expires)| id != &plan.plan_id && *expires > current_epoch_ms());
            if s.assistant_move_reviews.len() >= 32 {
                s.assistant_move_reviews.pop_front();
            }
            s.assistant_move_reviews
                .push_back((plan.plan_id.clone(), current_epoch_ms() + 300_000));
            Ok(json!(plan))
        }
        OP_APPLY_MOVE_DEVICE_ROOM_PLAN => {
            args.as_object_mut()
                .ok_or_else(invalid_args)?
                .remove("confirmed");
            let req: LightAssistantMoveApplyRequest =
                serde_json::from_value(args).map_err(|_| invalid_args())?;
            {
                let mut s = state.lock().map_err(|_| {
                    LightAssistantError::internal("state_unavailable", "Hub unavailable")
                })?;
                let pos = s
                    .assistant_move_reviews
                    .iter()
                    .position(|(id, expires)| id == &req.plan_id && *expires > current_epoch_ms())
                    .ok_or_else(|| {
                        LightAssistantError::conflict(
                            "plan_expired",
                            "Review a fresh move plan before applying it",
                        )
                    })?;
                s.assistant_move_reviews.remove(pos);
            }
            Ok(json!(apply_light_assistant_device_room_move(state, req)?))
        }
        OP_IDENTIFY_DEVICE => {
            let id = args["device_id"].as_str().ok_or_else(invalid_args)?;
            commands::do_flash_canonical_device(state, id).map_err(identify_error)?;
            Ok(json!({"server_acknowledged":true,"physical_verification":"ask_resident"}))
        }
        _ => Err(invalid_args()),
    }
}

pub(crate) async fn rest(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    auth: Option<Extension<ApiAuthRequestInfo>>,
    headers: HeaderMap,
    Json(args): Json<Value>,
) -> Response {
    if headers.contains_key("origin") && auth.and_then(|Extension(a)| a.role).is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    match execute(state, &id, args).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(error.envelope()),
        )
            .into_response(),
    }
}

fn rpc_error(id: &Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}

pub(crate) async fn mcp(
    State(state): State<SharedState>,
    auth: Option<Extension<ApiAuthRequestInfo>>,
    headers: HeaderMap,
    Json(message): Json<Value>,
) -> Response {
    // MCP accepts native agent clients. Browser SDK requests use REST with
    // an authenticated role, including on locally open installations.
    if headers.contains_key("origin") {
        return StatusCode::FORBIDDEN.into_response();
    }
    if auth.and_then(|Extension(a)| a.role) != Some(ApiTokenRole::Agent) {
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", "Bearer realm=\"Rhythm assistant\"")],
        )
            .into_response();
    }
    let supported = |v: &str| matches!(v, "2025-03-26" | "2025-06-18" | "2025-11-25");
    if headers
        .get("mcp-protocol-version")
        .is_some_and(|v| !v.to_str().is_ok_and(supported))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let id = message.get("id");
    let method = message["method"].as_str();
    if message["jsonrpc"] != "2.0"
        || method.is_none()
        || id.is_some_and(|v| !(v.is_string() || v.is_i64() || v.is_u64()))
    {
        return rpc_error(&Value::Null, -32600, "Invalid request");
    }
    let method = method.unwrap_or_default();
    let Some(id) = id else {
        return StatusCode::ACCEPTED.into_response();
    };
    let params = &message["params"];
    let result = match method {
        "initialize" => {
            let Some(version) = params["protocolVersion"].as_str() else {
                return rpc_error(id, -32602, "Missing protocolVersion");
            };
            json!({"protocolVersion":if supported(version) {version} else {VERSION},"capabilities":{"tools":{},"resources":{}},
                "serverInfo":{"name":"Rhythm","version":env!("CARGO_PKG_VERSION")},
                "instructions":"Resolve the selected hub and room. Consult the tool guidance, show plans and obtain explicit confirmation for persistent changes. Distinguish accepted commands from observed output."})
        }
        "ping" => json!({}),
        "tools/list" => {
            json!({"tools":operation_registry().iter().filter(|op|op.tier != OperationTier::OwnerOnly).map(|op|json!({
            "name":op.id,"description":op.description,"inputSchema":op.input_schema,
            "annotations":{"readOnlyHint":op.tier==OperationTier::Read,"idempotentHint":op.tier==OperationTier::Read,
                "destructiveHint":op.tier==OperationTier::PersistentChange,"openWorldHint":false},
        })).collect::<Vec<_>>()})
        }
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return rpc_error(id, -32602, "Missing tool name");
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match execute(state, name, arguments).await {
                Ok(value) => {
                    json!({"content":[{"type":"text","text":value.to_string()}],"isError":false})
                }
                Err(error) if error.code == "unknown_operation" => {
                    return rpc_error(id, -32602, "Unknown tool")
                }
                Err(error) => {
                    json!({"content":[{"type":"text","text":json!(error.envelope()).to_string()}],"isError":true})
                }
            }
        }
        "resources/list" => {
            json!({"resources":[{"uri":CONTRACT_URI,"name":"Hub operation guidance","mimeType":"application/json"}]})
        }
        "resources/templates/list" => json!({"resourceTemplates":[]}),
        "resources/read" if params["uri"] == CONTRACT_URI => {
            match build_light_assistant_contract(&state) {
                Ok(contract) => {
                    json!({"contents":[{"uri":CONTRACT_URI,"mimeType":"application/json","text":json!(contract).to_string()}]})
                }
                Err(_) => return rpc_error(id, -32603, "Contract unavailable"),
            }
        }
        "resources/read" => return rpc_error(id, -32002, "Unknown resource"),
        _ => return rpc_error(id, -32601, "Method not found"),
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

#[cfg(test)]
mod identify_tests {
    use super::*;
    use rhythm_core::controller::LightControlError;
    #[test]
    fn identify_separates_rejected_requests_from_lost_acknowledgements() {
        let rejected = identify_error(
            anyhow::Error::new(LightControlError::NotDispatched(
                "no identify action".into(),
            ))
            .context("flash"),
        );
        assert_eq!(rejected.code, "identify_unavailable");
        assert!(!rejected.mutation_may_have_applied);
        let timeout = identify_error(
            anyhow::Error::new(LightControlError::Timeout("light".into())).context("flash"),
        );
        assert_eq!(timeout.code, "identify_uncertain");
        assert!(timeout.mutation_may_have_applied);
        assert!(!timeout.retryable);
    }
}
