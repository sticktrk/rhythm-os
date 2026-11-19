//! Exercise both discovery and authorization through the shipped router.
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
    let shipped: std::collections::BTreeSet<_> = policy
        .lines()
        .filter_map(|l| l.trim().strip_prefix('\'')?.strip_suffix("',"))
        .collect();
    let expected = registry
        .iter()
        .filter(|op| op.tier == OperationTier::Read)
        .map(|op| op.id.as_str())
        .collect();
    assert_eq!(
        shipped, expected,
        "Regenerate the Dart policy with the assistant_read_policy example."
    );
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
