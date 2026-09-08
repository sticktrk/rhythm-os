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
use crate::light_usage::LightUsageCloudBatch;
use crate::pairing::{PairingHistoryEntry, PAIRING_HISTORY_LIMIT};
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
    light_usage: crate::light_usage::LightUsageDiagnostics,
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
        Ok(config) => json_ok(status_body_for_state(&state, config.as_ref())),
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
    if config.enabled {
        crate::light_usage::request_cloud_upload(&state);
    } else if let Ok(mut state) = state.lock() {
        state
            .light_usage
            .mark_cloud_unavailable("disabled", std::time::Instant::now());
    }
    json_ok(status_body_for_state(&state, Some(&config)))
}

pub async fn delete_config(State(state): State<SharedState>) -> ApiResponse {
    if let Err(e) = clear_config(&state) {
        return ApiResponse::server_error(e);
    }
    if let Ok(mut state) = state.lock() {
        state
            .light_usage
            .mark_cloud_unavailable("not_configured", std::time::Instant::now());
    }
    json_ok(status_body_for_state(&state, None))
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
    let Some(snapshot) = upload_snapshot(state) else {
        return;
    };
    if snapshot.activities.is_empty()
        && snapshot.device_events.is_empty()
        && snapshot.usage.is_none()
    {
        return;
    }

    let Some(handle) = upload_runtime_handle(state) else {
        log::warn!(
            target: "cmd",
            "Skipping server activity cloud upload: no Tokio runtime handle available"
        );
        if snapshot.usage.is_some() {
            if let Ok(mut state) = state.lock() {
                state
                    .light_usage
                    .fail_cloud_upload("failed", std::time::Instant::now());
            }
        }
        return;
    };

    let state_for_result = state.clone();
    handle.spawn(async move {
        let attempt_epoch_ms = current_epoch_ms();
        match upload_activity_batch(
            &snapshot.config,
            &snapshot.activities,
            &snapshot.device_events,
            snapshot.usage.as_ref(),
        )
        .await
        {
            Ok(result) => {
                crate::activity::remove_uploaded_light_activity(
                    &state_for_result,
                    &snapshot.activities,
                );
                if let Some(usage) = snapshot.usage.as_ref() {
                    if let Ok(mut state) = state_for_result.lock() {
                        state.light_usage.complete_cloud_upload(
                            usage,
                            result.usage_acknowledged(usage),
                            std::time::Instant::now(),
                            current_epoch_ms(),
                        );
                    }
                }
                if let Err(error) =
                    record_upload_success(&state_for_result, &snapshot.config, attempt_epoch_ms)
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
                if snapshot.usage.is_some() {
                    if let Ok(mut state) = state_for_result.lock() {
                        state.light_usage.fail_cloud_upload(
                            if error.is_auth_failure() {
                                UPLOAD_STATUS_AUTH_FAILED
                            } else {
                                UPLOAD_STATUS_FAILED
                            },
                            std::time::Instant::now(),
                        );
                    }
                }
                if let Err(record_error) = record_upload_failure(
                    &state_for_result,
                    &snapshot.config,
                    attempt_epoch_ms,
                    &error,
                ) {
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

#[derive(Debug)]
struct ActivityCloudUploadSnapshot {
    config: StoredActivityCloudConfig,
    activities: Vec<LightActivityEvent>,
    device_events: Vec<DeviceLifecycleCloudEvent>,
    usage: Option<LightUsageCloudBatch>,
}

fn upload_snapshot(state: &SharedState) -> Option<ActivityCloudUploadSnapshot> {
    let config = load_config(state).ok().flatten()?.normalized();
    if !config.is_usable() {
        return None;
    }

    let (activities, storage, usage) = state
        .lock()
        .ok()
        .map(|mut state| {
            let activities = state
                .light_activity
                .iter()
                .take(LIGHT_ACTIVITY_HISTORY_LIMIT)
                .cloned()
                .collect::<Vec<_>>();
            let usage = state
                .light_usage
                .begin_cloud_upload(std::time::Instant::now(), current_epoch_ms());
            (activities, state.storage.clone(), usage)
        })
        .unwrap_or_default();

    let device_events = storage
        .and_then(|storage| storage.load_pairing_history().ok().flatten())
        .map(|history| {
            history
                .entries
                .iter()
                .rev()
                .take(PAIRING_HISTORY_LIMIT)
                .filter_map(device_lifecycle_cloud_event)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(ActivityCloudUploadSnapshot {
        config,
        activities,
        device_events,
        usage,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DeviceLifecycleCloudEvent {
    id: String,
    epoch_ms: u64,
    action: String,
    hub_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_type: Option<String>,
    outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commissioner: Option<String>,
    force: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation_id: Option<String>,
}

fn privacy_safe_token(value: &str, max_len: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')))
    .then(|| value.to_string())
}

fn device_lifecycle_cloud_event(entry: &PairingHistoryEntry) -> Option<DeviceLifecycleCloudEvent> {
    let action = match entry.kind.as_str() {
        "pair" | "unpair" => entry.kind.clone(),
        _ => return None,
    };
    let hub_type = privacy_safe_token(&entry.hub_type, 64)?;
    let outcome = privacy_safe_token(&entry.status, 32)?;
    let device_type = entry
        .device_type
        .as_deref()
        .filter(|value| matches!(*value, "light" | "button" | "motion" | "contact"))
        .map(str::to_string);
    let correlation_id = entry
        .correlation_id
        .as_deref()
        .and_then(|value| privacy_safe_token(value, 96));
    let failure_stage = entry.failure_stage.map(|stage| stage.as_str().to_string());
    let commissioner = (action == "pair"
        && (hub_type == "matter"
            || matches!(entry.rendezvous.as_deref(), Some("phone" | "server"))))
    .then(|| {
        if entry.rendezvous.as_deref() == Some("phone") {
            "phone".to_string()
        } else {
            "server".to_string()
        }
    });
    let serialized = serde_json::to_vec(entry).ok()?;
    let event_hash = Sha256::digest(serialized);

    Some(DeviceLifecycleCloudEvent {
        id: format!("device-lifecycle-{}", hex_lower(&event_hash)),
        epoch_ms: entry.epoch_ms,
        action,
        hub_type,
        device_type,
        outcome,
        failure_stage,
        commissioner,
        force: entry.force.unwrap_or(false),
        correlation_id,
    })
}

#[derive(Debug, Serialize)]
struct ActivityCloudUploadBody<'a> {
    home_id: &'a str,
    hub_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_instance_id: Option<&'a str>,
    events: &'a [LightActivityEvent],
    device_events: &'a [DeviceLifecycleCloudEvent],
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_schema_version: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_batch_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_segments: Option<&'a [crate::light_usage::LightUsageUploadSegment]>,
}

#[derive(Debug, Default, Deserialize)]
struct ActivityCloudUploadResponse {
    usage_schema_version: Option<u8>,
    usage_batch_id: Option<String>,
}

#[derive(Debug)]
struct ActivityCloudUploadResult {
    response: ActivityCloudUploadResponse,
}

impl ActivityCloudUploadResult {
    fn usage_acknowledged(&self, batch: &LightUsageCloudBatch) -> bool {
        self.response.usage_schema_version == Some(batch.schema_version)
            && self.response.usage_batch_id.as_deref() == Some(batch.batch_id.as_str())
    }
}

async fn upload_activity_batch(
    config: &StoredActivityCloudConfig,
    activities: &[LightActivityEvent],
    device_events: &[DeviceLifecycleCloudEvent],
    usage: Option<&LightUsageCloudBatch>,
) -> Result<ActivityCloudUploadResult, UploadFailure> {
    let body = ActivityCloudUploadBody {
        home_id: &config.home_id,
        hub_id: &config.hub_id,
        server_instance_id: config.server_instance_id.as_deref(),
        events: activities,
        device_events,
        usage_schema_version: usage.map(|usage| usage.schema_version),
        usage_batch_id: usage.map(|usage| usage.batch_id.as_str()),
        usage_segments: usage.map(|usage| usage.segments.as_slice()),
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
    let response = response
        .json::<ActivityCloudUploadResponse>()
        .await
        .unwrap_or_default();
    Ok(ActivityCloudUploadResult { response })
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

#[cfg(test)]
fn status_body(config: Option<&StoredActivityCloudConfig>) -> ActivityCloudStatusBody {
    status_body_with_usage(
        config,
        crate::light_usage::LightUsageLedger::default().diagnostics(current_epoch_ms()),
    )
}

fn status_body_for_state(
    state: &SharedState,
    config: Option<&StoredActivityCloudConfig>,
) -> ActivityCloudStatusBody {
    let mut light_usage = state
        .lock()
        .ok()
        .map(|state| state.light_usage.diagnostics(current_epoch_ms()))
        .unwrap_or_else(|| {
            crate::light_usage::LightUsageLedger::default().diagnostics(current_epoch_ms())
        });
    light_usage.cloud_status = match config {
        None => "not_configured".to_string(),
        Some(config) if !config.enabled => "disabled".to_string(),
        _ => light_usage.cloud_status,
    };
    status_body_with_usage(config, light_usage)
}

fn status_body_with_usage(
    config: Option<&StoredActivityCloudConfig>,
    light_usage: crate::light_usage::LightUsageDiagnostics,
) -> ActivityCloudStatusBody {
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
            light_usage,
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
        light_usage,
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

    #[test]
    fn light_usage_status_distinguishes_disabled_and_unconfigured_cloud() {
        let state = test_state();
        let unconfigured = serde_json::to_value(status_body_for_state(&state, None)).unwrap();
        assert_eq!(
            unconfigured["light_usage"]["cloud_status"],
            "not_configured"
        );

        let mut disabled =
            usable_config("https://example.test/functions/v1/server-activity-ingest");
        disabled.enabled = false;
        let disabled =
            serde_json::to_value(status_body_for_state(&state, Some(&disabled))).unwrap();
        assert_eq!(disabled["light_usage"]["cloud_status"], "disabled");
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

    fn test_device_lifecycle_entry() -> PairingHistoryEntry {
        PairingHistoryEntry {
            at: "2026-08-09T12:00:00.000Z".into(),
            epoch_ms: 1_786_277_600_000,
            kind: "unpair".into(),
            hub_type: "hue".into(),
            correlation_id: Some("hue-bridge-remove-journey".into()),
            device_type: Some("button".into()),
            profile_id: None,
            device_id: Some("private-device-id".into()),
            force: Some(true),
            rendezvous: None,
            network: None,
            status: "complete".into(),
            error: Some("private raw bridge error".into()),
            failure_stage: None,
            device: Some("Private Switch Name (private-device-id)".into()),
            devices: vec!["Private Switch Name (private-device-id)".into()],
            warnings: vec!["private warning".into()],
        }
    }

    #[test]
    fn device_lifecycle_cloud_event_is_stable_and_privacy_bounded() {
        let entry = test_device_lifecycle_entry();
        let first = device_lifecycle_cloud_event(&entry).unwrap();
        let second = device_lifecycle_cloud_event(&entry).unwrap();
        assert_eq!(first, second);
        assert!(first.id.starts_with("device-lifecycle-"));
        assert_eq!(first.action, "unpair");
        assert_eq!(first.device_type.as_deref(), Some("button"));
        assert!(first.force);
        assert_eq!(first.commissioner, None);

        let serialized = serde_json::to_string(&first).unwrap();
        for private_value in [
            "private-device-id",
            "Private Switch Name",
            "private raw bridge error",
            "private warning",
        ] {
            assert!(!serialized.contains(private_value));
        }
    }

    #[test]
    fn matter_device_lifecycle_records_only_the_bounded_commissioner() {
        let mut entry = test_device_lifecycle_entry();
        entry.kind = "pair".into();
        entry.hub_type = "matter".into();
        entry.rendezvous = Some("phone".into());
        let phone = device_lifecycle_cloud_event(&entry).unwrap();
        assert_eq!(phone.commissioner.as_deref(), Some("phone"));

        entry.rendezvous = Some("ble".into());
        let server = device_lifecycle_cloud_event(&entry).unwrap();
        assert_eq!(server.commissioner.as_deref(), Some("server"));

        entry.hub_type = "future_wifi".into();
        entry.rendezvous = Some("phone".into());
        assert_eq!(
            device_lifecycle_cloud_event(&entry)
                .unwrap()
                .commissioner
                .as_deref(),
            Some("phone")
        );
        entry.rendezvous = Some("server".into());
        assert_eq!(
            device_lifecycle_cloud_event(&entry)
                .unwrap()
                .commissioner
                .as_deref(),
            Some("server")
        );
        entry.rendezvous = Some("private-unknown-value".into());
        assert!(device_lifecycle_cloud_event(&entry)
            .unwrap()
            .commissioner
            .is_none());

        let serialized = serde_json::to_string(&phone).unwrap();
        assert!(!serialized.contains("rendezvous"));
        assert!(!serialized.contains("handoff"));
    }

    #[test]
    fn usage_acknowledgement_requires_exact_schema_and_batch() {
        let batch = LightUsageCloudBatch {
            schema_version: crate::light_usage::LIGHT_USAGE_SCHEMA_VERSION,
            batch_id: "00112233445566778899aabbccddeeff".into(),
            backlog_capped: false,
            segments: Vec::new(),
        };
        let exact = ActivityCloudUploadResult {
            response: ActivityCloudUploadResponse {
                usage_schema_version: Some(crate::light_usage::LIGHT_USAGE_SCHEMA_VERSION),
                usage_batch_id: Some(batch.batch_id.clone()),
            },
        };
        assert!(exact.usage_acknowledged(&batch));

        let old_edge_function = ActivityCloudUploadResult {
            response: ActivityCloudUploadResponse::default(),
        };
        assert!(!old_edge_function.usage_acknowledged(&batch));
        let wrong_batch = ActivityCloudUploadResult {
            response: ActivityCloudUploadResponse {
                usage_schema_version: Some(crate::light_usage::LIGHT_USAGE_SCHEMA_VERSION),
                usage_batch_id: Some("ffeeddccbbaa99887766554433221100".into()),
            },
        };
        assert!(!wrong_batch.usage_acknowledged(&batch));
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

    fn temp_storage_state(name: &str) -> (SharedState, std::path::PathBuf) {
        let data_dir = std::env::temp_dir().join(format!(
            "rhythm_activity_cloud_{name}_{}_{}",
            std::process::id(),
            current_epoch_ms()
        ));
        let storage = crate::storage::FileStorage::new(data_dir.to_str().unwrap()).unwrap();
        let state = test_state();
        {
            let mut app = state.lock().unwrap();
            app.data_dir = data_dir.to_string_lossy().to_string();
            app.storage = Some(std::sync::Arc::new(storage));
        }
        (state, data_dir)
    }

    fn owner_auth() -> Option<Extension<ApiAuthRequestInfo>> {
        Some(Extension(ApiAuthRequestInfo {
            requires_auth: true,
            claim_available: false,
            via_remote_access: false,
            role: Some(ApiTokenRole::Owner),
            token_expires_at_epoch_ms: None,
        }))
    }

    #[test]
    fn stored_config_normalization_trims_and_bounds_persisted_fields() {
        let mut config = usable_config(" https://example.test/ingest ");
        config.schema_version = 0;
        config.upload_token = " token ".into();
        config.home_id = " home ".into();
        config.hub_id = " hub ".into();
        config.token_id = Some("  ".into());
        config.server_instance_id = Some(" server ".into());
        config.upload_status = Some(" ok ".into());
        config.last_upload_error = Some(format!("  {}  ", "x".repeat(300)));

        let normalized = config.normalized();
        assert_eq!(normalized.schema_version, ACTIVITY_CLOUD_SCHEMA_VERSION);
        assert_eq!(normalized.ingest_url, "https://example.test/ingest");
        assert_eq!(normalized.upload_token, "token");
        assert_eq!(normalized.home_id, "home");
        assert_eq!(normalized.hub_id, "hub");
        assert_eq!(normalized.token_id, None);
        assert_eq!(normalized.server_instance_id.as_deref(), Some("server"));
        assert_eq!(normalized.upload_status.as_deref(), Some(UPLOAD_STATUS_OK));
        assert_eq!(normalized.last_upload_error.unwrap().len(), 256);
    }

    #[test]
    fn stored_config_serde_defaults_and_usability_matrix() {
        let config: StoredActivityCloudConfig = serde_json::from_value(serde_json::json!({
            "ingest_url": "https://example.test/ingest",
            "upload_token": "token",
            "home_id": "home",
            "hub_id": "hub",
            "updated_at_epoch_ms": 1
        }))
        .unwrap();
        assert_eq!(config.schema_version, ACTIVITY_CLOUD_SCHEMA_VERSION);
        assert!(config.enabled);
        assert!(config.is_usable());

        let mutators: [fn(&mut StoredActivityCloudConfig); 6] = [
            |value: &mut StoredActivityCloudConfig| value.enabled = false,
            |value: &mut StoredActivityCloudConfig| value.ingest_url.clear(),
            |value: &mut StoredActivityCloudConfig| value.upload_token.clear(),
            |value: &mut StoredActivityCloudConfig| value.home_id.clear(),
            |value: &mut StoredActivityCloudConfig| value.hub_id.clear(),
            |value: &mut StoredActivityCloudConfig| value.auth_failed_at_epoch_ms = Some(1),
        ];
        for mutate in mutators {
            let mut candidate = config.clone();
            mutate(&mut candidate);
            assert!(!candidate.is_usable());
        }
    }

    #[test]
    fn config_from_body_accepts_http_and_trims_optional_metadata() {
        let config = config_from_body(PutActivityCloudConfig {
            enabled: false,
            ingest_url: Some(" http://127.0.0.1:54321/ingest ".into()),
            upload_token: Some(" token ".into()),
            home_id: Some(" home ".into()),
            hub_id: Some(" hub ".into()),
            token_id: Some("  ".into()),
            server_instance_id: Some(" server ".into()),
        })
        .unwrap();

        assert!(!config.enabled);
        assert_eq!(config.ingest_url, "http://127.0.0.1:54321/ingest");
        assert_eq!(config.upload_token, "token");
        assert_eq!(config.home_id, "home");
        assert_eq!(config.hub_id, "hub");
        assert_eq!(config.token_id, None);
        assert_eq!(config.server_instance_id.as_deref(), Some("server"));
        assert_eq!(config.upload_status.as_deref(), Some(UPLOAD_STATUS_PENDING));
    }

    #[test]
    fn config_from_body_reports_each_missing_required_value() {
        let base = || PutActivityCloudConfig {
            enabled: true,
            ingest_url: Some("https://example.test/ingest".into()),
            upload_token: Some("token".into()),
            home_id: Some("home".into()),
            hub_id: Some("hub".into()),
            token_id: None,
            server_instance_id: None,
        };

        let mut missing_url = base();
        missing_url.ingest_url = Some(" ".into());
        assert_eq!(
            config_from_body(missing_url).unwrap_err(),
            "Missing ingest_url"
        );
        let mut missing_token = base();
        missing_token.upload_token = None;
        assert_eq!(
            config_from_body(missing_token).unwrap_err(),
            "Missing upload_token"
        );
        let mut missing_home = base();
        missing_home.home_id = None;
        assert_eq!(
            config_from_body(missing_home).unwrap_err(),
            "Missing home_id"
        );
        let mut missing_hub = base();
        missing_hub.hub_id = Some(" ".into());
        assert_eq!(config_from_body(missing_hub).unwrap_err(), "Missing hub_id");
    }

    #[test]
    fn status_body_covers_unconfigured_disabled_and_unknown_status() {
        let unconfigured = serde_json::to_value(status_body(None)).unwrap();
        assert_eq!(unconfigured["configured"], false);
        assert_eq!(unconfigured["enabled"], false);
        assert_eq!(unconfigured["upload_status"], UPLOAD_STATUS_NOT_CONFIGURED);
        assert_eq!(unconfigured["needs_reprovision"], true);

        let mut disabled = usable_config("https://example.test/ingest");
        disabled.enabled = false;
        disabled.upload_status = Some("unexpected".into());
        let body = serde_json::to_value(status_body(Some(&disabled))).unwrap();
        assert_eq!(body["configured"], false);
        assert_eq!(body["enabled"], false);
        assert_eq!(body["upload_status"], UPLOAD_STATUS_PENDING);
        assert_eq!(body["needs_reprovision"], false);
    }

    #[test]
    fn upload_snapshot_requires_usable_config_and_caps_history() {
        let (state, data_dir) = temp_storage_state("snapshot");
        assert!(upload_snapshot(&state).is_none());

        let storage = storage_handle(&state).unwrap();
        let mut disabled = usable_config("https://example.test/ingest");
        disabled.enabled = false;
        storage.save_activity_cloud_config(&disabled).unwrap();
        assert!(upload_snapshot(&state).is_none());

        storage
            .save_activity_cloud_config(&usable_config("https://example.test/ingest"))
            .unwrap();
        {
            let mut app = state.lock().unwrap();
            for index in 0..(LIGHT_ACTIVITY_HISTORY_LIMIT + 5) {
                let mut activity = test_activity();
                activity.id = format!("activity-{index}");
                app.light_activity.push(activity);
            }
        }
        storage
            .save_pairing_history(&crate::pairing::PairingHistory {
                schema_version: crate::pairing::PAIRING_HISTORY_SCHEMA_VERSION,
                entries: vec![test_device_lifecycle_entry()],
                pairing_results: Vec::new(),
            })
            .unwrap();
        let snapshot = upload_snapshot(&state).unwrap();
        assert_eq!(snapshot.activities.len(), LIGHT_ACTIVITY_HISTORY_LIMIT);
        assert_eq!(snapshot.activities.first().unwrap().id, "activity-0");
        assert_eq!(snapshot.device_events.len(), 1);
        assert_eq!(
            snapshot.device_events[0].device_type.as_deref(),
            Some("button")
        );
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[tokio::test]
    async fn config_endpoints_round_trip_and_clear_without_exposing_token() {
        let missing_storage = test_state();
        assert_eq!(get_config(State(missing_storage.clone())).await.status, 500);
        assert_eq!(delete_config(State(missing_storage)).await.status, 500);

        let (state, data_dir) = temp_storage_state("endpoints");
        let response = put_config(
            State(state.clone()),
            Json(PutActivityCloudConfig {
                enabled: true,
                ingest_url: Some("https://example.test/ingest".into()),
                upload_token: Some("secret-token".into()),
                home_id: Some("home-1".into()),
                hub_id: Some("hub-1".into()),
                token_id: Some("token-1".into()),
                server_instance_id: Some("server-1".into()),
            }),
        )
        .await;
        assert_eq!(response.status, 200);
        assert!(!response.body.contains("secret-token"));

        let response = get_config(State(state.clone())).await;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(body["configured"], true);
        assert_eq!(body["home_id"], "home-1");
        assert_eq!(body["hub_id"], "hub-1");
        assert!(!response.body.contains("secret-token"));

        let response = delete_config(State(state.clone())).await;
        assert_eq!(response.status, 200);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["configured"],
            false
        );
        assert!(load_config(&state).unwrap().is_none());
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[tokio::test]
    async fn join_proof_requires_owner_auth_and_configured_credentials() {
        let state = test_state();
        assert_eq!(
            post_join_proof(State(state.clone()), None).await.status,
            403
        );
        let support = Some(Extension(ApiAuthRequestInfo {
            requires_auth: true,
            claim_available: false,
            via_remote_access: true,
            role: Some(ApiTokenRole::Support),
            token_expires_at_epoch_ms: Some(current_epoch_ms() + 1_000),
        }));
        assert_eq!(
            post_join_proof(State(state.clone()), support).await.status,
            403
        );
        assert_eq!(
            post_join_proof(State(state), owner_auth()).await.status,
            500
        );

        let (state, data_dir) = temp_storage_state("join-unconfigured");
        let response = post_join_proof(State(state), owner_auth()).await;
        assert_eq!(response.status, 409);
        assert!(response.body.contains("not configured"));
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[tokio::test]
    async fn join_proof_rejects_identity_gaps_and_signs_matching_identity() {
        let (state, data_dir) = temp_storage_state("join-proof");
        let storage = storage_handle(&state).unwrap();
        let mut config = usable_config("https://example.test/ingest");
        config.server_instance_id = Some("server-a".into());
        config.token_id = Some("token-id".into());
        storage.save_activity_cloud_config(&config).unwrap();
        state.lock().unwrap().server_instance_id.clear();

        let response = post_join_proof(State(state.clone()), owner_auth()).await;
        assert_eq!(response.status, 409);
        assert!(response.body.contains("Server identity"));

        state.lock().unwrap().server_instance_id = "server-b".into();
        let response = post_join_proof(State(state.clone()), owner_auth()).await;
        assert_eq!(response.status, 409);
        assert!(response.body.contains("another server identity"));

        state.lock().unwrap().server_instance_id = "SERVER-A".into();
        let response = post_join_proof(State(state), owner_auth()).await;
        assert_eq!(response.status, 200);
        let proof: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(proof["status"], "ok");
        assert_eq!(proof["proof_version"], JOIN_PROOF_VERSION);
        assert_eq!(proof["algorithm"], JOIN_PROOF_ALGORITHM);
        assert_eq!(proof["server_instance_id"], "SERVER-A");
        assert_eq!(proof["home_id"], "home-1");
        assert_eq!(proof["hub_id"], "hub-1");
        assert_eq!(proof["token_id"], "token-id");
        assert_eq!(proof["nonce"].as_str().unwrap().len(), 32);
        assert_eq!(proof["signature"].as_str().unwrap().len(), 64);
        assert_eq!(
            proof["expires_at_epoch_ms"].as_u64().unwrap()
                - proof["issued_at_epoch_ms"].as_u64().unwrap(),
            JOIN_PROOF_TTL_MS
        );
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn crypto_helpers_cover_long_keys_hashing_and_random_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let signature = hmac_sha256_hex(&vec![b'k'; 100], b"message");
        assert_eq!(signature.len(), 64);
        assert!(signature.chars().all(|value| value.is_ascii_hexdigit()));

        let nonce = random_hex(24);
        assert_eq!(nonce.len(), 48);
        assert!(nonce.chars().all(|value| value.is_ascii_hexdigit()));
    }

    #[test]
    fn upload_result_updates_are_bounded_and_ignore_replaced_credentials() {
        let (state, data_dir) = temp_storage_state("result-updates");
        let storage = storage_handle(&state).unwrap();
        let attempted = usable_config("https://example.test/ingest");
        storage.save_activity_cloud_config(&attempted).unwrap();

        let failure = UploadFailure {
            http_status: Some(401),
            message: "x".repeat(300),
        };
        record_upload_failure(&state, &attempted, 100, &failure).unwrap();
        let failed = storage.load_activity_cloud_config().unwrap().unwrap();
        assert_eq!(
            failed.upload_status.as_deref(),
            Some(UPLOAD_STATUS_AUTH_FAILED)
        );
        assert_eq!(failed.last_upload_attempt_epoch_ms, Some(100));
        assert_eq!(failed.last_upload_http_status, Some(401));
        assert_eq!(failed.last_upload_error.as_ref().unwrap().len(), 256);
        assert!(failed.auth_failed_at_epoch_ms.is_some());

        record_upload_success(&state, &failed, 200).unwrap();
        let succeeded = storage.load_activity_cloud_config().unwrap().unwrap();
        assert_eq!(succeeded.upload_status.as_deref(), Some(UPLOAD_STATUS_OK));
        assert_eq!(succeeded.last_upload_attempt_epoch_ms, Some(200));
        assert_eq!(succeeded.last_upload_http_status, Some(200));
        assert_eq!(succeeded.last_upload_error, None);
        assert_eq!(succeeded.auth_failed_at_epoch_ms, None);

        let mut replacement = succeeded.clone();
        replacement.upload_token = "replacement".into();
        storage.save_activity_cloud_config(&replacement).unwrap();
        record_upload_failure(
            &state,
            &succeeded,
            300,
            &UploadFailure {
                http_status: Some(500),
                message: "server error".into(),
            },
        )
        .unwrap();
        let unchanged = storage.load_activity_cloud_config().unwrap().unwrap();
        assert_eq!(unchanged.upload_token, "replacement");
        assert_eq!(unchanged.upload_status, replacement.upload_status);
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn same_upload_config_compares_every_credential_dimension() {
        let base = usable_config("https://example.test/ingest");
        assert!(same_upload_config(&base, &base));

        let mut variants = Vec::new();
        let mut value = base.clone();
        value.ingest_url.push_str("/other");
        variants.push(value);
        let mut value = base.clone();
        value.upload_token.push_str("-other");
        variants.push(value);
        let mut value = base.clone();
        value.home_id.push_str("-other");
        variants.push(value);
        let mut value = base.clone();
        value.hub_id.push_str("-other");
        variants.push(value);
        let mut value = base.clone();
        value.token_id = Some("other".into());
        variants.push(value);
        let mut value = base.clone();
        value.server_instance_id = Some("other".into());
        variants.push(value);

        for variant in variants {
            assert!(!same_upload_config(&base, &variant));
        }
    }
}
