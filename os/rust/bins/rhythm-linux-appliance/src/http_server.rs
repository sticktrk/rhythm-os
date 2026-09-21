//! Linux appliance HTTP router.
//!
//! Wraps the standard server router with platform-specific Wi-Fi recovery
//! endpoints for Linux appliances.

use std::thread;
use std::time::Duration;

use axum::extract::Path;
use axum::middleware;
use axum::routing::{get, put};
use axum::{Json, Router};
use log::{info, warn};
use rhythm_os::handlers::ApiResponse;
use rhythm_os::provisioning::WifiCredentials;
use rhythm_os::state::SharedState;

use crate::ble_provision::ProvisioningManager;
use crate::wifi;

const WIFI_CHANGE_APPLY_DELAY: Duration = Duration::from_secs(1);
const WIFI_CHANGE_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn create_router(
    state: SharedState,
    provisioning: ProvisioningManager,
    ota_status: rhythm_server::self_update::OtaStatusHandle,
) -> Router {
    let router_state = state.clone();
    rhythm_server::http_server::create_router(router_state, ota_status)
        .route(
            "/api/wifi",
            get({
                let provisioning = provisioning.clone();
                // spawn_blocking: these handlers shell out to wpa_cli /
                // init scripts. The Pi Zero runtime has a single tokio
                // worker, so running them inline freezes the whole HTTP/SSE
                // surface while a subprocess runs (or forever, if wedged).
                move || async move {
                    tokio::task::spawn_blocking(move || handle_get_wifi(&provisioning))
                        .await
                        .unwrap_or_else(|e| {
                            ApiResponse::server_error(format!("wifi status task failed: {e}"))
                        })
                }
            })
            .delete({
                let provisioning = provisioning.clone();
                let state = state.clone();
                move || async move {
                    tokio::task::spawn_blocking(move || handle_delete_wifi(&state, &provisioning))
                        .await
                        .unwrap_or_else(|e| {
                            ApiResponse::server_error(format!("wifi delete task failed: {e}"))
                        })
                }
            })
            .put({
                let state = state.clone();
                move |Json(creds): Json<WifiCredentials>| {
                    let state = state.clone();
                    async move { handle_put_wifi(state, creds) }
                }
            }),
        )
        // Move the Box to a saved network. The password never leaves the Box,
        // so this is owner-only like every other saved-network route.
        .route(
            "/api/wifi/profile/:id",
            put({
                let state = state.clone();
                move |Path(id): Path<String>| {
                    let state = state.clone();
                    async move {
                        tokio::task::spawn_blocking(move || handle_put_wifi_profile(state, &id))
                            .await
                            .unwrap_or_else(|e| {
                                ApiResponse::server_error(format!("wifi profile task failed: {e}"))
                            })
                    }
                }
            }),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rhythm_os::auth::require_api_auth_middleware,
        ))
}

fn handle_put_wifi_profile(state: SharedState, profile_id: &str) -> ApiResponse {
    match rhythm_os::wifi_profiles::selected_credentials(&state, profile_id) {
        Ok(creds) => handle_put_wifi(state, creds),
        // Storage errors may describe network configuration. Never echo them.
        Err(_) => ApiResponse::not_found("Saved network is unavailable"),
    }
}

fn handle_get_wifi(provisioning: &ProvisioningManager) -> ApiResponse {
    let status = wifi::status_snapshot();
    let body = serde_json::json!({
        "config_present": status.config_present,
        "connected": status.connected,
        "interface": status.interface,
        "ssid": status.ssid,
        "ip_address": status.ip_address,
        "wpa_state": status.wpa_state,
        "provisioning_active": provisioning.is_running(),
        "provisioning_forced": provisioning.force_enabled(),
    });
    ApiResponse::json_ok(body.to_string())
}

fn handle_put_wifi(state: SharedState, creds: WifiCredentials) -> ApiResponse {
    if let Err(error) = wifi::validate_credentials(&creds) {
        return ApiResponse::bad_request(&format!("{:#}", error));
    }

    let ssid = creds.ssid.clone();
    match thread::Builder::new()
        .name("wifi-change".to_string())
        .spawn(move || {
            thread::sleep(WIFI_CHANGE_APPLY_DELAY);
            match wifi::connect_with_credentials_or_restore(&creds, WIFI_CHANGE_CONNECT_TIMEOUT) {
                Ok(ip) => {
                    persist_commissioning_wifi_credentials(&state, &creds);
                    info!(
                        target: "sys",
                        "Changed appliance Wi-Fi credentials for SSID '{}' (ip={})",
                        creds.ssid,
                        ip
                    );
                }
                Err(error) => warn!(
                    target: "sys",
                    "Failed to change appliance Wi-Fi credentials for SSID '{}'; restored previous Wi-Fi config when available: {:#}",
                    creds.ssid,
                    error
                ),
            }
        }) {
        Ok(_) => {
            let body = serde_json::json!({
                "status": "accepted",
                "message": "Wi-Fi change scheduled",
                "ssid": ssid,
            });
            ApiResponse::json_ok(body.to_string())
        }
        Err(error) => ApiResponse::server_error(error),
    }
}

