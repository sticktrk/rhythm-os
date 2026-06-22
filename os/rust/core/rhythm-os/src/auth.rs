//! Local API authentication for Rhythm OS.
//!
//! The appliance owns its API credentials. Cloud/account identity can decide
//! who may use remote access, but requests that reach the appliance through
//! the remote tunnel are still authorized by a device-issued bearer token.
//! Direct LAN access can remain open for local-first setup and control.

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;

use crate::handlers::ApiResponse;
use crate::state::{current_epoch_ms, SharedState};

const TOKEN_RANDOM_BYTES: usize = 32;
const TOKEN_PREFIX: &str = "rhythm_owner_";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiAuthRequestInfo {
    pub requires_auth: bool,
    pub claim_available: bool,
    pub via_remote_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredApiAuth {
    #[serde(default = "default_schema_version")]
    pub schema_version: u8,
    /// Persisted override for the platform's default auth requirement.
    ///
    /// `None` means "use the platform default" so existing appliances keep
    /// their secure default when this field is absent in older auth files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_api_auth: Option<bool>,
    #[serde(default)]
    pub tokens: Vec<StoredApiToken>,
}

fn default_schema_version() -> u8 {
    1
}

impl Default for StoredApiAuth {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            require_api_auth: None,
            tokens: Vec::new(),
        }
    }
}

impl StoredApiAuth {
    pub fn has_owner(&self) -> bool {
        self.tokens
            .iter()
            .any(|token| token.role == ApiTokenRole::Owner)
    }

