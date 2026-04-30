//! Axum HTTP server — uses shared routes from rhythm-os plus server-specific endpoints.

use axum::body::Body;
use axum::http::StatusCode;
use std::net::IpAddr;

use axum::extract::State;
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

    logging::with_http_observability(
        rhythm_os::axum_router::api_routes()
            // Server-specific endpoints
            .route("/api/discover", get(discover))
            .route("/api/diag/debug-bundle", post(debug_bundle))
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
                    move || check_update(ota_status.clone())
                }),
            )
            .route(
                "/api/ota/update",
                post({
                    let ota_status = ota_status.clone();
                    move || do_update(ota_status.clone())
                }),
            )
            .route("/api/restart", post(restart_device))
            .with_state(state)
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

async fn check_update(ota_status: crate::self_update::OtaStatusHandle) -> Response {
    let version = crate::BUILD_VERSION;
    ota_status.mark_checking();
    match tokio::task::spawn_blocking(move || crate::self_update::check_blocking(version)).await {
        Ok(Ok(info)) => {
            ota_status.record_check_result(&info);
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
            err_500(e)
        }
        Err(e) => {
            ota_status.mark_error(e.to_string());
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

async fn restart_device() -> Response {
    log::info!(target: "http", "Restart requested via /api/restart");
    crate::self_update::schedule_user_initiated_restart();
    json_ok(r#"{"status":"ok","message":"Restart scheduled"}"#.to_string())
}

async fn do_update(ota_status: crate::self_update::OtaStatusHandle) -> Response {
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

    // Check for update
    let info = match tokio::task::spawn_blocking(move || {
        crate::self_update::check_blocking(version)
    })
    .await
    {
        Ok(Ok(info)) => info,
        Ok(Err(e)) => {
            ota_status.mark_error(e.clone());
            return err_500(e);
        }
        Err(e) => {
            ota_status.mark_error(e.to_string());
            return err_500(e);
        }
    };
    ota_status.record_check_result(&info);

    if !info.update_available {
        return json_ok(r#"{"status":"ok","message":"Already up to date"}"#.to_string());
    }

    if let Err(e) = ota_status.begin_update(&info.latest_version) {
        return err_409(e);
    }

    let latest = info.latest_version.clone();
    let previous = info.current_version.clone();

    // Download and install
    let apply_result = match tokio::task::spawn_blocking(move || info.apply_blocking()).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            ota_status.mark_error(e.clone());
            return err_500(e);
        }
        Err(e) => {
            ota_status.mark_error(e.to_string());
            return err_500(e);
        }
    };
    ota_status.mark_restarting(&previous, &latest, apply_result.checksum_verified);

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
    use std::sync::{Arc, Mutex, Once};
    use tower::ServiceExt;

    static DRY_RUN_INIT: Once = Once::new();

    // The restart helper spawns a thread that sleeps 1s then either reboots the
    // device or calls process::exit(1). Setting this env var keeps the spawned
    // thread inert so tests can exercise the route without nuking the runner.
    fn init_restart_dry_run() {
        DRY_RUN_INIT.call_once(|| std::env::set_var("RHYTHM_RESTART_DRY_RUN", "1"));
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
}
