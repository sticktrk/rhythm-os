//! Device-side cloud upload for server-origin light activity.
//!
//! The app provisions a scoped upload token, but Rhythm OS owns the event path:
//! local history remains the retry source and uploads go directly to Supabase.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::activity::{LightActivityEvent, LIGHT_ACTIVITY_HISTORY_LIMIT};
use crate::handlers::ApiResponse;
use crate::state::SharedState;

const ACTIVITY_CLOUD_SCHEMA_VERSION: u8 = 1;
const UPLOAD_STATUS_PENDING: &str = "pending";
const UPLOAD_STATUS_OK: &str = "ok";
const UPLOAD_STATUS_FAILED: &str = "failed";
const UPLOAD_STATUS_AUTH_FAILED: &str = "auth_failed";
const UPLOAD_STATUS_NOT_CONFIGURED: &str = "not_configured";

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

pub fn enqueue_light_activity_upload(state: &SharedState) {
    enqueue_recent_activity_upload(state);
}

pub fn enqueue_recent_activity_upload(state: &SharedState) {
    let Some((config, activities)) = upload_snapshot(state) else {
        return;
    };
    if activities.is_empty() {
        return;
    }

    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        log::debug!(
            target: "cmd",
            "Skipping server activity cloud upload outside a Tokio runtime"
        );
        return;
    };

    let state_for_result = state.clone();
    handle.spawn(async move {
        let attempt_epoch_ms = current_epoch_ms();
        match upload_activity_batch(config.clone(), activities).await {
            Ok(()) => {
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
    config: StoredActivityCloudConfig,
    activities: Vec<LightActivityEvent>,
) -> Result<(), UploadFailure> {
    let body = ActivityCloudUploadBody {
        home_id: &config.home_id,
        hub_id: &config.hub_id,
        server_instance_id: config.server_instance_id.as_deref(),
        events: &activities,
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

fn update_stored_config_after_upload(
    state: &SharedState,
    attempted_config: &StoredActivityCloudConfig,
    update: impl FnOnce(&mut StoredActivityCloudConfig),
) -> anyhow::Result<()> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
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
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
    Ok(storage
        .load_activity_cloud_config()?
        .map(StoredActivityCloudConfig::normalized))
}

fn save_config(state: &SharedState, config: &StoredActivityCloudConfig) -> anyhow::Result<()> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
    storage.save_activity_cloud_config(config)
}

fn clear_config(state: &SharedState) -> anyhow::Result<()> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
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
}