    pub fn verify_token(&self, raw_token: &str) -> bool {
        if raw_token.trim().is_empty() {
            return false;
        }
        let candidate_hash = hash_token(raw_token);
        self.tokens
            .iter()
            .any(|token| constant_time_eq(token.token_hash.as_bytes(), candidate_hash.as_bytes()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiTokenRole {
    Owner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredApiToken {
    pub id: String,
    pub role: ApiTokenRole,
    pub token_hash: String,
    pub created_at_epoch_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssuedOwnerToken {
    pub id: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueOwnerTokenResult {
    Issued(IssuedOwnerToken),
    AlreadyConfigured,
}

/// Issue a new owner token from a trusted local channel such as rpiz BLE.
///
/// Unlike the public HTTP claim endpoint, this does not stop after the first
/// owner token. Physical/local presence is the authorization boundary.
pub fn issue_local_owner_token(
    state: &SharedState,
    label: Option<String>,
) -> anyhow::Result<IssuedOwnerToken> {
    let token = generate_raw_token();
    let token_hash = hash_token(&token);
    let id = token_hash.chars().take(16).collect::<String>();

    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    s.api_auth.tokens.push(StoredApiToken {
        id: id.clone(),
        role: ApiTokenRole::Owner,
        token_hash,
        created_at_epoch_ms: current_epoch_ms(),
        label,
    });

    if let Some(storage) = s.storage.as_ref() {
        storage.save_api_auth(&s.api_auth)?;
    }

    Ok(IssuedOwnerToken { id, token })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiAuthSettingsUpdate {
    pub require_api_auth: bool,
    pub owner_configured: bool,
    pub token_count: usize,
    pub issued_owner_token: Option<IssuedOwnerToken>,
}

pub fn issue_owner_token(
    state: &SharedState,
    label: Option<String>,
) -> anyhow::Result<IssueOwnerTokenResult> {
    let token = generate_raw_token();
    let token_hash = hash_token(&token);
    let id = token_hash.chars().take(16).collect::<String>();

    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.api_auth.has_owner() {
        return Ok(IssueOwnerTokenResult::AlreadyConfigured);
    }

    s.api_auth.tokens.push(StoredApiToken {
        id: id.clone(),
        role: ApiTokenRole::Owner,
        token_hash,
        created_at_epoch_ms: current_epoch_ms(),
        label,
    });

    if let Some(storage) = s.storage.as_ref() {
        storage.save_api_auth(&s.api_auth)?;
    }

    Ok(IssueOwnerTokenResult::Issued(IssuedOwnerToken {
        id,
        token,
    }))
}

pub fn set_api_auth_required(
    state: &SharedState,
    require_api_auth: bool,
    label: Option<String>,
) -> anyhow::Result<ApiAuthSettingsUpdate> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let issued_owner_token = if require_api_auth && !s.api_auth.has_owner() {
        let token = generate_raw_token();
        let token_hash = hash_token(&token);
        let id = token_hash.chars().take(16).collect::<String>();

        s.api_auth.tokens.push(StoredApiToken {
            id: id.clone(),
            role: ApiTokenRole::Owner,
            token_hash,
            created_at_epoch_ms: current_epoch_ms(),
            label,
        });

        Some(IssuedOwnerToken { id, token })
    } else {
        None
    };

    s.require_api_auth = require_api_auth;
    s.api_auth.require_api_auth = Some(require_api_auth);

    if let Some(storage) = s.storage.as_ref() {
        storage.save_api_auth(&s.api_auth)?;
    }

    Ok(ApiAuthSettingsUpdate {
        require_api_auth: s.require_api_auth,
        owner_configured: s.api_auth.has_owner(),
        token_count: s.api_auth.tokens.len(),
        issued_owner_token,
    })
}

pub fn clear_api_auth(state: &SharedState) -> anyhow::Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    s.api_auth = StoredApiAuth::default();
    if let Some(storage) = s.storage.as_ref() {
        storage.save_api_auth(&s.api_auth)?;
    }
    Ok(())
}

pub fn handle_get_auth_status(
    state: &SharedState,
    request_info: Option<ApiAuthRequestInfo>,
) -> ApiResponse {
    match state.lock() {
        Ok(s) => {
            let request_info = request_info.unwrap_or(ApiAuthRequestInfo {
                requires_auth: s.require_api_auth,
                claim_available: s.require_api_auth && !s.api_auth.has_owner(),
                via_remote_access: false,
            });
            ApiResponse::json_ok(
                auth_status_payload(
                    request_info.requires_auth,
                    s.api_auth.has_owner(),
                    s.api_auth.tokens.len(),
                    request_info.claim_available,
                    request_info.via_remote_access,
                )
                .to_string(),
            )
        }
        Err(_) => ApiResponse::server_error("lock"),
    }
}

pub fn handle_claim_owner_token(state: &SharedState, label: Option<String>) -> ApiResponse {
    match issue_local_owner_token(state, label) {
        Ok(issued) => ApiResponse::json_ok(
            json!({
                "status": "ok",
                "token_id": issued.id,
                "token": issued.token,
            })
            .to_string(),
        ),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_auth_settings(state: &SharedState, body: &Value) -> ApiResponse {
    let require_api_auth = match body
        .get("require_api_auth")
        .or_else(|| body.get("requires_auth"))
        .and_then(Value::as_bool)
    {
        Some(value) => value,
        None => return ApiResponse::bad_request("Missing or invalid require_api_auth boolean"),
    };

    let label = body
        .get("label")
        .or_else(|| body.get("owner_label"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string);

    match set_api_auth_required(state, require_api_auth, label) {
        Ok(update) => {
            let mut body = auth_status_payload(
                update.require_api_auth,
                update.owner_configured,
                update.token_count,
                !update.require_api_auth || !update.owner_configured,
                false,
            );
            if let Some(object) = body.as_object_mut() {
                object.insert("status".to_string(), json!("ok"));
                if let Some(issued) = update.issued_owner_token {
                    object.insert("token_id".to_string(), json!(issued.id));
                    object.insert("token".to_string(), json!(issued.token));
                }
            }
            ApiResponse::json_ok(body.to_string())
        }
        Err(e) => ApiResponse::server_error(e),
    }
}

pub async fn require_api_auth_middleware(
    State(state): State<SharedState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let auth_info = auth_request_info(&state, &req);
    req.extensions_mut().insert(auth_info);

    if is_public_request(req.method(), req.uri().path(), auth_info) {
        return next.run(req).await;
    }

    if !auth_info.requires_auth {
        return next.run(req).await;
    }

    let Some(token) = bearer_token(req.headers().get(AUTHORIZATION)) else {
        return unauthorized("Missing bearer token");
    };

    let token_ok = state
        .lock()
        .map(|s| s.api_auth.verify_token(token))
        .unwrap_or(false);
    if token_ok {
        return next.run(req).await;
    }

    unauthorized("Invalid bearer token")
}

fn auth_status_payload(
    require_api_auth: bool,
    owner_configured: bool,
    token_count: usize,
    claim_available: bool,
    via_remote_access: bool,
) -> Value {
    json!({
        "requires_auth": require_api_auth,
        "owner_configured": owner_configured,
        "token_count": token_count,
        "claim_available": claim_available,
        "via_remote_access": via_remote_access,
    })
}

pub fn auth_request_info(state: &SharedState, req: &Request<Body>) -> ApiAuthRequestInfo {
    let (stored_requires_auth, owner_configured, is_appliance) = state
        .lock()
        .map(|s| {
            (
                s.require_api_auth,
                s.api_auth.has_owner(),
                s.platform_type == "appliance",
            )
        })
        .unwrap_or((true, true, false));

    let via_remote_access = is_appliance
        && req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip().is_loopback())
            .unwrap_or(false)
        && has_remote_access_forwarding_headers(&req);

    let requires_auth = if via_remote_access {
        true
    } else if is_appliance {
        false
    } else {
        stored_requires_auth
    };

    let claim_available = !via_remote_access && (!requires_auth || !owner_configured);

    ApiAuthRequestInfo {
        requires_auth,
        claim_available,
        via_remote_access,
    }
}

fn is_public_request(method: &Method, path: &str, auth_info: ApiAuthRequestInfo) -> bool {
    *method == Method::OPTIONS
        || path == "/health"
        || path == "/api/auth/status"
        || (*method == Method::POST && path == "/api/auth/claim" && auth_info.claim_available)
}

fn has_remote_access_forwarding_headers(req: &Request<Body>) -> bool {
    req.headers().contains_key("cf-connecting-ip")
        || req.headers().contains_key("cf-ray")
        || req.headers().contains_key("cf-visitor")
        || req.headers().contains_key("cdn-loop")
        || req.headers().contains_key("x-forwarded-for")
}

fn bearer_token(value: Option<&axum::http::HeaderValue>) -> Option<&str> {
    let value = value?.to_str().ok()?.trim();
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("content-type", "application/json")],
        json!({
            "status": "error",
            "message": message,
        })
        .to_string(),
    )
        .into_response()
}

fn generate_raw_token() -> String {
    let mut bytes = [0_u8; TOKEN_RANDOM_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{}{}", TOKEN_PREFIX, hex_encode(&bytes))
}

fn hash_token(raw_token: &str) -> String {
    let digest = Sha256::digest(raw_token.as_bytes());
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use axum::body::{to_bytes, Body};
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use tower::util::ServiceExt;

    fn test_state() -> SharedState {
        Arc::new(Mutex::new(crate::state::AppState::default()))
    }

    fn auth_test_router(state: SharedState) -> axum::Router {
        crate::axum_router::api_routes()
            .with_state(state.clone())
            .layer(middleware::from_fn_with_state(
                state,
                require_api_auth_middleware,
            ))
    }

    fn request_with_peer(method: Method, uri: &str, peer_ip: IpAddr, body: Body) -> Request<Body> {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .body(body)
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(peer_ip, 49152)));
        req
    }

    fn tunnel_request(method: Method, uri: &str, body: Body) -> Request<Body> {
        let mut req = request_with_peer(method, uri, IpAddr::V4(Ipv4Addr::LOCALHOST), body);
        req.headers_mut()
            .insert("cf-connecting-ip", "203.0.113.10".parse().unwrap());
        req
    }

    #[test]
    fn stored_auth_verifies_hashed_token_only() {
        let raw = "rhythm_owner_test";
        let stored = StoredApiAuth {
            schema_version: 1,
            require_api_auth: None,
            tokens: vec![StoredApiToken {
                id: "owner".into(),
                role: ApiTokenRole::Owner,
                token_hash: hash_token(raw),
                created_at_epoch_ms: 1,
                label: None,
            }],
        };

        assert!(stored.verify_token(raw));
        assert!(!stored.verify_token("wrong"));
    }

    #[test]
    fn generated_owner_tokens_are_prefixed_and_distinct() {
        let a = generate_raw_token();
        let b = generate_raw_token();

        assert!(a.starts_with(TOKEN_PREFIX));
        assert!(b.starts_with(TOKEN_PREFIX));
        assert_ne!(a, b);
    }

    #[test]
    fn owner_token_can_only_be_issued_once() {
        let state = test_state();

        let first = issue_owner_token(&state, Some("first".into())).unwrap();
        assert!(matches!(first, IssueOwnerTokenResult::Issued(_)));

        let second = issue_owner_token(&state, Some("second".into())).unwrap();
        assert_eq!(second, IssueOwnerTokenResult::AlreadyConfigured);
    }

    #[test]
    fn local_owner_tokens_can_be_issued_more_than_once() {
        let state = test_state();

        let first = issue_local_owner_token(&state, Some("first phone".into())).unwrap();
        let second = issue_local_owner_token(&state, Some("second phone".into())).unwrap();

        assert_ne!(first.token, second.token);
        let state = state.lock().unwrap();
        assert_eq!(state.api_auth.tokens.len(), 2);
        assert!(state.api_auth.verify_token(&first.token));
        assert!(state.api_auth.verify_token(&second.token));
        assert_eq!(
            state
                .api_auth
                .tokens
                .iter()
                .map(|token| token.label.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("first phone"), Some("second phone")]
        );
    }

    #[test]
    fn set_api_auth_required_enables_auth_and_issues_first_owner_token() {
        let state = test_state();

        let update = set_api_auth_required(&state, true, Some("phone".into())).unwrap();

        assert!(update.require_api_auth);
        assert!(update.owner_configured);
        assert_eq!(update.token_count, 1);
        let issued = update
            .issued_owner_token
            .expect("first enable should issue token");
        assert!(issued.token.starts_with(TOKEN_PREFIX));

        let state = state.lock().unwrap();
        assert!(state.require_api_auth);
        assert_eq!(state.api_auth.require_api_auth, Some(true));
        assert!(state.api_auth.verify_token(&issued.token));
        assert_eq!(state.api_auth.tokens[0].label.as_deref(), Some("phone"));
    }

    #[test]
    fn set_api_auth_required_reuses_existing_owner_token() {
        let state = test_state();
        let first = set_api_auth_required(&state, true, Some("first".into())).unwrap();
        assert!(first.issued_owner_token.is_some());

        let second = set_api_auth_required(&state, true, Some("second".into())).unwrap();

        assert!(second.issued_owner_token.is_none());
        assert_eq!(second.token_count, 1);
        assert!(second.owner_configured);
    }

    #[test]
    fn set_api_auth_required_can_disable_without_deleting_tokens() {
        let state = test_state();
        let enabled = set_api_auth_required(&state, true, Some("phone".into())).unwrap();
        let token = enabled.issued_owner_token.unwrap().token;

        let disabled = set_api_auth_required(&state, false, None).unwrap();

        assert!(!disabled.require_api_auth);
        assert!(disabled.owner_configured);
        assert_eq!(disabled.token_count, 1);
        let state = state.lock().unwrap();
        assert!(!state.require_api_auth);
        assert_eq!(state.api_auth.require_api_auth, Some(false));
        assert!(state.api_auth.verify_token(&token));
    }

    #[tokio::test]
    async fn auth_settings_endpoint_enables_auth_and_returns_first_owner_token() {
        let state = test_state();
        let app = auth_test_router(state.clone());

        let req = Request::builder()
            .method("PUT")
            .uri("/api/auth/settings")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"require_api_auth": true, "label": "phone"}).to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = serde_json::from_slice::<Value>(
            &to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        let token = body["token"]
            .as_str()
            .expect("enabling without owner should return token");
        assert_eq!(body["requires_auth"].as_bool(), Some(true));
        assert_eq!(body["owner_configured"].as_bool(), Some(true));
        assert_eq!(body["token_count"].as_u64(), Some(1));
        assert!(token.starts_with(TOKEN_PREFIX));

        let unauthenticated = Request::builder()
            .method("GET")
            .uri("/api/state")
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(unauthenticated).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let authenticated = Request::builder()
            .method("GET")
            .uri("/api/state")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(authenticated).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let state = state.lock().unwrap();
        assert!(state.require_api_auth);
        assert!(state.api_auth.verify_token(token));
    }

    #[tokio::test]
    async fn appliance_lan_request_stays_open_even_when_auth_policy_is_on() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
            state.require_api_auth = true;
            state.api_auth.require_api_auth = Some(true);
        }
        let app = auth_test_router(state);

        let status = app
            .clone()
            .oneshot(request_with_peer(
                Method::GET,
                "/api/auth/status",
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let body = serde_json::from_slice::<Value>(
            &to_bytes(status.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["requires_auth"].as_bool(), Some(false));
        assert_eq!(body["claim_available"].as_bool(), Some(true));
        assert_eq!(body["via_remote_access"].as_bool(), Some(false));

        let response = app
            .oneshot(request_with_peer(
                Method::GET,
                "/api/state",
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn appliance_loopback_without_tunnel_headers_stays_open() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
            state.require_api_auth = true;
            state.api_auth.require_api_auth = Some(true);
        }
        let app = auth_test_router(state);

        let status = app
            .clone()
            .oneshot(request_with_peer(
                Method::GET,
                "/api/auth/status",
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let body = serde_json::from_slice::<Value>(
            &to_bytes(status.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["requires_auth"].as_bool(), Some(false));
        assert_eq!(body["claim_available"].as_bool(), Some(true));
        assert_eq!(body["via_remote_access"].as_bool(), Some(false));

        let response = app
            .oneshot(request_with_peer(
                Method::GET,
                "/api/state",
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn appliance_tunnel_request_requires_owner_token() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
        }
        let issued = issue_local_owner_token(&state, Some("phone".into())).unwrap();
        let app = auth_test_router(state);

        let status = app
            .clone()
            .oneshot(tunnel_request(
                Method::GET,
                "/api/auth/status",
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let body = serde_json::from_slice::<Value>(
            &to_bytes(status.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["requires_auth"].as_bool(), Some(true));
        assert_eq!(body["claim_available"].as_bool(), Some(false));
        assert_eq!(body["via_remote_access"].as_bool(), Some(true));

        let unauthenticated = app
            .clone()
            .oneshot(tunnel_request(Method::GET, "/api/state", Body::empty()))
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let authenticated = app
            .oneshot({
                let mut req = tunnel_request(Method::GET, "/api/state", Body::empty());
                req.headers_mut().insert(
                    AUTHORIZATION,
                    format!("Bearer {}", issued.token).parse().unwrap(),
                );
                req
            })
            .await
            .unwrap();
        assert_eq!(authenticated.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn local_claim_endpoint_issues_additional_owner_tokens() {
        let state = test_state();
        let first = issue_local_owner_token(&state, Some("first".into())).unwrap();
        let app = auth_test_router(state.clone());

        let response = app
            .oneshot({
                let mut req = request_with_peer(
                    Method::POST,
                    "/api/auth/claim",
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                    Body::from(json!({"label": "second"}).to_string()),
                );
                req.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                req
            })
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = serde_json::from_slice::<Value>(
            &to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        let second = body["token"].as_str().expect("claim returns token");
        assert_ne!(first.token, second);
        let state = state.lock().unwrap();
        assert_eq!(state.api_auth.tokens.len(), 2);
        assert!(state.api_auth.verify_token(&first.token));
        assert!(state.api_auth.verify_token(second));
    }

    #[tokio::test]
    async fn auth_settings_endpoint_requires_token_to_disable_when_auth_is_on() {
        let state = test_state();
        let update = set_api_auth_required(&state, true, Some("phone".into())).unwrap();
        let token = update.issued_owner_token.unwrap().token;
        let app = auth_test_router(state.clone());

        let unauthenticated_disable = Request::builder()
            .method("PUT")
            .uri("/api/auth/settings")
            .header("content-type", "application/json")
            .body(Body::from(json!({"require_api_auth": false}).to_string()))
            .unwrap();
        let response = app.clone().oneshot(unauthenticated_disable).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(state.lock().unwrap().require_api_auth);

        let authenticated_disable = Request::builder()
            .method("PUT")
            .uri("/api/auth/settings")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(json!({"require_api_auth": false}).to_string()))
            .unwrap();
        let response = app.clone().oneshot(authenticated_disable).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = serde_json::from_slice::<Value>(
            &to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["requires_auth"].as_bool(), Some(false));
        assert!(!state.lock().unwrap().require_api_auth);

        let unauthenticated_state = Request::builder()
            .method("GET")
            .uri("/api/state")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(unauthenticated_state).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
