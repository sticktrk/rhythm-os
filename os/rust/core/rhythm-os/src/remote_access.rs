//! Remote access tunnel configuration and platform runtime control.
//!
//! Cloudflare owns the public hostname routing. Rhythm stores only the
//! connector token and asks the active platform controller to run cloudflared.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::handlers::ApiResponse;
use crate::state::SharedState;

const RUNTIME_DIR: &str = "cloudflared";
const CONNECTOR_TOKEN_FILE: &str = "connector_token";
const HOSTNAME_FILE: &str = "hostname";
const STATUS_FILE: &str = "status.env";
const DEFAULT_METRICS_ADDR: &str = "127.0.0.1:54449";
const DEFAULT_CLOUDFLARED_PROTOCOL: &str = "http2";
const DEFAULT_CLOUDFLARED_LOGLEVEL: &str = "warn";
const DEFAULT_CLOUDFLARED_HA_CONNECTIONS: u16 = 1;
const DEFAULT_CLOUDFLARED_BIN: &str = "cloudflared";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredRemoteAccessConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u8,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub hostname: String,
    pub connector_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_name: Option<String>,
    pub updated_at_epoch_ms: u64,
}

fn default_schema_version() -> u8 {
    1
}

fn default_enabled() -> bool {
    true
}

