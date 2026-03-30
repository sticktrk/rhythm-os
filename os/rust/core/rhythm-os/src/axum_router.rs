//! Shared Axum router for rhythm-server and rhythm-addon.
//!
//! Provides `api_routes()` for standard REST endpoints.

use std::collections::HashMap;
use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::stream::Stream;
use serde_json::Value;

use crate::handlers::{self, ApiResponse};
use crate::server_event::ServerEvent;
use crate::state::SharedState;

// ---------------------------------------------------------------------------
// IntoResponse for ApiResponse
// ---------------------------------------------------------------------------

impl IntoResponse for ApiResponse {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, [("content-type", self.content_type)], self.body).into_response()
    }
}

// ---------------------------------------------------------------------------
// Router constructors
// ---------------------------------------------------------------------------

/// Standard API routes (PUT-only for mutation endpoints).
pub fn api_routes() -> Router<SharedState> {
    shared_routes()
        .route("/api/rooms/action", put(room_action))
        .route("/api/rooms/brightness", put(set_brightness))
        .route("/api/rooms/offset", put(set_time_offset))
        .route("/api/rooms/fix", post(fix_my_lights))
        .route("/api/sync", post(post_sync))
}

/// Routes common to all variants.
fn shared_routes() -> Router<SharedState> {
    Router::new()
        .route("/health", get(health))
        .route("/api/state", get(get_state))
        .route("/api/rooms/state", get(get_rooms_state))
        .route("/api/events", get(sse_events))
        .route("/api/rooms", put(put_rooms).delete(delete_room))
        .route("/api/devices", put(put_devices).delete(delete_device))
        .route("/api/motion-timeout", put(put_motion_timeout))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/config/absorb-offset", post(absorb_time_offset))
        .route("/api/config/reset", post(reset_config))
        .route("/api/location", put(put_location))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route(
            "/api/hub/credentials",
            put(put_hub_credentials).delete(delete_hub),
        )
        .route("/api/rooms/preferences", put(put_room_preferences))
        .route("/api/ota/version", get(get_version))
        // Canonical device management
        .route("/api/devices/canonical", get(get_canonical_devices))
        .route("/api/devices/canonical/:id", get(get_canonical_device))
        .route("/api/devices/canonical/:id/room", put(put_device_room))
        .route(
            "/api/devices/canonical/:id/preferred",
            put(put_device_preferred),
        )
        // Triage queue (unified — devices + rooms)
        .route("/api/triage", get(get_triage))
        .route("/api/triage/count", get(get_triage_count))
        .route("/api/triage/:id/merge", put(put_triage_merge))
        .route("/api/triage/:id/new", put(put_triage_new))
        .route("/api/triage/:id/dismiss", put(put_triage_dismiss))
        .route("/api/triage/:id/bind", put(put_triage_bind))
        // Legacy triage paths (backward compat)
        .route("/api/devices/triage", get(get_triage))
        .route("/api/devices/triage/:id/merge", put(put_triage_merge))
        .route("/api/devices/triage/:id/new", put(put_triage_new))
        .route("/api/devices/triage/:id/dismiss", put(put_triage_dismiss))
        // Topology room management
        .route(
            "/api/topology/rooms",
            get(get_topology_rooms).post(post_topology_room),
        )
        .route("/api/topology/rooms/:id", put(put_topology_rename))
        .route("/api/topology/rooms/:id/merge", put(put_topology_merge))
        .route(
            "/api/topology/rooms/:id/devices/move",
            put(put_topology_move_device),
        )
        // Device pairing (Matter commissioning, Zigbee permit join)
        .route("/api/devices/pair", post(post_pair_device))
        // Curve visualization
        .route("/api/curve", get(get_curve).post(post_curve_preview))
        .route("/api/curve/now", get(get_curve_now))
        .route("/api/curve/solar", get(get_curve_solar))
}

// ---------------------------------------------------------------------------
// Non-blocking handlers (thin wrappers)
// ---------------------------------------------------------------------------

async fn health() -> ApiResponse {
    handlers::handle_health()
}

async fn get_state(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_state(&state)
}

async fn get_rooms_state(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_rooms_state(&state)
}

async fn put_rooms(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    handlers::handle_put_rooms(&state, &body, true)
}

