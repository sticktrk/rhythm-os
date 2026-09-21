//! Linux appliance HTTP router.
//!
//! Wraps the standard server router with platform-specific Wi-Fi recovery
//! endpoints for Linux appliances.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use axum::extract::Path;
use axum::middleware;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use log::{info, warn};
use rhythm_os::handlers::ApiResponse;
use rhythm_os::provisioning::WifiCredentials;
use rhythm_os::state::SharedState;

use crate::ble_provision::ProvisioningManager;
use crate::wifi;

const WIFI_CHANGE_APPLY_DELAY: Duration = Duration::from_secs(1);
const WIFI_CHANGE_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// One radio: a move and a saved-network check cannot overlap.
static WIFI_RADIO_BUSY: AtomicBool = AtomicBool::new(false);

/// The latest saved-network check. Memory only: a restart abandons the check
/// and startup recovery returns the Box to its own network.
static WIFI_VERIFICATION: Mutex<Option<WifiVerification>> = Mutex::new(None);

struct WifiVerification {
    operation_id: String,
    /// `None` while the Box is away checking.
    outcome: Option<Result<(), wifi::WifiVerifyFailure>>,
}

#[derive(serde::Deserialize)]
struct WifiVerifyRequest {
    operation_id: String,
    ssid: String,
    password: String,
}

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
        // Prove a network before it is saved for accessories. The Box leaves
        // its own network to do so, so the answer is collected afterwards.
        .route(
            "/api/wifi/verify",
            post(|Json(request): Json<WifiVerifyRequest>| async move {
                tokio::task::spawn_blocking(move || handle_post_wifi_verify(request))
                    .await
                    .unwrap_or_else(|e| {
                        ApiResponse::server_error(format!("wifi verify task failed: {e}"))
                    })
            }),
        )
        .route(
            "/api/wifi/verify/:operation_id",
            get(|Path(id): Path<String>| async move { handle_get_wifi_verify(&id) }),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rhythm_os::auth::require_api_auth_middleware,
        ))
}

fn verification_body(
    operation_id: &str,
    outcome: Option<Result<(), wifi::WifiVerifyFailure>>,
) -> String {
    let (state, reason) = match outcome {
        None => ("running", None),
        Some(Ok(())) => ("passed", None),
        Some(Err(failure)) => ("failed", Some(failure.as_str())),
    };
    serde_json::json!({ "operation_id": operation_id, "state": state, "reason": reason })
        .to_string()
}

fn record_verification(operation_id: &str, outcome: Option<Result<(), wifi::WifiVerifyFailure>>) {
    if let Ok(mut latest) = WIFI_VERIFICATION.lock() {
        *latest = Some(WifiVerification {
            operation_id: operation_id.to_string(),
            outcome,
        });
    }
}

fn handle_post_wifi_verify(request: WifiVerifyRequest) -> ApiResponse {
    if uuid::Uuid::parse_str(&request.operation_id).is_err() {
        return ApiResponse::bad_request("A UUID operation_id is required");
    }
    let candidate = WifiCredentials {
        ssid: request.ssid,
        password: request.password,
    };
    if let Err(error) = wifi::validate_credentials(&candidate) {
        return ApiResponse::bad_request(&format!("{:#}", error));
    }
    // Without a network of its own the Box has nowhere to return to.
    let Ok(Some(previous)) = wifi::load_configured_credentials() else {
        return ApiResponse::conflict(
            "The Rhythm Box is not on Wi-Fi, so it cannot check a network",
        );
    };
    let operation_id = request.operation_id;
    // The Box is already living proof of its own network.
    if previous.ssid == candidate.ssid && previous.password == candidate.password {
        return ApiResponse::json_ok(verification_body(&operation_id, Some(Ok(()))));
    }
    if WIFI_RADIO_BUSY.swap(true, Ordering::SeqCst) {
        return ApiResponse::conflict("The Rhythm Box is already changing or checking Wi-Fi");
    }
    record_verification(&operation_id, None);
    let spawned = thread::Builder::new()
        .name("wifi-verify".to_string())
        .spawn({
            let operation_id = operation_id.clone();
            move || {
                thread::sleep(WIFI_CHANGE_APPLY_DELAY);
                let outcome = wifi::verify_credentials_then_restore(
                    &candidate,
                    &previous,
                    WIFI_CHANGE_CONNECT_TIMEOUT,
                );
                info!(
                    target: "sys",
                    "Checked a saved network for accessories: {}",
                    match outcome {
                        Ok(()) => "passed",
                        Err(failure) => failure.as_str(),
                    }
                );
                record_verification(&operation_id, Some(outcome));
                WIFI_RADIO_BUSY.store(false, Ordering::SeqCst);
            }
        });
    match spawned {
        Ok(_) => ApiResponse::json_ok(verification_body(&operation_id, None)),
        Err(error) => {
            WIFI_RADIO_BUSY.store(false, Ordering::SeqCst);
            if let Ok(mut latest) = WIFI_VERIFICATION.lock() {
                *latest = None;
            }
            ApiResponse::server_error(error)
        }
    }
}

