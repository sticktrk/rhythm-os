//! Device-side cloud upload for server-origin light activity.
//!
//! The app provisions a scoped upload token, but Rhythm OS owns the event path:
//! the local buffer is the retry source, uploads go directly to Supabase, and
//! successfully uploaded events are drained from the buffer.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Extension, State};
use axum::Json;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::activity::{LightActivityEvent, LIGHT_ACTIVITY_HISTORY_LIMIT};
use crate::auth::{ApiAuthRequestInfo, ApiTokenRole};
use crate::handlers::ApiResponse;
use crate::state::SharedState;

const ACTIVITY_CLOUD_SCHEMA_VERSION: u8 = 1;
const UPLOAD_STATUS_PENDING: &str = "pending";
const UPLOAD_STATUS_OK: &str = "ok";
const UPLOAD_STATUS_FAILED: &str = "failed";
const UPLOAD_STATUS_AUTH_FAILED: &str = "auth_failed";
const UPLOAD_STATUS_NOT_CONFIGURED: &str = "not_configured";
const JOIN_PROOF_VERSION: &str = "activity-token-hmac-v1";
const JOIN_PROOF_ALGORITHM: &str = "hmac-sha256";
const JOIN_PROOF_TTL_MS: u64 = 2 * 60 * 1000;

fn default_schema_version() -> u8 {
    ACTIVITY_CLOUD_SCHEMA_VERSION
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredActivityCloudConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u8,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub ingest_url: String,
    pub upload_token: String,
    pub home_id: String,
    pub hub_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upload_attempt_epoch_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upload_success_epoch_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upload_failure_epoch_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upload_http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upload_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_failed_at_epoch_ms: Option<u64>,
    pub updated_at_epoch_ms: u64,
}

impl StoredActivityCloudConfig {
    fn normalized(mut self) -> Self {
        self.schema_version = ACTIVITY_CLOUD_SCHEMA_VERSION;
        self.ingest_url = self.ingest_url.trim().to_string();
        self.upload_token = self.upload_token.trim().to_string();
        self.home_id = self.home_id.trim().to_string();
        self.hub_id = self.hub_id.trim().to_string();
        self.token_id = clean_optional(self.token_id);
        self.server_instance_id = clean_optional(self.server_instance_id);
        self.upload_status = clean_optional(self.upload_status);
        self.last_upload_error =
            clean_optional(self.last_upload_error).map(|error| error.chars().take(256).collect());
        self
    }

    fn is_usable(&self) -> bool {
        self.enabled
            && !self.ingest_url.trim().is_empty()
            && !self.upload_token.trim().is_empty()
            && !self.home_id.trim().is_empty()
            && !self.hub_id.trim().is_empty()
            && !self.needs_reprovision()
    }

    fn needs_reprovision(&self) -> bool {
        self.auth_failed_at_epoch_ms.is_some()
            || self.upload_status.as_deref() == Some(UPLOAD_STATUS_AUTH_FAILED)
    }
}