impl StoredRemoteAccessConfig {
    pub fn redacted_status(&self) -> RemoteAccessConfigStatus {
        RemoteAccessConfigStatus {
            enabled: self.enabled,
            configured: !self.connector_token.trim().is_empty(),
            hostname: Some(self.hostname.clone()).filter(|host| !host.trim().is_empty()),
            tunnel_id: self.tunnel_id.clone(),
            tunnel_name: self.tunnel_name.clone(),
            updated_at_epoch_ms: self.updated_at_epoch_ms,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAccessConfigStatus {
    pub enabled: bool,
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_name: Option<String>,
    pub updated_at_epoch_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct PutRemoteAccessConfig {
    #[serde(default = "default_enabled")]
    enabled: bool,
    hostname: Option<String>,
    remote_url: Option<String>,
    connector_token: Option<String>,
    tunnel_id: Option<String>,
    tunnel_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RemoteAccessRuntimeStatus {
    pub cloudflared_available: bool,
    pub cloudflared_version: Option<String>,
    pub service_available: bool,
    pub service_running: bool,
    pub supervisor_state: Option<String>,
    pub supervisor_pid: Option<u32>,
    pub child_pid: Option<u32>,
    pub restart_count: u64,
    pub last_started_epoch_secs: Option<u64>,
    pub last_exit_epoch_secs: Option<u64>,
    pub last_exit_code: Option<i32>,
    pub next_restart_epoch_secs: Option<u64>,
    pub metrics_addr: Option<String>,
    pub metrics_available: bool,
    pub connector_healthy: bool,
    pub registered_connections: Option<u64>,
    pub metrics_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct RemoteAccessStatusBody {
    status: &'static str,
    enabled: bool,
    configured: bool,
    hostname: Option<String>,
    tunnel_id: Option<String>,
    tunnel_name: Option<String>,
    updated_at_epoch_ms: u64,
    cloudflared_available: bool,
    cloudflared_version: Option<String>,
    service_available: bool,
    service_running: bool,
    supervisor_state: Option<String>,
    supervisor_pid: Option<u32>,
    child_pid: Option<u32>,
    restart_count: u64,
    last_started_epoch_secs: Option<u64>,
    last_exit_epoch_secs: Option<u64>,
    last_exit_code: Option<i32>,
    next_restart_epoch_secs: Option<u64>,
    metrics_addr: Option<String>,
    metrics_available: bool,
    connector_healthy: bool,
    registered_connections: Option<u64>,
    metrics_error: Option<String>,
}

impl RemoteAccessStatusBody {
    fn from_parts(
        config: Option<RemoteAccessConfigStatus>,
        runtime: RemoteAccessRuntimeStatus,
    ) -> Self {
        let config = config.unwrap_or_default();
        Self {
            status: "ok",
            enabled: config.enabled,
            configured: config.configured,
            hostname: config.hostname,
            tunnel_id: config.tunnel_id,
            tunnel_name: config.tunnel_name,
            updated_at_epoch_ms: config.updated_at_epoch_ms,
            cloudflared_available: runtime.cloudflared_available,
            cloudflared_version: runtime.cloudflared_version,
            service_available: runtime.service_available,
            service_running: runtime.service_running,
            supervisor_state: runtime.supervisor_state,
            supervisor_pid: runtime.supervisor_pid,
            child_pid: runtime.child_pid,
            restart_count: runtime.restart_count,
            last_started_epoch_secs: runtime.last_started_epoch_secs,
            last_exit_epoch_secs: runtime.last_exit_epoch_secs,
            last_exit_code: runtime.last_exit_code,
            next_restart_epoch_secs: runtime.next_restart_epoch_secs,
            metrics_addr: runtime.metrics_addr,
            metrics_available: runtime.metrics_available,
            connector_healthy: runtime.connector_healthy,
            registered_connections: runtime.registered_connections,
            metrics_error: runtime.metrics_error,
        }
    }
}

pub trait RemoteAccessController: Send + Sync {
    fn status(&self, runtime_dir: &Path) -> RemoteAccessRuntimeStatus;
    fn start(&self, runtime_dir: &Path, config: &StoredRemoteAccessConfig) -> anyhow::Result<()>;
    fn stop(&self, runtime_dir: &Path) -> anyhow::Result<()>;
}

/// Run remote-access work on the blocking pool. These paths spawn processes
/// (`cloudflared --version`, `kill -0`), make raw TCP metrics reads, and do
/// fsync'd file writes — on the appliance's single-worker tokio runtime,
/// running them inline wedges the entire HTTP/SSE surface for the duration
/// (or forever, if cloudflared is wedged).
async fn run_remote_access_blocking<F>(f: F) -> ApiResponse
where
    F: FnOnce() -> ApiResponse + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(response) => response,
        Err(e) => ApiResponse::server_error(anyhow::anyhow!("remote access task failed: {e}")),
    }
}

pub async fn get_status(State(state): State<SharedState>) -> ApiResponse {
    run_remote_access_blocking(move || match load_config(&state) {
        Ok(config) => {
            let runtime = runtime_status(&state);
            json_ok(RemoteAccessStatusBody::from_parts(
                config.map(|config| config.redacted_status()),
                runtime,
            ))
        }
        Err(e) => ApiResponse::server_error(e),
    })
    .await
}

pub fn status_snapshot(state: &SharedState) -> serde_json::Value {
    let runtime = runtime_status(state);
    match load_config(state) {
        Ok(config) => serde_json::to_value(RemoteAccessStatusBody::from_parts(
            config.map(|config| config.redacted_status()),
            runtime,
        ))
        .unwrap_or_else(|_| json!({"status": "error", "error": "serialize status"})),
        Err(e) => {
            let mut value = serde_json::to_value(RemoteAccessStatusBody::from_parts(None, runtime))
                .unwrap_or_else(|_| json!({"status": "error"}));
            value["status"] = json!("error");
            value["config_error"] = json!(e.to_string());
            value
        }
    }
}

pub fn status_snapshot_json(state: &SharedState) -> String {
    serde_json::to_string_pretty(&status_snapshot(state))
        .unwrap_or_else(|_| "{\"status\":\"error\"}".to_string())
}

pub async fn put_config(
    State(state): State<SharedState>,
    Json(body): Json<PutRemoteAccessConfig>,
) -> ApiResponse {
    let hostname = match normalize_hostname(body.hostname.or(body.remote_url).as_deref()) {
        Ok(hostname) => hostname,
        Err(message) => return ApiResponse::bad_request(message),
    };

    let connector_token = body
        .connector_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let Some(connector_token) = connector_token else {
        return ApiResponse::bad_request("Missing connector_token");
    };

    let config = StoredRemoteAccessConfig {
        schema_version: 1,
        enabled: body.enabled,
        hostname,
        connector_token,
        tunnel_id: trim_optional(body.tunnel_id),
        tunnel_name: trim_optional(body.tunnel_name),
        updated_at_epoch_ms: current_epoch_ms(),
    };

    run_remote_access_blocking(move || {
        if let Err(e) = save_config(&state, &config) {
            return ApiResponse::server_error(e);
        }
        if let Err(e) = sync_runtime_files(&state, Some(&config)) {
            return ApiResponse::server_error(e);
        }

        let service_result = if config.enabled {
            controller_for_state(&state)
                .ok_or_else(|| anyhow::anyhow!("remote access controller not configured"))
                .and_then(|controller| controller.start(&runtime_dir(&state)?, &config))
        } else {
            controller_for_state(&state)
                .ok_or_else(|| anyhow::anyhow!("remote access controller not configured"))
                .and_then(|controller| controller.stop(&runtime_dir(&state)?))
        };

        let mut value = serde_json::to_value(RemoteAccessStatusBody::from_parts(
            Some(config.redacted_status()),
            runtime_status(&state),
        ))
        .unwrap_or_else(|_| json!({"status": "ok"}));
        if let Err(e) = service_result {
            value["service_error"] = json!(e.to_string());
        }
        ApiResponse::json_ok(value.to_string())
    })
    .await
}

pub async fn delete_config(State(state): State<SharedState>) -> ApiResponse {
    run_remote_access_blocking(move || {
        if let Err(e) = clear_config(&state) {
            return ApiResponse::server_error(e);
        }
        if let Err(e) = sync_runtime_files(&state, None) {
            return ApiResponse::server_error(e);
        }
        let service_result = controller_for_state(&state)
            .ok_or_else(|| anyhow::anyhow!("remote access controller not configured"))
            .and_then(|controller| controller.stop(&runtime_dir(&state)?));

        let mut value = serde_json::to_value(RemoteAccessStatusBody::from_parts(
            None,
            runtime_status(&state),
        ))
        .unwrap_or_else(|_| json!({"status": "ok"}));
        if let Err(e) = service_result {
            value["service_error"] = json!(e.to_string());
        }
        ApiResponse::json_ok(value.to_string())
    })
    .await
}

pub fn reconcile_remote_access_runtime(state: &SharedState) -> anyhow::Result<()> {
    let config = load_config(state)?;
    match config.as_ref() {
        Some(config) if config.enabled => {
            sync_runtime_files(state, Some(config))?;
            controller_for_state(state)
                .ok_or_else(|| anyhow::anyhow!("remote access controller not configured"))?
                .start(&runtime_dir(state)?, config)
        }
        _ => {
            sync_runtime_files(state, None)?;
            if let Some(controller) = controller_for_state(state) {
                controller.stop(&runtime_dir(state)?)?;
            }
            Ok(())
        }
    }
}

fn controller_for_state(state: &SharedState) -> Option<Arc<dyn RemoteAccessController>> {
    state
        .lock()
        .ok()
        .and_then(|state| state.remote_access_controller.clone())
}

fn runtime_status(state: &SharedState) -> RemoteAccessRuntimeStatus {
    let Some(controller) = controller_for_state(state) else {
        return RemoteAccessRuntimeStatus {
            supervisor_state: Some("unsupported".to_string()),
            metrics_addr: Some(DEFAULT_METRICS_ADDR.to_string()),
            ..RemoteAccessRuntimeStatus::default()
        };
    };
    match runtime_dir(state) {
        Ok(dir) => controller.status(&dir),
        Err(e) => RemoteAccessRuntimeStatus {
            supervisor_state: Some("error".to_string()),
            metrics_error: Some(e.to_string()),
            metrics_addr: Some(DEFAULT_METRICS_ADDR.to_string()),
            ..RemoteAccessRuntimeStatus::default()
        },
    }
}

fn load_config(state: &SharedState) -> anyhow::Result<Option<StoredRemoteAccessConfig>> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
    storage.load_remote_access_config()
}

fn save_config(state: &SharedState, config: &StoredRemoteAccessConfig) -> anyhow::Result<()> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
    storage.save_remote_access_config(config)
}

fn clear_config(state: &SharedState) -> anyhow::Result<()> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
    storage.clear_remote_access_config()
}

fn sync_runtime_files(
    state: &SharedState,
    config: Option<&StoredRemoteAccessConfig>,
) -> anyhow::Result<()> {
    let dir = runtime_dir(state)?;
    std::fs::create_dir_all(&dir)?;
    let token_path = dir.join(CONNECTOR_TOKEN_FILE);
    let hostname_path = dir.join(HOSTNAME_FILE);

    match config {
        Some(config) if config.enabled => {
            write_secret(&token_path, &config.connector_token)?;
            std::fs::write(hostname_path, format!("{}\n", config.hostname))?;
        }
        _ => {
            remove_if_exists(&token_path)?;
            remove_if_exists(&hostname_path)?;
        }
    }

    Ok(())
}

fn runtime_dir(state: &SharedState) -> anyhow::Result<PathBuf> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    Ok(Path::new(&state.data_dir).join(RUNTIME_DIR))
}

