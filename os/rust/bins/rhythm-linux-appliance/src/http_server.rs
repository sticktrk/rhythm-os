//! Linux appliance HTTP router.
//!
//! Wraps the standard server router with platform-specific Wi-Fi recovery
//! endpoints for Linux appliances.

use std::thread;
use std::time::Duration;

use axum::middleware;
use axum::routing::get;
use axum::{Json, Router};
use log::{info, warn};
use rhythm_os::handlers::ApiResponse;
use rhythm_os::provisioning::WifiCredentials;
use rhythm_os::state::SharedState;

use crate::ble_provision::ProvisioningManager;
use crate::wifi;

const WIFI_CHANGE_APPLY_DELAY: Duration = Duration::from_secs(1);
const WIFI_CHANGE_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn create_router(state: SharedState, provisioning: ProvisioningManager) -> Router {
    let router_state = state.clone();
    rhythm_server::http_server::create_router(router_state)
        .route(
            "/api/wifi",
            get({
                let provisioning = provisioning.clone();
                move || async move { handle_get_wifi(&provisioning) }
            })
            .delete({
                let provisioning = provisioning.clone();
                let state = state.clone();
                move || async move { handle_delete_wifi(&state, &provisioning) }
            })
            .put({
                let state = state.clone();
                move |Json(creds): Json<WifiCredentials>| {
                    let state = state.clone();
                    async move { handle_put_wifi(state, creds) }
                }
            }),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rhythm_os::auth::require_api_auth_middleware,
        ))
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
                    let _ = storage.clear_commissioning_wifi_credentials();
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
    use rhythm_os::state::AppState;
    use std::sync::{Arc, Mutex};

    #[test]
    fn create_router_builds_standard_and_wifi_routes() {
        let state = Arc::new(Mutex::new(AppState::default()));
        let provisioning = ProvisioningManager::new("test-version", state.clone());
        let _router = create_router(state, provisioning);
    }
}