#[derive(Debug, Deserialize)]
pub struct PutActivityCloudConfig {
    #[serde(default = "default_enabled")]
    enabled: bool,
    ingest_url: Option<String>,
    upload_token: Option<String>,
    home_id: Option<String>,
    hub_id: Option<String>,
    token_id: Option<String>,
    server_instance_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ActivityCloudStatusBody {
    status: &'static str,
    configured: bool,
    enabled: bool,
    home_id: Option<String>,
    hub_id: Option<String>,
    token_id: Option<String>,
    server_instance_id: Option<String>,
    updated_at_epoch_ms: u64,
    upload_status: &'static str,
    needs_reprovision: bool,
    last_upload_attempt_epoch_ms: Option<u64>,
    last_upload_success_epoch_ms: Option<u64>,
    last_upload_failure_epoch_ms: Option<u64>,
    last_upload_http_status: Option<u16>,
    last_upload_error: Option<String>,
    auth_failed_at_epoch_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CloudJoinProof {
    status: &'static str,
    proof_version: &'static str,
    algorithm: &'static str,
    server_instance_id: String,
    home_id: String,
    hub_id: String,
    token_id: Option<String>,
    issued_at_epoch_ms: u64,
    expires_at_epoch_ms: u64,
    nonce: String,
    signature: String,
}

pub async fn get_config(State(state): State<SharedState>) -> ApiResponse {
    match load_config(&state) {
        Ok(config) => json_ok(status_body(config.as_ref())),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub async fn put_config(
    State(state): State<SharedState>,
    Json(body): Json<PutActivityCloudConfig>,
) -> ApiResponse {
    let config = match config_from_body(body) {
        Ok(config) => config,
        Err(message) => return ApiResponse::bad_request(&message),
    };

    if let Err(e) = save_config(&state, &config) {
        return ApiResponse::server_error(e);
    }
    enqueue_recent_activity_upload(&state);
    json_ok(status_body(Some(&config)))
}

pub async fn delete_config(State(state): State<SharedState>) -> ApiResponse {
    if let Err(e) = clear_config(&state) {
        return ApiResponse::server_error(e);
    }
    json_ok(status_body(None))
}

pub async fn post_join_proof(
    State(state): State<SharedState>,
    auth_info: Option<Extension<ApiAuthRequestInfo>>,
) -> ApiResponse {
    if !matches!(
        auth_info.map(|Extension(info)| info.role),
        Some(Some(ApiTokenRole::Owner))
    ) {
        return ApiResponse::forbidden("Cloud Home join proof requires an owner token");
    }

    match create_join_proof(&state) {
        Ok(proof) => json_ok(proof),
        Err(JoinProofError::NotConfigured(message)) => ApiResponse::conflict(message),
        Err(JoinProofError::Server(message)) => ApiResponse::server_error(message),
    }
}

pub fn enqueue_light_activity_upload(state: &SharedState) {
    enqueue_recent_activity_upload(state);
}

enum JoinProofError {
    NotConfigured(&'static str),
    Server(anyhow::Error),
}

impl From<anyhow::Error> for JoinProofError {
    fn from(error: anyhow::Error) -> Self {
        JoinProofError::Server(error)
    }
}

fn create_join_proof(state: &SharedState) -> Result<CloudJoinProof, JoinProofError> {
    let config = load_config(state)?.ok_or(JoinProofError::NotConfigured(
        "Activity cloud is not configured",
    ))?;
    if !config.is_usable() {
        return Err(JoinProofError::NotConfigured(
            "Activity cloud credentials are not usable",
        ));
    }

    let state_server_instance_id = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .server_instance_id
        .trim()
        .to_string();
    if state_server_instance_id.is_empty() {
        return Err(JoinProofError::NotConfigured(
            "Server identity is not available",
        ));
    }

    if let Some(config_server_instance_id) = config.server_instance_id.as_deref() {
        if !config_server_instance_id.eq_ignore_ascii_case(&state_server_instance_id) {
            return Err(JoinProofError::NotConfigured(
                "Activity cloud credentials belong to another server identity",
            ));
        }
    }

    let issued_at_epoch_ms = current_epoch_ms();
    let expires_at_epoch_ms = issued_at_epoch_ms.saturating_add(JOIN_PROOF_TTL_MS);
    let nonce = random_hex(16);
    let token_hash = sha256_hex(config.upload_token.as_bytes());
    let canonical = join_proof_canonical_string(
        &state_server_instance_id,
        &config.home_id,
        &config.hub_id,
        issued_at_epoch_ms,
        expires_at_epoch_ms,
        &nonce,
    );
    let signature = hmac_sha256_hex(token_hash.as_bytes(), canonical.as_bytes());

    Ok(CloudJoinProof {
        status: "ok",
        proof_version: JOIN_PROOF_VERSION,
        algorithm: JOIN_PROOF_ALGORITHM,
        server_instance_id: state_server_instance_id,
        home_id: config.home_id,
        hub_id: config.hub_id,
        token_id: config.token_id,
        issued_at_epoch_ms,
        expires_at_epoch_ms,
        nonce,
        signature,
    })
}

fn join_proof_canonical_string(
    server_instance_id: &str,
    home_id: &str,
    hub_id: &str,
    issued_at_epoch_ms: u64,
    expires_at_epoch_ms: u64,
    nonce: &str,
) -> String {
    format!(
        "{JOIN_PROOF_VERSION}\n{server_instance_id}\n{home_id}\n{hub_id}\n{issued_at_epoch_ms}\n{expires_at_epoch_ms}\n{nonce}"
    )
}

fn random_hex(byte_count: usize) -> String {
    let mut bytes = vec![0u8; byte_count];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex_lower(&bytes)
}

fn sha256_hex(value: &[u8]) -> String {
    hex_lower(&Sha256::digest(value))
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = Sha256::digest(key);
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut outer_key_pad = [0x5c; BLOCK_SIZE];
    let mut inner_key_pad = [0x36; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        outer_key_pad[i] ^= key_block[i];
        inner_key_pad[i] ^= key_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(inner_key_pad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_key_pad);
    outer.update(inner_hash);
    hex_lower(&outer.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

pub fn enqueue_recent_activity_upload(state: &SharedState) {
    let Some((config, activities)) = upload_snapshot(state) else {
        return;
    };
    if activities.is_empty() {
        return;
    }

    let Some(handle) = upload_runtime_handle(state) else {
        log::warn!(
            target: "cmd",
            "Skipping server activity cloud upload: no Tokio runtime handle available"
        );
        return;
    };

    let state_for_result = state.clone();
    handle.spawn(async move {
        let attempt_epoch_ms = current_epoch_ms();
        match upload_activity_batch(&config, &activities).await {
            Ok(()) => {
                crate::activity::remove_uploaded_light_activity(&state_for_result, &activities);
                if let Err(error) =
                    record_upload_success(&state_for_result, &config, attempt_epoch_ms)
                {
                    log::warn!(
                        target: "cmd",
                        "Failed to record server activity cloud upload success: {:#}",
                        error
                    );
                }
            }
            Err(error) => {
                log::warn!(
                    target: "cmd",
                    "Failed to upload server light activity batch: {:#}",
                    error
                );
                if let Err(record_error) =
                    record_upload_failure(&state_for_result, &config, attempt_epoch_ms, &error)
                {
                    log::warn!(
                        target: "cmd",
                        "Failed to record server activity cloud upload failure: {:#}",
                        record_error
                    );
                }
            }
        }
    });
}

/// Resolve a Tokio runtime handle for spawning the upload task.
///
/// Activity is recorded on plain worker threads (`http-handler`, event-loop
/// dispatch) where `Handle::try_current()` fails, so fall back to the handle
/// captured into state at server startup.
fn upload_runtime_handle(state: &SharedState) -> Option<tokio::runtime::Handle> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return Some(handle);
    }
    state.lock().ok().and_then(|s| s.tokio_handle.clone())
}

fn config_from_body(body: PutActivityCloudConfig) -> Result<StoredActivityCloudConfig, String> {
    let ingest_url = required_trimmed(body.ingest_url, "ingest_url")?;
    if !ingest_url.starts_with("https://") && !ingest_url.starts_with("http://") {
        return Err("ingest_url must be an absolute URL".to_string());
    }
    let upload_token = required_trimmed(body.upload_token, "upload_token")?;
    let home_id = required_trimmed(body.home_id, "home_id")?;
    let hub_id = required_trimmed(body.hub_id, "hub_id")?;

    Ok(StoredActivityCloudConfig {
        schema_version: ACTIVITY_CLOUD_SCHEMA_VERSION,
        enabled: body.enabled,
        ingest_url,
        upload_token,
        home_id,
        hub_id,
        token_id: clean_optional(body.token_id),
        server_instance_id: clean_optional(body.server_instance_id),
        upload_status: Some(UPLOAD_STATUS_PENDING.to_string()),
        last_upload_attempt_epoch_ms: None,
        last_upload_success_epoch_ms: None,
        last_upload_failure_epoch_ms: None,
        last_upload_http_status: None,
        last_upload_error: None,
        auth_failed_at_epoch_ms: None,
        updated_at_epoch_ms: current_epoch_ms(),
    })
}

fn upload_snapshot(
    state: &SharedState,
) -> Option<(StoredActivityCloudConfig, Vec<LightActivityEvent>)> {
    let config = load_config(state).ok().flatten()?.normalized();
    if !config.is_usable() {
        return None;
    }

    let activities = state
        .lock()
        .ok()
        .map(|state| {
            state
                .light_activity
                .iter()
                .take(LIGHT_ACTIVITY_HISTORY_LIMIT)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some((config, activities))
}

#[derive(Debug, Serialize)]
struct ActivityCloudUploadBody<'a> {
    home_id: &'a str,
    hub_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_instance_id: Option<&'a str>,
    events: &'a [LightActivityEvent],
}

async fn upload_activity_batch(
    config: &StoredActivityCloudConfig,
    activities: &[LightActivityEvent],
) -> Result<(), UploadFailure> {
    let body = ActivityCloudUploadBody {
        home_id: &config.home_id,
        hub_id: &config.hub_id,
        server_instance_id: config.server_instance_id.as_deref(),
        events: activities,
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(UploadFailure::from_error)?;
    let response = client
        .post(&config.ingest_url)
        .bearer_auth(&config.upload_token)
        .json(&body)
        .send()
        .await
        .map_err(UploadFailure::from_error)?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(UploadFailure {
            http_status: Some(status.as_u16()),
            message: format!("ingest returned HTTP {}: {}", status, body),
        });
    }
    Ok(())
}

#[derive(Debug)]
struct UploadFailure {
    http_status: Option<u16>,
    message: String,
}

impl UploadFailure {
    fn from_error(error: impl std::error::Error) -> Self {
        Self {
            http_status: None,
            message: error.to_string(),
        }
    }

    fn is_auth_failure(&self) -> bool {
        matches!(self.http_status, Some(401 | 403))
    }
}

impl std::fmt::Display for UploadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UploadFailure {}

fn record_upload_success(
    state: &SharedState,
    attempted_config: &StoredActivityCloudConfig,
    attempt_epoch_ms: u64,
) -> anyhow::Result<()> {
    update_stored_config_after_upload(state, attempted_config, |config| {
        let now = current_epoch_ms();
        config.upload_status = Some(UPLOAD_STATUS_OK.to_string());
        config.last_upload_attempt_epoch_ms = Some(attempt_epoch_ms);
        config.last_upload_success_epoch_ms = Some(now);
        config.last_upload_http_status = Some(200);
        config.last_upload_error = None;
        config.auth_failed_at_epoch_ms = None;
        config.updated_at_epoch_ms = now;
    })
}

fn record_upload_failure(
    state: &SharedState,
    attempted_config: &StoredActivityCloudConfig,
    attempt_epoch_ms: u64,
    failure: &UploadFailure,
) -> anyhow::Result<()> {
    update_stored_config_after_upload(state, attempted_config, |config| {
        let now = current_epoch_ms();
        let auth_failed = failure.is_auth_failure();
        config.upload_status = Some(
            if auth_failed {
                UPLOAD_STATUS_AUTH_FAILED
            } else {
                UPLOAD_STATUS_FAILED
            }
            .to_string(),
        );
        config.last_upload_attempt_epoch_ms = Some(attempt_epoch_ms);
        config.last_upload_failure_epoch_ms = Some(now);
        config.last_upload_http_status = failure.http_status;
        config.last_upload_error = Some(failure.message.chars().take(256).collect());
        if auth_failed {
            config.auth_failed_at_epoch_ms = Some(now);
        }
        config.updated_at_epoch_ms = now;
    })
}

/// Serializes read-modify-write cycles on the stored config now that the
/// global state lock is no longer held across the storage I/O — the save is
/// fsync'd, and on SD-card storage a slow flush under the state lock stalls
/// light dispatch.
static CONFIG_STORAGE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Grab the storage handle and release the state lock before any I/O.
fn storage_handle(
    state: &SharedState,
) -> anyhow::Result<std::sync::Arc<dyn crate::storage::Storage>> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state
        .storage
        .clone()
        .ok_or_else(|| anyhow::anyhow!("storage not configured"))
}

fn update_stored_config_after_upload(
    state: &SharedState,
    attempted_config: &StoredActivityCloudConfig,
    update: impl FnOnce(&mut StoredActivityCloudConfig),
) -> anyhow::Result<()> {
    let storage = storage_handle(state)?;
    let _guard = CONFIG_STORAGE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(mut current) = storage
        .load_activity_cloud_config()?
        .map(StoredActivityCloudConfig::normalized)
    else {
        return Ok(());
    };
    if !same_upload_config(&current, attempted_config) {
        return Ok(());
    }
    update(&mut current);
    storage.save_activity_cloud_config(&current)
}

fn same_upload_config(left: &StoredActivityCloudConfig, right: &StoredActivityCloudConfig) -> bool {
    left.ingest_url == right.ingest_url
        && left.upload_token == right.upload_token
        && left.home_id == right.home_id
        && left.hub_id == right.hub_id
        && left.token_id == right.token_id
        && left.server_instance_id == right.server_instance_id
}

fn status_body(config: Option<&StoredActivityCloudConfig>) -> ActivityCloudStatusBody {
    let Some(config) = config else {
        return ActivityCloudStatusBody {
            status: "ok",
            configured: false,
            enabled: false,
            home_id: None,
            hub_id: None,
            token_id: None,
            server_instance_id: None,
            updated_at_epoch_ms: 0,
            upload_status: UPLOAD_STATUS_NOT_CONFIGURED,
            needs_reprovision: true,
            last_upload_attempt_epoch_ms: None,
            last_upload_success_epoch_ms: None,
            last_upload_failure_epoch_ms: None,
            last_upload_http_status: None,
            last_upload_error: None,
            auth_failed_at_epoch_ms: None,
        };
    };

    ActivityCloudStatusBody {
        status: "ok",
        configured: config.is_usable(),
        enabled: config.enabled,
        home_id: Some(config.home_id.clone()),
        hub_id: Some(config.hub_id.clone()),
        token_id: config.token_id.clone(),
        server_instance_id: config.server_instance_id.clone(),
        updated_at_epoch_ms: config.updated_at_epoch_ms,
        upload_status: upload_status(config),
        needs_reprovision: config.needs_reprovision(),
        last_upload_attempt_epoch_ms: config.last_upload_attempt_epoch_ms,
        last_upload_success_epoch_ms: config.last_upload_success_epoch_ms,
        last_upload_failure_epoch_ms: config.last_upload_failure_epoch_ms,
        last_upload_http_status: config.last_upload_http_status,
        last_upload_error: config.last_upload_error.clone(),
        auth_failed_at_epoch_ms: config.auth_failed_at_epoch_ms,
    }
}

fn upload_status(config: &StoredActivityCloudConfig) -> &'static str {
    match config.upload_status.as_deref() {
        Some(UPLOAD_STATUS_OK) => UPLOAD_STATUS_OK,
        Some(UPLOAD_STATUS_FAILED) => UPLOAD_STATUS_FAILED,
        Some(UPLOAD_STATUS_AUTH_FAILED) => UPLOAD_STATUS_AUTH_FAILED,
        Some(UPLOAD_STATUS_PENDING) => UPLOAD_STATUS_PENDING,
        _ => UPLOAD_STATUS_PENDING,
    }
}

fn load_config(state: &SharedState) -> anyhow::Result<Option<StoredActivityCloudConfig>> {
    let storage = storage_handle(state)?;
    Ok(storage
        .load_activity_cloud_config()?
        .map(StoredActivityCloudConfig::normalized))
}

fn save_config(state: &SharedState, config: &StoredActivityCloudConfig) -> anyhow::Result<()> {
    let storage = storage_handle(state)?;
    let _guard = CONFIG_STORAGE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    storage.save_activity_cloud_config(config)
}

fn clear_config(state: &SharedState) -> anyhow::Result<()> {
    let storage = storage_handle(state)?;
    let _guard = CONFIG_STORAGE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    storage.clear_activity_cloud_config()
}

fn json_ok(body: impl Serialize) -> ApiResponse {
    match serde_json::to_string(&body) {
        Ok(body) => ApiResponse::json_ok(body),
        Err(e) => ApiResponse::server_error(e),
    }
}

fn required_trimmed(value: Option<String>, key: &str) -> Result<String, String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Missing {key}"))
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage as _;

    #[test]
    fn status_body_redacts_upload_token() {
        let config = StoredActivityCloudConfig {
            schema_version: ACTIVITY_CLOUD_SCHEMA_VERSION,
            enabled: true,
            ingest_url: "https://example.test/functions/v1/server-activity-ingest".into(),
            upload_token: "secret-token".into(),
            home_id: "home-1".into(),
            hub_id: "hub-1".into(),
            token_id: Some("token-1".into()),
            server_instance_id: Some("srv-1".into()),
            upload_status: Some(UPLOAD_STATUS_OK.into()),
            last_upload_attempt_epoch_ms: Some(100),
            last_upload_success_epoch_ms: Some(101),
            last_upload_failure_epoch_ms: None,
            last_upload_http_status: Some(200),
            last_upload_error: None,
            auth_failed_at_epoch_ms: None,
            updated_at_epoch_ms: 123,
        };

        let json = serde_json::to_value(status_body(Some(&config))).unwrap();
        assert_eq!(json["configured"], true);
        assert_eq!(json["hub_id"], "hub-1");
        assert_eq!(json.get("upload_token"), None);
        assert_eq!(json.get("ingest_url"), None);
        assert_eq!(json["needs_reprovision"], false);
        assert_eq!(json["upload_status"], UPLOAD_STATUS_OK);
    }

    #[test]
    fn config_from_body_requires_absolute_url_and_ids() {
        let err = config_from_body(PutActivityCloudConfig {
            enabled: true,
            ingest_url: Some("/functions/v1/server-activity-ingest".into()),
            upload_token: Some("token".into()),
            home_id: Some("home".into()),
            hub_id: Some("hub".into()),
            token_id: None,
            server_instance_id: None,
        })
        .unwrap_err();
        assert_eq!(err, "ingest_url must be an absolute URL");
    }

    #[test]
    fn status_body_reports_auth_failure_as_reprovision_needed() {
        let mut config = StoredActivityCloudConfig {
            schema_version: ACTIVITY_CLOUD_SCHEMA_VERSION,
            enabled: true,
            ingest_url: "https://example.test/functions/v1/server-activity-ingest".into(),
            upload_token: "secret-token".into(),
            home_id: "home-1".into(),
            hub_id: "hub-1".into(),
            token_id: Some("token-1".into()),
            server_instance_id: Some("srv-1".into()),
            upload_status: Some(UPLOAD_STATUS_PENDING.into()),
            last_upload_attempt_epoch_ms: None,
            last_upload_success_epoch_ms: None,
            last_upload_failure_epoch_ms: None,
            last_upload_http_status: None,
            last_upload_error: None,
            auth_failed_at_epoch_ms: None,
            updated_at_epoch_ms: 123,
        };
        let failure = UploadFailure {
            http_status: Some(403),
            message: "forbidden".into(),
        };

        let now = current_epoch_ms();
        let auth_failed = failure.is_auth_failure();
        config.upload_status = Some(
            if auth_failed {
                UPLOAD_STATUS_AUTH_FAILED
            } else {
                UPLOAD_STATUS_FAILED
            }
            .to_string(),
        );
        config.last_upload_attempt_epoch_ms = Some(now);
        config.last_upload_failure_epoch_ms = Some(now);
        config.last_upload_http_status = failure.http_status;
        config.last_upload_error = Some(failure.message);
        config.auth_failed_at_epoch_ms = Some(now);

        let json = serde_json::to_value(status_body(Some(&config))).unwrap();
        assert_eq!(json["configured"], false);
        assert_eq!(json["needs_reprovision"], true);
        assert_eq!(json["upload_status"], UPLOAD_STATUS_AUTH_FAILED);
        assert_eq!(json["last_upload_http_status"], 403);
    }

    #[test]
    fn join_proof_hmac_uses_sha256_token_hash_key() {
        let token_hash = sha256_hex(b"activity-upload-token");
        let canonical =
            join_proof_canonical_string("srv-1", "home-1", "hub-1", 1_000, 121_000, "nonce-1");

        assert_eq!(
            hmac_sha256_hex(token_hash.as_bytes(), canonical.as_bytes()),
            "41b380e3ff97e37dd008c706e58ff422adbab4bcc0ad822dc98480319a585dc9",
        );
    }

    #[test]
    fn hmac_sha256_matches_known_vector() {
        assert_eq!(
            hmac_sha256_hex(b"key", b"The quick brown fox jumps over the lazy dog"),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8",
        );
    }

    fn test_state() -> SharedState {
        std::sync::Arc::new(std::sync::Mutex::new(crate::state::AppState::default()))
    }

    fn usable_config(ingest_url: &str) -> StoredActivityCloudConfig {
        StoredActivityCloudConfig {
            schema_version: ACTIVITY_CLOUD_SCHEMA_VERSION,
            enabled: true,
            ingest_url: ingest_url.into(),
            upload_token: "token-1".into(),
            home_id: "home-1".into(),
            hub_id: "hub-1".into(),
            token_id: None,
            server_instance_id: None,
            upload_status: Some(UPLOAD_STATUS_PENDING.into()),
            last_upload_attempt_epoch_ms: None,
            last_upload_success_epoch_ms: None,
            last_upload_failure_epoch_ms: None,
            last_upload_http_status: None,
            last_upload_error: None,
            auth_failed_at_epoch_ms: None,
            updated_at_epoch_ms: 1,
        }
    }

    fn test_activity() -> LightActivityEvent {
        LightActivityEvent {
            id: "activity-1-1783281103022".to_string(),
            node_id: "room-1".to_string(),
            action_id: "turn_on".to_string(),
            source: crate::activity::LightActivitySource {
                raw: "api".to_string(),
                kind: "app".to_string(),
                marks_touched: true,
                control_id: None,
            },
            epoch_ms: 1_783_281_103_022,
            server_instance_id: None,
            server_version: None,
            platform: None,
            active_mode: None,
            target: None,
            change: None,
            count: 1,
            correlation_id: None,
            fanout_of: None,
            payload: None,
            brightness: None,
            kelvin: None,
        }
    }

    #[test]
    fn upload_runtime_handle_prefers_ambient_runtime() {
        let state = test_state();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        assert!(upload_runtime_handle(&state).is_some());
    }

    #[test]
    fn upload_runtime_handle_falls_back_to_stored_handle() {
        let state = test_state();
        assert!(
            upload_runtime_handle(&state).is_none(),
            "no ambient runtime and no stored handle should resolve to None"
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        state.lock().unwrap().tokio_handle = Some(runtime.handle().clone());
        assert!(upload_runtime_handle(&state).is_some());
    }

    /// Regression: activity is recorded on plain worker threads (http-handler,
    /// event-loop dispatch) with no ambient Tokio runtime. The upload must
    /// still be attempted via the handle captured at startup.
    #[test]
    fn enqueue_attempts_upload_from_plain_thread_via_stored_handle() {
        // Reserve a local port with no listener so the upload fails fast
        // (connection refused) without DNS lookups.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };

        let data_dir = std::env::temp_dir().join(format!(
            "rhythm_activity_cloud_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = crate::storage::FileStorage::new(data_dir.to_str().unwrap()).unwrap();
        storage
            .save_activity_cloud_config(&usable_config(&format!("http://127.0.0.1:{port}/ingest")))
            .unwrap();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();

        let state = test_state();
        {
            let mut s = state.lock().unwrap();
            s.storage = Some(std::sync::Arc::new(
                crate::storage::FileStorage::new(data_dir.to_str().unwrap()).unwrap(),
            ));
            s.light_activity.push(test_activity());
            s.tokio_handle = Some(runtime.handle().clone());
        }

        // Test threads have no ambient runtime, matching the production
        // worker threads that record activity.
        assert!(tokio::runtime::Handle::try_current().is_err());
        enqueue_recent_activity_upload(&state);

        let mut recorded = None;
        for _ in 0..200 {
            let config = storage.load_activity_cloud_config().unwrap().unwrap();
            if config.last_upload_attempt_epoch_ms.is_some() {
                recorded = Some(config);
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        std::fs::remove_dir_all(&data_dir).ok();

        let config = recorded.expect("upload attempt was never recorded");
        assert_eq!(config.upload_status.as_deref(), Some(UPLOAD_STATUS_FAILED));
        assert!(config.last_upload_error.is_some());
        assert_eq!(
            state.lock().unwrap().light_activity.len(),
            1,
            "failed uploads must keep events buffered for retry"
        );
    }

    /// Accept one HTTP request, read it fully, and answer 200.
    fn spawn_one_shot_ok_server() -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};

        fn request_complete(buf: &[u8]) -> bool {
            let Some(headers_end) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
                return false;
            };
            let headers = String::from_utf8_lossy(&buf[..headers_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            buf.len() >= headers_end + 4 + content_length
        }

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            socket
                .set_read_timeout(Some(Duration::from_millis(500)))
                .ok();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            while !request_complete(&buf) {
                match socket.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => buf.extend_from_slice(&chunk[..read]),
                }
            }
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}");
            let _ = socket.flush();
        });
        (addr, handle)
    }

    #[test]
    fn successful_upload_drains_uploaded_events() {
        let (addr, server) = spawn_one_shot_ok_server();

        let data_dir = std::env::temp_dir().join(format!(
            "rhythm_activity_cloud_drain_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = crate::storage::FileStorage::new(data_dir.to_str().unwrap()).unwrap();
        storage
            .save_activity_cloud_config(&usable_config(&format!("http://{addr}/ingest")))
            .unwrap();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();

        let state = test_state();
        {
            let mut s = state.lock().unwrap();
            s.storage = Some(std::sync::Arc::new(
                crate::storage::FileStorage::new(data_dir.to_str().unwrap()).unwrap(),
            ));
            s.light_activity.push(test_activity());
            s.tokio_handle = Some(runtime.handle().clone());
        }

        enqueue_recent_activity_upload(&state);

        let mut drained_and_recorded = false;
        for _ in 0..200 {
            let drained = state.lock().unwrap().light_activity.is_empty();
            let recorded_ok = storage
                .load_activity_cloud_config()
                .unwrap()
                .unwrap()
                .upload_status
                .as_deref()
                == Some(UPLOAD_STATUS_OK);
            if drained && recorded_ok {
                drained_and_recorded = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        server.join().ok();
        std::fs::remove_dir_all(&data_dir).ok();

        assert!(
            drained_and_recorded,
            "successful upload should drain the buffer and record ok status"
        );
    }
}