async fn delete_room(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    match params.get("id") {
        Some(id) => handlers::handle_delete_room(&state, id),
        None => ApiResponse::bad_request("Missing ?id="),
    }
}

async fn put_devices(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    handlers::handle_put_devices(&state, &body, true)
}

async fn delete_device(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    match params.get("id") {
        Some(id) => handlers::handle_delete_device(&state, id),
        None => ApiResponse::bad_request("Missing ?id="),
    }
}

async fn put_motion_timeout(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_motion_timeout(&state, &body)
}

async fn get_config(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_config(&state)
}

async fn put_config(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    handlers::handle_put_config(&state, &body)
}

async fn absorb_time_offset(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_absorb_time_offset(&state, &body)
}

async fn reset_config(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_reset_config(&state)
}

async fn put_location(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    handlers::handle_put_location(&state, &body)
}

async fn get_settings(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_settings(&state)
}

async fn put_settings(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    run_blocking(move || handlers::handle_put_settings(&state, &body)).await
}

async fn put_room_preferences(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_put_room_preferences(&state, &body, true)).await
}

async fn get_version(State(state): State<SharedState>) -> ApiResponse {
    let version = state.lock().map(|s| s.firmware_version).unwrap_or("0.0.0");
    handlers::handle_get_version(version)
}

// ---------------------------------------------------------------------------
// Canonical device handlers
// ---------------------------------------------------------------------------

async fn get_canonical_devices(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_canonical_devices(&state)
}

async fn get_canonical_device(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> ApiResponse {
    handlers::handle_get_canonical_device(&state, &id)
}

async fn put_device_room(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_device_room(&state, &id, &body)
}

async fn put_device_preferred(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_device_preferred(&state, &id, &body)
}

// ---------------------------------------------------------------------------
// Triage handlers
// ---------------------------------------------------------------------------

async fn get_triage(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_triage(&state)
}

async fn put_triage_merge(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_triage_merge(&state, &id, &body)
}

async fn put_triage_new(State(state): State<SharedState>, Path(id): Path<String>) -> ApiResponse {
    handlers::handle_put_triage_new(&state, &id)
}

async fn put_triage_dismiss(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> ApiResponse {
    handlers::handle_put_triage_dismiss(&state, &id)
}

async fn put_triage_bind(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_triage_bind(&state, &id, &body)
}

async fn get_triage_count(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_triage_count(&state)
}

// ---------------------------------------------------------------------------
// Topology handlers
// ---------------------------------------------------------------------------

async fn get_topology_rooms(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_topology_rooms(&state)
}

async fn post_topology_room(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_post_topology_room(&state, &body)
}

async fn put_topology_rename(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_topology_rename(&state, &id, &body)
}

async fn put_topology_merge(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_topology_merge(&state, &id, &body)
}

async fn put_topology_move_device(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_topology_move_device(&state, &id, &body)
}

// ---------------------------------------------------------------------------
// Curve visualization handlers
// ---------------------------------------------------------------------------

async fn get_curve(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    let qp = parse_curve_query(&params);
    handlers::handle_get_curve(&state, &qp)
}

async fn post_curve_preview(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> ApiResponse {
    let qp = parse_curve_query(&params);
    handlers::handle_post_curve(&state, &body, &qp)
}

async fn get_curve_now(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    let hour = params.get("hour").and_then(|v| v.parse::<f32>().ok());
    handlers::handle_get_curve_now(&state, hour)
}

async fn get_curve_solar(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    handlers::handle_get_curve_solar(&state, params.get("date").map(|s| s.as_str()))
}

fn parse_curve_query(params: &HashMap<String, String>) -> handlers::CurveQueryParams {
    handlers::CurveQueryParams {
        samples_per_hour: params.get("samples_per_hour").and_then(|v| v.parse().ok()),
        date: params.get("date").cloned(),
        start_hour: params.get("start_hour").and_then(|v| v.parse().ok()),
        max_steps: params.get("max_steps").and_then(|v| v.parse().ok()),
    }
}

// ---------------------------------------------------------------------------
// Blocking handlers — run on a real std::thread (not spawn_blocking)
// because reqwest::blocking::Client panics if used inside a tokio runtime.
// ---------------------------------------------------------------------------

/// Run a closure on a dedicated std::thread, returning the result via oneshot.
async fn run_blocking<F, R>(f: F) -> ApiResponse
where
    F: FnOnce() -> R + Send + 'static,
    R: Into<ApiResponse> + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("http-handler".to_string())
        .spawn(move || {
            let result = f();
            let _ = tx.send(result.into());
        })
        .ok();
    match rx.await {
        Ok(resp) => resp,
        Err(_) => ApiResponse::server_error("Handler thread panicked"),
    }
}

pub async fn put_hub_credentials(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_put_hub_credentials(&state, &body)).await
}

pub async fn delete_hub(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    let hub_type = params.get("hub_type").cloned();
    let address = params.get("address").cloned();
    run_blocking(move || {
        handlers::handle_delete_hub(&state, hub_type.as_deref(), address.as_deref())
    })
    .await
}

pub async fn room_action(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    run_blocking(move || handlers::handle_room_action(&state, &body, true)).await
}

pub async fn set_brightness(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_set_brightness(&state, &body, true)).await
}

