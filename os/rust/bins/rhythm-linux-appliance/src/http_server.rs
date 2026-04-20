//! Linux appliance HTTP router.
//!
//! Wraps the standard server router with platform-specific Wi-Fi recovery
//! endpoints for Linux appliances.

use axum::routing::get;
use axum::Router;
use rhythm_os::handlers::ApiResponse;
use rhythm_os::state::SharedState;

use crate::ble_provision::ProvisioningManager;
use crate::wifi;

pub fn create_router(state: SharedState, provisioning: ProvisioningManager) -> Router {
    let router_state = state.clone();
    rhythm_server::http_server::create_router(router_state).route(
        "/api/wifi",
        get({
            let provisioning = provisioning.clone();
            move || async move { handle_get_wifi(&provisioning) }
        })
        .delete({
            let provisioning = provisioning.clone();
            let state = state.clone();
            move || async move { handle_delete_wifi(&state, &provisioning) }
        }),
    )
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
