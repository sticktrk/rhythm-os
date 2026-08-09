//! Durable, server-owned support bundle collection jobs.
//!
//! The app persists the cloud submission first, then asks the server to queue
//! one job. Queue state is fsynced before HTTP 202 so app suspension or closure
//! cannot cancel collection. Jobs survive process restart, serialize expensive
//! bundle builds, and retain completion callbacks until the cloud/GitHub
//! projection acknowledges them.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use rhythm_os::state::SharedState;

const QUEUE_SCHEMA_VERSION: u32 = 1;
pub const QUEUE_FILE_NAME: &str = "support_bundle_jobs.json";
const MAX_PENDING_UPLOAD_AGE: Duration = Duration::from_secs(2 * 60 * 60);
const MAX_APP_LOG_BYTES: usize = 768 * 1024;
const MAX_APP_METADATA_BYTES: usize = 16 * 1024;
const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_TOKEN_BYTES: usize = 200;
const CALLBACK_RETRY_INITIAL: Duration = Duration::from_secs(5);
const CALLBACK_RETRY_MAX: Duration = Duration::from_secs(15 * 60);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const UPLOAD_CACHE_CONTROL: &str = "3600";

static QUEUE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static ACTIVE_JOB_IDS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static BUILD_SEMAPHORE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct NewSupportBundleJob {
    pub submission_id: String,
    pub upload_url: String,
    pub completion_url: String,
    pub completion_token: String,
    pub app_log: Option<String>,
    pub app_metadata: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Queued,
    AlreadyQueued,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct QueueDocument {
    schema_version: u32,
    jobs: Vec<PersistedSupportBundleJob>,
}

impl Default for QueueDocument {
    fn default() -> Self {
        Self {
            schema_version: QUEUE_SCHEMA_VERSION,
            jobs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedSupportBundleJob {
    schema_version: u32,
    submission_id: String,
    upload_url: String,
    completion_url: String,
    completion_token: String,
    app_log: Option<String>,
    app_metadata: Option<serde_json::Value>,
    queued_at_epoch_ms: u64,
    auth_fingerprint: String,
    completion: Option<SupportBundleCompletion>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SupportBundleCompletion {
    submission_id: String,
    status: String,
    duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<crate::debug_bundle::DebugBundleSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_stage: Option<String>,
}

pub fn enqueue(state: SharedState, new_job: NewSupportBundleJob) -> Result<EnqueueOutcome> {
    validate_new_job(&new_job)?;
    let data_dir = data_dir(&state)?;
    let job = PersistedSupportBundleJob {
        schema_version: QUEUE_SCHEMA_VERSION,
        submission_id: new_job.submission_id,
        upload_url: new_job.upload_url,
        completion_url: new_job.completion_url,
        completion_token: new_job.completion_token,
        app_log: new_job.app_log,
        app_metadata: new_job.app_metadata,
        queued_at_epoch_ms: epoch_ms(),
        auth_fingerprint: auth_fingerprint(&data_dir)
            .ok_or_else(|| anyhow!("server auth identity is unavailable"))?,
        completion: None,
    };

    let _guard = queue_lock()
        .lock()
        .map_err(|_| anyhow!("support queue lock poisoned"))?;
    let mut document = load_document(&data_dir)?;
    if document
        .jobs
        .iter()
        .any(|existing| existing.submission_id == job.submission_id)
    {
        spawn_if_inactive(state, job.submission_id.clone());
        return Ok(EnqueueOutcome::AlreadyQueued);
    }
    document.jobs.push(job.clone());
    persist_document(&data_dir, &document)?;
    drop(_guard);

    log::info!(
        target: "support_bundle",
        "Queued asynchronous support bundle {}",
        job.submission_id
    );
    spawn_if_inactive(state, job.submission_id);
    Ok(EnqueueOutcome::Queued)
}

/// Resume current-identity jobs after the Tokio runtime starts. Jobs from a
/// pre-reset auth identity are deleted without calling their old cloud target.
pub fn resume_pending(state: SharedState) {
    let data_dir = match data_dir(&state) {
        Ok(value) => value,
        Err(error) => {
            log::warn!(target: "support_bundle", "Cannot inspect support queue: {error:#}");
            return;
        }
    };
    let current_fingerprint = auth_fingerprint(&data_dir);
    let ids = {
        let Ok(_guard) = queue_lock().lock() else {
            log::warn!(target: "support_bundle", "Support queue lock poisoned at startup");
            return;
        };
        let mut document = match load_document(&data_dir) {
            Ok(value) => value,
            Err(error) => {
                log::warn!(target: "support_bundle", "Cannot load support queue: {error:#}");
                return;
            }
        };
        let previous_len = document.jobs.len();
        document.jobs.retain(|job| {
            current_fingerprint.as_ref().is_some_and(|fingerprint| {
                job.schema_version == QUEUE_SCHEMA_VERSION && job.auth_fingerprint == *fingerprint
            })
        });
        if document.jobs.len() != previous_len {
            log::warn!(
                target: "support_bundle",
                "Discarding {} support bundle job(s) from an unknown schema or prior auth identity",
                previous_len - document.jobs.len()
            );
            if let Err(error) = persist_document(&data_dir, &document) {
                log::warn!(target: "support_bundle", "Cannot prune stale support jobs: {error:#}");
                return;
            }
        }
        document
            .jobs
            .iter()
            .map(|job| job.submission_id.clone())
            .collect::<Vec<_>>()
    };

    for submission_id in ids {
        spawn_if_inactive(state.clone(), submission_id);
    }
}

pub fn outbound_url_is_acceptable(url: &str) -> bool {
    if url.len() > MAX_URL_BYTES {
        return false;
    }
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let host_port = rest.split(['/', '?']).next().unwrap_or("");
    let host = host_port
        .strip_prefix('[')
        .and_then(|bracketed| bracketed.split(']').next())
        .unwrap_or_else(|| host_port.split(':').next().unwrap_or(""));
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

fn validate_new_job(job: &NewSupportBundleJob) -> Result<()> {
    if !valid_submission_id(&job.submission_id) {
        return Err(anyhow!("invalid support submission id"));
    }
    if !outbound_url_is_acceptable(&job.upload_url)
        || !outbound_url_is_acceptable(&job.completion_url)
    {
        return Err(anyhow!("support upload and completion URLs must use https"));
    }
    if job.completion_token.len() < 20 || job.completion_token.len() > MAX_TOKEN_BYTES {
        return Err(anyhow!("invalid support completion token"));
    }
    if job
        .app_log
        .as_ref()
        .is_some_and(|log| log.len() > MAX_APP_LOG_BYTES)
    {
        return Err(anyhow!("app log exceeds support bundle limit"));
    }
    if let Some(metadata) = &job.app_metadata {
        let encoded = serde_json::to_vec(metadata).context("serializing app metadata")?;
        if encoded.len() > MAX_APP_METADATA_BYTES {
            return Err(anyhow!("app metadata exceeds support bundle limit"));
        }
    }
    Ok(())
}

fn spawn_if_inactive(state: SharedState, submission_id: String) {
    let inserted = active_job_ids()
        .lock()
        .map(|mut ids| ids.insert(submission_id.clone()))
        .unwrap_or(false);
    if !inserted {
        return;
    }

    tokio::spawn(async move {
        if let Err(error) = process_job(state.clone(), &submission_id).await {
            log::error!(
                target: "support_bundle",
                "Support bundle job {} stopped: {:#}",
                submission_id,
                error
            );
        }
        if let Ok(mut ids) = active_job_ids().lock() {
            ids.remove(&submission_id);
        }
    });
}

async fn process_job(state: SharedState, submission_id: &str) -> Result<()> {
    let data_dir = data_dir(&state)?;
    let mut job = load_job(&data_dir, submission_id)?
        .ok_or_else(|| anyhow!("queued support bundle disappeared"))?;

    if job.completion.is_none() {
        let completion = if pending_job_expired(&job) {
            SupportBundleCompletion {
                submission_id: job.submission_id.clone(),
                status: "failed".to_string(),
                duration_ms: elapsed_ms(job.queued_at_epoch_ms),
                file_name: None,
                size_bytes: None,
                summary: None,
                failure_stage: Some("job_expired".to_string()),
            }
        } else {
            collect_and_upload(state, &job).await
        };
        update_completion(&data_dir, submission_id, completion.clone())?;
        job.completion = Some(completion);
    }

    let completion = job
        .completion
        .as_ref()
        .ok_or_else(|| anyhow!("support completion missing"))?;
    let mut retry_delay = CALLBACK_RETRY_INITIAL;
    loop {
        match send_completion(&job, completion).await {
            CallbackDisposition::Acknowledged => {
                remove_job(&data_dir, submission_id)?;
                log::info!(
                    target: "support_bundle",
                    "Completed asynchronous support bundle {} with status {}",
                    submission_id,
                    completion.status
                );
                return Ok(());
            }
            CallbackDisposition::Rejected(status) => {
                remove_job(&data_dir, submission_id)?;
                return Err(anyhow!("completion callback rejected with HTTP {status}"));
            }
            CallbackDisposition::Retry => {
                log::warn!(
                    target: "support_bundle",
                    "Completion callback for {} will retry in {} seconds",
                    submission_id,
                    retry_delay.as_secs()
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(CALLBACK_RETRY_MAX);
            }
        }
    }
}

async fn collect_and_upload(
    state: SharedState,
    job: &PersistedSupportBundleJob,
) -> SupportBundleCompletion {
    let started = std::time::Instant::now();
    let semaphore = build_semaphore();
    let permit = match semaphore.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            return failed_completion(job, started, "build");
        }
    };
    let app_log = job
        .app_log
        .clone()
        .map(|log_text| crate::debug_bundle::AppLogAttachment {
            log_text,
            metadata: job.app_metadata.clone(),
        });
    let build_state = state.clone();
    let build = tokio::task::spawn_blocking(move || {
        crate::debug_bundle::build_debug_bundle_with_app_log(&build_state, app_log)
    })
    .await;
    drop(permit);

    let bundle = match build {
        Ok(Ok(bundle)) => bundle,
        Ok(Err(error)) => {
            log::error!(
                target: "support_bundle",
                "Support bundle {} build failed: {}",
                job.submission_id,
                error
            );
            return failed_completion(job, started, "build");
        }
        Err(error) => {
            log::error!(
                target: "support_bundle",
                "Support bundle {} build task failed: {}",
                job.submission_id,
                error
            );
            return failed_completion(job, started, "build");
        }
    };

    let file_name = bundle.file_name;
    let size_bytes = bundle.bytes.len();
    let summary = bundle.summary;
    if let Err(error) = upload_bundle_to_signed_url(&job.upload_url, bundle.bytes).await {
        log::error!(
            target: "support_bundle",
            "Support bundle {} upload failed: {}",
            job.submission_id,
            error
        );
        return failed_completion(job, started, "upload");
    }

    SupportBundleCompletion {
        submission_id: job.submission_id.clone(),
        status: "uploaded".to_string(),
        duration_ms: duration_ms(started.elapsed()),
        file_name: Some(file_name),
        size_bytes: Some(size_bytes),
        summary: Some(summary),
        failure_stage: None,
    }
}

fn failed_completion(
    job: &PersistedSupportBundleJob,
    started: std::time::Instant,
    stage: &'static str,
) -> SupportBundleCompletion {
    SupportBundleCompletion {
        submission_id: job.submission_id.clone(),
        status: "failed".to_string(),
        duration_ms: duration_ms(started.elapsed()),
        file_name: None,
        size_bytes: None,
        summary: None,
        failure_stage: Some(stage.to_string()),
    }
}

pub async fn upload_bundle_to_signed_url(upload_url: &str, bundle_bytes: Vec<u8>) -> Result<()> {
    let boundary = upload_boundary(bundle_bytes.len());
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let body = signed_upload_body(bundle_bytes, &boundary);
    let response = reqwest::Client::builder()
        .timeout(UPLOAD_TIMEOUT)
        .build()?
        .put(upload_url)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .header("x-upsert", "false")
        .body(body)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(anyhow!("upload target returned HTTP {}", response.status()));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum CallbackDisposition {
    Acknowledged,
    Rejected(reqwest::StatusCode),
    Retry,
}

async fn send_completion(
    job: &PersistedSupportBundleJob,
    completion: &SupportBundleCompletion,
) -> CallbackDisposition {
    let response = reqwest::Client::new()
        .post(&job.completion_url)
        .bearer_auth(&job.completion_token)
        .json(completion)
        .send()
        .await;
    match response {
        Ok(response) => callback_status_disposition(response.status()),
        Err(_) => CallbackDisposition::Retry,
    }
}

fn callback_status_disposition(status: reqwest::StatusCode) -> CallbackDisposition {
    if status.is_success() {
        return CallbackDisposition::Acknowledged;
    }
    // These client statuses are explicitly transient or can resolve after the
    // cloud projection finishes a concurrent attempt. Other 4xx responses mean
    // the durable job can never authenticate or satisfy the callback contract.
    if status.is_client_error() && !matches!(status.as_u16(), 408 | 409 | 425 | 429) {
        return CallbackDisposition::Rejected(status);
    }
    CallbackDisposition::Retry
}

fn pending_job_expired(job: &PersistedSupportBundleJob) -> bool {
    epoch_ms().saturating_sub(job.queued_at_epoch_ms) > MAX_PENDING_UPLOAD_AGE.as_millis() as u64
}

fn data_dir(state: &SharedState) -> Result<PathBuf> {
    let state = state.lock().map_err(|_| anyhow!("state lock poisoned"))?;
    if state.data_dir.trim().is_empty() {
        return Err(anyhow!("server data directory is unavailable"));
    }
    Ok(PathBuf::from(&state.data_dir))
}

fn queue_path(data_dir: &Path) -> PathBuf {
    data_dir.join(QUEUE_FILE_NAME)
}

fn load_document(data_dir: &Path) -> Result<QueueDocument> {
    let path = queue_path(data_dir);
    if !path.exists() {
        return Ok(QueueDocument::default());
    }
    let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let document: QueueDocument =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    if document.schema_version != QUEUE_SCHEMA_VERSION {
        return Err(anyhow!(
            "unsupported support queue schema {}",
            document.schema_version
        ));
    }
    Ok(document)
}

fn persist_document(data_dir: &Path, document: &QueueDocument) -> Result<()> {
    let path = queue_path(data_dir);
    if document.jobs.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => sync_directory(data_dir),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
        }
    } else {
        let bytes = serde_json::to_vec_pretty(document).context("serializing support queue")?;
        atomic_write_secret(&path, &bytes)
    }
}

fn load_job(data_dir: &Path, submission_id: &str) -> Result<Option<PersistedSupportBundleJob>> {
    let _guard = queue_lock()
        .lock()
        .map_err(|_| anyhow!("support queue lock poisoned"))?;
    Ok(load_document(data_dir)?
        .jobs
        .into_iter()
        .find(|job| job.submission_id == submission_id))
}

fn update_completion(
    data_dir: &Path,
    submission_id: &str,
    completion: SupportBundleCompletion,
) -> Result<()> {
    let _guard = queue_lock()
        .lock()
        .map_err(|_| anyhow!("support queue lock poisoned"))?;
    let mut document = load_document(data_dir)?;
    let job = document
        .jobs
        .iter_mut()
        .find(|job| job.submission_id == submission_id)
        .ok_or_else(|| anyhow!("support job missing during completion"))?;
    job.completion = Some(completion);
    job.app_log = None;
    job.app_metadata = None;
    job.upload_url.clear();
    persist_document(data_dir, &document)
}

fn remove_job(data_dir: &Path, submission_id: &str) -> Result<()> {
    let _guard = queue_lock()
        .lock()
        .map_err(|_| anyhow!("support queue lock poisoned"))?;
    let mut document = load_document(data_dir)?;
    document
        .jobs
        .retain(|job| job.submission_id != submission_id);
    persist_document(data_dir, &document)
}

fn atomic_write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("support queue has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension("json.tmp");
    {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&tmp)?
        };
        #[cfg(not(unix))]
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    sync_directory(parent)
}

fn sync_directory(path: &Path) -> Result<()> {
    if let Ok(directory) = fs::File::open(path) {
        directory.sync_all()?;
    }
    Ok(())
}

fn auth_fingerprint(data_dir: &Path) -> Option<String> {
    let bytes = fs::read(data_dir.join("auth.json")).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

fn valid_submission_id(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(index, ch)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                ch == '-'
            } else {
                ch.is_ascii_hexdigit()
            }
        })
}

fn upload_boundary(size_bytes: usize) -> String {
    format!(
        "rhythm-support-{}-{}-{size_bytes}",
        std::process::id(),
        epoch_ms()
    )
}

fn signed_upload_body(bundle_bytes: Vec<u8>, boundary: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(bundle_bytes.len() + 512);
    append_text_field(&mut body, boundary, "cacheControl", UPLOAD_CACHE_CONTROL);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"content-type: application/gzip\r\n");
    body.extend_from_slice(b"content-disposition: form-data; name=\"\"; filename=\"\"\r\n\r\n");
    body.extend_from_slice(&bundle_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

fn append_text_field(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("content-disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(value.as_bytes());
    body.extend_from_slice(b"\r\n");
}

fn queue_lock() -> &'static Mutex<()> {
    QUEUE_LOCK.get_or_init(|| Mutex::new(()))
}

fn active_job_ids() -> &'static Mutex<HashSet<String>> {
    ACTIVE_JOB_IDS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn build_semaphore() -> Arc<tokio::sync::Semaphore> {
    BUILD_SEMAPHORE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1)))
        .clone()
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn elapsed_ms(started_at_epoch_ms: u64) -> u64 {
    epoch_ms().saturating_sub(started_at_epoch_ms)
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rhythm-support-jobs-{name}-{}-{}",
            std::process::id(),
            epoch_ms()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn persisted_job(submission_id: &str, fingerprint: &str) -> PersistedSupportBundleJob {
        PersistedSupportBundleJob {
            schema_version: QUEUE_SCHEMA_VERSION,
            submission_id: submission_id.to_string(),
            upload_url: "https://storage.example.test/upload".to_string(),
            completion_url: "https://example.test/complete".to_string(),
            completion_token: "completion-token-with-enough-entropy".to_string(),
            app_log: Some("app log".to_string()),
            app_metadata: None,
            queued_at_epoch_ms: epoch_ms(),
            auth_fingerprint: fingerprint.to_string(),
            completion: None,
        }
    }

    #[test]
    fn queue_file_is_secret_and_round_trips_jobs() {
        let dir = temp_dir("round-trip");
        let document = QueueDocument {
            schema_version: QUEUE_SCHEMA_VERSION,
            jobs: vec![persisted_job(
                "8f68c4ae-edf5-4e7d-9c9a-99cc2e86b1bc",
                "fingerprint",
            )],
        };
        persist_document(&dir, &document).unwrap();
        let loaded = load_document(&dir).unwrap();
        assert_eq!(loaded.jobs.len(), 1);
        assert_eq!(loaded.jobs[0].app_log.as_deref(), Some("app log"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(queue_path(&dir)).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn completion_phase_drops_upload_url_and_app_log() {
        let dir = temp_dir("completion");
        let id = "8f68c4ae-edf5-4e7d-9c9a-99cc2e86b1bc";
        persist_document(
            &dir,
            &QueueDocument {
                schema_version: QUEUE_SCHEMA_VERSION,
                jobs: vec![persisted_job(id, "fingerprint")],
            },
        )
        .unwrap();
        update_completion(
            &dir,
            id,
            SupportBundleCompletion {
                submission_id: id.to_string(),
                status: "failed".to_string(),
                duration_ms: 10,
                file_name: None,
                size_bytes: None,
                summary: None,
                failure_stage: Some("upload".to_string()),
            },
        )
        .unwrap();

        let job = load_job(&dir, id).unwrap().unwrap();
        assert!(job.upload_url.is_empty());
        assert!(job.app_log.is_none());
        assert_eq!(
            job.completion.unwrap().failure_stage.as_deref(),
            Some("upload")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn outbound_urls_require_https_except_loopback() {
        assert!(outbound_url_is_acceptable("https://example.test/path"));
        assert!(outbound_url_is_acceptable("http://127.0.0.1:8000/path"));
        assert!(!outbound_url_is_acceptable("http://192.168.1.5/path"));
        assert!(!outbound_url_is_acceptable("file:///tmp/report"));
    }

    #[test]
    fn missing_auth_has_no_resumable_identity() {
        let dir = temp_dir("missing-auth");
        assert_eq!(auth_fingerprint(&dir), None);
        fs::write(dir.join("auth.json"), b"identity").unwrap();
        assert!(auth_fingerprint(&dir).is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn callback_retries_transient_client_statuses_only() {
        assert_eq!(
            callback_status_disposition(reqwest::StatusCode::TOO_MANY_REQUESTS),
            CallbackDisposition::Retry
        );
        assert_eq!(
            callback_status_disposition(reqwest::StatusCode::CONFLICT),
            CallbackDisposition::Retry
        );
        assert_eq!(
            callback_status_disposition(reqwest::StatusCode::UNAUTHORIZED),
            CallbackDisposition::Rejected(reqwest::StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn resume_prunes_jobs_from_a_prior_auth_identity() {
        let dir = temp_dir("prior-identity");
        fs::write(dir.join("auth.json"), b"current-identity").unwrap();
        persist_document(
            &dir,
            &QueueDocument {
                schema_version: QUEUE_SCHEMA_VERSION,
                jobs: vec![persisted_job(
                    "687045aa-cb9b-4cd6-90ae-08600f2b6a7d",
                    "prior-fingerprint",
                )],
            },
        )
        .unwrap();
        let state: SharedState = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        state.lock().unwrap().data_dir = dir.display().to_string();

        resume_pending(state);

        assert!(!queue_path(&dir).exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn resume_delivers_a_persisted_completion_after_restart() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let dir = temp_dir("restart-callback");
        fs::write(dir.join("auth.json"), b"current-identity").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let callback_port = listener.local_addr().unwrap().port();
        let callback = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 16 * 1024];
            let read = socket.read(&mut request).await.unwrap();
            request.truncate(read);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
            String::from_utf8(request).unwrap()
        });

        let id = "dad2114d-8123-4cfb-92f2-91d711057b49";
        let mut job = persisted_job(id, &auth_fingerprint(&dir).unwrap());
        job.completion_url = format!("http://127.0.0.1:{callback_port}/complete");
        job.upload_url.clear();
        job.app_log = None;
        job.completion = Some(SupportBundleCompletion {
            submission_id: id.to_string(),
            status: "failed".to_string(),
            duration_ms: 42,
            file_name: None,
            size_bytes: None,
            summary: None,
            failure_stage: Some("upload".to_string()),
        });
        persist_document(
            &dir,
            &QueueDocument {
                schema_version: QUEUE_SCHEMA_VERSION,
                jobs: vec![job],
            },
        )
        .unwrap();
        let state: SharedState = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        state.lock().unwrap().data_dir = dir.display().to_string();

        resume_pending(state);

        let request = tokio::time::timeout(Duration::from_secs(2), callback)
            .await
            .unwrap()
            .unwrap();
        assert!(request.contains(id));
        assert!(request.contains("Bearer completion-token-with-enough-entropy"));
        for _ in 0..50 {
            if !queue_path(&dir).exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!queue_path(&dir).exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