fn write_secret(path: &Path, value: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("secret path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow::anyhow!("secret path has no file name: {}", path.display()))?;
    let tmp_path = parent.join(format!(".{file_name}.{}.tmp", current_epoch_ms()));

    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp_path)?;
    file.write_all(format!("{}\n", value.trim()).as_bytes())?;
    let _ = file.sync_all();
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp_path, path)?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn normalize_hostname(value: Option<&str>) -> Result<String, &'static str> {
    let Some(value) = value else {
        return Err("Missing hostname or remote_url");
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Missing hostname or remote_url");
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("Remote access hostname must not contain whitespace");
    }

    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .trim_matches('/');
    if host.is_empty() {
        return Err("Invalid remote access hostname");
    }
    Ok(host.to_ascii_lowercase())
}

fn trim_optional(value: Option<String>) -> Option<String> {
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

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn cloudflared_version_for(bin: &Path) -> Option<String> {
    // Fork/exec of the (large, Go) cloudflared binary takes seconds on a
    // Pi Zero and this runs on every status poll, starving the single core.
    // The version only changes when the binary does, so cache by mtime.
    type VersionCache = std::collections::HashMap<PathBuf, (SystemTime, Option<String>)>;
    static VERSION_CACHE: std::sync::OnceLock<Mutex<VersionCache>> = std::sync::OnceLock::new();

    let modified = std::fs::metadata(bin).and_then(|m| m.modified()).ok();
    let cache = VERSION_CACHE.get_or_init(Default::default);
    if let Some(modified) = modified {
        if let Ok(cache) = cache.lock() {
            if let Some((cached_mtime, version)) = cache.get(bin) {
                if *cached_mtime == modified {
                    return version.clone();
                }
            }
        }
    }

    let version = probe_cloudflared_version(bin);
    if let Some(modified) = modified {
        if let Ok(mut cache) = cache.lock() {
            cache.insert(bin.to_path_buf(), (modified, version.clone()));
        }
    }
    version
}

fn probe_cloudflared_version(bin: &Path) -> Option<String> {
    // Bounded wait: `.output()` has no timeout, and this runs on every
    // remote-access status poll — a wedged/corrupt cloudflared binary would
    // otherwise pin the calling thread forever.
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut stdout = String::new();
                child.stdout.take()?.read_to_string(&mut stdout).ok()?;
                let stdout = stdout.trim().to_string();
                return if stdout.is_empty() { None } else { Some(stdout) };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn read_supervisor_status(runtime_dir: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let path = runtime_dir.join(STATUS_FILE);
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    Ok(parse_status_env(&text))
}

fn parse_status_env(text: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::<String, String>::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        values.insert(key.trim().to_string(), value.trim().to_string());
    }
    values
}

fn read_pid_file(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|value| parse_pid(value.trim()))
}

fn parse_pid(value: &str) -> Option<u32> {
    value.parse::<u32>().ok().filter(|pid| *pid > 0)
}