pub async fn set_time_offset(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_set_time_offset(&state, &body, true)).await
}

pub async fn fix_my_lights(State(state): State<SharedState>) -> ApiResponse {
    run_blocking(move || handlers::handle_fix_my_lights(&state, true)).await
}

pub async fn post_sync(State(state): State<SharedState>) -> ApiResponse {
    run_blocking(move || handlers::handle_post_sync(&state)).await
}

pub async fn post_pair_device(
    State(state): State<SharedState>,
    Json(body): Json<crate::pairing::PairingRequest>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_pair_device(&state, &body)).await
}

// ---------------------------------------------------------------------------
// SSE endpoint
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SSE endpoint
// ---------------------------------------------------------------------------

async fn sse_events(
    State(state): State<SharedState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state
        .lock()
        .ok()
        .and_then(|s| s.event_tx.as_ref().map(|tx| tx.subscribe()));

    let stream = futures::stream::unfold(rx, |rx_opt| async move {
        let mut rx = rx_opt?;
        match rx.recv().await {
            Ok(event) => {
                let event_type = match &event {
                    ServerEvent::RoomState { .. } => "room_state",
                    ServerEvent::MotionTimer { .. } => "motion_timer",
                    ServerEvent::HubStatus { .. } => "hub_status",
                    ServerEvent::SettingsChanged => "settings_changed",
                    ServerEvent::ConfigChanged => "config_changed",
                    ServerEvent::RoomsChanged => "rooms_changed",
                    ServerEvent::TriageChanged { .. } => "triage_changed",
                };
                let data = serde_json::to_string(&event).unwrap_or_default();
                let sse_event = Event::default().event(event_type).data(data);
                Some((Ok::<_, Infallible>(sse_event), Some(rx)))
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                log::warn!(target: "sse", "SSE client lagged by {} events", n);
                let event = Event::default().event("lagged").data("{}");
                Some((Ok::<_, Infallible>(event), Some(rx)))
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use axum::http::{Method as HttpMethod, Request, StatusCode};
    use tower::util::ServiceExt;

    use crate::routes::SHARED_API_ROUTES;

    /// Verify every shared route is registered in the axum router.
    ///
    /// Sends a request for each (path, method) in `SHARED_API_ROUTES` and
    /// asserts the response is NOT 404. A 404 means the route was never
    /// registered, which is the exact class of bug we want to catch.
    #[tokio::test]
    async fn shared_routes_are_registered() {
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let app = api_routes().with_state(state);

        for route in SHARED_API_ROUTES {
            for method_str in route.methods {
                let method = HttpMethod::from_bytes(method_str.as_bytes())
                    .unwrap_or_else(|_| panic!("Invalid method: {}", method_str));

                let body = if method == HttpMethod::PUT || method == HttpMethod::POST {
                    axum::body::Body::from("{}")
                } else {
                    axum::body::Body::empty()
                };

                let req = Request::builder()
                    .method(&method)
                    .uri(route.path)
                    .header("content-type", "application/json")
                    .body(body)
                    .unwrap();

                let resp = app.clone().oneshot(req).await.unwrap();
                assert_ne!(
                    resp.status(),
                    StatusCode::NOT_FOUND,
                    "Route {} {} returned 404 - not registered in axum router",
                    method_str,
                    route.path,
                );
            }
        }
    }
}
