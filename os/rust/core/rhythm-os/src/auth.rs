//! Local API authentication for Rhythm OS.
//!
//! The appliance owns its API credentials. Cloud/account identity can decide
//! who receives a token later, but every HTTP request to the local server should
//! still be authorized by a device-issued bearer token.

use axum::body::Body;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::handlers::ApiResponse;
use crate::state::{current_epoch_ms, SharedState};

const TOKEN_RANDOM_BYTES: usize = 32;
const TOKEN_PREFIX: &str = "rhythm_owner_";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredApiAuth {
    #[serde(default = "default_schema_version")]
    pub schema_version: u8,
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

pub fn clear_api_auth(state: &SharedState) -> anyhow::Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    s.api_auth = StoredApiAuth::default();
    if let Some(storage) = s.storage.as_ref() {
        storage.save_api_auth(&s.api_auth)?;
    }
    Ok(())
}

pub fn handle_get_auth_status(state: &SharedState) -> ApiResponse {
    match state.lock() {
        Ok(s) => ApiResponse::json_ok(
            json!({
                "requires_auth": s.require_api_auth,
                "owner_configured": s.api_auth.has_owner(),
                "token_count": s.api_auth.tokens.len(),
                "claim_available": s.require_api_auth && !s.api_auth.has_owner(),
            })
            .to_string(),
        ),
        Err(_) => ApiResponse::server_error("lock"),
    }
}

pub fn handle_claim_owner_token(state: &SharedState, label: Option<String>) -> ApiResponse {
    match state.lock() {
        Ok(s) if !s.require_api_auth => {
            return ApiResponse::bad_request("API auth is not required on this server");
        }
        Ok(_) => {}
        Err(_) => return ApiResponse::server_error("lock"),
    }

    match issue_owner_token(state, label) {
        Ok(IssueOwnerTokenResult::Issued(issued)) => ApiResponse::json_ok(
            json!({
                "status": "ok",
                "token_id": issued.id,
                "token": issued.token,
            })
            .to_string(),
        ),
        Ok(IssueOwnerTokenResult::AlreadyConfigured) => ApiResponse {
            status: StatusCode::CONFLICT.as_u16(),
            body: json!({
                "status": "error",
                "message": "Owner token is already configured",
            })
            .to_string(),
            content_type: "application/json",
        },
        Err(e) => ApiResponse::server_error(e),
    }
}

pub async fn require_api_auth_middleware(
    State(state): State<SharedState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if is_public_request(req.method(), req.uri().path()) {
        return next.run(req).await;
    }

    let auth_required = state.lock().map(|s| s.require_api_auth).unwrap_or(true);
    if !auth_required {
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

fn is_public_request(method: &Method, path: &str) -> bool {
    *method == Method::OPTIONS
        || path == "/health"
        || path == "/api/auth/status"
        || (*method == Method::POST && path == "/api/auth/claim")
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

    #[test]
    fn stored_auth_verifies_hashed_token_only() {
        let raw = "rhythm_owner_test";
        let stored = StoredApiAuth {
            schema_version: 1,
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
        let state = std::sync::Arc::new(std::sync::Mutex::new(crate::state::AppState::default()));

        let first = issue_owner_token(&state, Some("first".into())).unwrap();
        assert!(matches!(first, IssueOwnerTokenResult::Issued(_)));

        let second = issue_owner_token(&state, Some("second".into())).unwrap();
        assert_eq!(second, IssueOwnerTokenResult::AlreadyConfigured);
    }
}
