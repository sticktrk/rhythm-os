//! HTTP server for REST API endpoints.
//!
//! Uses shared handler functions from `rhythm_os::handlers` for business logic.
//! ESP32-specific: deferred batch persistence via worker thread, WiFi/diag/OTA/reboot endpoints.

use anyhow::Result;
use embedded_svc::http::Headers;
use esp_idf_svc::http::server::{Configuration, EspHttpServer, Method};
use esp_idf_svc::io::{Read as _, Write as _};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{info, warn};
use serde::Serialize;
use serde_json::Value;

use crate::storage;
use rhythm_os::handlers::{self, ApiResponse};
use rhythm_os::state::{SharedState, WorkItem};

/// JSON Content-Type + CORS headers for all responses.
const JSON_CORS_HEADERS: &[(&str, &str)] = &[
    ("Content-Type", "application/json"),
    ("Access-Control-Allow-Origin", "*"),
    (
        "Access-Control-Allow-Methods",
        "GET, PUT, POST, DELETE, OPTIONS",
    ),
    ("Access-Control-Allow-Headers", "Content-Type"),
];

/// CORS-only headers for OPTIONS preflight responses (no Content-Type).
const CORS_HEADERS: &[(&str, &str)] = &[
    ("Access-Control-Allow-Origin", "*"),
    (
        "Access-Control-Allow-Methods",
        "GET, PUT, POST, DELETE, OPTIONS",
    ),
    ("Access-Control-Allow-Headers", "Content-Type"),
];

/// Maximum JSON body size for PUT endpoints (8KB).
const MAX_JSON_BODY: usize = 8192;

/// Write an `ApiResponse` to an esp-idf HTTP response.
fn write_api_response(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    resp: &ApiResponse,
) -> Result<()> {
    let uri = req.uri();
    log::info!(target: "http", "{} → {}", uri, resp.status);

    let reason = match resp.status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        _ => "Error",
    };
    let headers = if resp.content_type == "application/json" {
        JSON_CORS_HEADERS
    } else {
        CORS_HEADERS
    };
    let mut response = req.into_response(resp.status, Some(reason), headers)?;
    response.write_all(resp.body.as_bytes())?;
    Ok(())
}

/// Send a DeferredPersistState to the worker thread.
fn defer_persist(state: &SharedState) {
    let work_tx = {
        let Ok(s) = state.lock() else { return };
        s.work_tx.clone()
    };
    if let Some(tx) = work_tx {
        let _ = tx.try_send(WorkItem::DeferredPersistState);
    }
}

