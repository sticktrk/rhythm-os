//! Axum HTTP server — uses shared routes from rhythm-os plus server-specific endpoints.

use std::net::IpAddr;
use std::time::Duration;

use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rhythm_os::handlers::ApiResponse;
use rhythm_os::mdns::MDNS_HOSTNAME_PREFIX;
use rhythm_os::state::SharedState;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Create the Axum router with all API routes.
pub fn create_router(state: SharedState) -> Router {
    rhythm_os::axum_router::api_routes()
        // Server-specific endpoints
        .route("/api/discover", get(discover))
        .route("/api/ota/check", get(check_update))
        .route("/api/ota/update", post(do_update))
        .with_state(state)
        // CORS + trace at outer level so they cover fallback/404 responses too
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn json_ok(body: String) -> Response {
    ApiResponse::json_ok(body).into_response()
}

fn err_500(e: impl std::fmt::Display) -> Response {
    ApiResponse::server_error(e).into_response()
}

// ---------------------------------------------------------------------------
// Server-specific handlers
// ---------------------------------------------------------------------------

async fn check_update() -> Response {
    let version = env!("CARGO_PKG_VERSION");
    match tokio::task::spawn_blocking(move || crate::self_update::check_blocking(version)).await {
        Ok(Ok(info)) => {
            let json = serde_json::json!({
                "current_version": info.current_version,
                "latest_version": info.latest_version,
                "update_available": info.update_available,
            });
            json_ok(json.to_string())
        }
        Ok(Err(e)) => err_500(e),
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
    let deadline = std::time::Instant::now() + Duration::from_secs(3);

    while std::time::Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(100)) {
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

async fn do_update() -> Response {
    let version = env!("CARGO_PKG_VERSION");

    // Check for update
    let info = match tokio::task::spawn_blocking(move || {
        crate::self_update::check_blocking(version)
    })
    .await
    {
        Ok(Ok(info)) => info,
        Ok(Err(e)) => return err_500(e),
        Err(e) => return err_500(e),
    };

    if !info.update_available {
        return json_ok(r#"{"status":"ok","message":"Already up to date"}"#.to_string());
    }

    let download_url = match info.download_url {
        Some(url) => url,
        None => return err_500("No download URL for this platform"),
    };

    let latest = info.latest_version.clone();
    let previous = info.current_version.clone();

    // Download and install
    if let Err(e) =
        match tokio::task::spawn_blocking(move || crate::self_update::apply_blocking(&download_url))
            .await
        {
            Ok(result) => result,
            Err(e) => Err(e.to_string()),
        }
    {
        return err_500(e);
    }

    // Exit non-zero after responding so the service manager restarts us
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        log::info!(target: "sys", "Restarting after self-update...");
        std::process::exit(1);
    });

    json_ok(format!(
        r#"{{"status":"ok","message":"Updated to v{}, restarting...","previous_version":"{}","new_version":"{}"}}"#,
        latest, previous, latest
    ))
}
