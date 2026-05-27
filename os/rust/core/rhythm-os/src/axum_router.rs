//! Shared Axum router for rhythm-server and rhythm-addon.
//!
//! Provides `api_routes()` for standard REST endpoints.

use std::collections::HashMap;
use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures::stream::Stream;
use serde_json::Value;

use crate::handlers::{self, ApiResponse};
use crate::server_event::ServerEvent;
use crate::state::SharedState;

// ---------------------------------------------------------------------------
// IntoResponse for ApiResponse
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ApiErrorContext(pub String);

impl IntoResponse for ApiResponse {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let error_context = status
            .is_server_error()
            .then(|| ApiErrorContext(self.body.clone()));
        let mut response =
            (status, [("content-type", self.content_type)], self.body).into_response();
        if let Some(error_context) = error_context {
            response.extensions_mut().insert(error_context);
        }
        response
    }
}

// ---------------------------------------------------------------------------
// Router constructors
// ---------------------------------------------------------------------------

/// Standard API routes (PUT-only for mutation endpoints).
pub fn api_routes() -> Router<SharedState> {
    shared_routes()
        .route("/api/nodes/action", put(node_action))
        .route("/api/nodes/brightness", put(set_node_brightness))
        .route("/api/nodes/offset", put(set_node_time_offset))
        .route("/api/sync", post(post_sync))
}

/// Routes common to all variants.
fn shared_routes() -> Router<SharedState> {
    Router::new()
        .route("/health", get(health))
        .route("/api/auth/status", get(get_auth_status))
        .route("/api/auth/claim", post(post_auth_claim))
        .route("/api/auth/settings", put(put_auth_settings))
        .route("/api/state", get(get_state))
        .route(
            "/api/profile-bundle",
            get(get_profile_bundle).put(put_profile_bundle),
        )
        .route(
            "/api/profile-bundle/factory-default",
            get(get_factory_default_profile_bundle),
        )
        .route("/api/profile-bundle/reset", post(post_profile_bundle_reset))
        .route(
            "/api/share-bundle",
            get(get_profile_bundle).put(put_profile_bundle),
        )
        .route(
            "/api/share-bundle/factory-default",
            get(get_factory_default_profile_bundle),
        )
        .route("/api/share-bundle/reset", post(post_profile_bundle_reset))
        .route("/api/factory-reset", post(post_factory_reset))
        .route("/api/backup", get(get_backup).put(put_backup))
        .route("/api/nodes/state", get(get_nodes_state))
        .route("/api/events", get(sse_events))
        .route("/api/devices", delete(delete_device))
        .route("/api/nodes/motion-timeout", put(put_motion_timeout))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/config/absorb-offset", post(absorb_time_offset))
        .route("/api/config/reset", post(reset_config))
        .route("/api/location", put(put_location))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/mode", get(get_mode).put(put_mode))
        .route(
            "/api/transitions",
            get(get_transitions).put(put_transitions),
        )
        .route(
            "/api/transitions/:id/trigger",
            post(post_transition_trigger),
        )
        .route(
            "/api/input-bindings",
            get(get_input_bindings).post(post_input_binding),
        )
        .route(
            "/api/input-bindings/:id",
            put(put_input_binding).delete(delete_input_binding),
        )
        .route("/api/profiles", get(get_profiles))
        .route(
            "/api/hub/credentials",
            put(put_hub_credentials).delete(delete_hub),
        )
        .route("/api/hub/retry", post(post_hub_retry))
        .route("/api/nodes/preferences", put(put_node_preferences))
        // Canonical device management
        .route("/api/devices/canonical", get(get_canonical_devices))
        .route("/api/devices/canonical/:id", get(get_canonical_device))
        .route("/api/devices/canonical/:id/room", put(put_device_room))
        .route("/api/devices/canonical/:id/parent", put(put_device_parent))
        .route(
            "/api/devices/canonical/:id/preferred",
            put(put_device_preferred),
        )
        .route("/api/devices/canonical/:id/flash", post(post_device_flash))
        // Triage queue (unified — devices + rooms)
        .route("/api/triage", get(get_triage))
        .route("/api/triage/count", get(get_triage_count))
        .route("/api/triage/:id/merge", put(put_triage_merge))
        .route("/api/triage/:id/new", put(put_triage_new))
        .route("/api/triage/:id/dismiss", put(put_triage_dismiss))
        .route("/api/triage/:id/room", put(put_triage_room))
        .route("/api/triage/:id/bind", put(put_triage_bind))
        // Topology room management
        .route(
            "/api/topology/rooms",
            get(get_topology_rooms).post(post_topology_room),
        )
        .route("/api/topology/nodes", get(get_topology_nodes))
        .route(
            "/api/topology/rooms/:id",
            put(put_topology_rename).delete(delete_topology_room),
        )
        .route("/api/topology/rooms/:id/merge", put(put_topology_merge))
        .route(
            "/api/topology/rooms/:id/devices/move",
            put(put_topology_move_device),
        )
        .route(
            "/api/topology/nodes/:id/controls/:kind",
            put(put_topology_node_control),
        )
        // Device pairing / unpairing (Matter commissioning, Zigbee permit join)
        .route("/api/devices/pair", post(post_pair_device))
        .route("/api/devices/unpair", post(post_unpair_device))
        .route("/api/matter/captures", get(get_matter_captures))
        .route("/api/matter/captures/:id", get(get_matter_capture))
        .route("/api/matter/bulb-test/run", post(post_matter_bulb_test_run))
        .route(
            "/api/matter/bulb-test/report",
            post(post_matter_bulb_test_report),
        )
        // Curve visualization
        .route("/api/curve", get(get_curve).post(post_curve_preview))
        .route("/api/curve/now", get(get_curve_now))
        .route("/api/curve/solar", get(get_curve_solar))
        // Light profile selection
        .route("/api/light-profile", put(put_light_profile))
}