fn handle_delete_wifi(state: &SharedState, provisioning: &ProvisioningManager) -> ApiResponse {
    match wifi::clear_credentials_and_restart() {
        Ok(()) => {
            if let Ok(state) = state.lock() {
                if let Some(storage) = state.storage.as_ref() {
                    // Forget only the Box pointer. Startup recovery must not
                    // rejoin a network the owner removed; saved accessory
                    // networks are not the Box's to erase.
                    if let Err(error) = storage.clear_box_wifi_credentials() {
                        warn!(target: "sys", "Failed to forget the Box network: {:#}", error);
                    }
                }
            }
            if let Err(e) = provisioning.ensure_running("api-delete-wifi") {
                return ApiResponse::server_error(e);
            }
            let body = serde_json::json!({
                "status": "ok",
                "message": "Wi-Fi credentials cleared",
                "provisioning_active": provisioning.is_running(),
            });
            ApiResponse::json_ok(body.to_string())
        }
        Err(e) => ApiResponse::server_error(e),
    }
}

fn persist_commissioning_wifi_credentials(state: &SharedState, creds: &WifiCredentials) {
    let result = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))
        .and_then(|state| {
            let storage = state
                .storage
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
            storage.save_commissioning_wifi_credentials(creds)
        });

    if let Err(error) = result {
        warn!(
            target: "sys",
            "Failed to persist changed appliance Wi-Fi credentials for SSID '{}': {:#}",
            creds.ssid,
            error
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use rhythm_os::state::AppState;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    #[test]
    fn create_router_builds_standard_and_wifi_routes() {
        let state = Arc::new(Mutex::new(AppState::default()));
        let provisioning = ProvisioningManager::new("test-version", state.clone());
        let _router = create_router(
            state,
            provisioning,
            rhythm_server::self_update::OtaStatusHandle::new("test-version"),
        );
    }

    #[tokio::test]
    async fn box_move_to_saved_network_is_owner_only_and_never_echoes_secrets() {
        use axum::extract::ConnectInfo;
        use rhythm_os::storage::{FileStorage, Storage};
        let data_dir = unique_test_dir("wifi-profile-route");
        let storage = Arc::new(FileStorage::new(data_dir.to_str().unwrap()).unwrap());
        storage
            .save_commissioning_wifi_credentials(&WifiCredentials {
                ssid: "FixtureHome".into(),
                password: "fixture-password".into(),
            })
            .unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().storage = Some(storage);
        let owner = rhythm_os::auth::issue_local_owner_token(&state, Some("fixture".into()))
            .unwrap()
            .token;
        let provisioning = ProvisioningManager::new("test-version", state.clone());
        let router = create_router(
            state,
            provisioning,
            rhythm_server::self_update::OtaStatusHandle::new("test-version"),
        );
        let request = |token: Option<&str>| {
            let mut builder = Request::builder()
                .method("PUT")
                .uri("/api/wifi/profile/00000000-0000-4000-8000-000000000000");
            if let Some(token) = token {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            let mut request = builder.body(Body::empty()).unwrap();
            // A LAN peer: saved-network routes need the owner even here.
            request
                .extensions_mut()
                .insert(ConnectInfo(std::net::SocketAddr::from((
                    [192, 168, 1, 42],
                    49152,
                ))));
            request
        };
        let anonymous = router.clone().oneshot(request(None)).await.unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        let missing = router.oneshot(request(Some(&owner))).await.unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(missing.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(!body.contains("FixtureHome") && !body.contains("fixture-password"));
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[tokio::test]
    async fn debug_bundle_route_returns_download_attachment() {
        let data_dir = unique_test_dir("debug-bundle-rpiz");
        std::fs::create_dir_all(data_dir.join("log")).unwrap();
        std::fs::write(data_dir.join("log").join("rhythm-server.log"), b"rpiz-log").unwrap();

        let state = Arc::new(Mutex::new(AppState::default()));
        {
            let mut state = state.lock().unwrap();
            state.firmware_version = "1.2.3";
            state.platform_type = "appliance";
            state.platform_context = "rpiz";
            state.data_dir = data_dir.display().to_string();
        }
        let provisioning = ProvisioningManager::new("test-version", state.clone());

        let response = create_router(
            state,
            provisioning,
            rhythm_server::self_update::OtaStatusHandle::new("test-version"),
        )
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/diag/debug-bundle")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

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
                value.starts_with("attachment; filename=\"rhythm-debug-bundle-rpiz-")
                    && value.ends_with(".tar.gz\"")
            }));
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert!(!body.is_empty());

        let _ = std::fs::remove_dir_all(data_dir);
    }

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-rpiz-{}-{}", name, nanos));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
