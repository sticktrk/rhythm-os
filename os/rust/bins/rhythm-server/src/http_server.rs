//! Axum HTTP server — uses shared routes from rhythm-os plus server-specific endpoints.

use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::StatusCode;
use serde::Serialize;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rhythm_os::handlers::ApiResponse;
use rhythm_os::logging;
use rhythm_os::mdns::MDNS_HOSTNAME_PREFIX;
use rhythm_os::state::SharedState;
use tower_http::cors::CorsLayer;

/// Create the Axum router with all API routes.
pub fn create_router(state: SharedState) -> Router {
    let ota_status = crate::self_update::OtaStatusHandle::new(crate::BUILD_VERSION);
    let auth_state = state.clone();

    logging::with_http_observability(
        rhythm_os::axum_router::api_routes()
            // Server-specific endpoints
            .route("/api/discover", get(discover))
            .route("/api/diag/debug-bundle", post(debug_bundle))
            .route("/api/diag/reset-matter-fabric", post(reset_matter_fabric))
            .route(
                "/api/ota/capabilities",
                get({
                    let ota_status = ota_status.clone();
                    move || ota_capabilities(ota_status.clone())
                }),
            )
            .route(
                "/api/ota/status",
                get({
                    let ota_status = ota_status.clone();
                    move || ota_status_snapshot(ota_status.clone())
                }),
            )
            .route(
                "/api/ota/check",
                get({
                    let ota_status = ota_status.clone();
                    move |State(state): State<SharedState>| {
                        let ota_status = ota_status.clone();
                        async move { check_update(state, ota_status).await }
                    }
                }),
            )
            .route(
                "/api/ota/update",
                post({
                    let ota_status = ota_status.clone();
                    move |State(state): State<SharedState>| {
                        let ota_status = ota_status.clone();
                        async move { do_update(state, ota_status).await }
                    }
                }),
            )
            .route("/api/restart", post(restart_device))
            .with_state(state)
            .layer(middleware::from_fn_with_state(
                auth_state,
                rhythm_os::auth::require_api_auth_middleware,
            ))
            .layer(CorsLayer::permissive()),
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn json_ok(body: String) -> Response {
    ApiResponse::json_ok(body).into_response()
}

fn json_status(status: StatusCode, body: serde_json::Value) -> Response {
    (
        status,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn err_500(e: impl std::fmt::Display) -> Response {
    ApiResponse::server_error(e).into_response()
}

fn err_409(message: impl Into<String>) -> Response {
    json_status(
        StatusCode::CONFLICT,
        serde_json::json!({
            "status": "error",
            "message": message.into(),
        }),
    )
}

fn tar_gz_attachment(filename: &str, body: Vec<u8>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/gzip")
        .header(
            "content-disposition",
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(Body::from(body))
        .unwrap_or_else(|e| ApiResponse::server_error(e).into_response())
}

#[allow(clippy::too_many_arguments)]
fn emit_ota_progress(
    state: &SharedState,
    stage: rhythm_os::server_event::OtaUpdateStage,
    message: impl Into<String>,
    current_version: Option<String>,
    target_version: Option<String>,
    update_available: Option<bool>,
    downloaded_bytes: Option<u64>,
    total_bytes: Option<u64>,
    checksum_verified: Option<bool>,
    installed_targets: Vec<String>,
    error: Option<String>,
) {
    let percent = match (downloaded_bytes, total_bytes) {
        (Some(downloaded), Some(total)) if total > 0 => {
            let percent = downloaded
                .saturating_mul(100)
                .saturating_div(total)
                .min(100);
            Some(percent as u8)
        }
        _ => None,
    };

    rhythm_os::state::emit_server_event(
        state,
        rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
            stage,
            message: message.into(),
            current_version,
            target_version,
            update_available,
            downloaded_bytes,
            total_bytes,
            percent,
            checksum_verified,
            installed_targets,
            error,
        },
    );
}

fn emit_ota_apply_progress(
    state: &SharedState,
    current_version: &str,
    target_version: &str,
    progress: crate::self_update::UpdateProgress,
) {
    emit_ota_progress(
        state,
        progress.stage,
        progress.message,
        Some(current_version.to_string()),
        Some(target_version.to_string()),
        Some(true),
        progress.downloaded_bytes,
        progress.total_bytes,
        None,
        Vec::new(),
        None,
    );
}

// ---------------------------------------------------------------------------
// Server-specific handlers
// ---------------------------------------------------------------------------

async fn debug_bundle(State(state): State<SharedState>) -> Response {
    let started_at = std::time::Instant::now();
    match tokio::task::spawn_blocking(move || crate::debug_bundle::build_debug_bundle(&state)).await
    {
        Ok(Ok(bundle)) => {
            log::info!(
                target: "http",
                "Generated debug bundle {} ({} bytes) in {} ms",
                bundle.file_name,
                bundle.bytes.len(),
                started_at.elapsed().as_millis()
            );
            tar_gz_attachment(&bundle.file_name, bundle.bytes)
        }
        Ok(Err(e)) => {
            log::error!(
                target: "http",
                "Debug bundle generation failed after {} ms: {}",
                started_at.elapsed().as_millis(),
                e
            );
            err_500(e)
        }
        Err(e) => {
            log::error!(
                target: "http",
                "Debug bundle worker join failed after {} ms: {}",
                started_at.elapsed().as_millis(),
                e
            );
            err_500(e)
        }
    }
}

async fn check_update(
    state: SharedState,
    ota_status: crate::self_update::OtaStatusHandle,
) -> Response {
    let version = crate::BUILD_VERSION;
    ota_status.mark_checking();
    emit_ota_progress(
        &state,
        rhythm_os::server_event::OtaUpdateStage::Checking,
        "Checking for updates",
        Some(version.to_string()),
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
    );
    let channel = crate::self_update::channel_from_state(&state);
    match tokio::task::spawn_blocking(move || crate::self_update::check_blocking(version, channel))
        .await
    {
        Ok(Ok(info)) => {
            ota_status.record_check_result(&info);
            let (stage, message) = if info.update_available {
                (
                    rhythm_os::server_event::OtaUpdateStage::UpdateAvailable,
                    match info.update_reason {
                        Some(crate::self_update::UpdateReason::ComponentDrift) => {
                            format!("Repair update available for v{}", info.latest_version)
                        }
                        _ => format!("Update available: v{}", info.latest_version),
                    },
                )
            } else {
                (
                    rhythm_os::server_event::OtaUpdateStage::UpToDate,
                    "Already up to date".to_string(),
                )
            };
            emit_ota_progress(
                &state,
                stage,
                message,
                Some(info.current_version.clone()),
                Some(info.latest_version.clone()),
                Some(info.update_available),
                None,
                None,
                None,
                Vec::new(),
                None,
            );
            let json = serde_json::json!({
                "current_version": info.current_version,
                "latest_version": info.latest_version,
                "update_available": info.update_available,
                "update_reason": info.update_reason,
                "install_targets": info.install_targets,
                "image_assets": info.image_assets,
            });
            json_ok(json.to_string())
        }
        Ok(Err(e)) => {
            ota_status.mark_error(e.clone());
            emit_ota_progress(
                &state,
                rhythm_os::server_event::OtaUpdateStage::Failed,
                "Update check failed",
                Some(version.to_string()),
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                Some(e.clone()),
            );
            err_500(e)
        }
        Err(e) => {
            ota_status.mark_error(e.to_string());
            emit_ota_progress(
                &state,
                rhythm_os::server_event::OtaUpdateStage::Failed,
                "Update check failed",
                Some(version.to_string()),
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                Some(e.to_string()),
            );
            err_500(e)
        }
    }
}

async fn ota_capabilities(ota_status: crate::self_update::OtaStatusHandle) -> Response {
    match serde_json::to_value(ota_status.capabilities()) {
        Ok(value) => json_status(StatusCode::OK, value),
        Err(e) => err_500(e),
    }
}

async fn ota_status_snapshot(ota_status: crate::self_update::OtaStatusHandle) -> Response {
    match serde_json::to_value(ota_status.snapshot()) {
        Ok(value) => json_status(StatusCode::OK, value),
        Err(e) => err_500(e),
    }
}

/// Scan the local network for rhythm devices via mDNS (~3s).
async fn discover() -> Json<serde_json::Value> {
    let results = tokio::task::spawn_blocking(scan_mdns)
        .await
        .unwrap_or_default();
    Json(serde_json::Value::Array(results))
}

fn scan_mdns() -> Vec<serde_json::Value> {
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("mDNS daemon error: {:?}", e);
            return vec![];
        }
    };

    let service_type = "_http._tcp.local.";
    let receiver = match daemon.browse(service_type) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("mDNS browse error: {:?}", e);
            return vec![];
        }
    };

    let mut devices = vec![];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);

    while std::time::Instant::now() < deadline {
        match receiver.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let fullname = info.get_fullname().to_string();
                let hostname = info.get_hostname().to_string();

                let is_rhythm = hostname.starts_with(MDNS_HOSTNAME_PREFIX)
                    || fullname.to_lowercase().contains("rhythm");
                if !is_rhythm {
                    continue;
                }

                let addr = match info
                    .get_addresses()
                    .iter()
                    .find(|a| matches!(a, IpAddr::V4(_)))
                {
                    Some(a) => a.to_string(),
                    None => continue,
                };

                let device_type = info
                    .get_properties()
                    .get("type")
                    .map(|v| v.val_str().to_string())
                    .unwrap_or_default();

                devices.push(serde_json::json!({
                    "name": fullname,
                    "host": hostname,
                    "address": addr,
                    "port": info.get_port(),
                    "type": device_type,
                }));
            }
            Ok(_) => {}
            Err(_) => continue,
        }
    }

    let _ = daemon.stop_browse(service_type);
    let _ = daemon.shutdown();

    // Dedup: same server on multiple network interfaces produces separate
    // mDNS instances (different IPs/hostnames) but identical port.
    let mut seen_ports = std::collections::HashSet::new();
    devices.retain(|d| {
        let port = d["port"].as_u64().unwrap_or(0);
        seen_ports.insert(port)
    });

    devices
}