// ---------------------------------------------------------------------------
// Non-blocking handlers (thin wrappers)
// ---------------------------------------------------------------------------

async fn health() -> ApiResponse {
    handlers::handle_health()
}

async fn get_auth_status(State(state): State<SharedState>) -> ApiResponse {
    crate::auth::handle_get_auth_status(&state)
}

async fn post_auth_claim(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    let label = body
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string);
    crate::auth::handle_claim_owner_token(&state, label)
}

async fn put_auth_settings(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    crate::auth::handle_put_auth_settings(&state, &body)
}

async fn get_state(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    let authoritative = params
        .get("authoritative")
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(false);
    run_blocking(move || handlers::handle_get_state_with_options(&state, authoritative)).await
}

async fn get_profile_bundle(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_profile_bundle(&state)
}

async fn put_profile_bundle(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_put_profile_bundle(&state, &body)).await
}

async fn get_factory_default_profile_bundle() -> ApiResponse {
    handlers::handle_get_factory_default_profile_bundle()
}

async fn post_profile_bundle_reset(State(state): State<SharedState>) -> ApiResponse {
    run_blocking(move || handlers::handle_post_profile_bundle_reset(&state)).await
}

async fn post_factory_reset(State(state): State<SharedState>) -> ApiResponse {
    run_blocking(move || handlers::handle_post_factory_reset(&state)).await
}

async fn get_backup(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    let include_secrets = params
        .get("include_secrets")
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(false);
    run_blocking(move || handlers::handle_get_backup(&state, include_secrets)).await
}

async fn put_backup(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    run_blocking(move || handlers::handle_put_backup(&state, &body)).await
}

async fn get_nodes_state(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_nodes_state(&state)
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

async fn node_action(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    run_blocking(move || handlers::handle_node_action(&state, &body, true)).await
}

async fn set_node_brightness(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_set_node_brightness(&state, &body, true)).await
}

async fn set_node_time_offset(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_set_node_time_offset(&state, &body, true)).await
}

async fn get_config(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    handlers::handle_get_config(&state, params.get("id").map(|s| s.as_str()))
}

