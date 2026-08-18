//! Axum HTTP server — uses shared routes from rhythm-os plus server-specific endpoints.

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use serde::Deserialize;
use serde::Serialize;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::middleware;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{self, Stream};
use rhythm_os::handlers::ApiResponse;
use rhythm_os::logging;
use rhythm_os::mdns::MDNS_HOSTNAME_PREFIX;
use rhythm_os::state::SharedState;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
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
            .route("/api/diag/logs", get(list_logs))
            .route("/api/diag/logs/:source/tail", get(tail_log))
            .route("/api/diag/logs/:source/stream", get(stream_log))
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
                    move |State(state): State<SharedState>| {
                        ota_status_snapshot(state, ota_status.clone())
                    }
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
            .layer(CorsLayer::permissive())
            .layer(middleware::from_fn(record_http_activity)),
    )
}

async fn record_http_activity(request: axum::http::Request<Body>, next: Next) -> Response {
    crate::boot_diagnostics::record_http_request();
    next.run(request).await
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

#[derive(Debug, Deserialize)]
struct LogTailQuery {
    lines: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct LogStreamQuery {
    lines: Option<usize>,
    poll_ms: Option<u64>,
    max_seconds: Option<u64>,
}

struct LogStreamState {
    path: PathBuf,
    source_id: String,
    offset: u64,
    next_line_number: Option<usize>,
    pending: VecDeque<Event>,
    partial: String,
    poll_interval: Duration,
    deadline: Instant,
}

/// Optional body for POST /api/diag/debug-bundle. Legacy clients post no
/// body and download the tar.gz; newer apps pass `upload_url` (a signed
/// storage upload URL) so the device streams the bundle to storage directly
/// — large bundles used to time out the app-side download because the whole
/// archive had to round-trip through the phone first. `app_log` rides along
/// so the device can embed it (the app can no longer append it post-hoc).
#[derive(Debug, Default, serde::Deserialize)]
struct DebugBundleRequest {
    upload_url: Option<String>,
    app_log: Option<String>,
    app_metadata: Option<serde_json::Value>,
    async_submission: Option<AsyncDebugBundleRequest>,
}

#[derive(Debug, serde::Deserialize)]
struct AsyncDebugBundleRequest {
    submission_id: String,
    completion_url: String,
    completion_token: String,
}

/// The upload URL comes from the authenticated app, but the device still
/// refuses to POST its diagnostics anywhere unencrypted — except loopback,
/// which local development and the handler tests rely on.
fn upload_url_is_acceptable(url: &str) -> bool {
    crate::support_bundle_jobs::outbound_url_is_acceptable(url)
}

async fn debug_bundle(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Option<Json<DebugBundleRequest>>,
) -> Response {
    let started_at = std::time::Instant::now();
    if body.is_none() && request_declares_debug_bundle_body(&headers) {
        log::warn!(
            target: "support_bundle",
            "Rejected an unreadable debug bundle request body before collection"
        );
        return json_status(
            StatusCode::BAD_REQUEST,
            serde_json::json!({
                "status": "error",
                "message": "debug bundle request body was incomplete or invalid JSON",
            }),
        );
    }
    let request = body.map(|Json(request)| request).unwrap_or_default();

    if let Some(upload_url) = request.upload_url.as_deref() {
        if !upload_url_is_acceptable(upload_url) {
            return json_status(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "status": "error",
                    "message": "upload_url must be an https URL (or http to loopback)",
                }),
            );
        }
    }

    if let Some(async_submission) = request.async_submission.as_ref() {
        let Some(upload_url) = request.upload_url.clone() else {
            return json_status(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "status": "error",
                    "message": "async debug bundle submission requires upload_url",
                }),
            );
        };
        if !crate::support_bundle_jobs::outbound_url_is_acceptable(&async_submission.completion_url)
        {
            return json_status(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "status": "error",
                    "message": "completion_url must be an https URL (or http to loopback)",
                }),
            );
        }
        let job = crate::support_bundle_jobs::NewSupportBundleJob {
            submission_id: async_submission.submission_id.clone(),
            upload_url,
            completion_url: async_submission.completion_url.clone(),
            completion_token: async_submission.completion_token.clone(),
            app_log: request.app_log.clone(),
            app_metadata: request.app_metadata.clone(),
        };
        return match crate::support_bundle_jobs::enqueue(state, job) {
            Ok(outcome) => json_status(
                StatusCode::ACCEPTED,
                serde_json::json!({
                    "status": "queued",
                    "queued": true,
                    "already_queued": outcome
                        == crate::support_bundle_jobs::EnqueueOutcome::AlreadyQueued,
                    "submission_id": async_submission.submission_id,
                }),
            ),
            Err(error) => {
                log::error!(
                    target: "support_bundle",
                    "Failed to durably queue support bundle {}: {}",
                    async_submission.submission_id,
                    error
                );
                json_status(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({
                        "status": "error",
                        "message": "failed to durably queue debug bundle",
                    }),
                )
            }
        };
    }

    let app_log = request
        .app_log
        .map(|log_text| crate::debug_bundle::AppLogAttachment {
            log_text,
            metadata: request.app_metadata,
        });

    let build_state = state.clone();
    let bundle = match tokio::task::spawn_blocking(move || {
        crate::debug_bundle::build_debug_bundle_with_app_log(&build_state, app_log)
    })
    .await
    {
        Ok(Ok(bundle)) => bundle,
        Ok(Err(e)) => {
            log::error!(
                target: "http",
                "Debug bundle generation failed after {} ms: {}",
                started_at.elapsed().as_millis(),
                e
            );
            return err_500(e);
        }
        Err(e) => {
            log::error!(
                target: "http",
                "Debug bundle generation panicked after {} ms: {}",
                started_at.elapsed().as_millis(),
                e
            );
            return err_500(e);
        }
    };

    let Some(upload_url) = request.upload_url else {
        log::info!(
            target: "http",
            "Generated debug bundle {} ({} bytes) in {} ms",
            bundle.file_name,
            bundle.bytes.len(),
            started_at.elapsed().as_millis()
        );
        return tar_gz_attachment(&bundle.file_name, bundle.bytes);
    };

    let upload_file_name = bundle.file_name;
    let bundle_bytes = bundle.bytes;
    let size_bytes = bundle_bytes.len();
    let upload_result =
        crate::support_bundle_jobs::upload_bundle_to_signed_url(&upload_url, bundle_bytes).await;

    match upload_result {
        Ok(()) => {
            log::info!(
                target: "http",
                "Uploaded debug bundle {} ({} bytes) directly to storage in {} ms",
                upload_file_name,
                size_bytes,
                started_at.elapsed().as_millis()
            );
            json_status(
                StatusCode::OK,
                serde_json::json!({
                    "uploaded": true,
                    "file_name": upload_file_name,
                    "size_bytes": size_bytes,
                }),
            )
        }
        Err(e) => {
            log::error!(
                target: "http",
                "Debug bundle direct upload failed after {} ms: {:#}",
                started_at.elapsed().as_millis(),
                e
            );
            json_status(
                StatusCode::BAD_GATEWAY,
                serde_json::json!({
                    "status": "error",
                    "uploaded": false,
                    "message": format!("debug bundle direct upload failed: {e:#}"),
                }),
            )
        }
    }
}