async fn restart_device(State(state): State<SharedState>) -> Response {
    log::info!(target: "http", "Restart requested via /api/restart");
    crate::self_update::persist_before_restart(&state);
    crate::self_update::schedule_user_initiated_restart();
    json_ok(r#"{"status":"ok","message":"Restart scheduled"}"#.to_string())
}

#[derive(Debug, Serialize)]
struct MatterFabricResetSummary {
    status: &'static str,
    message: &'static str,
    matter_dir: String,
    removed_matter_state: bool,
    removed_matter_registry: bool,
    restart_scheduled: bool,
}

async fn reset_matter_fabric(State(state): State<SharedState>) -> Response {
    log::warn!(target: "http", "Matter fabric reset requested via undocumented diag endpoint");
    let reset_state = state.clone();
    let summary =
        match tokio::task::spawn_blocking(move || reset_matter_fabric_state(&reset_state)).await {
            Ok(Ok(summary)) => summary,
            Ok(Err(error)) => return err_500(error),
            Err(error) => return err_500(format!("Matter fabric reset task failed: {error}")),
        };

    crate::self_update::persist_before_restart(&state);
    crate::self_update::schedule_user_initiated_restart();

    match serde_json::to_string(&summary) {
        Ok(json) => json_ok(json),
        Err(error) => err_500(error),
    }
}

fn reset_matter_fabric_state(state: &SharedState) -> Result<MatterFabricResetSummary> {
    let data_dir = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if state.data_dir.trim().is_empty() {
            anyhow::bail!("data_dir not configured on AppState");
        }
        PathBuf::from(&state.data_dir)
    };

    rhythm_os::commands::do_hub_disconnect_one(state, rhythm_os::hub::HubType::MATTER, "local")
        .context("disconnecting Matter hub before fabric reset")?;

    let matter_dir = data_dir.join("matter");
    let matter_registry = data_dir.join("hub_registry_matter_local.json");
    let removed_matter_state = remove_dir_if_exists(&matter_dir)?;
    let removed_matter_registry = remove_file_if_exists(&matter_registry)?;

    Ok(MatterFabricResetSummary {
        status: "ok",
        message: "Matter fabric reset; restart scheduled",
        matter_dir: matter_dir.display().to_string(),
        removed_matter_state,
        removed_matter_registry,
        restart_scheduled: true,
    })
}