async fn put_config(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> ApiResponse {
    let apply_outputs = params
        .get("apply")
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(false);
    handlers::handle_put_config_with_options(
        &state,
        params.get("id").map(|s| s.as_str()),
        &body,
        apply_outputs,
    )
}

async fn absorb_time_offset(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> ApiResponse {
    let profile_id = params.get("id").cloned();
    run_blocking(move || handlers::handle_absorb_time_offset(&state, profile_id.as_deref(), &body))
        .await
}

async fn reset_config(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    handlers::handle_reset_config(&state, params.get("id").map(|s| s.as_str()))
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

async fn get_mode(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_mode(&state)
}

async fn put_mode(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    run_blocking(move || handlers::handle_put_mode(&state, &body)).await
}

async fn get_transitions(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_transitions(&state)
}

async fn put_transitions(State(state): State<SharedState>, Json(body): Json<Value>) -> ApiResponse {
    run_blocking(move || handlers::handle_put_transitions(&state, &body)).await
}

async fn post_transition_trigger(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_post_transition_trigger(&state, &id)).await
}

async fn get_input_bindings(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_input_bindings(&state)
}

async fn post_input_binding(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_post_input_binding(&state, &body)).await
}

async fn put_input_binding(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_put_input_binding(&state, &id, &body)).await
}

async fn delete_input_binding(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_delete_input_binding(&state, &id)).await
}

async fn get_profiles(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_profiles(&state)
}

async fn put_node_preferences(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_put_node_preferences(&state, &body, true)).await
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

async fn put_device_parent(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_device_parent(&state, &id, &body)
}

async fn put_device_preferred(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_device_preferred(&state, &id, &body)
}

async fn post_device_flash(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_post_device_flash(&state, &id)).await
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

