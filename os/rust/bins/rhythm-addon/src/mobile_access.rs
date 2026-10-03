//! Installation-bound mobile enrollment. The Ingress credential is deliberately
//! outside AppState/api-auth.json and cannot authenticate to the mobile listener.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rand::RngCore;
use rhythm_os::{auth::ApiTokenRole, state::SharedState};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

const ENROLLMENT_TTL_MS: u64 = 5 * 60 * 1000;
const MAX_ATTEMPTS: u8 = 8;

fn current_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct MobileAccess {
    admin_hash: [u8; 32],
    enrollment: Mutex<Option<Enrollment>>,
    resetting: AtomicBool,
    pub mutation_gate: Arc<tokio::sync::Mutex<()>>,
}

struct Enrollment {
    code_hash: [u8; 32],
    server_instance_id: String,
    expires_at_epoch_ms: u64,
    attempts: u8,
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn same_secret(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

impl MobileAccess {
    pub fn new(admin_token: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            admin_token.len() >= 32 && admin_token.bytes().all(|b| b.is_ascii_alphanumeric()),
            "Invalid local admin credential"
        );
        Ok(Self {
            admin_hash: digest(admin_token),
            enrollment: Mutex::new(None),
            resetting: AtomicBool::new(false),
            mutation_gate: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub fn verify_admin(&self, token: &str) -> bool {
        same_secret(&self.admin_hash, &digest(token))
    }

    pub fn invalidate_enrollment(&self) {
        let mut pending = self.enrollment.lock().expect("enrollment lock");
        self.resetting.store(true, Ordering::SeqCst);
        *pending = None;
    }

    pub fn is_resetting(&self) -> bool {
        self.resetting.load(Ordering::SeqCst)
    }
}

fn private_json(status: StatusCode, body: serde_json::Value) -> Response {
    (status, [("cache-control", "no-store")], Json(body)).into_response()
}

pub async fn create_enrollment(
    State(state): State<SharedState>,
    Extension(access): Extension<Arc<MobileAccess>>,
) -> Response {
    let mut pending = access.enrollment.lock().expect("enrollment lock");
    if access.is_resetting() {
        return private_json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Installation is resetting"}),
        );
    }
    let server_instance_id = state.lock().expect("state lock").server_instance_id.clone();
    if server_instance_id.is_empty() {
        return private_json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Installation identity unavailable"}),
        );
    }
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let code: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let expires_at_epoch_ms = current_epoch_ms().saturating_add(ENROLLMENT_TTL_MS);
    *pending = Some(Enrollment {
        code_hash: digest(&code),
        server_instance_id: server_instance_id.clone(),
        expires_at_epoch_ms,
        attempts: 0,
    });
    private_json(
        StatusCode::OK,
        json!({
            "format":"rhythm-mobile-enrollment", "version":1,
            "code":code, "server_instance_id":server_instance_id,
            "expires_at_epoch_ms":expires_at_epoch_ms,
        }),
    )
}

#[derive(Deserialize)]
pub struct Exchange {
    code: String,
    server_instance_id: String,
    label: Option<String>,
}

pub async fn exchange_enrollment(
    State(state): State<SharedState>,
    Extension(access): Extension<Arc<MobileAccess>>,
    Json(body): Json<Exchange>,
) -> Response {
    // Hold the enrollment lock through durable issuance: concurrent exchanges
    // cannot both redeem the same approval. Failure consumes the code too.
    let mut pending = access.enrollment.lock().expect("enrollment lock");
    if access.is_resetting() {
        return private_json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Installation is resetting"}),
        );
    }
    let server_instance_id = state.lock().expect("state lock").server_instance_id.clone();
    let valid = if let Some(enrollment) = pending.as_mut() {
        enrollment.attempts = enrollment.attempts.saturating_add(1);
        enrollment.attempts <= MAX_ATTEMPTS
            && enrollment.expires_at_epoch_ms > current_epoch_ms()
            && enrollment.server_instance_id == server_instance_id
            && body.server_instance_id == server_instance_id
            && body.code.len() == 64
            && same_secret(&enrollment.code_hash, &digest(&body.code))
    } else {
        false
    };
    if !valid {
        if pending.as_ref().is_some_and(|e| {
            e.attempts >= MAX_ATTEMPTS || e.expires_at_epoch_ms <= current_epoch_ms()
        }) {
            *pending = None;
        }
        return private_json(
            StatusCode::UNAUTHORIZED,
            json!({"error":"Enrollment code invalid or expired", "code":"enrollment_unavailable"}),
        );
    }
    *pending = None;
    let label = body
        .label
        .map(|label| label.trim().chars().take(80).collect::<String>())
        .filter(|label| !label.is_empty());
    match rhythm_os::auth::issue_local_owner_token(&state, label) {
        Ok(issued) => private_json(
            StatusCode::OK,
            json!({"status":"ok", "token":issued.token, "token_id":issued.id, "server_instance_id":server_instance_id}),
        ),
        Err(_) => private_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"Unable to save mobile credential"}),
        ),
    }
}