fn request_declares_debug_bundle_body(headers: &HeaderMap) -> bool {
    let json_content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/json"));
    let nonempty_content_length = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > 0);
    let chunked = headers.contains_key(axum::http::header::TRANSFER_ENCODING);
    json_content_type || nonempty_content_length || chunked
}

async fn list_logs(State(state): State<SharedState>) -> Response {
    match tokio::task::spawn_blocking(move || crate::debug_bundle::list_log_sources(&state)).await {
        Ok(Ok(sources)) => json_ok(
            serde_json::json!({
                "status": "ok",
                "sources": sources,
            })
            .to_string(),
        ),
        Ok(Err(e)) => err_500(e),
        Err(e) => err_500(e),
    }
}

async fn tail_log(
    State(state): State<SharedState>,
    AxumPath(source): AxumPath<String>,
    Query(query): Query<LogTailQuery>,
) -> Response {
    let lines = query.lines.unwrap_or(500);
    match tokio::task::spawn_blocking(move || {
        crate::debug_bundle::tail_log_source(&state, &source, lines)
    })
    .await
    {
        Ok(Ok(tail)) => json_ok(
            serde_json::json!({
                "status": "ok",
                "tail": tail,
            })
            .to_string(),
        ),
        Ok(Err(e)) if e.to_string().contains("log source not found") => json_status(
            StatusCode::NOT_FOUND,
            serde_json::json!({
                "status": "error",
                "message": "Log source not found",
            }),
        ),
        Ok(Err(e)) => err_500(e),
        Err(e) => err_500(e),
    }
}

