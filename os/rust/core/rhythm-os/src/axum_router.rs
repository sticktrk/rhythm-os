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
        // Device pairing / unpairing (Matter commissioning, Zigbee permit join)
        .route("/api/devices/pair", post(post_pair_device))
        .route("/api/devices/unpair", post(post_unpair_device))
        // Curve visualization
        .route("/api/curve", get(get_curve).post(post_curve_preview))
        .route("/api/curve/now", get(get_curve_now))
        .route("/api/curve/solar", get(get_curve_solar))
        // Sleep mode
        .route("/api/sleep", put(put_sleep))
        .route("/api/wake", put(put_wake))
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
    run_blocking(move || handlers::handle_absorb_time_offset(&state, &body)).await
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

async fn put_sleep(State(state): State<SharedState>) -> ApiResponse {
    run_blocking(move || handlers::handle_put_sleep(&state)).await
}

async fn put_wake(State(state): State<SharedState>) -> ApiResponse {
    run_blocking(move || handlers::handle_put_wake(&state)).await
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

pub async fn post_unpair_device(
    State(state): State<SharedState>,
    Json(body): Json<crate::pairing::UnpairingRequest>,
) -> ApiResponse {
    run_blocking(move || handlers::handle_unpair_device(&state, &body)).await
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
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::{Method as HttpMethod, Request, StatusCode};
    use rhythm_core::{CurveConfig, RoomSnapshot, RuntimeHandle, SolarTime};
    use serde_json::json;
    use tower::util::ServiceExt;

    use crate::canonical::identity::HubKey;
    use crate::hub::{ActiveHub, HubType};
    use crate::routes::SHARED_API_ROUTES;

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

        fn set_curve_config(&self, _: CurveConfig) -> anyhow::Result<()> {
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

        fn restore_room_state(&self, _: &str, _: bool, _: bool, _: f32, _: f32, _: bool) {}

        fn add_room(&self, _: &str, _: &str) {}

        fn remove_room(&self, _: &str) {}

        fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }

        fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
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

        fn set_curve_module(&self, _: &str) -> bool {
            self.record("set_curve_module");
            true
        }

        fn active_curve_module_id(&self) -> String {
            "rhythm".to_string()
        }
    }

    fn test_state_with_runtime(
        runtime: Arc<dyn RuntimeHandle>,
        room_lights_on: &[(&str, bool)],
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
        for (room_id, lights_on) in room_lights_on {
            app_state
                .room_lights_on
                .insert((*room_id).to_string(), *lights_on);
        }
        Arc::new(Mutex::new(app_state))
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
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 15.0,
                brightness_offset: 0.0,
                soft_off: false,
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
            .any(|call| call == "set_room_time_offset@http-handler"));
    }

    #[tokio::test]
    async fn sleep_runs_on_handler_thread() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ThreadRecordingRuntime {
            calls: calls.clone(),
            snapshots: vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
            }],
            current_hour: 12.0,
        });
        let state = test_state_with_runtime(runtime, &[("room1", true)]);
        let app = api_routes().with_state(state);

        let req = Request::builder()
            .method(HttpMethod::PUT)
            .uri("/api/sleep")
            .body(Body::empty())
            .unwrap();
        let status = app.oneshot(req).await.unwrap().status();

        assert_eq!(status, StatusCode::NO_CONTENT);
        let calls = calls.lock().unwrap();
        assert!(calls
            .iter()
            .any(|call| call == "set_curve_module@http-handler"));
        assert!(calls.iter().any(|call| call == "handle_event@http-handler"));
    }

    #[tokio::test]
    async fn wake_runs_on_handler_thread() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ThreadRecordingRuntime {
            calls: calls.clone(),
            snapshots: vec![RoomSnapshot {
                id: "room1".into(),
                name: "Room 1".into(),
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
            }],
            current_hour: 12.0,
        });
        let state = test_state_with_runtime(runtime, &[("room1", true)]);
        let app = api_routes().with_state(state);

        let req = Request::builder()
            .method(HttpMethod::PUT)
            .uri("/api/wake")
            .body(Body::empty())
            .unwrap();
        let status = app.oneshot(req).await.unwrap().status();

        assert_eq!(status, StatusCode::NO_CONTENT);
        let calls = calls.lock().unwrap();
        assert!(calls
            .iter()
            .any(|call| call == "set_curve_module@http-handler"));
        assert!(calls.iter().any(|call| call == "handle_event@http-handler"));
    }
}