fn pid_running(pid: u32) -> bool {
    // /proc lookup avoids a fork/exec per check on Linux appliances; the
    // `kill -0` fallback covers macOS/dev hosts.
    if Path::new("/proc").is_dir() {
        return Path::new(&format!("/proc/{pid}")).is_dir();
    }
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[derive(Debug, Default)]
struct MetricsStatus {
    available: bool,
    healthy: bool,
    registered_connections: Option<u64>,
    error: Option<String>,
}

fn read_metrics_status(addr: &str) -> MetricsStatus {
    let mut status = MetricsStatus::default();
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        status.error = Some(format!("invalid metrics address {addr}"));
        return status;
    };
    let Some(socket_addr) = addrs.next() else {
        status.error = Some(format!("invalid metrics address {addr}"));
        return status;
    };

    let stream = TcpStream::connect_timeout(&socket_addr, Duration::from_millis(300));
    let Ok(mut stream) = stream else {
        status.error = Some("metrics endpoint unavailable".to_string());
        return status;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let request = format!("GET /metrics HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if let Err(e) = stream.write_all(request.as_bytes()) {
        status.error = Some(format!("metrics request failed: {e}"));
        return status;
    }
    let mut response = String::new();
    if let Err(e) = stream.read_to_string(&mut response) {
        status.error = Some(format!("metrics read failed: {e}"));
        return status;
    }
    if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
        status.error = Some("metrics endpoint returned non-200 status".to_string());
        return status;
    }
    status.available = true;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(response.as_str());
    status.registered_connections = parse_registered_connections(body);
    status.healthy = status.registered_connections.unwrap_or(0) > 0;
    status
}

fn parse_registered_connections(metrics: &str) -> Option<u64> {
    let mut best = None::<u64>;
    let mut server_locations = 0_u64;
    for line in metrics.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let before_labels = line.split_once('{').map(|(name, _)| name).unwrap_or(line);
        let metric_name = before_labels
            .split_whitespace()
            .next()
            .unwrap_or(before_labels);
        let value = line
            .split_whitespace()
            .last()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0);
        if metric_name == "cloudflared_tunnel_ha_connections" {
            best = Some(best.unwrap_or(0).max(value.max(0.0).round() as u64));
        } else if metric_name == "cloudflared_tunnel_server_locations" && value > 0.0 {
            server_locations = server_locations.saturating_add(1);
        }
    }
    best.or_else(|| (server_locations > 0).then_some(server_locations))
}

fn json_ok(body: impl Serialize) -> ApiResponse {
    match serde_json::to_string(&body) {
        Ok(body) => ApiResponse::json_ok(body),
        Err(e) => ApiResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            body: e.to_string(),
            content_type: "text/plain",
        },
    }
}

#[derive(Debug, Clone)]
pub struct InitScriptRemoteAccessController {
    cloudflared_bin: PathBuf,
    init_script: PathBuf,
    pidfile: PathBuf,
    child_pidfile: PathBuf,
    metrics_addr: String,
    stop_delay: Duration,
    actions: Arc<InitScriptActions>,
}

/// Start/stop fire detached threads; without coordination a delayed stop can
/// run after a newer restart and kill the fresh connector, and concurrent
/// restarts interleave the init script's stop+start phases.
#[derive(Debug, Default)]
struct InitScriptActions {
    /// Serializes init-script invocations.
    run_lock: Mutex<()>,
    /// Monotonic action id; a pending action aborts if superseded.
    generation: AtomicU64,
}

impl InitScriptRemoteAccessController {
    pub fn new(
        cloudflared_bin: impl Into<PathBuf>,
        init_script: impl Into<PathBuf>,
        pidfile: impl Into<PathBuf>,
        child_pidfile: impl Into<PathBuf>,
    ) -> Self {
        Self {
            cloudflared_bin: cloudflared_bin.into(),
            init_script: init_script.into(),
            pidfile: pidfile.into(),
            child_pidfile: child_pidfile.into(),
            metrics_addr: DEFAULT_METRICS_ADDR.to_string(),
            stop_delay: Duration::from_millis(750),
            actions: Arc::new(InitScriptActions::default()),
        }
    }

    fn next_generation(&self) -> u64 {
        self.actions.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn is_current_generation(&self, generation: u64) -> bool {
        self.actions.generation.load(Ordering::SeqCst) == generation
    }

    pub fn with_metrics_addr(mut self, metrics_addr: impl Into<String>) -> Self {
        self.metrics_addr = metrics_addr.into();
        self
    }

    fn run_init_script(&self, action: &str) -> anyhow::Result<()> {
        if !self.init_script.exists() {
            return Ok(());
        }
        let output = Command::new(&self.init_script).arg(action).output()?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cloudflared service {} failed: {}", action, stderr.trim());
    }
}

impl RemoteAccessController for InitScriptRemoteAccessController {
    fn status(&self, runtime_dir: &Path) -> RemoteAccessRuntimeStatus {
        let cloudflared_version = cloudflared_version_for(&self.cloudflared_bin);
        let supervisor = read_supervisor_status(runtime_dir).unwrap_or_default();
        let metrics_addr = supervisor
            .get("metrics_addr")
            .filter(|value| !value.is_empty())
            .cloned()
            .or_else(|| Some(self.metrics_addr.clone()));
        let supervisor_pid = supervisor
            .get("supervisor_pid")
            .and_then(|value| parse_pid(value))
            .or_else(|| read_pid_file(&self.pidfile));
        let child_pid = supervisor
            .get("child_pid")
            .and_then(|value| parse_pid(value))
            .or_else(|| read_pid_file(&self.child_pidfile));
        let service_running =
            supervisor_pid.is_some_and(pid_running) || child_pid.is_some_and(pid_running);
        let metrics = if service_running {
            metrics_addr
                .as_deref()
                .map(read_metrics_status)
                .unwrap_or_default()
        } else {
            MetricsStatus::default()
        };

        RemoteAccessRuntimeStatus {
            cloudflared_available: cloudflared_version.is_some(),
            cloudflared_version,
            service_available: self.init_script.exists(),
            service_running,
            supervisor_state: supervisor
                .get("state")
                .filter(|value| !value.is_empty())
                .cloned(),
            supervisor_pid,
            child_pid,
            restart_count: supervisor
                .get("restart_count")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            last_started_epoch_secs: supervisor
                .get("last_started_epoch_secs")
                .and_then(|value| value.parse().ok()),
            last_exit_epoch_secs: supervisor
                .get("last_exit_epoch_secs")
                .and_then(|value| value.parse().ok()),
            last_exit_code: supervisor
                .get("last_exit_code")
                .and_then(|value| value.parse().ok()),
            next_restart_epoch_secs: supervisor
                .get("next_restart_epoch_secs")
                .and_then(|value| value.parse().ok()),
            metrics_addr,
            metrics_available: metrics.available,
            connector_healthy: metrics.healthy,
            registered_connections: metrics.registered_connections,
            metrics_error: metrics.error,
        }
    }