fn handle_get_wifi_verify(operation_id: &str) -> ApiResponse {
    match WIFI_VERIFICATION.lock() {
        Ok(latest) => match latest.as_ref().filter(|v| v.operation_id == operation_id) {
            Some(found) => ApiResponse::json_ok(verification_body(operation_id, found.outcome)),
            None => ApiResponse::not_found("No such Wi-Fi check"),
        },
        Err(_) => ApiResponse::server_error("Wi-Fi check state is unavailable"),
    }
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

    if WIFI_RADIO_BUSY.swap(true, Ordering::SeqCst) {
        return ApiResponse::conflict("The Rhythm Box is already changing or checking Wi-Fi");
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
            WIFI_RADIO_BUSY.store(false, Ordering::SeqCst);
        }) {
        Ok(_) => {
            let body = serde_json::json!({
                "status": "accepted",
                "message": "Wi-Fi change scheduled",
                "ssid": ssid,
            });
            ApiResponse::json_ok(body.to_string())
        }
        Err(error) => {
            WIFI_RADIO_BUSY.store(false, Ordering::SeqCst);
            ApiResponse::server_error(error)
        }
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

    #[test]
    fn wifi_check_reports_state_without_the_credential_and_never_leaves_without_a_way_back() {
        let id = "00000000-0000-4000-8000-000000000001";
        assert_eq!(handle_get_wifi_verify(id).status, 404);
        assert_eq!(
            handle_post_wifi_verify(WifiVerifyRequest {
                operation_id: "not-a-uuid".into(),
                ssid: "Garage".into(),
                password: "fixture-password".into(),
            })
            .status,
            400
        );
        // This host has no Box network to return to, so the radio is untouched.
        let refused = handle_post_wifi_verify(WifiVerifyRequest {
            operation_id: id.into(),
            ssid: "Garage".into(),
            password: "fixture-password".into(),
        });
        assert_eq!(refused.status, 409);
        assert!(!WIFI_RADIO_BUSY.load(Ordering::SeqCst));
        assert_eq!(handle_get_wifi_verify(id).status, 404);

        for (outcome, state, reason) in [
            (None, "running", serde_json::Value::Null),
            (Some(Ok(())), "passed", serde_json::Value::Null),
            (
                Some(Err(wifi::WifiVerifyFailure::NotFound)),
                "failed",
                "not_found".into(),
            ),
            (
                Some(Err(wifi::WifiVerifyFailure::JoinFailed)),
                "failed",
                "join_failed".into(),
            ),
        ] {
            record_verification(id, outcome);
            let response = handle_get_wifi_verify(id);
            assert_eq!(response.status, 200);
            let body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
            assert_eq!(body["state"], state);
            assert_eq!(body["reason"], reason);
            assert!(!response.body.contains("fixture-password"));
        }
        *WIFI_VERIFICATION.lock().unwrap() = None;
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