/// Start the HTTP server with all API endpoints.
pub fn start_server(
    state: SharedState,
    nvs: EspDefaultNvsPartition,
) -> Result<EspHttpServer<'static>> {
    let config = Configuration {
        max_uri_handlers: 32,
        max_open_sockets: 7,
        stack_size: 12288,
        lru_purge_enable: true,
        ..Default::default()
    };
    let mut server = EspHttpServer::new(&config)?;

    // =========================================================================
    // Simple endpoints — delegate to shared handlers
    // =========================================================================

    server.fn_handler::<anyhow::Error, _>("/health", Method::Get, |req| {
        write_api_response(req, &handlers::handle_health())
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/state", Method::Get, move |req| {
        write_api_response(req, &handlers::handle_get_state(&s))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/rooms/state", Method::Get, move |req| {
        write_api_response(req, &handlers::handle_get_rooms_state(&s))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/rooms", Method::Delete, move |req| {
        let uri = req.uri().to_string();
        let resp = match parse_query_param(&uri, "id") {
            Some(id) => handlers::handle_delete_room(&s, &id),
            None => ApiResponse::bad_request("Missing ?id= parameter"),
        };
        write_api_response(req, &resp)
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/devices", Method::Delete, move |req| {
        let uri = req.uri().to_string();
        let resp = match parse_query_param(&uri, "id") {
            Some(id) => handlers::handle_delete_device(&s, &id),
            None => ApiResponse::bad_request("Missing ?id= parameter"),
        };
        write_api_response(req, &resp)
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/motion-timeout", Method::Put, move |mut req| {
        let body = match read_json_body(&mut req) {
            Ok(b) => b,
            Err(e) => return write_api_response(req, &ApiResponse::bad_request(&e.to_string())),
        };
        write_api_response(req, &handlers::handle_put_motion_timeout(&s, &body))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/config", Method::Get, move |req| {
        write_api_response(req, &handlers::handle_get_config(&s))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/config", Method::Put, move |mut req| {
        let body = match read_json_body(&mut req) {
            Ok(b) => b,
            Err(e) => return write_api_response(req, &ApiResponse::bad_request(&e.to_string())),
        };
        write_api_response(req, &handlers::handle_put_config(&s, &body))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/config/reset", Method::Post, move |req| {
        write_api_response(req, &handlers::handle_reset_config(&s))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>(
        "/api/config/absorb-offset",
        Method::Post,
        move |mut req| {
            let body = match read_json_body(&mut req) {
                Ok(b) => b,
                Err(e) => {
                    return write_api_response(req, &ApiResponse::bad_request(&e.to_string()))
                }
            };
            write_api_response(req, &handlers::handle_absorb_time_offset(&s, &body))
        },
    )?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/curve/now", Method::Get, move |req| {
        write_api_response(req, &handlers::handle_get_curve_now(&s, None))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/location", Method::Put, move |mut req| {
        let body = match read_json_body(&mut req) {
            Ok(b) => b,
            Err(e) => return write_api_response(req, &ApiResponse::bad_request(&e.to_string())),
        };
        write_api_response(req, &handlers::handle_put_location(&s, &body))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/settings", Method::Get, move |req| {
        write_api_response(req, &handlers::handle_get_settings(&s))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/settings", Method::Put, move |mut req| {
        let body = match read_json_body(&mut req) {
            Ok(b) => b,
            Err(e) => return write_api_response(req, &ApiResponse::bad_request(&e.to_string())),
        };
        write_api_response(req, &handlers::handle_put_settings(&s, &body))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>(
        "/api/hub/credentials",
        Method::Put,
        move |mut req| {
            let body = match read_json_body(&mut req) {
                Ok(b) => b,
                Err(e) => {
                    return write_api_response(req, &ApiResponse::bad_request(&e.to_string()))
                }
            };
            write_api_response(req, &handlers::handle_put_hub_credentials(&s, &body))
        },
    )?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>(
        "/api/rooms/preferences",
        Method::Put,
        move |mut req| {
            let body = match read_json_body(&mut req) {
                Ok(b) => b,
                Err(e) => {
                    return write_api_response(req, &ApiResponse::bad_request(&e.to_string()))
                }
            };
            let is_batch = body.is_array() && body.as_array().map(|a| a.len() > 1).unwrap_or(false);
            let resp = handlers::handle_put_room_preferences(&s, &body, !is_batch);
            if is_batch && resp.status == 200 {
                defer_persist(&s);
            }
            write_api_response(req, &resp)
        },
    )?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/rooms/action", Method::Put, move |mut req| {
        let body = match read_json_body(&mut req) {
            Ok(b) => b,
            Err(e) => return write_api_response(req, &ApiResponse::bad_request(&e.to_string())),
        };
        let is_batch = body.is_array() && body.as_array().map(|a| a.len() > 1).unwrap_or(false);
        let resp = handlers::handle_room_action(&s, &body, !is_batch);
        if is_batch && resp.status == 200 {
            defer_persist(&s);
        }
        write_api_response(req, &resp)
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>(
        "/api/rooms/brightness",
        Method::Put,
        move |mut req| {
            let body = match read_json_body(&mut req) {
                Ok(b) => b,
                Err(e) => {
                    return write_api_response(req, &ApiResponse::bad_request(&e.to_string()))
                }
            };
            let is_batch = body.is_array() && body.as_array().map(|a| a.len() > 1).unwrap_or(false);
            let resp = handlers::handle_set_brightness(&s, &body, !is_batch);
            if is_batch && resp.status == 200 {
                defer_persist(&s);
            }
            write_api_response(req, &resp)
        },
    )?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/rooms/offset", Method::Put, move |mut req| {
        let body = match read_json_body(&mut req) {
            Ok(b) => b,
            Err(e) => return write_api_response(req, &ApiResponse::bad_request(&e.to_string())),
        };
        let is_batch = body.is_array() && body.as_array().map(|a| a.len() > 1).unwrap_or(false);
        let resp = handlers::handle_set_time_offset(&s, &body, !is_batch);
        if is_batch && resp.status == 200 {
            defer_persist(&s);
        }
        write_api_response(req, &resp)
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/hub/credentials", Method::Delete, move |req| {
        write_api_response(req, &handlers::handle_delete_hub(&s, None, None))
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/rooms/fix", Method::Post, move |req| {
        let resp = handlers::handle_fix_my_lights(&s, false);
        if resp.status == 200 {
            defer_persist(&s);
        }
        write_api_response(req, &resp)
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/sync", Method::Post, move |req| {
        write_api_response(req, &handlers::handle_post_sync(&s))
    })?;

    // =========================================================================
    // Batch endpoints — ESP32 deferred persistence via worker thread
    // =========================================================================
    // Single items persist inline; batch items defer persist to the 16KB worker
    // thread stack to avoid stack-heavy serde on the 12KB HTTP handler stack.

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/rooms", Method::Put, move |mut req| {
        let body = match read_json_body(&mut req) {
            Ok(b) => b,
            Err(e) => return write_api_response(req, &ApiResponse::bad_request(&e.to_string())),
        };
        let is_batch = body.is_array() && body.as_array().map(|a| a.len() > 1).unwrap_or(false);
        let resp = handlers::handle_put_rooms(&s, &body, !is_batch);
        if is_batch && resp.status == 200 {
            defer_persist(&s);
        }
        write_api_response(req, &resp)
    })?;

    let s = state.clone();
    server.fn_handler::<anyhow::Error, _>("/api/devices", Method::Put, move |mut req| {
        let body = match read_json_body(&mut req) {
            Ok(b) => b,
            Err(e) => return write_api_response(req, &ApiResponse::bad_request(&e.to_string())),
        };
        let is_batch = body.is_array() && body.as_array().map(|a| a.len() > 1).unwrap_or(false);
        let resp = handlers::handle_put_devices(&s, &body, !is_batch);
        if is_batch && resp.status == 200 {
            defer_persist(&s);
        }
        write_api_response(req, &resp)
    })?;

    // =========================================================================
    // ESP32-only: WiFi, Diagnostics, OTA, System
    // =========================================================================

    let wifi_nvs = nvs.clone();
    server.fn_handler::<anyhow::Error, _>("/api/wifi", Method::Delete, move |req| {
        handle_delete_wifi(req, &wifi_nvs)
    })?;

    server.fn_handler("/api/diag/vitals", Method::Get, move |req| {
        handle_get_diag_vitals(req)
    })?;

    server.fn_handler("/api/diag/logs", Method::Get, move |req| {
        handle_get_diag_logs(req)
    })?;

    let crash_nvs = nvs.clone();
    server.fn_handler::<anyhow::Error, _>("/api/diag/crash", Method::Delete, move |req| {
        handle_delete_crash(req, &crash_nvs)
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/ota/version", Method::Get, |req| {
        write_api_response(req, &handlers::handle_get_version(crate::FIRMWARE_VERSION))
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/ota/upload", Method::Post, move |mut req| {
        let content_length =
            req.content_len()
                .ok_or_else(|| anyhow::anyhow!("Missing Content-Length"))? as usize;

        if content_length == 0 || content_length > 4 * 1024 * 1024 {
            let mut resp = req.into_response(400, Some("Bad Request"), JSON_CORS_HEADERS)?;
            resp.write_all(br#"{"status":"error","message":"Invalid content length"}"#)?;
            return Ok(());
        }

        info!(target: "ota", "OTA upload starting: {} bytes", content_length);

        match crate::ota::write_ota(&mut req, content_length) {
            Ok(()) => {
                let mut resp = req.into_response(200, Some("OK"), JSON_CORS_HEADERS)?;
                resp.write_all(br#"{"status":"ok"}"#)?;

                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    unsafe { esp_idf_svc::sys::esp_restart() };
                });
                Ok(())
            }
            Err(e) => {
                warn!(target: "ota", "OTA upload failed: {}", e);
                let msg = serde_json::to_string(&format!("{}", e))
                    .unwrap_or_else(|_| "\"OTA failed\"".to_string());
                let body = format!(r#"{{"status":"error","message":{}}}"#, msg);
                let mut resp =
                    req.into_response(500, Some("Internal Server Error"), JSON_CORS_HEADERS)?;
                resp.write_all(body.as_bytes())?;
                Ok(())
            }
        }
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/system/reboot", Method::Post, |req| {
        info!("Reboot requested via API");
        let mut response = req.into_response(200, Some("OK"), JSON_CORS_HEADERS)?;
        response.write_all(br#"{"status":"ok","message":"Rebooting in 2s..."}"#)?;

        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(2));
            unsafe { esp_idf_svc::sys::esp_restart() };
        });

        Ok(())
    })?;

    // =========================================================================
    // CORS Preflight (OPTIONS) Handlers
    // =========================================================================

    // Single wildcard OPTIONS handler for CORS preflight on all paths.
    server.fn_handler::<anyhow::Error, _>("/*", Method::Options, |req| {
        let mut resp = req.into_response(204, Some("No Content"), CORS_HEADERS)?;
        resp.write_all(b"")?;
        Ok(())
    })?;

    info!("HTTP server endpoints registered");

    Ok(server)
}