    fn start(&self, _runtime_dir: &Path, _config: &StoredRemoteAccessConfig) -> anyhow::Result<()> {
        if !self.init_script.exists() {
            return Ok(());
        }
        let generation = self.next_generation();
        let controller = self.clone();
        thread::spawn(move || {
            let _guard = controller
                .actions
                .run_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // A newer start/stop request replaces this one.
            if !controller.is_current_generation(generation) {
                return;
            }
            if let Err(error) = controller.run_init_script("restart") {
                log::warn!(
                    target: "sys",
                    "cloudflared service restart failed: {:#}",
                    error
                );
            }
        });
        Ok(())
    }

    fn stop(&self, _runtime_dir: &Path) -> anyhow::Result<()> {
        if !self.init_script.exists() {
            return Ok(());
        }
        let generation = self.next_generation();
        let controller = self.clone();
        thread::spawn(move || {
            thread::sleep(controller.stop_delay);
            let _guard = controller
                .actions
                .run_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // If a start arrived while we slept, do not tear it down.
            if !controller.is_current_generation(generation) {
                return;
            }
            let _ = controller.run_init_script("stop");
        });
        Ok(())
    }
}

#[derive(Clone)]
pub struct ChildProcessRemoteAccessController {
    inner: Arc<ChildProcessInner>,
}

struct ChildProcessInner {
    cloudflared_bin: PathBuf,
    protocol: String,
    loglevel: String,
    ha_connections: u16,
    metrics_addr: String,
    backoff_initial: Duration,
    backoff_max: Duration,
    stop_delay: Duration,
    shared: Mutex<ChildProcessShared>,
    child: Mutex<Option<Child>>,
}

#[derive(Clone)]
struct ChildDesired {
    token: String,
    runtime_dir: PathBuf,
}

#[derive(Default)]
struct ChildProcessShared {
    desired: Option<ChildDesired>,
    generation: u64,
    supervisor_running: bool,
    status: ChildSupervisorStatus,
}

#[derive(Clone)]
struct ChildSupervisorStatus {
    state: String,
    supervisor_pid: Option<u32>,
    child_pid: Option<u32>,
    restart_count: u64,
    last_started_epoch_secs: Option<u64>,
    last_exit_epoch_secs: Option<u64>,
    last_exit_code: Option<i32>,
    next_restart_epoch_secs: Option<u64>,
}

impl Default for ChildSupervisorStatus {
    fn default() -> Self {
        Self {
            state: "stopped".to_string(),
            supervisor_pid: None,
            child_pid: None,
            restart_count: 0,
            last_started_epoch_secs: None,
            last_exit_epoch_secs: None,
            last_exit_code: None,
            next_restart_epoch_secs: None,
        }
    }
}

impl ChildProcessRemoteAccessController {
    pub fn new(cloudflared_bin: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(ChildProcessInner {
                cloudflared_bin: cloudflared_bin.into(),
                protocol: DEFAULT_CLOUDFLARED_PROTOCOL.to_string(),
                loglevel: DEFAULT_CLOUDFLARED_LOGLEVEL.to_string(),
                ha_connections: DEFAULT_CLOUDFLARED_HA_CONNECTIONS,
                metrics_addr: DEFAULT_METRICS_ADDR.to_string(),
                backoff_initial: Duration::from_secs(2),
                backoff_max: Duration::from_secs(60),
                stop_delay: Duration::from_millis(750),
                shared: Mutex::new(ChildProcessShared::default()),
                child: Mutex::new(None),
            }),
        }
    }

    pub fn with_metrics_addr(mut self, metrics_addr: impl Into<String>) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("controller not shared yet")
            .metrics_addr = metrics_addr.into();
        self
    }

    pub fn with_protocol(mut self, protocol: impl Into<String>) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("controller not shared yet")
            .protocol = protocol.into();
        self
    }

    pub fn with_loglevel(mut self, loglevel: impl Into<String>) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("controller not shared yet")
            .loglevel = loglevel.into();
        self
    }

    pub fn with_ha_connections(mut self, ha_connections: u16) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("controller not shared yet")
            .ha_connections = ha_connections;
        self
    }

    fn ensure_supervisor(&self) {
        // Recover from poisoning — these mutexes guard plain status data, and
        // panicking here would make every future PUT /api/remote-access/config
        // panic too, permanently disabling remote access until restart.
        let mut shared = self
            .inner
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if shared.supervisor_running {
            return;
        }
        shared.supervisor_running = true;
        shared.status.state = "starting".to_string();
        shared.status.supervisor_pid = Some(std::process::id());
        drop(shared);

        let inner = self.inner.clone();
        let spawned = thread::Builder::new()
            .name("remote-access-cloudflared".to_string())
            .spawn(move || child_supervisor_loop(inner));
        if let Err(e) = spawned {
            // Roll back so a later attempt can retry instead of believing a
            // supervisor is running that never started.
            log::warn!("failed to spawn cloudflared supervisor: {e}");
            let mut shared = self
                .inner
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            shared.supervisor_running = false;
            shared.status.state = "error".to_string();
        }
    }
}