async fn put_triage_room(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_triage_room(&state, &id, &body)
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

async fn get_topology_nodes(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_topology_nodes(&state)
}

async fn post_topology_room(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_post_topology_room(&state, &body)
}

async fn delete_topology_room(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> ApiResponse {
    handlers::handle_delete_topology_room(&state, &id)
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

async fn put_topology_node_control(
    State(state): State<SharedState>,
    Path((id, kind)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> ApiResponse {
    handlers::handle_put_topology_node_control(&state, &id, &kind, &body)
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
    handlers::handle_get_curve_now(&state, params.get("id").map(|s| s.as_str()), hour)
}

async fn get_curve_solar(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResponse {
    handlers::handle_get_curve_solar(&state, params.get("date").map(|s| s.as_str()))
}

fn parse_curve_query(params: &HashMap<String, String>) -> handlers::CurveQueryParams {
    handlers::CurveQueryParams {
        id: params.get("id").cloned(),
        samples_per_hour: params.get("samples_per_hour").and_then(|v| v.parse().ok()),
        date: params.get("date").cloned(),
        start_hour: params.get("start_hour").and_then(|v| v.parse().ok()),
        max_steps: params.get("max_steps").and_then(|v| v.parse().ok()),
    }
}

async fn put_light_profile(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_put_light_profile(&state, &body)).await
}

// ---------------------------------------------------------------------------
// Blocking handlers — run on a real std::thread (not spawn_blocking)
// because reqwest::blocking::Client panics if used inside a tokio runtime.
// ---------------------------------------------------------------------------

const HTTP_HANDLER_STACK_SIZE: usize = 8 * 1024 * 1024;

/// Run a closure on a dedicated std::thread, returning the result via oneshot.
async fn run_blocking<F, R>(f: F) -> ApiResponse
where
    F: FnOnce() -> R + Send + 'static,
    R: Into<ApiResponse> + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("http-handler".to_string())
        .stack_size(HTTP_HANDLER_STACK_SIZE)
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

pub async fn post_hub_retry(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_post_hub_retry(&state, &body)).await
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

pub async fn post_sync(State(state): State<SharedState>) -> ApiResponse {
    run_blocking(move || handlers::handle_post_sync(&state)).await
}

pub async fn post_pair_device(
    State(state): State<SharedState>,
    Json(body): Json<crate::pairing::PairingRequest>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_pair_device(&state, &body)).await
}

pub async fn post_unpair_device(
    State(state): State<SharedState>,
    Json(body): Json<crate::pairing::UnpairingRequest>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_unpair_device(&state, &body)).await
}

pub async fn get_matter_captures(State(state): State<SharedState>) -> ApiResponse {
    handlers::handle_get_matter_captures(&state)
}

pub async fn get_matter_capture(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> ApiResponse {
    handlers::handle_get_matter_capture(&state, &id)
}

pub async fn post_matter_bulb_test_run(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_matter_bulb_test_run(&state, &body)).await
}

pub async fn post_matter_bulb_test_report(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_matter_bulb_test_report(&state, &body)).await
}

// ---------------------------------------------------------------------------
// SSE endpoint
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SSE endpoint
// ---------------------------------------------------------------------------

fn server_event_name(event: &ServerEvent) -> &'static str {
    match event {
        ServerEvent::NodeState { .. } => "node_state",
        ServerEvent::MotionTimer { .. } => "motion_timer",
        ServerEvent::InputEvent(_) => "input_event",
        ServerEvent::HubStatus { .. } => "hub_status",
        ServerEvent::SettingsChanged { .. } => "settings_changed",
        ServerEvent::ModeChanged { .. } => "mode_changed",
        ServerEvent::ConfigChanged => "config_changed",
        ServerEvent::NodesChanged => "nodes_changed",
        ServerEvent::TriageChanged { .. } => "triage_changed",
        ServerEvent::PairingProgress { .. } => "pairing_progress",
        ServerEvent::OtaUpdateProgress { .. } => "ota_update_progress",
    }
}

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
                let event_type = server_event_name(&event);
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
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    use axum::body::{to_bytes, Body};
    use axum::http::{Method as HttpMethod, Request, StatusCode};
    use rhythm_core::{LightProfileConfig, RoomSnapshot, RuntimeHandle, SolarTime};
    use serde_json::json;
    use tower::util::ServiceExt;

    use crate::canonical::identity::HubKey;
    use crate::hub::{ActiveHub, HubType};
    use crate::routes::SHARED_API_ROUTES;

    #[test]
    fn sse_event_name_includes_progress_events() {
        let input = ServerEvent::InputEvent(crate::server_event::InputEventResource::Motion {
            epoch_ms: 1778058932588,
            route: crate::server_event::InputEventRoute::NodeControl,
            hub_type: Some("test".to_string()),
            address: Some("hub.local".to_string()),
            source_node_id: Some("sensor-1".to_string()),
            target_node_id: Some("room-1".to_string()),
            source_room_id: Some("native-room-1".to_string()),
            native_sensor_id: "sensor-native-1".to_string(),
            detected: true,
        });
        assert_eq!(server_event_name(&input), "input_event");

        let pairing = ServerEvent::PairingProgress {
            hub_type: "matter".to_string(),
            session_id: Some("pair-1".to_string()),
            status: crate::pairing::PairingStatus::Searching,
            stage: crate::pairing::PairingStage::Requested,
            message: "Pairing request received".to_string(),
            device: None,
            error: None,
        };
        assert_eq!(server_event_name(&pairing), "pairing_progress");

        let ota = ServerEvent::OtaUpdateProgress {
            stage: crate::server_event::OtaUpdateStage::Downloading,
            message: "Downloading update bundle".to_string(),
            current_version: Some("0.4.192-beta".to_string()),
            target_version: Some("0.4.193-beta".to_string()),
            update_available: Some(true),
            downloaded_bytes: Some(10),
            total_bytes: Some(100),
            percent: Some(10),
            checksum_verified: None,
            installed_targets: Vec::new(),
            error: None,
        };
        assert_eq!(server_event_name(&ota), "ota_update_progress");
    }

    struct ThreadRecordingRuntime {
        calls: Arc<Mutex<Vec<String>>>,
        snapshots: Vec<RoomSnapshot>,
        current_hour: f32,
    }

    impl ThreadRecordingRuntime {
        fn record(&self, op: &str) {
            let current_thread = std::thread::current();
            let thread_name = current_thread.name().unwrap_or("unnamed").to_string();
            self.calls
                .lock()
                .unwrap()
                .push(format!("{op}@{thread_name}"));
        }
    }

    impl RuntimeHandle for ThreadRecordingRuntime {
        fn handle_event(&self, _: &rhythm_core::InputEvent) -> anyhow::Result<bool> {
            self.record("handle_event");
            Ok(true)
        }

        fn sync_rooms(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_solar(&self, _: SolarTime) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_light_profile_config(&self, _: LightProfileConfig) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_mode_configs(&self, _: Vec<rhythm_core::ModeConfig>) -> anyhow::Result<()> {
            Ok(())
        }

        fn periodic_tick_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }

        fn engine_room_snapshot(&self, room_id: &str) -> Option<RoomSnapshot> {
            self.snapshots
                .iter()
                .find(|snap| snap.id == room_id)
                .cloned()
        }

        fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
            self.snapshots.clone()
        }

        fn restore_room_state(&self, _: &str, _: rhythm_core::RestoredRoomState) {
            self.record("restore_room_state");
        }

        fn add_room(&self, _: &str, _: &str) {}

        fn remove_room(&self, _: &str) {}

        fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }

        fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn apply_room_command(
            &self,
            _: &str,
            _: rhythm_core::LightingCommand,
        ) -> anyhow::Result<()> {
            self.record("apply_room_command");
            Ok(())
        }

        fn lights_off_room(&self, _: &str, _: Option<u32>) -> anyhow::Result<()> {
            self.record("lights_off_room");
            Ok(())
        }

        fn set_power_save(&self, _: bool) -> Vec<String> {
            Vec::new()
        }

        fn is_power_save(&self) -> bool {
            false
        }

        fn set_room_brightness(&self, _: &str, _: u8) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_room_time_offset(&self, _: &str, _: f32) -> anyhow::Result<()> {
            self.record("set_room_time_offset");
            Ok(())
        }

        fn idle_brightness(&self) -> u8 {
            1
        }

        fn soft_off_tick_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn any_lights_on(&self, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn current_hour(&self) -> f32 {
            self.current_hour
        }

        fn set_light_profile(&self, _: &str) -> bool {
            self.record("set_light_profile");
            true
        }

        fn active_light_profile_id(&self) -> String {
            "rhythm".to_string()
        }
        fn available_light_profiles(&self) -> Vec<(String, String)> {
            vec![
                ("rhythm".into(), "Rhythm Curve".into()),
                ("sleep".into(), "Sleep Curve".into()),
            ]
        }
    }

    fn test_state_with_runtime(
        runtime: Arc<dyn RuntimeHandle>,
        observed_power: &[(&str, bool)],
    ) -> SharedState {
        let mut app_state = crate::state::AppState::default();
        let hub_type = HubType::new("test");
        let hub_key = HubKey::new(hub_type.clone(), "hub.local");
        app_state.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime),
                hub_data: Box::new(()),
                registry: None,
                discovery: None,
                shutdown: Arc::new(AtomicBool::new(false)),
            },
        );
        for (room_id, lights_on) in observed_power {
            app_state.room_observed_power.insert(
                (*room_id).to_string(),
                crate::state::ObservedPowerState::new(
                    *lights_on,
                    crate::state::ObservedPowerSource::Command,
                ),
            );
        }
        Arc::new(Mutex::new(app_state))
    }

    #[test]
    fn api_response_server_error_adds_trace_context_extension() {
        let response = ApiResponse::server_error("boom").into_response();
        let context = response
            .extensions()
            .get::<ApiErrorContext>()
            .expect("500 responses should carry trace context");

        assert_eq!(context.0, "boom");
    }

    async fn call_json_route(
        app: Router,
        method: HttpMethod,
        uri: &str,
        body: serde_json::Value,
    ) -> StatusCode {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    }

    fn room_state_json<'a>(body: &'a serde_json::Value, room_id: &str) -> &'a serde_json::Value {
        body["nodes"]
            .as_array()
            .expect("state nodes should be an array")
            .iter()
            .find(|node| node["id"].as_str() == Some(room_id))
            .expect("room should exist in state response")
    }

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

    #[tokio::test]
    async fn absorb_offset_runs_on_handler_thread() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ThreadRecordingRuntime {
            calls: calls.clone(),
            snapshots: vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 15.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: rhythm_core::RoomProfileSettings::default(),
            }],
            current_hour: 8.0,
        });
        let state = test_state_with_runtime(runtime, &[]);
        let app = api_routes().with_state(state);

        let status = call_json_route(
            app,
            HttpMethod::POST,
            "/api/config/absorb-offset",
            json!({ "offset_minutes": 30.0 }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call == "restore_room_state@http-handler"));
        assert!(!calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("set_room_time_offset@")));
    }

    #[tokio::test]
    async fn put_config_query_id_targets_requested_profile() {
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ThreadRecordingRuntime {
            calls: Arc::new(Mutex::new(Vec::new())),
            snapshots: vec![],
            current_hour: 12.0,
        });
        let state = test_state_with_runtime(runtime, &[]);
        let app = api_routes().with_state(state.clone());

        let status = call_json_route(
            app,
            HttpMethod::PUT,
            "/api/config?id=sleep",
            json!({
                "id": "rhythm",
                "name": "Sleep",
                "curve": { "type": "super-gaussian" },
                "min_brightness": 8,
                "max_brightness": 18,
                "min_color_temp": 1000,
                "max_color_temp": 1500,
                "max_dim_steps": 4
            }),
        )
        .await;

        assert_eq!(status, StatusCode::NO_CONTENT);
        let state = state.lock().unwrap();
        assert_eq!(
            state
                .light_profile_config(rhythm_core::SLEEP_PROFILE_ID)
                .unwrap()
                .min_brightness,
            8
        );
        let factory_rhythm =
            crate::factory_default_config::factory_default_light_profile_config_map();
        assert_eq!(
            state
                .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
                .unwrap()
                .min_brightness,
            factory_rhythm[rhythm_core::RHYTHM_PROFILE_ID].min_brightness
        );
    }

    #[tokio::test]
    async fn put_config_apply_true_reapplies_active_profile() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ThreadRecordingRuntime {
            calls: calls.clone(),
            snapshots: vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: rhythm_core::RoomProfileSettings::default(),
            }],
            current_hour: 12.0,
        });
        let state = test_state_with_runtime(runtime, &[]);
        let app = api_routes().with_state(state);

        let status = call_json_route(
            app,
            HttpMethod::PUT,
            "/api/config?id=rhythm&apply=true",
            json!({
                "id": "rhythm",
                "name": "Day",
                "curve": { "type": "super-gaussian" },
                "min_brightness": 8,
                "max_brightness": 33,
                "min_color_temp": 1000,
                "max_color_temp": 3000,
                "max_dim_steps": 4
            }),
        )
        .await;

        assert_eq!(status, StatusCode::NO_CONTENT);
        let calls = calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .any(|call| call.starts_with("apply_room_command@")),
            "expected apply_room_command call, saw {:?}",
            *calls
        );
    }

    #[tokio::test]
    async fn absorb_offset_query_id_targets_requested_profile() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ThreadRecordingRuntime {
            calls: calls.clone(),
            snapshots: vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 15.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: rhythm_core::RoomProfileSettings::default(),
            }],
            current_hour: 8.0,
        });
        let state = test_state_with_runtime(runtime, &[]);
        let mut day_alt = rhythm_core::default_rhythm_profile();
        day_alt.id = "day_alt".into();
        state.lock().unwrap().set_light_profile_config(day_alt);
        let before_day_alt = state
            .lock()
            .unwrap()
            .light_profile_config("day_alt")
            .unwrap()
            .clone();
        let before_rhythm = state
            .lock()
            .unwrap()
            .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
            .unwrap()
            .clone();
        let app = api_routes().with_state(state.clone());

        let status = call_json_route(
            app,
            HttpMethod::POST,
            "/api/config/absorb-offset?id=day_alt",
            json!({ "offset_minutes": 30.0 }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let state = state.lock().unwrap();
        assert_ne!(
            state.light_profile_config("day_alt").unwrap(),
            &before_day_alt
        );
        assert_eq!(
            state
                .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
                .unwrap(),
            &before_rhythm
        );
        assert!(calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call == "restore_room_state@http-handler"));
        assert!(!calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("set_room_time_offset@")));
    }

    #[tokio::test]
    async fn get_state_authoritative_query_reconciles_stale_observed_power() {
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ThreadRecordingRuntime {
            calls: Arc::new(Mutex::new(Vec::new())),
            snapshots: vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: rhythm_core::RoomProfileSettings::default(),
            }],
            current_hour: 12.0,
        });
        let state = test_state_with_runtime(runtime, &[("room1", true)]);
        let app = api_routes().with_state(state.clone());

        let stale_request = Request::builder()
            .method(HttpMethod::GET)
            .uri("/api/state")
            .body(Body::empty())
            .unwrap();
        let stale_response = app.clone().oneshot(stale_request).await.unwrap();
        assert_eq!(stale_response.status(), StatusCode::OK);
        let stale_body = serde_json::from_slice::<serde_json::Value>(
            &to_bytes(stale_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let stale_room = room_state_json(&stale_body, "room1");
        assert_eq!(stale_room["lights_on"].as_bool(), Some(true));
        assert_eq!(
            stale_room["observed_power"]["source"].as_str(),
            Some("command")
        );

        let fresh_request = Request::builder()
            .method(HttpMethod::GET)
            .uri("/api/state?authoritative=true")
            .body(Body::empty())
            .unwrap();
        let fresh_response = app.oneshot(fresh_request).await.unwrap();
        assert_eq!(fresh_response.status(), StatusCode::OK);
        let fresh_body = serde_json::from_slice::<serde_json::Value>(
            &to_bytes(fresh_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let fresh_room = room_state_json(&fresh_body, "room1");
        assert_eq!(fresh_room["lights_on"].as_bool(), Some(false));
        assert_eq!(
            fresh_room["observed_power"]["lights_on"].as_bool(),
            Some(false)
        );
        assert_eq!(
            fresh_room["observed_power"]["source"].as_str(),
            Some("authoritative_refresh")
        );
        assert_eq!(fresh_room["observed_power"]["fresh"].as_bool(), Some(true));
        assert_eq!(
            state
                .lock()
                .unwrap()
                .room_observed_power
                .get("room1")
                .map(|observed| observed.lights_on),
            Some(false)
        );
    }

    #[tokio::test]
    async fn set_sleep_profile_runs_on_handler_thread() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ThreadRecordingRuntime {
            calls: calls.clone(),
            snapshots: vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: rhythm_core::RoomProfileSettings::default(),
            }],
            current_hour: 12.0,
        });
        let state = test_state_with_runtime(runtime, &[("room1", true)]);
        let app = api_routes().with_state(state);

        let req = Request::builder()
            .method(HttpMethod::PUT)
            .uri("/api/light-profile")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"id":"sleep"}"#))
            .unwrap();
        let status = app.oneshot(req).await.unwrap().status();

        assert_eq!(status, StatusCode::BAD_REQUEST);
        let calls = calls.lock().unwrap();
        assert!(!calls
            .iter()
            .any(|call| call == "set_light_profile@http-handler"));
        assert!(!calls.iter().any(|call| call == "handle_event@http-handler"));
    }

    #[tokio::test]
    async fn set_rhythm_profile_runs_on_handler_thread() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ThreadRecordingRuntime {
            calls: calls.clone(),
            snapshots: vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                kind: rhythm_core::LightNodeKind::Room,
                parent_id: None,
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                hard_off: false,
                profile_settings: rhythm_core::RoomProfileSettings::default(),
            }],
            current_hour: 12.0,
        });
        let state = test_state_with_runtime(runtime, &[("room1", true)]);
        let app = api_routes().with_state(state);

        let req = Request::builder()
            .method(HttpMethod::PUT)
            .uri("/api/light-profile")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"id":"rhythm"}"#))
            .unwrap();
        let status = app.oneshot(req).await.unwrap().status();

        assert_eq!(status, StatusCode::BAD_REQUEST);
        let calls = calls.lock().unwrap();
        assert!(!calls
            .iter()
            .any(|call| call == "set_light_profile@http-handler"));
        assert!(!calls.iter().any(|call| call == "handle_event@http-handler"));
    }
}