// =========================================================================
// JSON Body Reader
// =========================================================================

fn read_json_body(
    req: &mut esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
) -> Result<Value> {
    let content_length = req.content_len().unwrap_or(0) as usize;
    if content_length == 0 || content_length > MAX_JSON_BODY {
        return Err(anyhow::anyhow!(
            "Invalid content length: {} (max {})",
            content_length,
            MAX_JSON_BODY
        ));
    }

    let mut buf = vec![0u8; content_length];
    req.read_exact(&mut buf)?;

    serde_json::from_slice(&buf).map_err(|e| anyhow::anyhow!("JSON parse error: {}", e))
}

// =========================================================================
// ESP32-only handlers
// =========================================================================

fn handle_delete_wifi(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    nvs: &EspDefaultNvsPartition,
) -> Result<(), anyhow::Error> {
    info!("Clearing WiFi credentials and restarting...");
    storage::clear_wifi_credentials(nvs)?;

    let mut response = req.into_response(200, Some("OK"), JSON_CORS_HEADERS)?;
    response.write_all(br#"{"status":"credentials_cleared","message":"Restarting in 2s..."}"#)?;

    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(2));
        unsafe { esp_idf_svc::sys::esp_restart() };
    });

    Ok(())
}

fn handle_delete_crash(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    nvs: &EspDefaultNvsPartition,
) -> Result<(), anyhow::Error> {
    storage::clear_crash_info(nvs)?;
    crate::diag::clear_boot_crash_info();
    let mut response = req.into_response(200, Some("OK"), JSON_CORS_HEADERS)?;
    response.write_all(br#"{"status":"ok"}"#)?;
    Ok(())
}

fn handle_get_diag_vitals(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
) -> Result<(), anyhow::Error> {
    let vitals = crate::diag::get_vitals();
    send_json_response(req, &vitals)
}

#[derive(Serialize)]
struct DiagLogsResponse {
    count: usize,
    logs: Vec<crate::diag::DiagEntry>,
}

fn handle_get_diag_logs(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
) -> Result<(), anyhow::Error> {
    let uri = req.uri().to_string();

    let category =
        parse_query_param(&uri, "category").and_then(|s| crate::diag::DiagCategory::from_str(&s));
    let limit = parse_query_param(&uri, "limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(10);
    let since = parse_query_param(&uri, "since").and_then(|s| s.parse::<u32>().ok());

    let logs = crate::diag::get_logs(category, limit, since);
    let response = DiagLogsResponse {
        count: logs.len(),
        logs,
    };

    send_json_response(req, &response)
}

/// Parse a query parameter value from a URI string.
fn parse_query_param(uri: &str, key: &str) -> Option<String> {
    let query_start = uri.find('?')?;
    let query = &uri[query_start + 1..];
    for param in query.split('&') {
        if let Some(value) = param.strip_prefix(key) {
            if let Some(value) = value.strip_prefix('=') {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn send_json_response<T: Serialize>(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    data: &T,
) -> Result<(), anyhow::Error> {
    let json = serde_json::to_string(data)?;
    let mut response = req.into_response(200, Some("OK"), JSON_CORS_HEADERS)?;
    response.write_all(json.as_bytes())?;
    Ok(())
}