fn remove_dir_if_exists(path: &Path) -> Result<bool> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

fn remove_file_if_exists(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

async fn do_update(
    state: SharedState,
    ota_status: crate::self_update::OtaStatusHandle,
) -> Response {
    let snapshot = ota_status.snapshot();
    if matches!(
        snapshot.state,
        crate::self_update::OtaUpdateState::Updating
            | crate::self_update::OtaUpdateState::Restarting
    ) {
        return err_409("Update already in progress");
    }

    let version = crate::BUILD_VERSION;
    ota_status.mark_checking();
    emit_ota_progress(
        &state,
        rhythm_os::server_event::OtaUpdateStage::Checking,
        "Checking for updates",
        Some(version.to_string()),
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
    );

    // Check for update
    let channel = crate::self_update::channel_from_state(&state);
    let info = match tokio::task::spawn_blocking(move || {
        crate::self_update::check_blocking(version, channel)
    })
    .await
    {
        Ok(Ok(info)) => info,
        Ok(Err(e)) => {
            ota_status.mark_error(e.clone());
            emit_ota_progress(
                &state,
                rhythm_os::server_event::OtaUpdateStage::Failed,
                "Update check failed",
                Some(version.to_string()),
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                Some(e.clone()),
            );
            return err_500(e);
        }
        Err(e) => {
            ota_status.mark_error(e.to_string());
            emit_ota_progress(
                &state,
                rhythm_os::server_event::OtaUpdateStage::Failed,
                "Update check failed",
                Some(version.to_string()),
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                Some(e.to_string()),
            );
            return err_500(e);
        }
    };
    ota_status.record_check_result(&info);

    if !info.update_available {
        emit_ota_progress(
            &state,
            rhythm_os::server_event::OtaUpdateStage::UpToDate,
            "Already up to date",
            Some(info.current_version.clone()),
            Some(info.latest_version.clone()),
            Some(false),
            None,
            None,
            None,
            Vec::new(),
            None,
        );
        return json_ok(r#"{"status":"ok","message":"Already up to date"}"#.to_string());
    }
    emit_ota_progress(
        &state,
        rhythm_os::server_event::OtaUpdateStage::UpdateAvailable,
        format!("Update available: v{}", info.latest_version),
        Some(info.current_version.clone()),
        Some(info.latest_version.clone()),
        Some(true),
        None,
        None,
        None,
        Vec::new(),
        None,
    );

    if let Err(e) = ota_status.begin_update(&info.latest_version) {
        return err_409(e);
    }

    let latest = info.latest_version.clone();
    let previous = info.current_version.clone();
    let apply_state = state.clone();
    let apply_previous = previous.clone();
    let apply_latest = latest.clone();

    // Download and install
    let apply_result = match tokio::task::spawn_blocking(move || {
        info.apply_blocking_with_progress(move |progress| {
            emit_ota_apply_progress(&apply_state, &apply_previous, &apply_latest, progress);
        })
    })
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            ota_status.mark_error(e.clone());
            emit_ota_progress(
                &state,
                rhythm_os::server_event::OtaUpdateStage::Failed,
                "Update failed",
                Some(previous.clone()),
                Some(latest.clone()),
                Some(true),
                None,
                None,
                None,
                Vec::new(),
                Some(e.clone()),
            );
            return err_500(e);
        }
        Err(e) => {
            ota_status.mark_error(e.to_string());
            emit_ota_progress(
                &state,
                rhythm_os::server_event::OtaUpdateStage::Failed,
                "Update failed",
                Some(previous.clone()),
                Some(latest.clone()),
                Some(true),
                None,
                None,
                None,
                Vec::new(),
                Some(e.to_string()),
            );
            return err_500(e);
        }
    };
    ota_status.mark_restarting(&previous, &latest, apply_result.checksum_verified);
    emit_ota_progress(
        &state,
        rhythm_os::server_event::OtaUpdateStage::Restarting,
        format!("Updated to v{}, restarting...", latest),
        Some(previous.clone()),
        Some(latest.clone()),
        Some(false),
        None,
        None,
        apply_result.checksum_verified,
        apply_result.installed_targets.clone(),
        None,
    );

    crate::self_update::persist_before_restart(&state);
    crate::self_update::schedule_post_update_restart();

    json_ok(format!(
        r#"{{"status":"ok","message":"Updated to v{}, restarting...","previous_version":"{}","new_version":"{}","checksum_verified":{},"installed_targets":{}}}"#,
        latest,
        previous,
        latest,
        apply_result
            .checksum_verified
            .map(serde_json::Value::Bool)
            .unwrap_or(serde_json::Value::Null),
        serde_json::to_string(&apply_result.installed_targets).unwrap_or_else(|_| "[]".to_string())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use rhythm_os::state::AppState;
    use rhythm_os::storage::FileStorage;
    use std::sync::{Arc, Mutex, Once};
    use tower::ServiceExt;

    static DRY_RUN_INIT: Once = Once::new();

    // The restart helper spawns a thread that sleeps 1s then either reboots the
    // device or calls process::exit(1). Setting this env var keeps the spawned
    // thread inert so tests can exercise the route without nuking the runner.
    fn init_restart_dry_run() {
        DRY_RUN_INIT.call_once(|| std::env::set_var("RHYTHM_RESTART_DRY_RUN", "1"));
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rhythm_server_{}_{}_{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[tokio::test]
    async fn emit_ota_progress_broadcasts_percent_payload() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().event_tx = Some(tx);

        emit_ota_progress(
            &state,
            rhythm_os::server_event::OtaUpdateStage::Downloading,
            "Downloading update bundle",
            Some("0.4.192-beta".to_string()),
            Some("0.4.193-beta".to_string()),
            Some(true),
            Some(25),
            Some(100),
            None,
            Vec::new(),
            None,
        );

        let event = rx.recv().await.expect("ota progress event");
        match event {
            rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                stage,
                message,
                current_version,
                target_version,
                update_available,
                downloaded_bytes,
                total_bytes,
                percent,
                error,
                ..
            } => {
                assert_eq!(stage, rhythm_os::server_event::OtaUpdateStage::Downloading);
                assert_eq!(message, "Downloading update bundle");
                assert_eq!(current_version.as_deref(), Some("0.4.192-beta"));
                assert_eq!(target_version.as_deref(), Some("0.4.193-beta"));
                assert_eq!(update_available, Some(true));
                assert_eq!(downloaded_bytes, Some(25));
                assert_eq!(total_bytes, Some(100));
                assert_eq!(percent, Some(25));
                assert_eq!(error, None);
            }
            other => panic!("expected OTA progress event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn restart_endpoint_returns_ok_and_schedules_restart() {
        init_restart_dry_run();

        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let router = create_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/api/restart")
            .body(axum::body::Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "POST /api/restart must be registered and return 200"
        );

        let body_bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["message"], "Restart scheduled");
    }

    #[tokio::test]
    async fn reset_matter_fabric_endpoint_removes_matter_state_and_schedules_restart() {
        init_restart_dry_run();

        let data_dir = unique_test_dir("matter-reset");
        let matter_chip_dir = data_dir.join("matter").join("chip");
        std::fs::create_dir_all(&matter_chip_dir).unwrap();
        std::fs::write(
            data_dir.join("matter").join("fabric-identity.json"),
            br#"{"schema_version":1,"label":"default","operational_fabric_id":100,"ipk_hex":"00112233445566778899aabbccddeeff"}"#,
        )
        .unwrap();
        std::fs::write(
            matter_chip_dir.join("chip_tool_config.controller-storage.ini"),
            b"chip-storage",
        )
        .unwrap();
        std::fs::write(data_dir.join("hub_registry_matter_local.json"), b"{}").unwrap();

        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        {
            let mut state = state.lock().unwrap();
            state.data_dir = data_dir.display().to_string();
            state.storage = Some(Box::new(FileStorage::new(&state.data_dir).unwrap()));
        }
        let router = create_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/api/diag/reset-matter-fabric")
            .body(axum::body::Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "POST /api/diag/reset-matter-fabric must be registered and return 200"
        );

        let body_bytes = to_bytes(response.into_body(), 2048).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["removed_matter_state"], true);
        assert_eq!(body["removed_matter_registry"], true);
        assert_eq!(body["restart_scheduled"], true);
        assert!(
            !data_dir.join("matter").exists(),
            "Matter fabric state directory should be removed"
        );
        assert!(
            !data_dir.join("hub_registry_matter_local.json").exists(),
            "Matter hub registry snapshot should be removed"
        );

        let _ = std::fs::remove_dir_all(data_dir);
    }
}
