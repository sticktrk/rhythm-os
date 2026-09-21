//! The add-on exposes lighting operations for one local HA connection.
//! New shared appliance routes are unavailable until explicitly admitted here.

use axum::{
    body::Body,
    extract::State,
    http::{Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use rhythm_os::state::SharedState;
use serde_json::json;

const READ_PATHS: &[&str] = &[
    "/health",
    "/api/state",
    "/api/nodes/state",
    "/api/settings",
    "/api/light-breaker",
    "/api/mode",
    "/api/transitions",
    "/api/light-schedules",
    "/api/input-bindings",
    "/api/profiles",
    "/api/scenes",
    "/api/config",
    "/api/curve",
    "/api/curve/now",
    "/api/curve/solar",
    "/api/topology/nodes",
    "/api/devices/canonical",
    "/api/triage",
    "/api/triage/count",
    "/api/history",
    "/api/light-runtime",
    "/api/light-runtimes",
    "/api/light-runtimes/:id/manifest",
    "/api/profile-bundle",
    "/api/profile-bundle/factory-default",
    "/api/addon/status",
    "/api/addon/lights",
];

const WRITE_PATHS: &[(&str, &str)] = &[
    ("PUT", "/api/addon/lights"),
    ("PUT", "/api/light-runtime"),
    ("POST", "/api/sync"),
    ("POST", "/api/hub/retry"),
    ("POST", "/api/curve"),
    ("PUT", "/api/config"),
    ("POST", "/api/config/absorb-offset"),
    ("POST", "/api/config/reset"),
    ("PUT", "/api/nodes/action"),
    ("PUT", "/api/nodes/brightness"),
    ("PUT", "/api/nodes/curve"),
    ("PUT", "/api/nodes/color"),
    ("PUT", "/api/nodes/offset"),
    ("PUT", "/api/nodes/preferences"),
    ("PUT", "/api/nodes/motion-activation"),
    ("PUT", "/api/nodes/profile-overrides"),
    ("PUT", "/api/light-breaker"),
    ("PUT", "/api/mode"),
    ("PUT", "/api/transitions"),
    ("POST", "/api/transitions/:id/trigger"),
    ("PUT", "/api/light-schedules"),
    ("PUT", "/api/light-schedules/assignment"),
    ("PUT", "/api/light-schedules/override"),
    (
        "POST",
        "/api/light-schedules/:id/transitions/:transition/trigger",
    ),
    ("POST", "/api/input-bindings"),
    ("PUT", "/api/input-bindings/:id"),
    ("DELETE", "/api/input-bindings/:id"),
    ("POST", "/api/scenes"),
    ("PUT", "/api/scenes/:id"),
    ("DELETE", "/api/scenes/:id"),
    ("POST", "/api/scenes/preview"),
    ("POST", "/api/scenes/:id/apply"),
    ("POST", "/api/scenes/:id/apply-home"),
    ("POST", "/api/scenes/:id/preview"),
    ("POST", "/api/scene-previews/:id/commit"),
    ("POST", "/api/scene-previews/:id/cancel"),
    ("PUT", "/api/profile-bundle"),
    ("POST", "/api/profile-bundle/reset"),
    ("POST", "/api/factory-reset"),
];

fn matches_path(pattern: &str, path: &str) -> bool {
    let pattern: Vec<_> = pattern.split('/').collect();
    let path: Vec<_> = path.split('/').collect();
    pattern.len() == path.len()
        && pattern.iter().zip(path).all(|(part, actual)| {
            if part.starts_with(':') {
                !actual.is_empty() && actual != "." && actual != ".." && !actual.contains('%')
            } else {
                *part == actual
            }
        })
}

pub fn allows(method: &Method, path: &str) -> bool {
    if path.contains('%') || path.contains('\\') {
        return false;
    }
    if method == Method::GET {
        return READ_PATHS.iter().any(|pattern| matches_path(pattern, path));
    }
    WRITE_PATHS
        .iter()
        .any(|(verb, pattern)| method.as_str() == *verb && matches_path(pattern, path))
}

pub async fn enforce(
    State(state): State<SharedState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !allows(request.method(), request.uri().path()) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "This operation is unavailable in the Home Assistant add-on",
                "code": "addon_operation_unavailable"
            })),
        )
            .into_response();
    }
    // Positive proof is required even during the interval after factory reset,
    // when the shared state has returned to its ordinary permissive defaults.
    let token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if !token.is_some_and(|token| {
        state
            .lock()
            .ok()
            .is_some_and(|s| s.api_auth.verify_token(token))
    }) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Local admin authentication required"})),
        )
            .into_response();
    }
    next.run(request).await
}

pub async fn status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.lock().expect("state lock");
    Json(json!({
        "deployment": "home_assistant_addon",
        "schema_version": 1,
        "connection": {"provider": "homeassistant", "managed": true,
            "connected": s.has_any_connected_hub(), "configured_count": s.hub_credentials.len()},
        "updates": "home_assistant",
        "location_source": "home_assistant",
        "light_breaker_enabled": s.light_breaker_enabled,
        "version": s.firmware_version,
        "capabilities": {"direct_hubs": false, "pairing": false, "cloud": false,
            "appliance_ota": false, "profile_export": true, "full_backup_import": false},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appliance_and_generic_escape_routes_are_closed() {
        for (method, path) in [
            (Method::PUT, "/api/hub/credentials"),
            (Method::POST, "/api/devices/pair"),
            (Method::PUT, "/api/backup"),
            (Method::GET, "/api/backup"),
            (Method::PUT, "/api/remote-access/config"),
            (Method::PUT, "/api/settings"),
            (Method::POST, "/api/auth/claim"),
            (Method::GET, "/api/matter/setup-code/1"),
            (Method::POST, "/api/ota/update"),
            (Method::PUT, "/api/location"),
            (Method::POST, "/api/scenes/%2e%2e/apply"),
            (Method::PUT, "/api/light-runtimes/adaptive/arbitrary"),
        ] {
            assert!(!allows(&method, path), "{method} {path}");
        }
        assert!(allows(&Method::GET, "/api/state"));
        assert!(allows(&Method::POST, "/api/scenes/evening/apply"));
        assert!(allows(&Method::PUT, "/api/config"));
        assert!(!allows(&Method::DELETE, "/api/config"));
    }
}