async fn stream_log(
    State(state): State<SharedState>,
    AxumPath(source): AxumPath<String>,
    Query(query): Query<LogStreamQuery>,
) -> Response {
    let lines = query.lines.unwrap_or(200);
    let poll_interval = Duration::from_millis(query.poll_ms.unwrap_or(1000).clamp(250, 10_000));
    let max_seconds = query.max_seconds.unwrap_or(30 * 60).clamp(10, 60 * 60);

    let prepared = tokio::task::spawn_blocking(move || {
        let Some(resolved) = crate::debug_bundle::resolve_log_source(&state, &source)? else {
            anyhow::bail!("log source not found");
        };
        let requested_lines = crate::debug_bundle::clamp_log_tail_lines(lines);
        let tail = crate::debug_bundle::read_log_tail_lines(
            &resolved.path,
            &resolved.source.id,
            requested_lines,
        )?;
        let offset = std::fs::metadata(&resolved.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let next_line_number = Some(tail.last().map(|line| line.line_number + 1).unwrap_or(1));
        let pending = tail
            .into_iter()
            .map(|line| log_sse_event(&line.source, Some(line.line_number), &line.text))
            .collect::<VecDeque<_>>();
        Ok::<_, anyhow::Error>((resolved, offset, next_line_number, pending))
    })
    .await;

    let (resolved, offset, next_line_number, pending) = match prepared {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(e)) if e.to_string().contains("log source not found") => {
            return json_status(
                StatusCode::NOT_FOUND,
                serde_json::json!({
                    "status": "error",
                    "message": "Log source not found",
                }),
            );
        }
        Ok(Err(e)) => return err_500(e),
        Err(e) => return err_500(e),
    };

    let stream_state = LogStreamState {
        path: resolved.path,
        source_id: resolved.source.id,
        offset,
        next_line_number,
        pending,
        partial: String::new(),
        poll_interval,
        deadline: Instant::now() + Duration::from_secs(max_seconds),
    };
    Sse::new(log_event_stream(stream_state))
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn log_event_stream(
    state: LogStreamState,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    stream::unfold(state, |mut state| async move {
        loop {
            if let Some(event) = state.pending.pop_front() {
                return Some((Ok(event), state));
            }

            if Instant::now() >= state.deadline {
                return None;
            }

            tokio::time::sleep(state.poll_interval).await;
            if let Err(error) = read_new_log_events(&mut state).await {
                let event = Event::default().event("error").data(
                    serde_json::json!({
                        "source": state.source_id,
                        "message": error.to_string(),
                    })
                    .to_string(),
                );
                return Some((Ok(event), state));
            }
        }
    })
}

async fn read_new_log_events(state: &mut LogStreamState) -> Result<()> {
    let metadata = tokio::fs::metadata(&state.path).await?;
    if metadata.len() < state.offset {
        state.offset = 0;
        state.partial.clear();
        state.next_line_number = Some(1);
        let source_id = state.source_id.clone();
        state.pending.push_back(
            Event::default()
                .event("reset")
                .data(serde_json::json!({ "source": source_id }).to_string()),
        );
    }

    if metadata.len() == state.offset {
        return Ok(());
    }

    let mut file = tokio::fs::File::open(&state.path).await?;
    file.seek(std::io::SeekFrom::Start(state.offset)).await?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).await?;
    state.offset = state.offset.saturating_add(bytes.len() as u64);
    if bytes.is_empty() {
        return Ok(());
    }

    let chunk = String::from_utf8_lossy(&bytes);
    state.partial.push_str(&chunk);
    let complete_through = state
        .partial
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    if complete_through == 0 {
        return Ok(());
    }

    let complete = state.partial[..complete_through].to_string();
    state.partial = state.partial[complete_through..].to_string();

    for line in complete.lines() {
        let line_number = state.next_line_number;
        state.next_line_number = line_number.map(|value| value + 1);
        let source_id = state.source_id.clone();
        state.pending.push_back(log_sse_event(
            &source_id,
            line_number,
            &truncate_stream_log_line(line),
        ));
    }

    Ok(())
}

fn log_sse_event(source: &str, line_number: Option<usize>, text: &str) -> Event {
    Event::default().event("log").data(
        serde_json::json!({
            "source": source,
            "line_number": line_number,
            "text": text,
        })
        .to_string(),
    )
}

