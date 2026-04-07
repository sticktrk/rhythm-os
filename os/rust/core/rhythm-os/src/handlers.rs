//! Framework-agnostic HTTP handler functions.
//!
//! Each function takes parsed inputs (SharedState + JSON body) and returns an
//! `ApiResponse` that any HTTP framework can convert into its native response.
//! All batch logic (single-or-array detection, deferred persist) is centralized here.
//!
//! ## Response convention
//!
//! | Endpoint type                  | Response                                  |
//! |-------------------------------|-------------------------------------------|
//! | GET (reads)                   | Raw data, 200                             |
//! | Mutation returning data       | Raw data, 200 (no `{"status":"ok"}` wrap) |
//! | Mutation returning nothing    | 204 No Content, empty body                |
//! | Batch mutations returning data| Always `{"rooms":[...]}` regardless of N  |
//! | Errors                        | 400/500 with plain text                   |

use std::fmt::Display;

use serde_json::Value;

use rhythm_core::runtime::hub_registry::DeviceType;

use crate::api_types::{HubCredentialsResponse, SyncResponse};
use crate::commands::{self, RoomParams};
use crate::state::SharedState;

/// Framework-agnostic HTTP response.
pub struct ApiResponse {
    pub status: u16,
    pub body: String,
    pub content_type: &'static str,
}

impl ApiResponse {
    pub fn json_ok(body: String) -> Self {
        Self {
            status: 200,
            body,
            content_type: "application/json",
        }
    }

    pub fn bad_request(msg: &str) -> Self {
        Self {
            status: 400,
            body: msg.to_string(),
            content_type: "text/plain",
        }
    }

    pub fn server_error(e: impl Display) -> Self {
        Self {
            status: 500,
            body: e.to_string(),
            content_type: "text/plain",
        }
    }