pub fn child_process_controller_from_env() -> ChildProcessRemoteAccessController {
    let bin = std::env::var("RHYTHM_CLOUDFLARED_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLOUDFLARED_BIN.to_string());
    let metrics = std::env::var("RHYTHM_CLOUDFLARED_METRICS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_METRICS_ADDR.to_string());
    let protocol = std::env::var("RHYTHM_CLOUDFLARED_PROTOCOL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLOUDFLARED_PROTOCOL.to_string());
    let loglevel = std::env::var("RHYTHM_CLOUDFLARED_LOGLEVEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLOUDFLARED_LOGLEVEL.to_string());
    let ha_connections = std::env::var("RHYTHM_CLOUDFLARED_HA_CONNECTIONS")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_CLOUDFLARED_HA_CONNECTIONS);

    ChildProcessRemoteAccessController::new(bin)
        .with_metrics_addr(metrics)
        .with_protocol(protocol)
        .with_loglevel(loglevel)
        .with_ha_connections(ha_connections)
}

impl RemoteAccessController for ChildProcessRemoteAccessController {
    fn status(&self, _runtime_dir: &Path) -> RemoteAccessRuntimeStatus {
        let cloudflared_version = cloudflared_version_for(&self.inner.cloudflared_bin);
        let status = self
            .inner
            .shared
            .lock()
            .map(|shared| shared.status.clone())
            .unwrap_or_default();
        let service_running = status.child_pid.is_some()
            && matches!(status.state.as_str(), "running" | "starting" | "backoff");
        let metrics = if service_running {
            read_metrics_status(&self.inner.metrics_addr)
        } else {
            MetricsStatus::default()
        };

        RemoteAccessRuntimeStatus {
            cloudflared_available: cloudflared_version.is_some(),
            service_available: cloudflared_version.is_some(),
            cloudflared_version,
            service_running,
            supervisor_state: Some(status.state),
            supervisor_pid: status.supervisor_pid,
            child_pid: status.child_pid,
            restart_count: status.restart_count,
            last_started_epoch_secs: status.last_started_epoch_secs,
            last_exit_epoch_secs: status.last_exit_epoch_secs,
            last_exit_code: status.last_exit_code,
            next_restart_epoch_secs: status.next_restart_epoch_secs,
            metrics_addr: Some(self.inner.metrics_addr.clone()),
            metrics_available: metrics.available,
            connector_healthy: metrics.healthy,
            registered_connections: metrics.registered_connections,
            metrics_error: metrics.error,
        }
    }

    fn start(&self, runtime_dir: &Path, config: &StoredRemoteAccessConfig) -> anyhow::Result<()> {
        if cloudflared_version_for(&self.inner.cloudflared_bin).is_none() {
            anyhow::bail!(
                "cloudflared unavailable at {}",
                self.inner.cloudflared_bin.display()
            );
        }

        {
            let mut shared = self
                .inner
                .shared
                .lock()
                .map_err(|_| anyhow::anyhow!("remote access lock"))?;
            shared.generation = shared.generation.saturating_add(1);
            shared.desired = Some(ChildDesired {
                token: config.connector_token.trim().to_string(),
                runtime_dir: runtime_dir.to_path_buf(),
            });
            shared.status.state = "starting".to_string();
            shared.status.next_restart_epoch_secs = None;
        }
        self.ensure_supervisor();
        Ok(())
    }

    fn stop(&self, runtime_dir: &Path) -> anyhow::Result<()> {
        let target_generation = self
            .inner
            .shared
            .lock()
            .map(|shared| shared.generation.saturating_add(1))
            .unwrap_or(1);
        let inner = self.inner.clone();
        let runtime_dir = runtime_dir.to_path_buf();
        thread::spawn(move || {
            thread::sleep(inner.stop_delay);
            {
                let Ok(mut shared) = inner.shared.lock() else {
                    return;
                };
                if shared.generation >= target_generation {
                    return;
                }
                shared.generation = target_generation;
                shared.desired = None;
                shared.status.state = "stopping".to_string();
                let _ = write_child_status_env(&runtime_dir, &inner, &shared.status);
            }
            kill_child(&inner);
            {
                let Ok(mut shared) = inner.shared.lock() else {
                    return;
                };
                shared.status.state = "stopped".to_string();
                shared.status.child_pid = None;
                shared.status.supervisor_pid = None;
                shared.supervisor_running = false;
                let _ = write_child_status_env(&runtime_dir, &inner, &shared.status);
            }
        });
        Ok(())
    }
}

fn child_supervisor_loop(inner: Arc<ChildProcessInner>) {
    let mut backoff = inner.backoff_initial;
    loop {
        let (desired, generation) = {
            let Ok(mut shared) = inner.shared.lock() else {
                return;
            };
            let Some(desired) = shared.desired.clone() else {
                shared.supervisor_running = false;
                shared.status.state = "stopped".to_string();
                shared.status.supervisor_pid = None;
                shared.status.child_pid = None;
                return;
            };
            shared.status.state = "starting".to_string();
            shared.status.supervisor_pid = Some(std::process::id());
            shared.status.next_restart_epoch_secs = None;
            let _ = write_child_status_env(&desired.runtime_dir, &inner, &shared.status);
            (desired, shared.generation)
        };

        let spawn_result = spawn_cloudflared(&inner, &desired);
        match spawn_result {
            Ok(child) => {
                let child_pid = child.id();
                {
                    let mut child_slot = inner.child.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    *child_slot = Some(child);
                }
                {
                    let mut shared = inner.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    shared.status.state = "running".to_string();
                    shared.status.child_pid = Some(child_pid);
                    shared.status.last_started_epoch_secs = Some(current_epoch_secs());
                    shared.status.next_restart_epoch_secs = None;
                    let _ = write_child_status_env(&desired.runtime_dir, &inner, &shared.status);
                }
                backoff = inner.backoff_initial;
            }
            Err(e) => {
                let mut shared = inner.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                shared.status.state = "backoff".to_string();
                shared.status.last_exit_epoch_secs = Some(current_epoch_secs());
                shared.status.last_exit_code = None;
                shared.status.next_restart_epoch_secs =
                    Some(current_epoch_secs() + backoff.as_secs());
                let _ = write_child_status_env(&desired.runtime_dir, &inner, &shared.status);
                log::warn!(target: "sys", "Failed to spawn cloudflared: {:#}", e);
                drop(shared);
                sleep_backoff_or_change(&inner, generation, backoff);
                backoff = (backoff * 2).min(inner.backoff_max);
                continue;
            }
        }

        loop {
            thread::sleep(Duration::from_millis(250));
            if desired_changed(&inner, generation) {
                kill_child(&inner);
                clear_child_slot(&inner);
                break;
            }

            let exit_status = {
                let mut child_slot = inner.child.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                child_slot
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten())
            };

            if let Some(exit_status) = exit_status {
                clear_child_slot(&inner);
                let should_restart = {
                    let mut shared = inner.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    shared.status.state = "exited".to_string();
                    shared.status.child_pid = None;
                    shared.status.last_exit_epoch_secs = Some(current_epoch_secs());
                    shared.status.last_exit_code = exit_status.code();
                    shared.status.restart_count = shared.status.restart_count.saturating_add(1);
                    let should_restart =
                        shared.desired.is_some() && shared.generation == generation;
                    if should_restart {
                        shared.status.state = "backoff".to_string();
                        shared.status.next_restart_epoch_secs =
                            Some(current_epoch_secs() + backoff.as_secs());
                    }
                    let _ = write_child_status_env(&desired.runtime_dir, &inner, &shared.status);
                    should_restart
                };
                if should_restart {
                    sleep_backoff_or_change(&inner, generation, backoff);
                    backoff = (backoff * 2).min(inner.backoff_max);
                }
                break;
            }
        }
    }
}

fn spawn_cloudflared(inner: &ChildProcessInner, desired: &ChildDesired) -> anyhow::Result<Child> {
    std::fs::create_dir_all(&desired.runtime_dir)?;
    let log_path = desired.runtime_dir.join("cloudflared.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let err_file = log_file.try_clone()?;

    let mut command = Command::new(&inner.cloudflared_bin);
    command
        .arg("tunnel")
        .arg("--no-autoupdate")
        .arg("--protocol")
        .arg(&inner.protocol)
        .arg("--loglevel")
        .arg(&inner.loglevel)
        .arg("--metrics")
        .arg(&inner.metrics_addr)
        .arg("--ha-connections")
        .arg(inner.ha_connections.to_string())
        .arg("run")
        .arg("--token")
        .arg(&desired.token)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file));
    Ok(command.spawn()?)
}

fn desired_changed(inner: &ChildProcessInner, generation: u64) -> bool {
    inner
        .shared
        .lock()
        .map(|shared| shared.generation != generation || shared.desired.is_none())
        .unwrap_or(true)
}

fn sleep_backoff_or_change(inner: &ChildProcessInner, generation: u64, backoff: Duration) {
    let deadline = std::time::Instant::now() + backoff;
    while std::time::Instant::now() < deadline {
        if desired_changed(inner, generation) {
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn kill_child(inner: &ChildProcessInner) {
    let mut child_slot = inner.child.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(child) = child_slot.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    *child_slot = None;
}

fn clear_child_slot(inner: &ChildProcessInner) {
    let mut child_slot = inner.child.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *child_slot = None;
}

fn write_child_status_env(
    runtime_dir: &Path,
    inner: &ChildProcessInner,
    status: &ChildSupervisorStatus,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(runtime_dir)?;
    let path = runtime_dir.join(STATUS_FILE);
    let tmp = runtime_dir.join(format!(".{STATUS_FILE}.{}.tmp", current_epoch_ms()));
    {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)?;
        writeln!(file, "state={}", status.state)?;
        writeln!(
            file,
            "supervisor_pid={}",
            status
                .supervisor_pid
                .map(|pid| pid.to_string())
                .unwrap_or_default()
        )?;
        writeln!(
            file,
            "child_pid={}",
            status
                .child_pid
                .map(|pid| pid.to_string())
                .unwrap_or_default()
        )?;
        writeln!(file, "restart_count={}", status.restart_count)?;
        writeln!(
            file,
            "last_started_epoch_secs={}",
            status
                .last_started_epoch_secs
                .map(|value| value.to_string())
                .unwrap_or_default()
        )?;
        writeln!(
            file,
            "last_exit_epoch_secs={}",
            status
                .last_exit_epoch_secs
                .map(|value| value.to_string())
                .unwrap_or_default()
        )?;
        writeln!(
            file,
            "last_exit_code={}",
            status
                .last_exit_code
                .map(|value| value.to_string())
                .unwrap_or_default()
        )?;
        writeln!(
            file,
            "next_restart_epoch_secs={}",
            status
                .next_restart_epoch_secs
                .map(|value| value.to_string())
                .unwrap_or_default()
        )?;
        writeln!(file, "metrics_addr={}", inner.metrics_addr)?;
        writeln!(file, "protocol={}", inner.protocol)?;
        writeln!(file, "loglevel={}", inner.loglevel)?;
        writeln!(file, "ha_connections={}", inner.ha_connections)?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use crate::storage::{FileStorage, Storage};

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rhythm-remote-{name}-{}-{}",
            std::process::id(),
            current_epoch_ms()
        ))
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn normalize_hostname_accepts_urls_and_hosts() {
        assert_eq!(
            normalize_hostname(Some("https://ABC.devices.rhythm.lighting/path")).unwrap(),
            "abc.devices.rhythm.lighting"
        );
        assert_eq!(
            normalize_hostname(Some("hub.devices.rhythm.lighting")).unwrap(),
            "hub.devices.rhythm.lighting"
        );
    }

    #[test]
    fn normalize_hostname_rejects_empty_and_whitespace() {
        assert!(normalize_hostname(None).is_err());
        assert!(normalize_hostname(Some(" ")).is_err());
        assert!(normalize_hostname(Some("bad host")).is_err());
    }

    #[test]
    fn parses_supervisor_status_file() {
        let status = parse_status_env(
            "state=backoff\nrestart_count=3\nlast_exit_code=1\nmetrics_addr=127.0.0.1:54449\n",
        );

        assert_eq!(status.get("state").map(String::as_str), Some("backoff"));
        assert_eq!(status.get("restart_count").map(String::as_str), Some("3"));
        assert_eq!(
            status.get("metrics_addr").map(String::as_str),
            Some("127.0.0.1:54449")
        );
    }

    #[test]
    fn parses_registered_connections_from_cloudflared_metrics() {
        let metrics = "\
# HELP cloudflared_tunnel_ha_connections Number of HA connections\n\
cloudflared_tunnel_ha_connections 1\n";
        assert_eq!(parse_registered_connections(metrics), Some(1));

        let location_metrics = "\
cloudflared_tunnel_server_locations{edge_location=\"iad01\"} 1\n\
cloudflared_tunnel_server_locations{edge_location=\"ewr01\"} 1\n";
        assert_eq!(parse_registered_connections(location_metrics), Some(2));
    }

    #[tokio::test]
    async fn put_config_persists_secret_and_redacts_response() {
        let root = temp_root("put");
        std::fs::create_dir_all(&root).unwrap();
        let storage = FileStorage::new(root.to_str().unwrap()).unwrap();
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        {
            let mut state = state.lock().unwrap();
            state.data_dir = root.to_string_lossy().to_string();
            state.storage = Some(std::sync::Arc::new(storage));
        }

        let response = put_config(
            State(state.clone()),
            Json(PutRemoteAccessConfig {
                enabled: true,
                hostname: Some("https://Hub.devices.rhythm.lighting".into()),
                remote_url: None,
                connector_token: Some("connector-secret".into()),
                tunnel_id: Some("tunnel-id".into()),
                tunnel_name: Some("tunnel-name".into()),
            }),
        )
        .await;

        assert_eq!(response.status, 200);
        assert!(!response.body.contains("connector-secret"));
        assert!(response.body.contains("hub.devices.rhythm.lighting"));
        assert!(response
            .body
            .contains("remote access controller not configured"));

        let storage = FileStorage::new(root.to_str().unwrap()).unwrap();
        let config = storage.load_remote_access_config().unwrap().unwrap();
        assert_eq!(config.connector_token, "connector-secret");
        assert_eq!(config.hostname, "hub.devices.rhythm.lighting");
        assert_eq!(
            std::fs::read_to_string(root.join("cloudflared").join("connector_token"))
                .unwrap()
                .trim(),
            "connector-secret"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn init_script_start_is_nonblocking() {
        let root = temp_root("init-script-start");
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("cloudflared-service");
        let marker = root.join("marker");
        write_executable(
            &script,
            r#"#!/bin/sh
dir="$(dirname "$0")"
sleep 1
printf '%s\n' "$1" > "$dir/marker"
"#,
        );

        let controller =
            InitScriptRemoteAccessController::new("/bin/true", &script, "/tmp/pid", "/tmp/child");
        let started = std::time::Instant::now();
        controller
            .start(
                &root,
                &StoredRemoteAccessConfig {
                    schema_version: 1,
                    enabled: true,
                    hostname: "hub.devices.rhythm.lighting".into(),
                    connector_token: "secret".into(),
                    tunnel_id: None,
                    tunnel_name: None,
                    updated_at_epoch_ms: current_epoch_ms(),
                },
            )
            .unwrap();

        assert!(
            started.elapsed() < Duration::from_millis(250),
            "init script start should not block API request"
        );

        for _ in 0..20 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(std::fs::read_to_string(marker).unwrap().trim(), "restart");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn child_process_controller_reports_missing_binary() {
        let controller = ChildProcessRemoteAccessController::new("/definitely/not/cloudflared");
        let status = controller.status(Path::new("/tmp"));

        assert!(!status.cloudflared_available);
        assert!(!status.service_available);
        assert_eq!(status.supervisor_state.as_deref(), Some("stopped"));
    }
}
