//! Local API authentication for Rhythm OS.
//!
//! The appliance owns its API credentials. Cloud/account identity can decide
//! who may use remote access, but requests that reach the appliance through
//! the remote tunnel are still authorized by a device-issued bearer token.
//! Direct LAN access can remain open for local-first setup and control.

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{Method, Request, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;

use crate::handlers::ApiResponse;
use crate::state::{current_epoch_ms, SharedState};

const TOKEN_RANDOM_BYTES: usize = 32;
const TOKEN_PREFIX: &str = "rhythm_owner_";
const SUPPORT_TOKEN_PREFIX: &str = "rhythm_support_";
const SUPPORT_SESSION_TOKEN_PREFIX: &str = "rhythm_support_session_";
const SUPPORT_AUDIT_FILE: &str = "support-audit.log";
const DEFAULT_SUPPORT_SESSION_TTL_SECS: u64 = 60 * 60;
const MIN_SUPPORT_SESSION_TTL_SECS: u64 = 1;
const MAX_SUPPORT_SESSION_TTL_SECS: u64 = 4 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiAuthRequestInfo {
    pub requires_auth: bool,
    pub claim_available: bool,
    pub via_remote_access: bool,
    pub role: Option<ApiTokenRole>,
    pub token_expires_at_epoch_ms: Option<u64>,
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
        self.verify_token_info(raw_token).is_some()
    }

    pub fn verify_token_role(&self, raw_token: &str) -> Option<ApiTokenRole> {
        self.verify_token_info(raw_token).map(|token| token.role)
    }

    fn verify_token_info(&self, raw_token: &str) -> Option<VerifiedApiToken> {
        if raw_token.trim().is_empty() {
            return None;
        }
        let candidate_hash = hash_token(raw_token);
        let now = current_epoch_ms();
        self.tokens
            .iter()
            .find(|token| {
                !token.is_expired(now)
                    && constant_time_eq(token.token_hash.as_bytes(), candidate_hash.as_bytes())
            })
            .map(|token| VerifiedApiToken {
                id: token.id.clone(),
                role: token.role,
                expires_at_epoch_ms: token.expires_at_epoch_ms,
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiTokenRole {
    Owner,
    Support,
}

impl Default for ApiTokenRole {
    fn default() -> Self {
        Self::Owner
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedApiToken {
    id: String,
    role: ApiTokenRole,
    expires_at_epoch_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredApiToken {
    pub id: String,
    #[serde(default)]
    pub role: ApiTokenRole,
    pub token_hash: String,
    pub created_at_epoch_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_epoch_ms: Option<u64>,
}

impl StoredApiToken {
    fn is_expired(&self, now_epoch_ms: u64) -> bool {
        self.expires_at_epoch_ms
            .map(|expires_at| expires_at <= now_epoch_ms)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssuedToken {
    pub id: String,
    pub token: String,
}

pub type IssuedOwnerToken = IssuedToken;

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
    issue_local_token(state, ApiTokenRole::Owner, TOKEN_PREFIX, label, None)
}

/// Issue a support token from a trusted local or owner-authenticated channel.
///
/// Support tokens are never publicly claimable. The long-lived support token is
/// held by the cloud only; employee clients receive short-lived support session
/// tokens minted from it and scoped by the middleware gate.
pub fn issue_local_support_token(
    state: &SharedState,
    label: Option<String>,
) -> anyhow::Result<IssuedToken> {
    issue_local_token(
        state,
        ApiTokenRole::Support,
        SUPPORT_TOKEN_PREFIX,
        label,
        None,
    )
}

fn issue_local_token(
    state: &SharedState,
    role: ApiTokenRole,
    prefix: &str,
    label: Option<String>,
    expires_at_epoch_ms: Option<u64>,
) -> anyhow::Result<IssuedToken> {
    let token = generate_raw_token(prefix);
    let token_hash = hash_token(&token);
    let id = token_hash.chars().take(16).collect::<String>();

    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    prune_expired_tokens(&mut s.api_auth.tokens, current_epoch_ms());
    s.api_auth.tokens.push(StoredApiToken {
        id: id.clone(),
        role,
        token_hash,
        created_at_epoch_ms: current_epoch_ms(),
        label,
        expires_at_epoch_ms,
    });

    // Save outside the lock — the write is fsync'd, and holding the global
    // state lock through a slow SD-card flush stalls light control.
    let persist = s
        .storage
        .as_ref()
        .map(|st| (st.clone(), s.api_auth.clone()));
    drop(s);
    if let Some((storage, api_auth)) = persist {
        storage.save_api_auth(&api_auth)?;
    }

    Ok(IssuedToken { id, token })
}

pub fn issue_support_session_token(
    state: &SharedState,
    label: Option<String>,
    ttl_seconds: Option<u64>,
) -> anyhow::Result<(IssuedToken, u64)> {
    let ttl_seconds = clamp_support_session_ttl(ttl_seconds);
    let expires_at_epoch_ms = current_epoch_ms().saturating_add(ttl_seconds.saturating_mul(1000));
    let issued = issue_local_token(
        state,
        ApiTokenRole::Support,
        SUPPORT_SESSION_TOKEN_PREFIX,
        label,
        Some(expires_at_epoch_ms),
    )?;
    Ok((issued, expires_at_epoch_ms))
}

pub fn revoke_support_session_token(state: &SharedState, token_id: &str) -> anyhow::Result<bool> {
    let clean_id = token_id.trim();
    if clean_id.is_empty() {
        return Ok(false);
    }

    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let before = s.api_auth.tokens.len();
    s.api_auth.tokens.retain(|token| {
        !(token.id == clean_id
            && token.role == ApiTokenRole::Support
            && token.expires_at_epoch_ms.is_some())
    });
    let revoked = s.api_auth.tokens.len() != before;
    prune_expired_tokens(&mut s.api_auth.tokens, current_epoch_ms());

    if revoked {
        let persist = s
            .storage
            .as_ref()
            .map(|st| (st.clone(), s.api_auth.clone()));
        drop(s);
        if let Some((storage, api_auth)) = persist {
            storage.save_api_auth(&api_auth)?;
        }
    }

    Ok(revoked)
}

fn prune_expired_tokens(tokens: &mut Vec<StoredApiToken>, now_epoch_ms: u64) {
    tokens.retain(|token| !token.is_expired(now_epoch_ms));
}

fn clamp_support_session_ttl(ttl_seconds: Option<u64>) -> u64 {
    ttl_seconds
        .unwrap_or(DEFAULT_SUPPORT_SESSION_TTL_SECS)
        .clamp(MIN_SUPPORT_SESSION_TTL_SECS, MAX_SUPPORT_SESSION_TTL_SECS)
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
    let token = generate_raw_token(TOKEN_PREFIX);
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
        expires_at_epoch_ms: None,
    });

    let persist = s
        .storage
        .as_ref()
        .map(|st| (st.clone(), s.api_auth.clone()));
    drop(s);
    if let Some((storage, api_auth)) = persist {
        storage.save_api_auth(&api_auth)?;
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
        let token = generate_raw_token(TOKEN_PREFIX);
        let token_hash = hash_token(&token);
        let id = token_hash.chars().take(16).collect::<String>();

        s.api_auth.tokens.push(StoredApiToken {
            id: id.clone(),
            role: ApiTokenRole::Owner,
            token_hash,
            created_at_epoch_ms: current_epoch_ms(),
            label,
            expires_at_epoch_ms: None,
        });

        Some(IssuedOwnerToken { id, token })
    } else {
        None
    };

    s.require_api_auth = require_api_auth;
    s.api_auth.require_api_auth = Some(require_api_auth);

    let update = ApiAuthSettingsUpdate {
        require_api_auth: s.require_api_auth,
        owner_configured: s.api_auth.has_owner(),
        token_count: s.api_auth.tokens.len(),
        issued_owner_token,
    };
    let persist = s
        .storage
        .as_ref()
        .map(|st| (st.clone(), s.api_auth.clone()));
    drop(s);
    if let Some((storage, api_auth)) = persist {
        storage.save_api_auth(&api_auth)?;
    }

    Ok(update)
}

pub fn clear_api_auth(state: &SharedState) -> anyhow::Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    s.api_auth = StoredApiAuth::default();
    let persist = s
        .storage
        .as_ref()
        .map(|st| (st.clone(), s.api_auth.clone()));
    drop(s);
    if let Some((storage, api_auth)) = persist {
        storage.save_api_auth(&api_auth)?;
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
                role: None,
                token_expires_at_epoch_ms: None,
            });
            ApiResponse::json_ok(
                auth_status_payload(
                    request_info.requires_auth,
                    s.api_auth.has_owner(),
                    s.api_auth.tokens.len(),
                    request_info.claim_available,
                    request_info.via_remote_access,
                    request_info.role,
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

pub fn handle_issue_support_token(
    state: &SharedState,
    request_info: Option<ApiAuthRequestInfo>,
    label: Option<String>,
) -> ApiResponse {
    let Some(request_info) = request_info else {
        return ApiResponse::forbidden("Support tokens require an owner token");
    };
    if request_info.via_remote_access {
        return ApiResponse::forbidden("Support tokens can only be issued locally");
    }
    if request_info.role != Some(ApiTokenRole::Owner) {
        return ApiResponse::forbidden("Support tokens require an owner token");
    }

    match issue_local_support_token(state, label) {
        Ok(issued) => ApiResponse::json_ok(
            json!({
                "status": "ok",
                "token_id": issued.id,
                "token": issued.token,
                "role": "support",
            })
            .to_string(),
        ),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_issue_support_session_token(
    state: &SharedState,
    request_info: Option<ApiAuthRequestInfo>,
    body: &Value,
) -> ApiResponse {
    if !request_has_privileged_token(request_info) {
        return ApiResponse::forbidden(
            "Support session tokens require an authenticated support token",
        );
    }

    let label = body
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string);
    let ttl_seconds = body.get("ttl_seconds").and_then(Value::as_u64);

    match issue_support_session_token(state, label, ttl_seconds) {
        Ok((issued, expires_at_epoch_ms)) => ApiResponse::json_ok(
            json!({
                "status": "ok",
                "token_id": issued.id,
                "token": issued.token,
                "role": "support",
                "expires_at_epoch_ms": expires_at_epoch_ms,
            })
            .to_string(),
        ),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_revoke_support_session_token(
    state: &SharedState,
    request_info: Option<ApiAuthRequestInfo>,
    body: &Value,
) -> ApiResponse {
    if !request_has_privileged_token(request_info) {
        return ApiResponse::forbidden(
            "Support session tokens require an authenticated support token",
        );
    }

    let token_id = body
        .get("token_id")
        .or_else(|| body.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(token_id) = token_id else {
        return ApiResponse::bad_request("Missing token_id");
    };

    match revoke_support_session_token(state, token_id) {
        Ok(revoked) => ApiResponse::json_ok(
            json!({
                "status": "ok",
                "token_id": token_id,
                "revoked": revoked,
            })
            .to_string(),
        ),
        Err(e) => ApiResponse::server_error(e),
    }
}

fn request_has_privileged_token(request_info: Option<ApiAuthRequestInfo>) -> bool {
    let Some(info) = request_info else {
        return false;
    };
    match info.role {
        Some(ApiTokenRole::Owner) => true,
        Some(ApiTokenRole::Support) => info.token_expires_at_epoch_ms.is_none(),
        None => false,
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
                None,
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
    let mut auth_info = auth_request_info(&state, &req);
    req.extensions_mut().insert(auth_info);

    let verified_bearer = verified_bearer_token(&state, &req);
    if let Some(verified_token) = verified_bearer.as_ref() {
        auth_info.role = Some(verified_token.role);
        auth_info.token_expires_at_epoch_ms = verified_token.expires_at_epoch_ms;
        req.extensions_mut().insert(auth_info);
        if verified_token.role == ApiTokenRole::Support {
            write_support_audit_line(
                &state,
                &verified_token.id,
                req.method(),
                req.uri().path(),
                req.headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok()),
            );
            if let Some(reason) = support_token_forbidden_reason(req.method(), req.uri()) {
                return forbidden(reason);
            }
        }
    }

    if is_public_request(req.method(), req.uri().path(), auth_info) {
        return next.run(req).await;
    }

    // Appliance LAN traffic is intentionally open for local-first control,
    // but secret-bearing recovery endpoints require positive owner proof even
    // on that trusted network. Do this before the general `requires_auth`
    // bypass so an unauthenticated LAN caller cannot read retained secrets.
    if owner_token_required(req.method(), req.uri())
        && !matches!(
            verified_bearer.as_ref().map(|token| token.role),
            Some(ApiTokenRole::Owner)
        )
    {
        return unauthorized("Owner bearer token required");
    }

    if !auth_info.requires_auth {
        return next.run(req).await;
    }

    let Some(token) = bearer_token(req.headers().get(AUTHORIZATION)) else {
        return unauthorized("Missing bearer token");
    };

    let verified_token = verified_bearer.or_else(|| {
        state
            .lock()
            .map(|s| s.api_auth.verify_token_info(token))
            .unwrap_or(None)
    });
    let Some(verified_token) = verified_token else {
        return unauthorized("Invalid bearer token");
    };

    auth_info.role = Some(verified_token.role);
    auth_info.token_expires_at_epoch_ms = verified_token.expires_at_epoch_ms;
    req.extensions_mut().insert(auth_info);

    if verified_token.role == ApiTokenRole::Support {
        if let Some(reason) = support_token_forbidden_reason(req.method(), req.uri()) {
            return forbidden(reason);
        }
    }

    next.run(req).await
}

fn auth_status_payload(
    require_api_auth: bool,
    owner_configured: bool,
    token_count: usize,
    claim_available: bool,
    via_remote_access: bool,
    authenticated_role: Option<ApiTokenRole>,
) -> Value {
    json!({
        "requires_auth": require_api_auth,
        "owner_configured": owner_configured,
        "token_count": token_count,
        "claim_available": claim_available,
        "via_remote_access": via_remote_access,
        "authenticated_role": authenticated_role.map(|role| match role {
            ApiTokenRole::Owner => "owner",
            ApiTokenRole::Support => "support",
        }),
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
        role: None,
        token_expires_at_epoch_ms: None,
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

fn verified_bearer_token(state: &SharedState, req: &Request<Body>) -> Option<VerifiedApiToken> {
    let token = bearer_token(req.headers().get(AUTHORIZATION))?;
    state
        .lock()
        .map(|s| s.api_auth.verify_token_info(token))
        .unwrap_or(None)
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

fn forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        [("content-type", "application/json")],
        json!({
            "status": "error",
            "message": message,
        })
        .to_string(),
    )
        .into_response()
}

fn support_token_forbidden_reason(method: &Method, uri: &Uri) -> Option<&'static str> {
    let path = uri.path();
    if *method == Method::GET && path.starts_with("/api/matter/setup-code/") {
        return Some("Support token cannot read Matter setup codes");
    }
    if *method == Method::DELETE && path.starts_with("/api/devices/canonical/") {
        return Some("Support token cannot permanently delete archived devices");
    }
    if *method == Method::GET
        && path == "/api/backup"
        && query_flag_truthy(uri.query(), "include_secrets")
    {
        return Some("Support token cannot export backup secrets");
    }

    // Beta admin support tokens are intentionally broad. Keep ownership,
    // credential, tunnel, network, and destructive recovery operations owner-only.
    let forbidden = matches!(
        (method, path),
        (&Method::POST, "/api/auth/claim")
            | (&Method::POST, "/api/auth/support-token")
            | (&Method::PUT, "/api/auth/settings")
            | (&Method::POST, "/api/factory-reset")
            | (&Method::PUT, "/api/hub/credentials")
            | (&Method::DELETE, "/api/hub/credentials")
            | (&Method::PUT, "/api/remote-access/config")
            | (&Method::DELETE, "/api/remote-access/config")
            | (&Method::PUT, "/api/wifi")
            | (&Method::DELETE, "/api/wifi")
            | (&Method::POST, "/api/diag/reset-matter-fabric")
            | (&Method::PUT, "/api/backup")
    );
    if forbidden {
        Some("Support token is not allowed for this endpoint")
    } else {
        None
    }
}

fn owner_token_required(method: &Method, uri: &Uri) -> bool {
    (*method == Method::GET && uri.path().starts_with("/api/matter/setup-code/"))
        || (*method == Method::DELETE && uri.path().starts_with("/api/devices/canonical/"))
}

fn query_flag_truthy(query: Option<&str>, key: &str) -> bool {
    let Some(query) = query else {
        return false;
    };
    query.split('&').any(|pair| {
        let (pair_key, value) = pair.split_once('=').unwrap_or((pair, "true"));
        pair_key == key && matches!(value, "" | "1" | "true" | "yes" | "on")
    })
}

fn write_support_audit_line(
    state: &SharedState,
    token_id: &str,
    method: &Method,
    path: &str,
    request_id: Option<&str>,
) {
    let data_dir = state
        .lock()
        .ok()
        .map(|s| s.data_dir.clone())
        .unwrap_or_default();
    if data_dir.trim().is_empty() {
        return;
    }

    let audit_path = Path::new(&data_dir).join(SUPPORT_AUDIT_FILE);
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(audit_path)
    else {
        return;
    };
    let _ = writeln!(
        file,
        "{}\t{}\t{}\t{}\t{}",
        current_epoch_ms(),
        token_id,
        method,
        path,
        sanitized_support_request_id(request_id),
    );
}

fn sanitized_support_request_id(request_id: Option<&str>) -> &str {
    let Some(request_id) = request_id.map(str::trim) else {
        return "-";
    };
    if (8..=128).contains(&request_id.len())
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        request_id
    } else {
        "-"
    }
}

fn generate_raw_token(prefix: &str) -> String {
    let mut bytes = [0_u8; TOKEN_RANDOM_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{}{}", prefix, hex_encode(&bytes))
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
    use std::fs;
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

    fn test_uri(uri: &str) -> Uri {
        uri.parse().unwrap()
    }

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rhythm-auth-{name}-{nanos}"));
        fs::create_dir_all(&path).unwrap();
        path
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
                expires_at_epoch_ms: None,
            }],
        };

        assert!(stored.verify_token(raw));
        assert!(!stored.verify_token("wrong"));
    }

    #[test]
    fn generated_owner_tokens_are_prefixed_and_distinct() {
        let a = generate_raw_token(TOKEN_PREFIX);
        let b = generate_raw_token(TOKEN_PREFIX);

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
    fn local_support_token_is_stored_with_support_role() {
        let state = test_state();

        let issued = issue_local_support_token(&state, Some("support".into())).unwrap();

        assert!(issued.token.starts_with(SUPPORT_TOKEN_PREFIX));
        let state = state.lock().unwrap();
        assert!(state.api_auth.verify_token(&issued.token));
        assert_eq!(
            state.api_auth.verify_token_role(&issued.token),
            Some(ApiTokenRole::Support)
        );
        assert_eq!(state.api_auth.tokens[0].label.as_deref(), Some("support"));
    }

    #[test]
    fn support_session_token_expires_and_can_be_revoked() {
        let state = test_state();

        let (issued, expires_at_epoch_ms) =
            issue_support_session_token(&state, Some("grant".into()), Some(60)).unwrap();

        assert!(issued.token.starts_with(SUPPORT_SESSION_TOKEN_PREFIX));
        assert!(expires_at_epoch_ms > current_epoch_ms());
        {
            let state = state.lock().unwrap();
            assert_eq!(
                state.api_auth.verify_token_role(&issued.token),
                Some(ApiTokenRole::Support)
            );
            assert_eq!(
                state.api_auth.tokens[0].expires_at_epoch_ms,
                Some(expires_at_epoch_ms)
            );
        }

        assert!(revoke_support_session_token(&state, &issued.id).unwrap());
        assert!(!state.lock().unwrap().api_auth.verify_token(&issued.token));
    }

    #[test]
    fn expiring_support_session_token_cannot_mint_more_sessions() {
        assert!(request_has_privileged_token(Some(ApiAuthRequestInfo {
            requires_auth: true,
            claim_available: false,
            via_remote_access: true,
            role: Some(ApiTokenRole::Support),
            token_expires_at_epoch_ms: None,
        })));
        assert!(!request_has_privileged_token(Some(ApiAuthRequestInfo {
            requires_auth: true,
            claim_available: false,
            via_remote_access: true,
            role: Some(ApiTokenRole::Support),
            token_expires_at_epoch_ms: Some(current_epoch_ms() + 60_000),
        })));
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
    async fn appliance_lan_setup_code_recovery_requires_explicit_owner_token() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
            state.load_pairing_recovery_fn = Some(Arc::new(|_, hub_type, native_device_id| {
                assert_eq!(hub_type, "matter");
                assert_eq!(native_device_id, "matter-42-2");
                Ok(Some(crate::pairing::PairingRecoverySecret {
                    payload_kind: "qr_code".to_string(),
                    setup_payload: "MT:OWNER-ONLY-SECRET".to_string(),
                    captured_at: "2026-08-12T00:00:00Z".to_string(),
                }))
            }));
        }
        let owner = issue_local_owner_token(&state, Some("BLE Wi-Fi".into())).unwrap();
        let app = auth_test_router(state);

        let unauthenticated = app
            .clone()
            .oneshot(request_with_peer(
                Method::GET,
                "/api/matter/setup-code/matter-42-2",
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
        let unauthenticated_body = to_bytes(unauthenticated.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&unauthenticated_body).contains("MT:"));

        let authenticated = app
            .oneshot({
                let mut request = request_with_peer(
                    Method::GET,
                    "/api/matter/setup-code/matter-42-2",
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                    Body::empty(),
                );
                request.headers_mut().insert(
                    AUTHORIZATION,
                    format!("Bearer {}", owner.token).parse().unwrap(),
                );
                request
            })
            .await
            .unwrap();
        assert_eq!(authenticated.status(), StatusCode::OK);
        let authenticated_body = to_bytes(authenticated.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&authenticated_body).contains("MT:OWNER-ONLY-SECRET"));
    }

    #[tokio::test]
    async fn appliance_lan_auth_status_reports_whether_the_bearer_is_an_owner() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
        }
        let owner = issue_local_owner_token(&state, Some("BLE Wi-Fi".into())).unwrap();
        let app = auth_test_router(state);

        let invalid = app
            .clone()
            .oneshot({
                let mut request = request_with_peer(
                    Method::GET,
                    "/api/auth/status",
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                    Body::empty(),
                );
                request
                    .headers_mut()
                    .insert(AUTHORIZATION, "Bearer rhythm_owner_wrong".parse().unwrap());
                request
            })
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::OK);
        let invalid_body = serde_json::from_slice::<Value>(
            &to_bytes(invalid.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(invalid_body["authenticated_role"], Value::Null);

        let valid = app
            .oneshot({
                let mut request = request_with_peer(
                    Method::GET,
                    "/api/auth/status",
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                    Body::empty(),
                );
                request.headers_mut().insert(
                    AUTHORIZATION,
                    format!("Bearer {}", owner.token).parse().unwrap(),
                );
                request
            })
            .await
            .unwrap();
        assert_eq!(valid.status(), StatusCode::OK);
        let valid_body = serde_json::from_slice::<Value>(
            &to_bytes(valid.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(valid_body["authenticated_role"], "owner");
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
    async fn assistant_contract_uses_remote_owner_auth() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
        }
        let issued = issue_local_owner_token(&state, Some("phone".into())).unwrap();
        let app = auth_test_router(state);

        let unauthenticated = app
            .clone()
            .oneshot(tunnel_request(
                Method::GET,
                crate::assistant::LIGHT_ASSISTANT_CONTRACT_PATH,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let authenticated = app
            .oneshot({
                let mut request = tunnel_request(
                    Method::GET,
                    crate::assistant::LIGHT_ASSISTANT_CONTRACT_PATH,
                    Body::empty(),
                );
                request.headers_mut().insert(
                    AUTHORIZATION,
                    format!("Bearer {}", issued.token).parse().unwrap(),
                );
                request
            })
            .await
            .unwrap();
        assert_eq!(authenticated.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn appliance_tunnel_request_cannot_issue_support_token() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
        }
        let owner = issue_local_owner_token(&state, Some("phone".into())).unwrap();
        let app = auth_test_router(state.clone());

        let response = app
            .oneshot({
                let mut req = tunnel_request(
                    Method::POST,
                    "/api/auth/support-token",
                    Body::from(json!({"label": "support"}).to_string()),
                );
                req.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                req.headers_mut().insert(
                    AUTHORIZATION,
                    format!("Bearer {}", owner.token).parse().unwrap(),
                );
                req
            })
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(state.lock().unwrap().api_auth.tokens.len(), 1);
    }

    #[tokio::test]
    async fn local_support_token_endpoint_requires_owner_bearer() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
        }
        let app = auth_test_router(state.clone());

        let response = app
            .clone()
            .oneshot({
                let mut req = request_with_peer(
                    Method::POST,
                    "/api/auth/support-token",
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                    Body::from(json!({"label": "support"}).to_string()),
                );
                req.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                req
            })
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(state.lock().unwrap().api_auth.tokens.is_empty());

        let owner = issue_local_owner_token(&state, Some("owner".into())).unwrap();
        let response = app
            .oneshot({
                let mut req = request_with_peer(
                    Method::POST,
                    "/api/auth/support-token",
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                    Body::from(json!({"label": "support"}).to_string()),
                );
                req.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                req.headers_mut().insert(
                    AUTHORIZATION,
                    format!("Bearer {}", owner.token).parse().unwrap(),
                );
                req
            })
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let state = state.lock().unwrap();
        assert_eq!(state.api_auth.tokens.len(), 2);
        assert_eq!(state.api_auth.tokens[1].role, ApiTokenRole::Support);
    }

    #[tokio::test]
    async fn appliance_tunnel_request_accepts_support_token_for_allowed_routes_and_audits() {
        let data_dir = unique_test_dir("support-audit");
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
            state.data_dir = data_dir.to_string_lossy().to_string();
        }
        let issued = issue_local_support_token(&state, Some("support".into())).unwrap();
        let app = auth_test_router(state);

        let response = app
            .oneshot({
                let mut req = tunnel_request(Method::GET, "/api/state", Body::empty());
                req.headers_mut().insert(
                    AUTHORIZATION,
                    format!("Bearer {}", issued.token).parse().unwrap(),
                );
                req.headers_mut()
                    .insert("x-request-id", "customer-tuning:test-1234".parse().unwrap());
                req
            })
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let audit = fs::read_to_string(data_dir.join(SUPPORT_AUDIT_FILE)).unwrap();
        assert!(audit.contains(&issued.id));
        assert!(audit.contains("\tGET\t/api/state"));
        assert!(audit.contains("\tcustomer-tuning:test-1234"));
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn support_audit_request_ids_fail_closed_to_a_placeholder() {
        assert_eq!(sanitized_support_request_id(None), "-");
        assert_eq!(sanitized_support_request_id(Some("short")), "-");
        assert_eq!(sanitized_support_request_id(Some("unsafe request id")), "-");
        assert_eq!(
            sanitized_support_request_id(Some("customer-tuning:abc_123")),
            "customer-tuning:abc_123"
        );
    }

    #[tokio::test]
    async fn support_token_is_forbidden_from_owner_only_routes() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
        }
        let issued = issue_local_support_token(&state, Some("support".into())).unwrap();
        let app = auth_test_router(state);

        let response = app
            .oneshot({
                let mut req = tunnel_request(Method::POST, "/api/factory-reset", Body::empty());
                req.headers_mut().insert(
                    AUTHORIZATION,
                    format!("Bearer {}", issued.token).parse().unwrap(),
                );
                req
            })
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn support_token_cannot_export_backup_secrets() {
        let state = test_state();
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
        }
        let issued = issue_local_support_token(&state, Some("support".into())).unwrap();
        let app = auth_test_router(state);

        let response = app
            .oneshot({
                let mut req = tunnel_request(
                    Method::GET,
                    "/api/backup?include_secrets=true",
                    Body::empty(),
                );
                req.headers_mut().insert(
                    AUTHORIZATION,
                    format!("Bearer {}", issued.token).parse().unwrap(),
                );
                req
            })
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn support_policy_blocks_owner_only_routes() {
        let denied = [
            (Method::POST, "/api/auth/claim", "auth claim"),
            (
                Method::POST,
                "/api/auth/support-token",
                "support token mint",
            ),
            (Method::PUT, "/api/auth/settings", "auth settings"),
            (Method::POST, "/api/factory-reset", "factory reset"),
            (Method::PUT, "/api/hub/credentials", "hub credentials"),
            (Method::DELETE, "/api/hub/credentials", "hub delete"),
            (
                Method::PUT,
                "/api/remote-access/config",
                "remote access config",
            ),
            (
                Method::DELETE,
                "/api/remote-access/config",
                "remote access clear",
            ),
            (Method::PUT, "/api/wifi", "wifi"),
            (Method::DELETE, "/api/wifi", "wifi clear"),
            (
                Method::POST,
                "/api/diag/reset-matter-fabric",
                "matter fabric reset",
            ),
            (Method::PUT, "/api/backup", "backup restore"),
            (
                Method::GET,
                "/api/backup?include_secrets=true",
                "secret backup export",
            ),
            (
                Method::GET,
                "/api/matter/setup-code/matter-42",
                "Matter setup code recovery",
            ),
            (
                Method::DELETE,
                "/api/devices/canonical/removed-light-42",
                "removed device permanent delete",
            ),
        ];

        for (method, uri, label) in denied {
            assert!(
                support_token_forbidden_reason(&method, &test_uri(uri)).is_some(),
                "{label} should stay owner-only"
            );
        }
    }

    #[test]
    fn support_policy_allows_beta_admin_routes() {
        let allowed = [
            (Method::GET, "/api/backup", "redacted backup export"),
            (Method::GET, "/api/ota/status", "ota status"),
            (Method::POST, "/api/ota/upload", "ota upload"),
            (Method::POST, "/api/ota/update", "ota apply"),
            (Method::POST, "/api/diag/debug-bundle", "debug bundle"),
            (Method::POST, "/api/restart", "restart"),
            (Method::PUT, "/api/config", "runtime config"),
            (Method::PUT, "/api/settings", "settings"),
            (Method::PUT, "/api/mode", "mode"),
            (Method::PUT, "/api/transitions", "transitions"),
            (Method::POST, "/api/scenes", "scene create"),
            (Method::PUT, "/api/topology/rooms/kitchen", "room rename"),
            (Method::POST, "/api/devices/pair", "device pairing"),
            (Method::POST, "/api/devices/unpair", "device unpairing"),
        ];

        for (method, uri, label) in allowed {
            assert!(
                support_token_forbidden_reason(&method, &test_uri(uri)).is_none(),
                "{label} should be allowed for beta admin support"
            );
        }
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