fn truncate_stream_log_line(line: &str) -> String {
    const MAX_CHARS: usize = 2_000;
    let mut chars = line.chars();
    let mut truncated = String::new();
    for _ in 0..MAX_CHARS {
        let Some(ch) = chars.next() else {
            return line.to_string();
        };
        truncated.push(ch);
    }
    if chars.next().is_some() {
        truncated.push_str("...");
    }
    truncated
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
                "current_package_version": info.current_package_version,
                "latest_package_version": info.latest_package_version,
                "current_image_version": info.current_image_version,
                "latest_image_version": info.latest_image_version,
                "update_available": info.update_available,
                "update_reason": info.update_reason,
                "install_targets": info.install_targets,
                "image_assets": info.image_assets,
                "last_rollback": crate::self_update::last_rollback(),
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

async fn ota_status_snapshot(
    state: SharedState,
    ota_status: crate::self_update::OtaStatusHandle,
) -> Response {
    match serde_json::to_value(ota_status.snapshot()) {
        Ok(mut value) => {
            if let Some(object) = value.as_object_mut() {
                let channel = crate::self_update::channel_from_state(&state);
                object.insert(
                    "channel".to_string(),
                    serde_json::Value::String(channel.as_str().to_string()),
                );
                if let Some(auto_update_status) = crate::auto_update::load_status_json(&state) {
                    object.insert("auto_update_status".to_string(), auto_update_status);
                }
            }
            json_status(StatusCode::OK, value)
        }
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
    if let Err(error) =
        crate::self_update::schedule_user_initiated_restart_with_best_effort_persist(state)
    {
        log::warn!(
            target: "http",
            "Restart scheduled, but failed to spawn restart persistence worker: {}",
            error
        );
    }
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

    if let Err(error) =
        crate::self_update::schedule_user_initiated_restart_with_best_effort_persist(state)
    {
        log::warn!(
            target: "http",
            "Matter fabric reset restart scheduled, but failed to spawn persistence worker: {}",
            error
        );
    }

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
        crate::self_update::OtaUpdateState::Checking
            | crate::self_update::OtaUpdateState::Updating
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
    let worker_status = ota_status.clone();
    let worker_state = state.clone();
    tokio::spawn(async move {
        apply_accepted_update(worker_state, worker_status, info).await;
    });

    json_ok(
        serde_json::json!({
            "status": "accepted",
            "message": format!("Update to v{} accepted", latest),
            "previous_version": previous,
            "new_version": latest,
        })
        .to_string(),
    )
}

async fn apply_accepted_update(
    state: SharedState,
    ota_status: crate::self_update::OtaStatusHandle,
    info: crate::self_update::UpdateInfo,
) {
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
            return;
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
            return;
        }
    };
    ota_status.mark_restarting(&previous, &latest, apply_result.checksum_verified);
    {
        let data_dir = state
            .lock()
            .map(|s| std::path::PathBuf::from(&s.data_dir))
            .unwrap_or_default();
        crate::ota_history::record(
            &data_dir,
            crate::ota_history::entry(Some(&previous), Some(&latest), "manual", "applied"),
        );
    }
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

    if let Err(error) =
        crate::self_update::schedule_post_update_restart_with_best_effort_persist(state.clone())
    {
        log::warn!(
            target: "http",
            "OTA restart scheduled, but failed to spawn persistence worker: {}",
            error
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use rhythm_os::state::AppState;
    use rhythm_os::storage::FileStorage;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, Once};
    use std::thread;
    use std::time::Duration;
    use tower::ServiceExt;

    static DRY_RUN_INIT: Once = Once::new();

    // Keep the legacy dry-run env initialized for routes that inspect it
    // directly. Scheduled restart threads are also forced to dry-run under
    // cfg(test), so parallel tests cannot exit the runner asynchronously.
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

    fn update_manifest(version: &str, package_url: &str) -> String {
        format!(
            r#"{{
                "version": "{version}",
                "package": {{
                    "name": "",
                    "url": "{package_url}",
                    "sha256": "abc123",
                    "kind": "archive_bundle",
                    "install": [
                        {{"slot": "sibling", "path": "missing-helper", "required": false}}
                    ]
                }},
                "images": []
            }}"#
        )
    }

    fn spawn_http_sequence(responses: Vec<(u16, String)>) -> String {
        spawn_http_sequence_with_delays(
            responses
                .into_iter()
                .map(|(status, body)| (status, body, Duration::ZERO))
                .collect(),
        )
    }

    fn spawn_http_sequence_with_delays(responses: Vec<(u16, String, Duration)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for (status, body, delay) in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut request = [0_u8; 1024];
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = stream.read(&mut request);
                thread::sleep(delay);
                let reason = match status {
                    200 => "OK",
                    404 => "Not Found",
                    500 => "Internal Server Error",
                    _ => "OK",
                };
                let headers = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            }
        });
        thread::sleep(Duration::from_millis(10));
        format!("http://127.0.0.1:{port}/feeds/manifest.json")
    }

    fn set_update_manifest_env(manifest_url: &str) {
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "desktop");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");
        std::env::set_var("RHYTHM_UPDATE_MANIFEST_URL", manifest_url);
    }

    fn clear_update_manifest_env() {
        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
    }

    async fn response_json(response: Response) -> serde_json::Value {
        let status = response.status();
        let body_bytes = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        serde_json::from_slice(&body_bytes)
            .unwrap_or_else(|_| panic!("response {status} did not contain JSON"))
    }

    async fn response_text(response: Response) -> String {
        let body_bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        String::from_utf8_lossy(&body_bytes).into_owned()
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
    async fn response_helpers_build_json_errors_and_attachments() {
        let response = json_ok(r#"{"status":"ok"}"#.to_string());
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["status"], "ok");

        let response = json_status(
            StatusCode::ACCEPTED,
            serde_json::json!({"status": "queued", "id": 7}),
        );
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let body = response_json(response).await;
        assert_eq!(body["status"], "queued");
        assert_eq!(body["id"], 7);

        let response = err_409("busy");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = response_json(response).await;
        assert_eq!(body["status"], "error");
        assert_eq!(body["message"], "busy");

        let response = err_500("boom");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let response = tar_gz_attachment("debug.tar.gz", b"bundle".to_vec());
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/gzip")
        );
        assert_eq!(
            response
                .headers()
                .get("content-disposition")
                .and_then(|value| value.to_str().ok()),
            Some("attachment; filename=\"debug.tar.gz\"")
        );
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(body.as_ref(), b"bundle");
    }

    #[tokio::test]
    async fn emit_ota_apply_progress_broadcasts_target_versions() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().event_tx = Some(tx);

        emit_ota_apply_progress(
            &state,
            "0.4.192-beta",
            "0.4.193-beta",
            crate::self_update::UpdateProgress {
                stage: rhythm_os::server_event::OtaUpdateStage::Installing,
                message: "Installing bundle".to_string(),
                downloaded_bytes: Some(150),
                total_bytes: Some(100),
            },
        );

        match rx.recv().await.expect("ota progress event") {
            rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                stage,
                current_version,
                target_version,
                update_available,
                downloaded_bytes,
                total_bytes,
                percent,
                ..
            } => {
                assert_eq!(stage, rhythm_os::server_event::OtaUpdateStage::Installing);
                assert_eq!(current_version.as_deref(), Some("0.4.192-beta"));
                assert_eq!(target_version.as_deref(), Some("0.4.193-beta"));
                assert_eq!(update_available, Some(true));
                assert_eq!(downloaded_bytes, Some(150));
                assert_eq!(total_bytes, Some(100));
                assert_eq!(percent, Some(100));
            }
            other => panic!("expected OTA progress event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn ota_status_endpoints_serialize_handle_state() {
        let data_dir = unique_test_dir("ota-status");
        std::fs::create_dir_all(data_dir.join("ota")).unwrap();
        std::fs::write(
            data_dir.join(crate::auto_update::AUTO_UPDATE_STATE_RELATIVE_PATH),
            br#"{"schema_version":1,"last_check":{"decision":"up_to_date"}}"#,
        )
        .unwrap();
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().data_dir = data_dir.display().to_string();
        let ota_status = crate::self_update::OtaStatusHandle::new("0.4.192-beta");

        let capabilities = ota_capabilities(ota_status.clone()).await;
        assert_eq!(capabilities.status(), StatusCode::OK);
        let capabilities = response_json(capabilities).await;
        assert_eq!(capabilities["strategy"], "self_pull");
        assert_eq!(capabilities["can_check"], true);
        assert_eq!(capabilities["requires_restart"], true);

        ota_status.mark_checking();
        let status = ota_status_snapshot(state, ota_status).await;
        assert_eq!(status.status(), StatusCode::OK);
        let status = response_json(status).await;
        assert_eq!(status["state"], "checking");
        assert_eq!(status["current_version"], "0.4.192-beta");
        assert_eq!(status["message"], "Checking for updates...");
        // Default desktop state resolves to the beta channel.
        assert_eq!(status["channel"], "beta");
        assert_eq!(status["auto_update_status"]["schema_version"], 1);
        assert_eq!(
            status["auto_update_status"]["last_check"]["decision"],
            "up_to_date"
        );

        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn do_update_rejects_when_update_is_already_in_progress() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let ota_status = crate::self_update::OtaStatusHandle::new("0.4.192-beta");
        ota_status.begin_update("0.4.193-beta").unwrap();

        let response = do_update(state, ota_status).await;

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = response_json(response).await;
        assert_eq!(body["status"], "error");
        assert_eq!(body["message"], "Update already in progress");
    }

    #[test]
    fn check_update_handler_reports_available_and_failed_progress() {
        let _guard = crate::self_update::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async {
            let manifest_url = spawn_http_sequence(vec![(
                200,
                update_manifest("999.0.0", "release/rhythm-server.tar.gz"),
            )]);
            set_update_manifest_env(&manifest_url);
            let (tx, mut rx) = tokio::sync::broadcast::channel(8);
            let state: SharedState = Arc::new(Mutex::new(AppState::default()));
            state.lock().unwrap().event_tx = Some(tx);
            let ota_status = crate::self_update::OtaStatusHandle::new(crate::BUILD_VERSION);

            let response = check_update(state.clone(), ota_status.clone()).await;

            let status = response.status();
            if status != StatusCode::OK {
                let text = response_text(response).await;
                panic!("expected update check 200, got {status}: {text}");
            }
            let body = response_json(response).await;
            assert_eq!(body["update_available"], true);
            assert_eq!(body["latest_version"], "999.0.0");
            assert!(matches!(
                rx.recv().await.unwrap(),
                rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                    stage: rhythm_os::server_event::OtaUpdateStage::Checking,
                    ..
                }
            ));
            assert!(matches!(
                rx.recv().await.unwrap(),
                rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                    stage: rhythm_os::server_event::OtaUpdateStage::UpdateAvailable,
                    ..
                }
            ));
            assert_eq!(
                ota_status.snapshot().latest_version.as_deref(),
                Some("999.0.0")
            );

            let manifest_url = spawn_http_sequence(vec![(404, String::new())]);
            set_update_manifest_env(&manifest_url);
            let (tx, mut rx) = tokio::sync::broadcast::channel(8);
            let state: SharedState = Arc::new(Mutex::new(AppState::default()));
            state.lock().unwrap().event_tx = Some(tx);
            let ota_status = crate::self_update::OtaStatusHandle::new(crate::BUILD_VERSION);

            let response = check_update(state, ota_status.clone()).await;

            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert!(response_text(response)
                .await
                .contains("Update manifest returned 404"));
            assert!(matches!(
                rx.recv().await.unwrap(),
                rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                    stage: rhythm_os::server_event::OtaUpdateStage::Checking,
                    ..
                }
            ));
            assert!(matches!(
                rx.recv().await.unwrap(),
                rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                    stage: rhythm_os::server_event::OtaUpdateStage::Failed,
                    ..
                }
            ));
            assert!(ota_status.snapshot().last_error.is_some());
        });

        clear_update_manifest_env();
    }

    #[test]
    fn do_update_handler_reports_up_to_date_and_accepts_background_apply() {
        let _guard = crate::self_update::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        init_restart_dry_run();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async {
            let manifest_url = spawn_http_sequence(vec![(
                200,
                update_manifest(crate::BUILD_VERSION, "release/rhythm-server.tar.gz"),
            )]);
            set_update_manifest_env(&manifest_url);
            let (tx, mut rx) = tokio::sync::broadcast::channel(8);
            let state: SharedState = Arc::new(Mutex::new(AppState::default()));
            state.lock().unwrap().event_tx = Some(tx);
            let ota_status = crate::self_update::OtaStatusHandle::new(crate::BUILD_VERSION);

            let response = do_update(state, ota_status).await;

            let status = response.status();
            if status != StatusCode::OK {
                let text = response_text(response).await;
                panic!("expected do_update 200, got {status}: {text}");
            }
            let body = response_json(response).await;
            assert_eq!(body["status"], "ok");
            assert_eq!(body["message"], "Already up to date");
            assert!(matches!(
                rx.recv().await.unwrap(),
                rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                    stage: rhythm_os::server_event::OtaUpdateStage::Checking,
                    ..
                }
            ));
            assert!(matches!(
                rx.recv().await.unwrap(),
                rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                    stage: rhythm_os::server_event::OtaUpdateStage::UpToDate,
                    ..
                }
            ));

            let manifest_url = spawn_http_sequence_with_delays(vec![
                (
                    200,
                    update_manifest("999.0.1", "release/missing-rhythm-server.tar.gz"),
                    Duration::ZERO,
                ),
                (404, String::new(), Duration::from_millis(200)),
            ]);
            set_update_manifest_env(&manifest_url);
            let (tx, mut rx) = tokio::sync::broadcast::channel(16);
            let state: SharedState = Arc::new(Mutex::new(AppState::default()));
            state.lock().unwrap().event_tx = Some(tx);
            let ota_status = crate::self_update::OtaStatusHandle::new(crate::BUILD_VERSION);

            let response = do_update(state, ota_status.clone()).await;

            assert_eq!(response.status(), StatusCode::OK);
            let body = response_json(response).await;
            assert_eq!(body["status"], "accepted");
            assert_eq!(body["new_version"], "999.0.1");
            assert_eq!(
                ota_status.snapshot().state,
                crate::self_update::OtaUpdateState::Updating,
                "the server must own the accepted update after the client response"
            );
            assert!(matches!(
                rx.recv().await.unwrap(),
                rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                    stage: rhythm_os::server_event::OtaUpdateStage::Checking,
                    ..
                }
            ));
            assert!(matches!(
                rx.recv().await.unwrap(),
                rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                    stage: rhythm_os::server_event::OtaUpdateStage::UpdateAvailable,
                    ..
                }
            ));

            let failed = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let event = rx.recv().await.unwrap();
                    if matches!(
                        event,
                        rhythm_os::server_event::ServerEvent::OtaUpdateProgress {
                            stage: rhythm_os::server_event::OtaUpdateStage::Failed,
                            ..
                        }
                    ) {
                        break;
                    }
                }
            })
            .await;
            assert!(
                failed.is_ok(),
                "background failure should emit OTA SSE progress"
            );
            assert!(ota_status.snapshot().last_error.is_some());
        });

        clear_update_manifest_env();
    }

    #[tokio::test]
    async fn debug_bundle_handler_returns_download_attachment() {
        let data_dir = unique_test_dir("debug-bundle-http");
        std::fs::create_dir_all(data_dir.join("log")).unwrap();
        std::fs::write(
            data_dir.join("log").join("rhythm-server.log"),
            b"server-log",
        )
        .unwrap();

        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        {
            let mut state = state.lock().unwrap();
            state.firmware_version = "1.2.3";
            state.platform_type = "server";
            state.platform_context = "desktop";
            state.data_dir = data_dir.display().to_string();
        }

        let response = debug_bundle(State(state), HeaderMap::new(), None).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/gzip")
        );
        assert!(response
            .headers()
            .get("content-disposition")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.starts_with("attachment; filename=\"rhythm-debug-")
                    && value.ends_with(".tar.gz\"")
            }));
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert!(!body.is_empty());

        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn upload_url_gate_allows_https_and_loopback_only() {
        assert!(upload_url_is_acceptable("https://storage.example.com/x"));
        assert!(upload_url_is_acceptable("http://127.0.0.1:8080/upload"));
        assert!(upload_url_is_acceptable("http://localhost/upload"));
        assert!(upload_url_is_acceptable("http://[::1]:9000/upload"));
        assert!(!upload_url_is_acceptable("http://192.168.5.1/upload"));
        assert!(!upload_url_is_acceptable("http://evil.example.com/upload"));
        assert!(!upload_url_is_acceptable("ftp://example.com/upload"));
    }

    /// Direct-to-storage upload: the device PUTs a Supabase-compatible
    /// multipart body to the signed URL itself (bypassing the app's memory
    /// and receive timeout) and embeds the app log the request carried.
    #[tokio::test]
    async fn debug_bundle_handler_uploads_directly_and_embeds_app_log() {
        let data_dir = unique_test_dir("debug-bundle-upload");
        std::fs::create_dir_all(data_dir.join("log")).unwrap();
        std::fs::write(
            data_dir.join("log").join("rhythm-server.log"),
            b"server-log",
        )
        .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let received = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let (headers_end, content_length, head) = loop {
                let n = socket.read(&mut tmp).await.unwrap();
                assert!(n > 0, "upload connection closed before headers");
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                    assert!(head.starts_with("PUT /upload"), "expected PUT, got: {head}");
                    let content_length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap())
                        })
                        .expect("content-length header");
                    break (pos + 4, content_length, head);
                }
            };
            while buf.len() < headers_end + content_length {
                let n = socket.read(&mut tmp).await.unwrap();
                assert!(n > 0, "upload connection closed before body finished");
                buf.extend_from_slice(&tmp[..n]);
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .await
                .unwrap();
            socket.flush().await.unwrap();
            (
                head,
                buf[headers_end..headers_end + content_length].to_vec(),
            )
        });

        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        {
            let mut state = state.lock().unwrap();
            state.firmware_version = "1.2.3";
            state.platform_type = "server";
            state.platform_context = "desktop";
            state.data_dir = data_dir.display().to_string();
        }

        let request = DebugBundleRequest {
            upload_url: Some(format!("http://127.0.0.1:{}/upload", addr.port())),
            app_log: Some("app log line one\napp log line two\n".to_string()),
            app_metadata: Some(serde_json::json!({
                "kind": "rhythm_app_log",
                "app_version": "9.9.9",
            })),
            async_submission: None,
        };
        let response = debug_bundle(State(state), HeaderMap::new(), Some(Json(request))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["uploaded"], true);
        assert!(json["size_bytes"].as_u64().unwrap() > 0);

        let (upload_headers, uploaded_body) = received.await.unwrap();
        let lower_headers = upload_headers.to_ascii_lowercase();
        assert!(
            lower_headers.contains("x-upsert: false"),
            "signed uploads must preserve no-upsert semantics: {upload_headers}"
        );
        let content_type = upload_headers
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("content-type:"))
            .expect("content-type header");
        assert!(
            content_type.contains("multipart/form-data; boundary="),
            "Supabase signed upload expects multipart/form-data, got: {content_type}"
        );
        let boundary = content_type
            .split("boundary=")
            .nth(1)
            .expect("multipart boundary")
            .trim();
        let body_text = String::from_utf8_lossy(&uploaded_body);
        assert!(
            body_text
                .contains("content-disposition: form-data; name=\"cacheControl\"\r\n\r\n3600\r\n"),
            "missing Supabase cacheControl form field: {body_text:?}"
        );
        let file_header = b"content-type: application/gzip\r\ncontent-disposition: form-data; name=\"\"; filename=\"\"\r\n\r\n";
        let file_start = uploaded_body
            .windows(file_header.len())
            .position(|window| window == file_header)
            .expect("multipart gzip file part")
            + file_header.len();
        let file_end_marker = format!("\r\n--{boundary}").into_bytes();
        let file_end = uploaded_body[file_start..]
            .windows(file_end_marker.len())
            .position(|window| window == file_end_marker)
            .expect("multipart file closing boundary")
            + file_start;
        let uploaded = uploaded_body[file_start..file_end].to_vec();
        assert_eq!(
            uploaded.len(),
            json["size_bytes"].as_u64().unwrap() as usize
        );
        let tar_bytes = {
            use std::io::Read as _;
            let mut decoder = flate2::read::GzDecoder::new(uploaded.as_slice());
            let mut tar_bytes = Vec::new();
            decoder.read_to_end(&mut tar_bytes).unwrap();
            tar_bytes
        };
        let mut archive = tar::Archive::new(tar_bytes.as_slice());
        let mut entries = std::collections::BTreeMap::new();
        for entry in archive.entries().unwrap() {
            use std::io::Read as _;
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().display().to_string();
            let mut contents = Vec::new();
            entry.read_to_end(&mut contents).unwrap();
            entries.insert(path, contents);
        }
        assert_eq!(
            entries.get("app/app.log").map(|bytes| bytes.as_slice()),
            Some("app log line one\napp log line two\n".as_bytes())
        );
        let metadata: serde_json::Value =
            serde_json::from_slice(entries.get("app/metadata.json").unwrap()).unwrap();
        assert_eq!(metadata["app_version"], "9.9.9");
        assert!(entries.contains_key("manifest.json"));

        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn async_debug_bundle_returns_after_durable_queue_acceptance() {
        let data_dir = unique_test_dir("debug-bundle-async-queue");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join("auth.json"), br#"{"schema_version":1}"#).unwrap();
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().data_dir = data_dir.display().to_string();
        let submission_id = "8f68c4ae-edf5-4e7d-9c9a-99cc2e86b1bc";
        let request = DebugBundleRequest {
            upload_url: Some("http://127.0.0.1:9/upload".to_string()),
            app_log: Some("bounded app log".to_string()),
            app_metadata: Some(serde_json::json!({"app_version": "9.9.9"})),
            async_submission: Some(AsyncDebugBundleRequest {
                submission_id: submission_id.to_string(),
                completion_url: "http://127.0.0.1:9/complete".to_string(),
                completion_token: "one-time-completion-token-with-entropy".to_string(),
            }),
        };

        let response = tokio::time::timeout(
            Duration::from_secs(1),
            debug_bundle(State(state), HeaderMap::new(), Some(Json(request))),
        )
        .await
        .expect("queue acknowledgement must not wait for bundle generation");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response_json(response).await;
        assert_eq!(body["queued"], true);
        assert_eq!(body["submission_id"], submission_id);

        let queue =
            std::fs::read_to_string(data_dir.join(crate::support_bundle_jobs::QUEUE_FILE_NAME))
                .unwrap();
        assert!(queue.contains(submission_id));
        assert!(queue.contains("one-time-completion-token-with-entropy"));
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn debug_bundle_handler_rejects_non_loopback_http_upload_url() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let request = DebugBundleRequest {
            upload_url: Some("http://192.168.5.9/upload".to_string()),
            app_log: None,
            app_metadata: None,
            async_submission: None,
        };
        let response = debug_bundle(State(state), HeaderMap::new(), Some(Json(request))).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn debug_bundle_handler_rejects_a_dropped_json_body() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers.insert(
            axum::http::header::CONTENT_LENGTH,
            "512000".parse().unwrap(),
        );

        let response = debug_bundle(State(state), headers, None).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(
            body["message"],
            "debug bundle request body was incomplete or invalid JSON"
        );
    }

    #[test]
    fn matter_fabric_reset_helpers_cover_absent_and_missing_data_dir_paths() {
        let missing_state: SharedState = Arc::new(Mutex::new(AppState::default()));
        assert!(reset_matter_fabric_state(&missing_state)
            .unwrap_err()
            .to_string()
            .contains("data_dir not configured"));

        let root = unique_test_dir("matter-reset-helpers");
        std::fs::create_dir_all(&root).unwrap();
        assert!(!remove_dir_if_exists(&root.join("missing-dir")).unwrap());
        assert!(!remove_file_if_exists(&root.join("missing-file")).unwrap());

        let dir = root.join("present-dir");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(remove_dir_if_exists(&dir).unwrap());
        assert!(!dir.exists());

        let file = root.join("present-file");
        std::fs::write(&file, b"state").unwrap();
        assert!(remove_file_if_exists(&file).unwrap());
        assert!(!file.exists());

        let _ = std::fs::remove_dir_all(root);
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
            state.storage = Some(std::sync::Arc::new(
                FileStorage::new(&state.data_dir).unwrap(),
            ));
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
