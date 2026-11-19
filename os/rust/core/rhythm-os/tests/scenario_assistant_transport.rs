//! Exercise both discovery and authorization through the shipped router.
mod harness;

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    middleware, Router,
};
use rhythm_os::{
    assistant::{operation_registry, OperationTier, MCP_PATH},
    auth, axum_router,
    state::{AppState, SharedState},
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn router() -> (Router, String) {
    let state: SharedState = Arc::new(Mutex::new(AppState {
        server_instance_id: "fixture".into(),
        ..Default::default()
    }));
    router_for(state)
}

fn router_for(state: SharedState) -> (Router, String) {
    let owner = auth::issue_local_owner_token(&state, None).unwrap();
    let router =
        axum_router::api_routes()
            .with_state(state.clone())
            .layer(middleware::from_fn_with_state(
                state,
                auth::require_api_auth_middleware,
            ));
    (router, owner.token)
}
async fn request(
    r: &Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"))
    }
    let response = r
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4_000_000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn browser_calls_to_rest_require_a_credential_on_an_open_hub() {
    let (router, owner) = router();
    for (origin, token, expected) in [
        (true, None, StatusCode::FORBIDDEN),
        (false, None, StatusCode::OK),
        (true, Some(owner.as_str()), StatusCode::OK),
    ] {
        let mut req = Request::post("/api/assistant/operations/read_hub_status")
            .header("content-type", "application/json");
        if origin {
            req = req.header("origin", "https://ui.example");
        }
        if let Some(token) = token {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let res = router
            .clone()
            .oneshot(req.body(Body::from("{}")).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), expected);
    }
}

#[tokio::test]
async fn scoped_tools_resources_and_revocation() {
    let (r, owner) = router();
    let issue = "/api/auth/agent-tokens";
    assert_eq!(
        request(&r, Method::POST, issue, None, json!({})).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, issued) = request(&r, Method::POST, issue, Some(&owner), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let token = Some(issued["token"].as_str().unwrap());
    let rpc = |method, params| json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    let list = rpc("tools/list", json!({}));
    assert_eq!(
        request(&r, Method::POST, MCP_PATH, None, list.clone())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, init) = request(
        &r,
        Method::POST,
        MCP_PATH,
        token,
        rpc("initialize", json!({"protocolVersion":"2025-06-18"})),
    )
    .await;
    assert!(init["result"]["capabilities"]["tools"].is_object());
    let (_, tools) = request(&r, Method::POST, MCP_PATH, token, list.clone()).await;
    let tools = tools["result"]["tools"].as_array().unwrap();
    for op in operation_registry()
        .iter()
        .filter(|op| op.tier != OperationTier::OwnerOnly)
    {
        let tool = tools.iter().find(|t| t["name"] == op.id).unwrap();
        assert_eq!(tool["inputSchema"], op.input_schema);
        assert_eq!(
            tool["annotations"]["destructiveHint"],
            op.tier == OperationTier::PersistentChange
        );
        assert_eq!(
            tool["annotations"]["idempotentHint"],
            op.tier == OperationTier::Read
        );
        assert_eq!(
            tool["annotations"]["readOnlyHint"],
            op.tier == OperationTier::Read
        );
    }
    let (_, resources) = request(
        &r,
        Method::POST,
        MCP_PATH,
        token,
        rpc(
            "resources/read",
            json!({"uri":"rhythm://hub/assistant/contract"}),
        ),
    )
    .await;
    let contract: Value =
        serde_json::from_str(resources["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert!(contract["operations"].as_array().unwrap().len() >= tools.len());
    let (_, out) = request(
        &r,
        Method::POST,
        MCP_PATH,
        token,
        rpc(
            "tools/call",
            json!({"name":"read_hub_status","arguments":{}}),
        ),
    )
    .await;
    let result: Value =
        serde_json::from_str(out["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["server_instance_id"], "fixture");
    for path in [
        "/api/backup?include_secrets=true",
        "/api/pairing/wifi-credentials",
        "/api/matter/setup-code/1",
        issue,
    ] {
        assert_eq!(
            request(&r, Method::GET, path, token, json!({})).await.0,
            StatusCode::FORBIDDEN
        );
    }
    let (_, err) = request(
        &r,
        Method::POST,
        MCP_PATH,
        token,
        rpc(
            "tools/call",
            json!({"name":"apply_room_tuning","arguments":{"plan_id":"old"}}),
        ),
    )
    .await;
    assert_eq!(err["result"]["isError"], true);
    assert!(err["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("confirmation_required"));
    let revoked = format!("{issue}/{}", issued["token_id"].as_str().unwrap());
    assert_eq!(
        request(&r, Method::DELETE, &revoked, Some(&owner), Value::Null)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&r, Method::POST, MCP_PATH, token, list).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn settings_preview_and_stale_transfer_never_mutate_the_hub() {
    let (r, owner) = router();
    let op = |id: &str| format!("/api/assistant/operations/{id}");
    let (_, settings) = request(
        &r,
        Method::POST,
        &op("export_lighting_settings"),
        Some(&owner),
        json!({}),
    )
    .await;
    let (status, plan) = request(
        &r,
        Method::POST,
        &op("plan_lighting_transfer"),
        Some(&owner),
        json!({"settings":settings,"node_mappings":{}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["operation"], "apply_lighting_transfer");
    let mut apply = plan["apply_arguments"].clone();
    assert_eq!(
        request(
            &r,
            Method::POST,
            &op("apply_lighting_transfer"),
            Some(&owner),
            apply.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (_, current) = request(
        &r,
        Method::POST,
        &op("export_lighting_settings"),
        Some(&owner),
        json!({}),
    )
    .await;
    assert_eq!(current, settings);
    let (_, status) = request(
        &r,
        Method::POST,
        &op("read_hub_status"),
        Some(&owner),
        json!({}),
    )
    .await;
    let changed = !status["light_breaker_enabled"].as_bool().unwrap();
    assert_eq!(
        request(
            &r,
            Method::PUT,
            "/api/light-breaker",
            Some(&owner),
            json!({"enabled":changed})
        )
        .await
        .0,
        StatusCode::OK
    );
    apply["confirmed"] = json!(true);
    assert_eq!(
        request(
            &r,
            Method::POST,
            &op("apply_lighting_transfer"),
            Some(&owner),
            apply
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, after) = request(
        &r,
        Method::POST,
        &op("export_lighting_settings"),
        Some(&owner),
        json!({}),
    )
    .await;
    assert_eq!(after, current);
}

#[test]
fn descriptor_projections_and_ledger_are_in_sync() {
    let registry = operation_registry();
    let policy = include_str!("../../../../../admin-api/lib/src/assistant_read_operations.dart");
    for (name, expected) in [
        (
            "assistantReadOperations",
            registry
                .iter()
                .filter(|op| op.tier == OperationTier::Read)
                .map(|op| op.id.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
        ),
        (
            "assistantHouseholdOperations",
            registry
                .iter()
                .filter(|op| op.observes_household)
                .map(|op| op.id.as_str())
                .collect(),
        ),
    ] {
        let header = format!("const {name} = <String>{{");
        let section = policy
            .split_once(&header)
            .unwrap()
            .1
            .split_once("};")
            .unwrap()
            .0;
        let shipped: std::collections::BTreeSet<_> = section
            .lines()
            .filter_map(|l| l.trim().strip_prefix('\'')?.strip_suffix("',"))
            .collect();
        assert_eq!(
            expected, shipped,
            "Regenerate {name} with the assistant_read_policy example."
        );
    }
    let ledger = include_str!("../../../../../docs/architecture/assistant-route-coverage.tsv");
    for line in ledger.lines().skip(1) {
        let fields: Vec<_> = line.split('\t').collect();
        if fields[3] == "operation" {
            assert!(
                registry.iter().any(|op| op.id == fields[4]),
                "Unadvertised operation for {line}"
            );
        }
    }
    for op in registry {
        assert_eq!(
            op.invocation_path,
            format!("/api/assistant/operations/{}", op.id)
        );
        assert!(op.input_schema["properties"].is_object());
    }
}

async fn agent(r: &Router, owner: &str, unattended: bool) -> Value {
    let (status, issued) = request(
        r,
        Method::POST,
        "/api/auth/agent-tokens",
        Some(owner),
        json!({"unattended_changes":unattended}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    assert_eq!(issued["unattended_changes"], unattended);
    issued
}

async fn operation(r: &Router, token: &str, id: &str, args: Value) -> (StatusCode, Value) {
    request(
        r,
        Method::POST,
        &format!("/api/assistant/operations/{id}"),
        Some(token),
        args,
    )
    .await
}

async fn preview(r: &Router, token: &str, id: &str, args: Value) -> Value {
    let (status, plan) = operation(r, token, id, args).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert!(plan["plan_id"].is_string(), "{plan}");
    plan
}

fn confirmed(plan: &Value) -> Value {
    let mut args = plan["apply_arguments"].clone();
    args["confirmed"] = json!(true);
    args
}

#[tokio::test]
async fn owners_decide_the_original_agent_review_and_the_agent_can_read_its_outcome() {
    let state = Arc::new(Mutex::new(AppState::default()));
    let support = auth::issue_local_support_token(&state, None).unwrap();
    let (r, owner) = router_for(state.clone());
    let first = agent(&r, &owner, false).await;
    let first_token = first["token"].as_str().unwrap();
    let other = agent(&r, &owner, false).await;
    let other_token = other["token"].as_str().unwrap();
    let approvals = "/api/assistant/approvals";
    for (decision, rooms_after) in [("decline", 0), ("approve", 1)] {
        let plan = preview(
            &r,
            first_token,
            "plan_create_room",
            json!({"name":"Office"}),
        )
        .await;
        let id = &plan["plan_id"];
        let path = format!("{approvals}/{}", id.as_str().unwrap());
        let apply = confirmed(&plan);
        for different_credential in [&owner[..], other_token, &support.token] {
            assert_eq!(
                operation(&r, different_credential, "apply_device_plan", apply.clone())
                    .await
                    .0,
                StatusCode::CONFLICT
            );
        }
        let rpc = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"apply_device_plan","arguments":apply}});
        let (status, response) = request(&r, Method::POST, MCP_PATH, Some(first_token), rpc).await;
        assert_eq!(status, StatusCode::OK);
        let pending: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(pending["status"], "awaiting_owner_approval");
        assert_eq!(pending["plan_id"], *id);
        assert!(pending["expires_at_epoch_ms"].is_u64());
        assert_eq!(state.lock().unwrap().topology.room_count(), 0);
        let read = json!({"plan_id":id});
        assert_eq!(
            operation(&r, first_token, "read_plan_outcome", read.clone())
                .await
                .1,
            pending
        );
        let (status, queue) = request(&r, Method::GET, approvals, Some(&owner), Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        let row = &queue["approvals"][0];
        assert_eq!(row["review"], plan);
        assert_eq!(row["agent_credential_id"], first["token_id"]);
        assert_eq!(row["plan_operation"], "plan_create_room");
        assert_eq!(row["operation"], "apply_device_plan");
        let choice = json!({"decision":decision});
        for token in [first_token, &support.token] {
            assert_eq!(
                request(&r, Method::GET, approvals, Some(token), Value::Null)
                    .await
                    .0,
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                request(&r, Method::POST, &path, Some(token), choice.clone())
                    .await
                    .0,
                StatusCode::FORBIDDEN
            );
        }
        let (status, decided) =
            request(&r, Method::POST, &path, Some(&owner), choice.clone()).await;
        assert_eq!(status, StatusCode::OK, "{decided}");
        assert_eq!(
            decided["status"],
            if decision == "approve" {
                "acknowledged"
            } else {
                "declined"
            }
        );
        assert_eq!(state.lock().unwrap().topology.room_count(), rooms_after);
        assert_eq!(
            operation(&r, first_token, "read_plan_outcome", read)
                .await
                .1,
            decided
        );
        assert_eq!(
            request(&r, Method::POST, &path, Some(&owner), choice)
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            operation(&r, first_token, "apply_device_plan", apply)
                .await
                .0,
            StatusCode::CONFLICT
        );
        let (_, queue) = request(&r, Method::GET, approvals, Some(&owner), Value::Null).await;
        assert_eq!(queue["approvals"], json!([]));
    }

    let plan = preview(&r, first_token, "plan_create_room", json!({"name":"Hall"})).await;
    assert_eq!(
        operation(&r, first_token, "apply_device_plan", confirmed(&plan))
            .await
            .1["status"],
        "awaiting_owner_approval"
    );
    let revoke = format!(
        "/api/auth/agent-tokens/{}",
        first["token_id"].as_str().unwrap()
    );
    assert_eq!(
        request(&r, Method::DELETE, &revoke, Some(&owner), Value::Null)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let path = format!("{approvals}/{}", plan["plan_id"].as_str().unwrap());
    assert_eq!(
        request(
            &r,
            Method::POST,
            &path,
            Some(&owner),
            json!({"decision":"approve"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(state.lock().unwrap().topology.room_count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unattended_room_tuning_applies_and_hub_restart_always_waits_for_the_owner() {
    let (rooms, devices) = harness::rooms_with_lights(&[("kitchen", "Kitchen")]);
    let h = harness::TestHarness::new_at(17., 170).with_discovery(rooms, devices);
    h.sync();
    h.state.lock().unwrap().assistant_devices.platform = Some(Arc::new(|_, _| {
        Box::pin(async {
            panic!("restart should await owner approval");
        })
    }));
    let (r, owner) = router_for(h.state.clone());
    let issued = agent(&r, &owner, true).await;
    let token = issued["token"].as_str().unwrap();
    let (_, list) = request(
        &r,
        Method::GET,
        "/api/auth/agent-tokens",
        Some(&owner),
        Value::Null,
    )
    .await;
    assert_eq!(list["tokens"][0]["unattended_changes"], true, "{list}");
    let plan = preview(
        &r,
        token,
        "plan_room_tuning",
        json!({"room_id":h.resolve("kitchen"),"setting":"minimum_brightness","value":40}),
    )
    .await;
    let (status, receipt) = operation(&r, token, "apply_room_tuning", confirmed(&plan)).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["canonical_readback_verified"], true);
    let plan = preview(&r, token, "plan_hub_restart", json!({})).await;
    let (status, pending) = operation(&r, token, "apply_device_plan", confirmed(&plan)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pending["status"], "awaiting_owner_approval");
}

#[test]
fn lifecycle_and_control_boundary_reviews_require_owner_approval() {
    for id in [
        "plan_hub_update",
        "plan_hub_restart",
        "plan_update_settings_edit",
        "plan_managed_ha_lights",
        "plan_light_breaker_edit",
        "plan_remove_device",
        "plan_unassign_device",
        "plan_delete_room",
        "plan_merge_rooms",
        "plan_hue_authority",
        "plan_bridge_pairing",
        "plan_nearby_hue_lights",
        "plan_lighting_transfer",
        "plan_profile_bundle_reset",
        "plan_profile_reset",
        "plan_activity_upload",
    ] {
        assert!(
            operation_registry()
                .iter()
                .any(|op| op.id == id && op.requires_owner_approval),
            "{id}"
        );
    }
}

async fn staff_session(r: &Router, issuer: &str, actor: &str, grant: &str) -> String {
    let (status, session) = request(
        r,
        Method::POST,
        "/api/auth/support-session-token",
        Some(issuer),
        json!({"actor_id":actor,"grant_id":grant}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    session["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn support_session_refresh_keeps_plans_scoped_to_one_actor_grant_and_issuer() {
    let state = Arc::new(Mutex::new(AppState::default()));
    let issuer = auth::issue_local_support_token(&state, None).unwrap();
    let other_issuer = auth::issue_local_support_token(&state, None).unwrap();
    let (r, _) = router_for(state);
    let first = staff_session(&r, &issuer.token, "staff-a", "grant-a").await;
    let plan = preview(&r, &first, "plan_create_room", json!({"name":"Office"})).await;
    for (issuer, actor, grant) in [
        (&issuer.token, "staff-b", "grant-a"),
        (&issuer.token, "staff-a", "grant-b"),
        (&other_issuer.token, "staff-a", "grant-a"),
    ] {
        let other = staff_session(&r, issuer, actor, grant).await;
        assert_eq!(
            operation(&r, &other, "apply_device_plan", confirmed(&plan))
                .await
                .0,
            StatusCode::CONFLICT
        );
    }
    let refreshed = staff_session(&r, &issuer.token, "staff-a", "grant-a").await;
    let (status, receipt) = operation(&r, &refreshed, "apply_device_plan", confirmed(&plan)).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["status"], "acknowledged");
}

#[tokio::test]
async fn legacy_and_operation_move_routes_share_credential_bound_reviews() {
    use rhythm_os::assistant::{LIGHT_ASSISTANT_MOVE_APPLY_PATH, LIGHT_ASSISTANT_MOVE_PLAN_PATH};
    let (rooms, devices) = harness::rooms_with_lights(&[("kitchen", "Kitchen"), ("hall", "Hall")]);
    let h = harness::TestHarness::new().with_discovery(rooms, devices);
    h.sync();
    let kitchen = h.resolve("kitchen");
    let hall = h.resolve("hall");
    let (r, owner) = router_for(h.state.clone());
    let other = auth::issue_local_owner_token(&h.state, None).unwrap();
    let (_, topology) = operation(&r, &owner, "get_topology_snapshot", json!({})).await;
    let light = &topology["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["parent_id"] == kitchen)
        .unwrap()["id"];
    let (status, plan) = request(
        &r,
        Method::POST,
        LIGHT_ASSISTANT_MOVE_PLAN_PATH,
        Some(&owner),
        json!({"device_id":light,"to_room_id":hall,"correlation_id":"move"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let args = plan["apply_arguments"].clone();
    assert_eq!(
        request(
            &r,
            Method::POST,
            LIGHT_ASSISTANT_MOVE_APPLY_PATH,
            Some(&other.token),
            args.clone()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        operation(
            &r,
            &other.token,
            "apply_move_device_room_plan",
            confirmed(&plan)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, receipt) = request(
        &r,
        Method::POST,
        LIGHT_ASSISTANT_MOVE_APPLY_PATH,
        Some(&owner),
        args,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["current_parent_id"], hall);
    assert_eq!(receipt["plan_id"], plan["plan_id"]);
    let return_plan = preview(
        &r,
        &other.token,
        "plan_move_device_room",
        json!({"device_id":light,"to_room_id":kitchen,"correlation_id":"return"}),
    )
    .await;
    let (status, receipt) = request(
        &r,
        Method::POST,
        LIGHT_ASSISTANT_MOVE_APPLY_PATH,
        Some(&other.token),
        return_plan["apply_arguments"].clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["current_parent_id"], kitchen);
}