pub async fn list_tokens(State(state): State<SharedState>) -> Response {
    let s = state.lock().expect("state lock");
    let tokens: Vec<_> = s.api_auth.tokens.iter().filter(|token| token.role == ApiTokenRole::Owner).map(|token| json!({
        "id":token.id, "label":token.label, "role":token.role,
        "created_at_epoch_ms":token.created_at_epoch_ms, "expires_at_epoch_ms":token.expires_at_epoch_ms,
    })).collect();
    private_json(StatusCode::OK, json!({"tokens":tokens}))
}

pub async fn revoke_token(State(state): State<SharedState>, Path(id): Path<String>) -> Response {
    match rhythm_os::auth::revoke_local_token(&state, &id) {
        Ok(revoked) => private_json(StatusCode::OK, json!({"revoked":revoked, "token_id":id})),
        Err(_) => private_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"Unable to revoke mobile credential"}),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_os::state::AppState;

    async fn fixture() -> (SharedState, Arc<MobileAccess>, serde_json::Value) {
        let state = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().server_instance_id = "fixture-installation".into();
        let access = Arc::new(MobileAccess::new(&"a".repeat(64)).unwrap());
        let response = create_enrollment(State(state.clone()), Extension(access.clone())).await;
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        (state, access, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn expired_approval_cannot_issue_a_credential() {
        let (state, access, code) = fixture().await;
        access
            .enrollment
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .expires_at_epoch_ms = 0;
        let response = exchange_enrollment(
            State(state.clone()),
            Extension(access.clone()),
            Json(serde_json::from_value(code).unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(state.lock().unwrap().api_auth.tokens.is_empty());
        assert!(access.enrollment.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn repeated_invalid_exchanges_exhaust_the_approval_and_reset_blocks_reissue() {
        let (state, access, code) = fixture().await;
        for _ in 0..MAX_ATTEMPTS {
            let mut wrong = code.clone();
            wrong["code"] = json!("b".repeat(64));
            let response = exchange_enrollment(
                State(state.clone()),
                Extension(access.clone()),
                Json(serde_json::from_value(wrong).unwrap()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let response = exchange_enrollment(
            State(state.clone()),
            Extension(access.clone()),
            Json(serde_json::from_value(code).unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        access.invalidate_enrollment();
        assert_eq!(
            create_enrollment(State(state), Extension(access))
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn admin_credential_validation_and_separation() {
        assert!(MobileAccess::new("short").is_err());
        let access = MobileAccess::new(&"a".repeat(64)).unwrap();
        assert!(access.verify_admin(&"a".repeat(64)));
        assert!(!access.verify_admin(&"b".repeat(64)));
        assert!(!access.verify_admin(""));
    }
}