    pub fn no_content() -> Self {
        Self {
            status: 204,
            body: String::new(),
            content_type: "text/plain",
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub fn handle_health() -> ApiResponse {
    ApiResponse::json_ok(r#"{"status":"healthy"}"#.to_string())
}

pub fn handle_get_state(state: &SharedState) -> ApiResponse {
    match commands::build_state_snapshot(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_rooms_state(state: &SharedState) -> ApiResponse {
    match commands::build_rooms_state(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

/// Upsert room(s). Accepts single object or array.
///
/// Always returns `{"rooms":[...]}` regardless of count.
pub fn handle_put_rooms(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let rooms: Vec<Value> = if body.is_array() {
        body.as_array().cloned().unwrap_or_default()
    } else {
        vec![body.clone()]
    };

    let mut results = Vec::new();
    let batch = rooms.len() > 1;

    for room_json in &rooms {
        match RoomParams::from_json(room_json) {
            Ok(params) => {
                let per_room_persist = persist && !batch;
                match commands::do_room_set(state, &params, None, per_room_persist) {
                    Ok(json) => results.push(json),
                    Err(e) => return ApiResponse::server_error(e),
                }
            }
            Err(e) => return ApiResponse::bad_request(&e.to_string()),
        }
    }

    if batch && persist {
        commands::persist_state(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

pub fn handle_delete_room(state: &SharedState, id: &str) -> ApiResponse {
    let id = commands::resolve_room_id(state, id);
    match commands::do_room_remove(state, &id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

/// Upsert device(s). Accepts single object or array.
///
/// Supports optional `"device_type"` field (default: `"button"`).
pub fn handle_put_devices(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let devices: Vec<Value> = if body.is_array() {
        body.as_array().cloned().unwrap_or_default()
    } else {
        vec![body.clone()]
    };

    let batch = devices.len() > 1;

    for dev in &devices {
        let device_id = match dev.get("device_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing device.device_id"),
        };
        let room_id = match dev.get("room_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing device.room_id"),
        };
        let buttons = commands::parse_buttons(dev);
        let device_type = match dev.get("device_type").and_then(|v| v.as_str()) {
            Some("motion") => DeviceType::Motion,
            Some("light") => DeviceType::Light,
            _ => DeviceType::Button, // default
        };
        let per_item_persist = persist && !batch;
        if let Err(e) = commands::do_device_set(
            state,
            device_id,
            room_id,
            &buttons,
            device_type,
            None,
            per_item_persist,
        ) {
            return ApiResponse::server_error(e);
        }
    }

    if batch && persist {
        commands::persist_state(state);
    }

    ApiResponse::no_content()
}

pub fn handle_delete_device(state: &SharedState, id: &str) -> ApiResponse {
    match commands::do_device_remove(state, id, None) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_motion_timeout(state: &SharedState, body: &Value) -> ApiResponse {
    let raw_room_id = match body.get("room_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return ApiResponse::bad_request("Missing room_id"),
    };
    let room_id = commands::resolve_room_id(state, raw_room_id);

    // null timeout_secs → remove per-room override (use profile default)
    let timeout_val = body.get("timeout_secs");
    if timeout_val.is_some_and(|v| v.is_null()) {
        match commands::do_motion_timeout_clear(state, &room_id, None) {
            Ok(()) => return ApiResponse::no_content(),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    let timeout = match timeout_val.and_then(|v| v.as_u64()) {
        Some(t) => t,
        None => return ApiResponse::bad_request("Missing timeout_secs"),
    };
    match commands::do_motion_timeout_set(state, &room_id, timeout, None) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

fn parse_timer_patch_value(
    body: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<Option<rhythm_core::TimerSetting>>, String> {
    match body.get(key) {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(Some(None)),
        Some(v) => serde_json::from_value(v.clone())
            .map(|value| Some(Some(value)))
            .map_err(|e| format!("Invalid {}: {}", key, e)),
    }
}

fn parse_room_profile_patch(value: Option<&Value>) -> Result<Option<commands::RoomProfileSettingsPatch>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(Some(commands::RoomProfileSettingsPatch {
            clear_all: true,
            ..Default::default()
        }));
    }

    let body = value
        .as_object()
        .ok_or_else(|| "room_profile must be an object or null".to_string())?;

    let profile_id = match body.get("profile_id") {
        None => None,
        Some(v) if v.is_null() => Some(None),
        Some(v) => Some(Some(
            v.as_str()
                .ok_or_else(|| "room_profile.profile_id must be a string or null".to_string())?
                .to_string(),
        )),
    };

    Ok(Some(commands::RoomProfileSettingsPatch {
        clear_all: false,
        profile_id,
        fade_ms: parse_timer_patch_value(body, "fade_ms")?,
        motion_timeout_secs: parse_timer_patch_value(body, "motion_timeout_secs")?,
    }))
}

fn default_profile_config_for(
    state: &SharedState,
    requested_id: Option<&str>,
) -> Result<rhythm_core::LightProfileConfig, String> {
    let s = state.lock().map_err(|_| "lock".to_string())?;
    let id = requested_id.unwrap_or(&s.active_light_profile_id);
    match id {
        rhythm_core::RHYTHM_PROFILE_ID => Ok(rhythm_core::default_rhythm_profile()),
        rhythm_core::SLEEP_PROFILE_ID => Ok(rhythm_core::default_sleep_profile()),
        rhythm_core::IDLE_PROFILE_ID => Ok(rhythm_core::default_idle_profile()),
        _ => Err(format!("Unknown light profile: {}", id)),
    }
}

pub fn handle_get_config(state: &SharedState, profile_id: Option<&str>) -> ApiResponse {
    match commands::build_config(state, profile_id) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_config(
    state: &SharedState,
    profile_id: Option<&str>,
    body: &Value,
) -> ApiResponse {
    let mut config: rhythm_core::LightProfileConfig = match serde_json::from_value(body.clone()) {
        Ok(c) => c,
        Err(e) => return ApiResponse::bad_request(&format!("Invalid config: {}", e)),
    };
    if let Some(id) = profile_id {
        config.id = id.to_string();
    }
    match commands::do_config_set(state, config) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_reset_config(state: &SharedState, profile_id: Option<&str>) -> ApiResponse {
    let default_config = match default_profile_config_for(state, profile_id) {
        Ok(config) => config,
        Err(e) => return ApiResponse::bad_request(&e),
    };
    match commands::do_config_set(state, default_config) {
        Ok(()) => match commands::build_config(state, profile_id) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(e) => ApiResponse::server_error(e),
        },
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_absorb_time_offset(
    state: &SharedState,
    profile_id: Option<&str>,
    body: &Value,
) -> ApiResponse {
    let offset_minutes = match body.get("offset_minutes").and_then(|v| v.as_f64()) {
        Some(v) => v as f32,
        None => return ApiResponse::bad_request("Missing offset_minutes"),
    };
    match commands::do_absorb_time_offset(state, profile_id, offset_minutes) {
        Ok(()) => match commands::build_config(state, profile_id) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(e) => ApiResponse::server_error(e),
        },
        Err(e) => ApiResponse::server_error(e),
    }
}

// ---- Curve visualization ----

/// Query parameters for `GET /api/curve` and `POST /api/curve`.
pub struct CurveQueryParams {
    pub id: Option<String>,
    pub samples_per_hour: Option<u32>,
    pub date: Option<String>,
    pub start_hour: Option<f32>,
    pub max_steps: Option<u8>,
}

pub fn handle_get_curve(state: &SharedState, params: &CurveQueryParams) -> ApiResponse {
    match commands::build_curve(
        state,
        None,
        params.id.as_deref(),
        params.date.as_deref(),
        params.samples_per_hour,
        params.start_hour,
        params.max_steps,
    ) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_post_curve(
    state: &SharedState,
    body: &Value,
    params: &CurveQueryParams,
) -> ApiResponse {
    let mut config: rhythm_core::LightProfileConfig = match serde_json::from_value(body.clone()) {
        Ok(c) => c,
        Err(e) => return ApiResponse::bad_request(&format!("Invalid config: {}", e)),
    };
    if let Some(id) = params.id.as_deref() {
        config.id = id.to_string();
    }
    match commands::build_curve(
        state,
        Some(config),
        params.id.as_deref(),
        params.date.as_deref(),
        params.samples_per_hour,
        params.start_hour,
        params.max_steps,
    ) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_curve_now(
    state: &SharedState,
    profile_id: Option<&str>,
    hour: Option<f32>,
) -> ApiResponse {
    match commands::build_curve_now(state, profile_id, hour) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_curve_solar(state: &SharedState, date: Option<&str>) -> ApiResponse {
    match commands::build_curve_solar(state, date) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_location(state: &SharedState, body: &Value) -> ApiResponse {
    let lat = match body.get("lat").and_then(|v| v.as_f64()) {
        Some(v) => v as f32,
        None => return ApiResponse::bad_request("Missing lat"),
    };
    let lon = match body.get("lon").and_then(|v| v.as_f64()) {
        Some(v) => v as f32,
        None => return ApiResponse::bad_request("Missing lon"),
    };
    let utc_offset = body
        .get("utc_offset")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);
    let timezone_name = body
        .get("timezone_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match commands::do_location_set(state, lat, lon, utc_offset, timezone_name) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_settings(state: &SharedState) -> ApiResponse {
    match commands::build_settings(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_settings(state: &SharedState, body: &Value) -> ApiResponse {
    if body.get("rhythm_interval_secs").is_some() {
        return ApiResponse::bad_request(
            "rhythm_interval_secs now belongs in light profile config",
        );
    }
    let power_save = body.get("power_save").and_then(|v| v.as_bool());

    match commands::do_settings_set(state, power_save) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_light_profile(state: &SharedState, body: &Value) -> ApiResponse {
    let id = match body.get("id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return ApiResponse::bad_request("Missing id"),
    };
    match commands::do_set_light_profile(state, id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_hub_credentials(state: &SharedState, body: &Value) -> ApiResponse {
    // Platform-specific interceptor (e.g., addon auto-fills SUPERVISOR_TOKEN for HA)
    let interceptor = state
        .lock()
        .ok()
        .and_then(|s| s.hub_credentials_interceptor.clone());
    if let Some(ref intercept_fn) = interceptor {
        if let Some(result) = intercept_fn(state, body) {
            return match result {
                Ok(json) => ApiResponse::json_ok(json),
                Err(e) => ApiResponse::server_error(e),
            };
        }
    }

    let hub_type = match body.get("hub_type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return ApiResponse::bad_request("Missing hub_type"),
    };
    let address = match body.get("address").and_then(|v| v.as_str()) {
        Some(a) => a,
        None => return ApiResponse::bad_request("Missing address"),
    };
    let credentials = body.get("credentials").cloned().unwrap_or(Value::Null);

    match commands::do_hub_credentials(state, hub_type, address, &credentials) {
        Ok(()) => {
            let hub_connected = state.lock().map(|s| s.has_any_hub()).unwrap_or(false);
            let resp = HubCredentialsResponse { hub_connected };
            match serde_json::to_string(&resp) {
                Ok(json) => ApiResponse::json_ok(json),
                Err(e) => ApiResponse::server_error(e),
            }
        }
        Err(e) => ApiResponse::server_error(e),
    }
}

/// Disconnect hub(s). If `hub_type` and `address` are provided, disconnects
/// only that hub. Otherwise disconnects all hubs.
pub fn handle_delete_hub(
    state: &SharedState,
    hub_type: Option<&str>,
    address: Option<&str>,
) -> ApiResponse {
    match (hub_type, address) {
        (Some(ht), Some(addr)) => match commands::do_hub_disconnect_one(state, ht, addr) {
            Ok(()) => ApiResponse::no_content(),
            Err(e) => ApiResponse::server_error(e),
        },
        _ => match commands::do_hub_disconnect(state) {
            Ok(()) => ApiResponse::no_content(),
            Err(e) => ApiResponse::server_error(e),
        },
    }
}

/// Dispatch room action(s). Accepts single object or array.
///
/// Always returns `{"rooms":[...]}` regardless of count.
pub fn handle_room_action(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let items: Vec<Value> = if body.is_array() {
        body.as_array().cloned().unwrap_or_default()
    } else {
        vec![body.clone()]
    };

    let mut results = Vec::new();
    let batch = items.len() > 1;

    for item in &items {
        let raw_room_id = match item.get("room_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing room_id"),
        };
        let room_id = commands::resolve_room_id(state, raw_room_id);
        let action = match item.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return ApiResponse::bad_request("Missing action"),
        };

        let per_item_persist = persist && !batch;
        match commands::do_room_action(state, &room_id, action, per_item_persist) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Set room brightness. Accepts single object or array.
///
/// Always returns `{"rooms":[...]}` regardless of count.
pub fn handle_set_brightness(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let items: Vec<Value> = if body.is_array() {
        body.as_array().cloned().unwrap_or_default()
    } else {
        vec![body.clone()]
    };

    let mut results = Vec::new();
    let batch = items.len() > 1;

    for item in &items {
        let raw_room_id = match item.get("room_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing room_id"),
        };
        let room_id = commands::resolve_room_id(state, raw_room_id);
        let brightness = match item.get("brightness").and_then(|v| v.as_u64()) {
            Some(b) => b as u8,
            None => return ApiResponse::bad_request("Missing brightness"),
        };

        let per_item_persist = persist && !batch;
        match commands::do_set_brightness(state, &room_id, brightness, per_item_persist) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Set room time offset. Accepts single object or array.
///
/// Always returns `{"rooms":[...]}` regardless of count.
pub fn handle_set_time_offset(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let items: Vec<Value> = if body.is_array() {
        body.as_array().cloned().unwrap_or_default()
    } else {
        vec![body.clone()]
    };

    let mut results = Vec::new();
    let batch = items.len() > 1;

    for item in &items {
        let raw_room_id = match item.get("room_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing room_id"),
        };
        let room_id = commands::resolve_room_id(state, raw_room_id);
        let time_offset = match item.get("time_offset").and_then(|v| v.as_f64()) {
            Some(t) => t as f32,
            None => return ApiResponse::bad_request("Missing time_offset"),
        };

        let per_item_persist = persist && !batch;
        match commands::do_set_time_offset(state, &room_id, time_offset, per_item_persist) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Update room preferences. Accepts single object or array.
///
/// Always returns `{"rooms":[...]}` regardless of count.
pub fn handle_put_room_preferences(
    state: &SharedState,
    body: &Value,
    persist: bool,
) -> ApiResponse {
    let items: Vec<Value> = if body.is_array() {
        body.as_array().cloned().unwrap_or_default()
    } else {
        vec![body.clone()]
    };

    let mut results = Vec::new();
    let batch = items.len() > 1;

    for item in &items {
        let raw_room_id = match item.get("room_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing room_id"),
        };
        let room_id = commands::resolve_room_id(state, raw_room_id);
        let rhythm_enabled = item.get("rhythm_enabled").and_then(|v| v.as_bool());
        let disabled = item.get("disabled").and_then(|v| v.as_bool());
        let soft_off = item.get("soft_off").and_then(|v| v.as_bool());
        let room_profile = match parse_room_profile_patch(item.get("room_profile")) {
            Ok(patch) => patch,
            Err(e) => return ApiResponse::bad_request(&e),
        };

        let per_item_persist = persist && !batch;
        match commands::do_room_preferences_set(
            state,
            &room_id,
            rhythm_enabled,
            disabled,
            soft_off,
            room_profile.as_ref(),
            per_item_persist,
        ) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Reset all on-rooms back to their current adaptive curve position.
///
/// When `persist` is `true`, persists state after reset.
/// When `false` (ESP32), the caller is responsible for deferred persistence.
pub fn handle_fix_my_lights(state: &SharedState, persist: bool) -> ApiResponse {
    match commands::do_fix_my_lights(state, persist) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_post_sync(state: &SharedState) -> ApiResponse {
    match crate::room_sync::sync_all_hubs(state) {
        Ok(report) => {
            let resp = SyncResponse {
                rooms_added: report.rooms_added,
                rooms_updated: report.rooms_updated,
                rooms_removed: report.rooms_removed,
                devices_synced: report.devices_synced,
            };
            match serde_json::to_string(&resp) {
                Ok(json) => ApiResponse::json_ok(json),
                Err(e) => ApiResponse::server_error(e),
            }
        }
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_version(version: &str) -> ApiResponse {
    ApiResponse::json_ok(format!(r#"{{"version":"{}"}}"#, version))
}

// ---------------------------------------------------------------------------
// Device pairing handler
// ---------------------------------------------------------------------------

pub fn handle_pair_device(
    state: &SharedState,
    request: &crate::pairing::PairingRequest,
) -> ApiResponse {
    let start_pairing = {
        let Ok(s) = state.lock() else {
            return ApiResponse::server_error("lock");
        };
        s.start_pairing_fn.clone()
    };

    let Some(start_fn) = start_pairing else {
        return ApiResponse::server_error("No pairing support configured");
    };

    log::info!(target: "pair", "Pairing request: hub_type={}, params={}", request.hub_type, request.params);

    match start_fn(state, &request.hub_type, &request.params) {
        Ok(session) => {
            log::info!(target: "pair", "Pairing result: status={:?} error={:?}", session.status, session.error);
            if session.status == crate::pairing::PairingStatus::Complete {
                // Persist canonical registry (resolve() was called during pairing)
                if let Ok(s) = state.lock() {
                    commands::persist_canonical(&s);
                }
                // Persist hub device registry (upsert_room was called during pairing)
                commands::persist_registry(state);
                // Notify SSE clients
                #[cfg(feature = "desktop")]
                {
                    commands::emit_triage_changed(state);
                    crate::state::emit_server_event(
                        state,
                        crate::server_event::ServerEvent::RoomsChanged,
                    );
                }
            }
            match serde_json::to_string(&session) {
                Ok(json) => ApiResponse::json_ok(json),
                Err(e) => ApiResponse::server_error(e),
            }
        }
        Err(e) => {
            log::error!(target: "pair", "Pairing failed: {}", e);
            ApiResponse::server_error(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Device unpairing handler
// ---------------------------------------------------------------------------

pub fn handle_unpair_device(
    state: &SharedState,
    request: &crate::pairing::UnpairingRequest,
) -> ApiResponse {
    let start_unpairing = {
        let Ok(s) = state.lock() else {
            return ApiResponse::server_error("lock");
        };
        s.start_unpairing_fn.clone()
    };

    let Some(start_fn) = start_unpairing else {
        return ApiResponse::server_error("No unpairing support configured");
    };

    log::info!(target: "pair", "Unpairing request: hub_type={}, params={}", request.hub_type, request.params);

    match start_fn(state, &request.hub_type, &request.params) {
        Ok(result) => {
            log::info!(target: "pair", "Unpairing result: status={:?} error={:?}", result.status, result.error);
            if result.status == crate::pairing::PairingStatus::Complete {
                if let Some(device_id) = &result.device_id {
                    // Remove from hub device registry
                    let hub_key = crate::canonical::identity::HubKey::new(
                        crate::hub::HubType::new(&request.hub_type),
                        "local",
                    );
                    let _ = commands::do_device_remove(state, device_id, Some(&hub_key));

                    // Soft-remove from canonical registry
                    commands::do_canonical_soft_remove(state, device_id, &hub_key);
                }

                // Persist registries
                commands::persist_registry(state);

                // Notify SSE clients
                #[cfg(feature = "desktop")]
                {
                    commands::emit_triage_changed(state);
                    crate::state::emit_server_event(
                        state,
                        crate::server_event::ServerEvent::RoomsChanged,
                    );
                }
            }
            match serde_json::to_string(&result) {
                Ok(json) => ApiResponse::json_ok(json),
                Err(e) => ApiResponse::server_error(e),
            }
        }
        Err(e) => {
            log::error!(target: "pair", "Unpairing failed: {}", e);
            ApiResponse::server_error(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical device handlers
// ---------------------------------------------------------------------------

pub fn handle_get_canonical_devices(state: &SharedState) -> ApiResponse {
    match commands::build_canonical_devices(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_canonical_device(state: &SharedState, id: &str) -> ApiResponse {
    match commands::build_canonical_device(state, id) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_device_room(state: &SharedState, device_id: &str, body: &Value) -> ApiResponse {
    let room_id = body.get("room_id").and_then(|v| v.as_str());
    match commands::do_canonical_assign_room(state, device_id, room_id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_device_preferred(
    state: &SharedState,
    device_id: &str,
    body: &Value,
) -> ApiResponse {
    let hub_type = match body.get("hub_type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return ApiResponse::bad_request("Missing hub_type"),
    };
    let hub_address = match body.get("hub_address").and_then(|v| v.as_str()) {
        Some(a) => a,
        None => return ApiResponse::bad_request("Missing hub_address"),
    };
    let native_id = match body.get("native_id").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return ApiResponse::bad_request("Missing native_id"),
    };
    match commands::do_canonical_set_preferred(state, device_id, hub_type, hub_address, native_id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

// ---------------------------------------------------------------------------
// Triage handlers
// ---------------------------------------------------------------------------

pub fn handle_get_triage(state: &SharedState) -> ApiResponse {
    match commands::build_triage_queue(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_triage_merge(state: &SharedState, entry_id: &str, body: &Value) -> ApiResponse {
    let canonical_id = match body.get("canonical_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return ApiResponse::bad_request("Missing canonical_id"),
    };
    match commands::do_triage_merge(state, entry_id, canonical_id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_triage_new(state: &SharedState, entry_id: &str) -> ApiResponse {
    match commands::do_triage_new_device(state, entry_id) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_triage_dismiss(state: &SharedState, entry_id: &str) -> ApiResponse {
    match commands::do_triage_dismiss(state, entry_id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_triage_bind(state: &SharedState, entry_id: &str, body: &Value) -> ApiResponse {
    // Optional target_room_id for the 3+ hub case
    let target = body.get("target_room_id").and_then(|v| v.as_str());
    match commands::do_triage_bind_room_to(state, entry_id, target) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_triage_count(state: &SharedState) -> ApiResponse {
    match commands::build_triage_count(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

// ---------------------------------------------------------------------------
// Topology handlers
// ---------------------------------------------------------------------------

pub fn handle_get_topology_rooms(state: &SharedState) -> ApiResponse {
    match commands::build_topology_rooms(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_post_topology_room(state: &SharedState, body: &Value) -> ApiResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return ApiResponse::bad_request("Missing name"),
    };
    match commands::do_topology_create_room(state, name) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_topology_rename(state: &SharedState, room_id: &str, body: &Value) -> ApiResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return ApiResponse::bad_request("Missing name"),
    };
    let mut s = match state.lock() {
        Ok(s) => s,
        Err(_) => return ApiResponse::server_error(anyhow::anyhow!("lock")),
    };
    if s.topology.rename_room(room_id, name) {
        commands::persist_topology(&s);
        ApiResponse::no_content()
    } else {
        ApiResponse::bad_request("Room not found")
    }
}

pub fn handle_put_topology_merge(
    state: &SharedState,
    target_id: &str,
    body: &Value,
) -> ApiResponse {
    let source_id = match body.get("source_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return ApiResponse::bad_request("Missing source_id"),
    };
    match commands::do_topology_merge_rooms(state, target_id, source_id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_topology_move_device(
    state: &SharedState,
    room_id: &str,
    body: &Value,
) -> ApiResponse {
    let device_id = match body.get("device_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return ApiResponse::bad_request("Missing device_id"),
    };
    let from_room = match body.get("from_room").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return ApiResponse::bad_request("Missing from_room"),
    };
    match commands::do_topology_move_device(state, device_id, from_room, room_id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    // ---- ApiResponse construction ----

    #[test]
    fn api_response_json_ok() {
        let r = ApiResponse::json_ok("{}".to_string());
        assert_eq!(r.status, 200);
        assert_eq!(r.content_type, "application/json");
    }

    #[test]
    fn api_response_bad_request() {
        let r = ApiResponse::bad_request("err");
        assert_eq!(r.status, 400);
        assert_eq!(r.body, "err");
    }

    #[test]
    fn api_response_no_content() {
        let r = ApiResponse::no_content();
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());
    }

    #[test]
    fn api_response_server_error() {
        let r = ApiResponse::server_error("boom");
        assert_eq!(r.status, 500);
        assert_eq!(r.body, "boom");
    }

    // ---- Handler validation (400 paths) ----

    fn test_state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    #[test]
    fn put_devices_missing_device_id() {
        let state = test_state();
        let r = handle_put_devices(&state, &json!({"room_id": "r"}), true);
        assert_eq!(r.status, 400);
        assert!(r.body.contains("device_id"));
    }

    #[test]
    fn put_devices_missing_room_id() {
        let state = test_state();
        let r = handle_put_devices(&state, &json!({"device_id": "d"}), true);
        assert_eq!(r.status, 400);
        assert!(r.body.contains("room_id"));
    }

    #[test]
    fn put_motion_timeout_missing_room_id() {
        let state = test_state();
        let r = handle_put_motion_timeout(&state, &json!({"timeout_secs": 60}));
        assert_eq!(r.status, 400);
    }

    #[test]
    fn put_motion_timeout_missing_timeout() {
        let state = test_state();
        let r = handle_put_motion_timeout(&state, &json!({"room_id": "r"}));
        assert_eq!(r.status, 400);
    }

    #[test]
    fn put_config_invalid_json() {
        let state = test_state();
        let r = handle_put_config(&state, None, &json!({"min_brightness": "not_a_number"}));
        assert_eq!(r.status, 400);
    }

    #[test]
    fn put_location_missing_lat() {
        let state = test_state();
        let r = handle_put_location(&state, &json!({"lon": 1.0}));
        assert_eq!(r.status, 400);
    }

    #[test]
    fn put_location_missing_lon() {
        let state = test_state();
        let r = handle_put_location(&state, &json!({"lat": 1.0}));
        assert_eq!(r.status, 400);
    }

    #[test]
    fn room_action_missing_room_id() {
        let state = test_state();
        let r = handle_room_action(&state, &json!({"action": "on"}), true);
        assert_eq!(r.status, 400);
    }

    #[test]
    fn room_action_missing_action() {
        let state = test_state();
        let r = handle_room_action(&state, &json!({"room_id": "r"}), true);
        assert_eq!(r.status, 400);
    }

    #[test]
    fn set_brightness_missing_room_id() {
        let state = test_state();
        let r = handle_set_brightness(&state, &json!({"brightness": 50}), false);
        assert_eq!(r.status, 400);
    }

    #[test]
    fn set_brightness_missing_brightness() {
        let state = test_state();
        let r = handle_set_brightness(&state, &json!({"room_id": "r"}), false);
        assert_eq!(r.status, 400);
    }

    // ---- Simple handlers ----

    #[test]
    fn health_returns_healthy() {
        let r = handle_health();
        assert_eq!(r.status, 200);
        assert!(r.body.contains("healthy"));
    }

    #[test]
    fn get_version_formats() {
        let r = handle_get_version("1.2.3");
        assert_eq!(r.status, 200);
        assert!(r.body.contains("1.2.3"));
    }

    // ---- Hub credentials validation ----

    #[test]
    fn hub_credentials_missing_hub_type() {
        let state = test_state();
        let r = handle_put_hub_credentials(&state, &json!({"address": "x", "credentials": {}}));
        assert_eq!(r.status, 400);
    }

    #[test]
    fn hub_credentials_missing_address() {
        let state = test_state();
        let r = handle_put_hub_credentials(&state, &json!({"hub_type": "hue", "credentials": {}}));
        assert_eq!(r.status, 400);
    }

    // ---- Unified response convention tests ----

    // Helper: set up state with a mock runtime for handler tests
    fn handler_state_with_runtime() -> SharedState {
        use crate::hub::{ActiveHub, HubType};
        use rhythm_core::{
            ButtonAction, InputEvent, LightProfileConfig, RoomSnapshot, RuntimeHandle,
        };

        struct HandlerMockRuntime {
            snapshots: Vec<RoomSnapshot>,
        }
        impl RuntimeHandle for HandlerMockRuntime {
            fn handle_event(&self, event: &InputEvent) -> anyhow::Result<bool> {
                Ok(!matches!(
                    event.action,
                    ButtonAction::LightsOff | ButtonAction::OffPress
                ))
            }
            fn sync_rooms(&self) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_solar(&self, _: rhythm_core::SolarTime) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_light_profile_config(&self, _: LightProfileConfig) -> anyhow::Result<()> {
                Ok(())
            }
            fn periodic_tick_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
                Ok(())
            }
            fn engine_room_snapshot(&self, room_id: &str) -> Option<RoomSnapshot> {
                self.snapshots.iter().find(|s| s.id == room_id).cloned()
            }
            fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
                self.snapshots.clone()
            }
            fn restore_room_state(
                &self,
                _: &str,
                _: bool,
                _: bool,
                _: f32,
                _: f32,
                _: bool,
                _: rhythm_core::RoomProfileSettings,
            ) {
            }
            fn add_room(&self, _: &str, _: &str) {}
            fn remove_room(&self, _: &str) {}
            fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
                Ok(())
            }
            fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_power_save(&self, _: bool) -> Vec<String> {
                vec![]
            }
            fn is_power_save(&self) -> bool {
                false
            }
            fn set_room_brightness(&self, _: &str, _: u8) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_room_time_offset(&self, _: &str, _: f32) -> anyhow::Result<()> {
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
                12.0
            }
            fn set_light_profile(&self, _: &str) -> bool {
                true
            }
            fn active_light_profile_id(&self) -> String {
                "rhythm".into()
            }
            fn available_light_profiles(&self) -> Vec<(String, String)> {
                vec![
                    ("rhythm".into(), "Rhythm Curve".into()),
                    ("sleep".into(), "Sleep Curve".into()),
                ]
            }
        }

        let runtime = std::sync::Arc::new(HandlerMockRuntime {
            snapshots: vec![
                RoomSnapshot {
                    id: "room1".into(),
                    name: "Room 1".into(),
                    rhythm_enabled: true,
                    disabled: false,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                },
                RoomSnapshot {
                    id: "room2".into(),
                    name: "Room 2".into(),
                    rhythm_enabled: false,
                    disabled: false,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                },
            ],
        });
        let mut app = AppState::default();
        let hub_type = HubType::parse("mock").unwrap();
        let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime as std::sync::Arc<dyn RuntimeHandle>),
                hub_data: Box::new(()),
                registry: None,
                discovery: None,
                shutdown: Default::default(),
            },
        );
        Arc::new(Mutex::new(app))
    }

    // -- Void mutations return 204 --

    #[test]
    fn delete_room_returns_204() {
        let state = handler_state_with_runtime();
        let r = handle_delete_room(&state, "room1");
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());
    }

    #[test]
    fn delete_hub_returns_204() {
        let state = handler_state_with_runtime();
        let r = handle_delete_hub(&state, None, None);
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());
    }

    #[test]
    fn reset_config_returns_200_with_defaults() {
        let state = handler_state_with_runtime();
        let r = handle_reset_config(&state, None);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed.get("max_brightness").is_some());
    }

    #[test]
    fn put_config_returns_204() {
        let state = handler_state_with_runtime();
        let r = handle_put_config(
            &state,
            None,
            &json!({
                "id": "rhythm",
                "name": "Day",
                "curve": { "type": "super-gaussian" },
                "min_brightness": 1,
                "max_brightness": 100,
                "min_color_temp": 2200,
                "max_color_temp": 6500,
                "max_dim_steps": 6
            }),
        );
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());
    }

    #[test]
    fn get_config_for_sleep_profile_returns_requested_profile() {
        let state = handler_state_with_runtime();
        let r = handle_get_config(&state, Some(rhythm_core::SLEEP_PROFILE_ID));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["id"], rhythm_core::SLEEP_PROFILE_ID);
        assert_eq!(parsed["name"], rhythm_core::SLEEP_PROFILE_NAME);
        assert_eq!(parsed["curve"]["type"], "super-gaussian");
        assert_eq!(parsed["curve"]["direct_color"]["rgb"]["r"], 255);
        assert!(parsed.get("direct_color").is_none());
    }

    #[test]
    fn put_config_query_id_overrides_body_id() {
        let state = handler_state_with_runtime();
        let r = handle_put_config(
            &state,
            Some(rhythm_core::SLEEP_PROFILE_ID),
            &json!({
                "id": "rhythm",
                "name": "Sleep",
                "curve": { "type": "super-gaussian" },
                "min_brightness": 7,
                "max_brightness": 21,
                "min_color_temp": 1200,
                "max_color_temp": 1600,
                "max_dim_steps": 4
            }),
        );
        assert_eq!(r.status, 204);

        let state = state.lock().unwrap();
        assert_eq!(
            state
                .light_profile_config(rhythm_core::SLEEP_PROFILE_ID)
                .unwrap()
                .min_brightness,
            7
        );
        assert_eq!(
            state
                .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
                .unwrap()
                .min_brightness,
            rhythm_core::default_rhythm_profile().min_brightness
        );
    }

    #[test]
    fn reset_config_for_idle_returns_inherit_active_default() {
        let state = handler_state_with_runtime();
        let r = handle_reset_config(&state, Some(rhythm_core::IDLE_PROFILE_ID));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["id"], rhythm_core::IDLE_PROFILE_ID);
        assert_eq!(parsed["curve"]["type"], "inherit-active");
        assert_eq!(parsed["min_brightness"], 1);
    }

    #[test]
    fn get_config_unknown_profile_returns_error() {
        let state = handler_state_with_runtime();
        let r = handle_get_config(&state, Some("unknown-profile"));
        assert_eq!(r.status, 500);
        assert!(r.body.contains("Unknown light profile"));
    }

    #[test]
    fn reset_config_unknown_profile_returns_bad_request() {
        let state = handler_state_with_runtime();
        let r = handle_reset_config(&state, Some("unknown-profile"));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("Unknown light profile"));
    }

    #[test]
    fn put_location_returns_204() {
        let state = handler_state_with_runtime();
        let r = handle_put_location(&state, &json!({"lat": 35.0, "lon": -97.0}));
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());
    }

    #[test]
    fn put_motion_timeout_returns_204() {
        let state = handler_state_with_runtime();
        let r = handle_put_motion_timeout(&state, &json!({"room_id": "room1", "timeout_secs": 60}));
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());
    }

    #[test]
    fn put_devices_returns_204() {
        let state = handler_state_with_runtime();
        let r = handle_put_devices(
            &state,
            &json!({
                "device_id": "d1", "room_id": "room1"
            }),
            false,
        );
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());
    }

    // -- Single room action returns {"rooms":[...]} --

    #[test]
    fn room_action_single_wraps_in_rooms_array() {
        let state = handler_state_with_runtime();
        let r = handle_room_action(&state, &json!({"room_id": "room1", "action": "on"}), false);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        // Must have rooms array, even for single item
        assert!(parsed["rooms"].is_array());
        assert_eq!(parsed["rooms"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["rooms"][0]["id"], "room1");
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn room_action_batch_wraps_in_rooms_array() {
        let state = handler_state_with_runtime();
        let r = handle_room_action(
            &state,
            &json!([
                {"room_id": "room1", "action": "on"},
                {"room_id": "room2", "action": "on"}
            ]),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["rooms"].as_array().unwrap().len(), 2);
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    // -- Single preferences returns {"rooms":[...]} --

    #[test]
    fn room_preferences_single_wraps_in_rooms_array() {
        let state = handler_state_with_runtime();
        let r = handle_put_room_preferences(
            &state,
            &json!({"room_id": "room1", "rhythm_enabled": true}),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed["rooms"].is_array());
        assert_eq!(parsed["rooms"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["rooms"][0]["id"], "room1");
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn room_preferences_batch_wraps_in_rooms_array() {
        let state = handler_state_with_runtime();
        let r = handle_put_room_preferences(
            &state,
            &json!([
                {"room_id": "room1", "rhythm_enabled": true},
                {"room_id": "room2", "rhythm_enabled": false}
            ]),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["rooms"].as_array().unwrap().len(), 2);
        assert!(parsed.get("status").is_none());
    }

    // -- Data mutations return raw data, 200, no status --

    #[test]
    fn set_brightness_returns_rooms_array() {
        let state = handler_state_with_runtime();
        let r = handle_set_brightness(
            &state,
            &json!({"room_id": "room1", "brightness": 50}),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed["rooms"].is_array());
        assert_eq!(parsed["rooms"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["rooms"][0]["id"], "room1");
        assert!(parsed["rooms"][0]["brightness"].is_number());
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn put_settings_returns_raw_settings() {
        let state = handler_state_with_runtime();
        let r = handle_put_settings(&state, &json!({"power_save": true}));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["power_save"], true);
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn get_settings_returns_raw_settings() {
        let state = handler_state_with_runtime();
        let r = handle_get_settings(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed["power_save"].is_boolean());
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn put_settings_rejects_legacy_interval_field() {
        let state = handler_state_with_runtime();
        let r = handle_put_settings(&state, &json!({"rhythm_interval_secs": 120}));
        assert_eq!(r.status, 400);
        assert!(r
            .body
            .contains("rhythm_interval_secs now belongs in light profile config"));
    }

    // -- fix_my_lights returns rooms as objects --

    #[test]
    fn fix_returns_rooms_as_objects_not_ids() {
        let state = handler_state_with_runtime();
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("room1".into(), true);

        let r = handle_fix_my_lights(&state, false);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        // rooms contains objects, not ID strings
        assert!(parsed["rooms"].is_array());
        if let Some(first) = parsed["rooms"].as_array().unwrap().first() {
            assert!(first.is_object());
            assert!(first["id"].is_string());
            assert!(first["brightness"].is_number());
        }
        // No status or room_states keys
        assert!(parsed.get("status").is_none());
        assert!(parsed.get("room_states").is_none());
    }

    // -- No handler returns {"status":"ok"} --

    #[test]
    fn no_status_ok_in_get_state() {
        let state = handler_state_with_runtime();
        let r = handle_get_state(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn no_status_ok_in_get_rooms_state() {
        let state = handler_state_with_runtime();
        let r = handle_get_rooms_state(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed.get("status").is_none());
    }
}
