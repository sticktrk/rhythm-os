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
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::api_types::{HubCredentialsResponse, NodesResponse, SyncResponse};
use crate::commands::{self};
use crate::logging;
use crate::state::SharedState;
use crate::topology::NodeControlKind;

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

    pub fn not_found(msg: &str) -> Self {
        Self {
            status: 404,
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

fn matter_capture_dir(state: &SharedState) -> Option<PathBuf> {
    let data_dir = state.lock().ok()?.data_dir.clone();
    if data_dir.is_empty() {
        return None;
    }
    Some(Path::new(&data_dir).join("matter").join("captures"))
}

fn normalize_matter_capture_id(id: &str) -> Result<String, String> {
    let normalized = id.strip_suffix(".json").unwrap_or(id);
    if normalized.is_empty()
        || !normalized
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("Invalid Matter capture id".to_string());
    }
    Ok(normalized.to_string())
}

fn summarize_matter_capture(id: &str, file_name: &str, capture: &Value) -> Value {
    json!({
        "id": id,
        "file": file_name,
        "source": capture.get("source").cloned().unwrap_or(Value::Null),
        "captured_at_unix_ms": capture
            .get("captured_at_unix_ms")
            .cloned()
            .unwrap_or(Value::Null),
        "vendor_name": capture
            .pointer("/commissioned/vendor_name")
            .cloned()
            .unwrap_or(Value::Null),
        "product_name": capture
            .pointer("/commissioned/product_name")
            .cloned()
            .unwrap_or(Value::Null),
        "vendor_id": capture
            .pointer("/commissioned/vendor_id")
            .cloned()
            .unwrap_or(Value::Null),
        "product_id": capture
            .pointer("/commissioned/product_id")
            .cloned()
            .unwrap_or(Value::Null),
        "node_id": capture
            .pointer("/commissioned/node_id")
            .cloned()
            .unwrap_or(Value::Null),
        "light_endpoint": capture
            .pointer("/commissioned/light_endpoint")
            .cloned()
            .unwrap_or(Value::Null),
        "color_modes": capture
            .pointer("/commissioned/color_modes")
            .cloned()
            .unwrap_or(Value::Null),
        "derived_quirks": capture
            .get("derived_quirks")
            .cloned()
            .unwrap_or(Value::Null),
        "db_match_name": capture
            .pointer("/db_match/name")
            .cloned()
            .unwrap_or(Value::Null),
    })
}

pub fn handle_get_matter_captures(state: &SharedState) -> ApiResponse {
    let Some(capture_dir) = matter_capture_dir(state) else {
        return ApiResponse::json_ok(r#"{"captures":[]}"#.to_string());
    };

    let entries = match fs::read_dir(&capture_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return ApiResponse::json_ok(r#"{"captures":[]}"#.to_string());
        }
        Err(err) => return ApiResponse::server_error(err),
    };

    let mut captures = Vec::<(u64, Value)>::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => return ApiResponse::server_error(err),
        };
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(file_name);

        let summary = match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<Value>(&contents) {
                Ok(capture) => summarize_matter_capture(id, file_name, &capture),
                Err(err) => json!({
                    "id": id,
                    "file": file_name,
                    "parse_error": err.to_string(),
                }),
            },
            Err(err) => json!({
                "id": id,
                "file": file_name,
                "read_error": err.to_string(),
            }),
        };

        let captured_at = summary
            .get("captured_at_unix_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        captures.push((captured_at, summary));
    }

    captures.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1["id"].as_str().cmp(&right.1["id"].as_str()))
    });

    let body = json!({
        "captures": captures
            .into_iter()
            .map(|(_, capture)| capture)
            .collect::<Vec<_>>(),
    });
    match serde_json::to_string(&body) {
        Ok(body) => ApiResponse::json_ok(body),
        Err(err) => ApiResponse::server_error(err),
    }
}

pub fn handle_get_matter_capture(state: &SharedState, id: &str) -> ApiResponse {
    let id = match normalize_matter_capture_id(id) {
        Ok(id) => id,
        Err(err) => return ApiResponse::bad_request(&err),
    };

    let Some(capture_dir) = matter_capture_dir(state) else {
        return ApiResponse::not_found("Matter capture not found");
    };
    let path = capture_dir.join(format!("{}.json", id));

    let body = match fs::read_to_string(&path) {
        Ok(body) => body,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return ApiResponse::not_found("Matter capture not found");
        }
        Err(err) => return ApiResponse::server_error(err),
    };

    if let Err(err) = serde_json::from_str::<Value>(&body) {
        return ApiResponse::server_error(format!(
            "Invalid Matter capture JSON in {}: {}",
            path.display(),
            err
        ));
    }

    ApiResponse::json_ok(body)
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

pub fn handle_get_configuration(state: &SharedState) -> ApiResponse {
    match commands::build_configuration_bundle(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_configuration(state: &SharedState, body: &Value) -> ApiResponse {
    let payload: crate::bundle::ConfigurationImportPayload =
        match serde_json::from_value(body.clone()) {
            Ok(payload) => payload,
            Err(e) => {
                return ApiResponse::bad_request(&format!("Invalid configuration bundle: {}", e));
            }
        };

    match commands::do_configuration_import(state, payload) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_get_factory_default_configuration() -> ApiResponse {
    match commands::build_factory_default_configuration_bundle() {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_post_configuration_reset(state: &SharedState) -> ApiResponse {
    match commands::do_configuration_reset(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_get_backup(state: &SharedState, include_secrets: bool) -> ApiResponse {
    match commands::build_backup_bundle(state, include_secrets) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_backup(state: &SharedState, body: &Value) -> ApiResponse {
    let bundle: crate::bundle::BackupBundle = match serde_json::from_value(body.clone()) {
        Ok(bundle) => bundle,
        Err(e) => return ApiResponse::bad_request(&format!("Invalid backup bundle: {}", e)),
    };

    match commands::do_backup_restore(state, bundle) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_get_rooms_state(state: &SharedState) -> ApiResponse {
    match commands::build_rooms_state(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_nodes_state(state: &SharedState) -> ApiResponse {
    match commands::build_nodes_state(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

fn perform_unpair_device(
    state: &SharedState,
    request: &crate::pairing::UnpairingRequest,
) -> anyhow::Result<crate::pairing::UnpairingResult> {
    let start_unpairing = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.start_unpairing_fn.clone()
    };

    let Some(start_fn) = start_unpairing else {
        anyhow::bail!("No unpairing support configured");
    };

    log::info!(
        target: "pair",
        "Unpairing request: hub_type={}, params={}",
        request.hub_type,
        logging::summarize_json_for_log(&request.params)
    );

    let result = start_fn(state, &request.hub_type, &request.params)?;
    log::info!(
        target: "pair",
        "Unpairing result: status={:?} error={:?}",
        result.status,
        result.error
    );

    if result.status == crate::pairing::PairingStatus::Complete {
        if let Some(device_id) = &result.device_id {
            let hub_key = crate::canonical::identity::HubKey::new(
                crate::hub::HubType::new(&request.hub_type),
                "local",
            );
            commands::do_device_hard_remove(state, device_id, Some(&hub_key))?;
        }
    }

    Ok(result)
}

fn embedded_delete_unpair_request(
    state: &SharedState,
    id: &str,
) -> anyhow::Result<Option<crate::pairing::UnpairingRequest>> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.platform_type != "embedded" {
        return Ok(None);
    }

    let matter_hub_key =
        crate::canonical::identity::HubKey::new(crate::hub::HubType::new("matter"), "local");
    let native_id = if id.starts_with("matter-") {
        Some(id.to_string())
    } else {
        s.canonical_registry.get(id).and_then(|device| {
            device
                .endpoints
                .iter()
                .find(|endpoint| {
                    endpoint.hub_key == matter_hub_key && endpoint.native_id.starts_with("matter-")
                })
                .map(|endpoint| endpoint.native_id.clone())
        })
    };

    Ok(native_id.map(|device_id| crate::pairing::UnpairingRequest {
        hub_type: "matter".to_string(),
        params: serde_json::json!({ "device_id": device_id }),
    }))
}

pub fn handle_delete_device(state: &SharedState, id: &str) -> ApiResponse {
    match embedded_delete_unpair_request(state, id) {
        Ok(Some(request)) => match perform_unpair_device(state, &request) {
            Ok(result) if result.status == crate::pairing::PairingStatus::Complete => {
                return ApiResponse::no_content();
            }
            Ok(result) => {
                let message = result
                    .error
                    .unwrap_or_else(|| format!("Unpairing failed for {}", request.hub_type));
                return ApiResponse::server_error(message);
            }
            Err(e) => return ApiResponse::server_error(e),
        },
        Ok(None) => {}
        Err(e) => return ApiResponse::server_error(e),
    }

    match commands::do_device_hard_remove(state, id, None) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_motion_timeout(state: &SharedState, body: &Value) -> ApiResponse {
    let raw_room_id = match body.get("node_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return ApiResponse::bad_request("Missing node_id"),
    };
    let room_id = commands::resolve_node_id(state, raw_room_id);

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

fn parse_profile_settings_patch(
    value: Option<&Value>,
    field_name: &str,
) -> Result<Option<commands::RoomProfileSettingsPatch>, String> {
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
        .ok_or_else(|| format!("{field_name} must be an object or null"))?;

    let profile_id = match body.get("profile_id") {
        None => None,
        Some(v) if v.is_null() => Some(None),
        Some(v) => Some(Some(
            v.as_str()
                .ok_or_else(|| format!("{field_name}.profile_id must be a string or null"))?
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
    let active_profile_id = s.active_mode_profile_id();
    let id = requested_id.unwrap_or(active_profile_id.as_str());
    crate::factory_default_config::factory_default_light_profile_config(id)
        .ok_or_else(|| format!("Unknown light profile: {}", id))
}

fn parse_node_control_kind(kind: &str) -> Result<NodeControlKind, String> {
    serde_json::from_value(serde_json::Value::String(kind.to_string()))
        .map_err(|_| format!("Invalid control kind: {}", kind))
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
    if body.get("transitions").is_some() || body.get("mode_transitions").is_some() {
        return ApiResponse::bad_request("Transitions moved to /api/transitions");
    }
    if body.get("mode").is_some()
        || body.get("active_mode").is_some()
        || body.get("modes").is_some()
        || body.get("last_active_mode_trigger").is_some()
        || body.get("last_active_mode_change_utc_ms").is_some()
    {
        return ApiResponse::bad_request("Mode fields moved to /api/mode");
    }
    if body.get("profiles").is_some() {
        return ApiResponse::bad_request("Profiles moved to /api/profiles and /api/config");
    }
    let power_save = body.get("power_save").and_then(|v| v.as_bool());

    match commands::do_settings_set(state, power_save, None, None, None) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_mode(state: &SharedState) -> ApiResponse {
    match commands::build_mode(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_mode(state: &SharedState, body: &Value) -> ApiResponse {
    if body.get("rhythm_interval_secs").is_some() {
        return ApiResponse::bad_request(
            "rhythm_interval_secs now belongs in light profile config",
        );
    }
    if body.get("mode").is_some()
        || body.get("active_mode").is_some()
        || body.get("modes").is_some()
    {
        return ApiResponse::bad_request("Use active/configs in /api/mode");
    }
    if body.get("transitions").is_some() || body.get("mode_transitions").is_some() {
        return ApiResponse::bad_request("Use /api/transitions");
    }
    if body.get("last_change").is_some() {
        return ApiResponse::bad_request("last_change is read-only");
    }
    if body.get("power_save").is_some() {
        return ApiResponse::bad_request("power_save belongs in /api/settings");
    }
    if body.get("profiles").is_some() {
        return ApiResponse::bad_request("Profiles moved to /api/profiles and /api/config");
    }
    let active_mode = match body.get("active").cloned() {
        Some(value) => match serde_json::from_value::<rhythm_core::RhythmMode>(value) {
            Ok(mode) => Some(mode),
            Err(_) => return ApiResponse::bad_request("Invalid active"),
        },
        None => None,
    };
    let modes = match body.get("configs").cloned() {
        Some(value) => match serde_json::from_value::<Vec<rhythm_core::ModeConfig>>(value) {
            Ok(modes) => Some(modes),
            Err(_) => return ApiResponse::bad_request("Invalid configs"),
        },
        None => None,
    };
    if let Some(ref modes) = modes {
        if let Err(e) = commands::validate_mode_configs(modes) {
            return ApiResponse::bad_request(&e.to_string());
        }
    }
    match commands::do_mode_set(state, active_mode, modes) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_transitions(state: &SharedState) -> ApiResponse {
    match commands::build_transitions(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_transitions(state: &SharedState, body: &Value) -> ApiResponse {
    if body.get("rhythm_interval_secs").is_some() {
        return ApiResponse::bad_request(
            "rhythm_interval_secs now belongs in light profile config",
        );
    }
    if body.get("power_save").is_some() {
        return ApiResponse::bad_request("power_save belongs in /api/settings");
    }
    if body.get("active").is_some()
        || body.get("configs").is_some()
        || body.get("last_change").is_some()
        || body.get("mode").is_some()
        || body.get("active_mode").is_some()
        || body.get("modes").is_some()
        || body.get("last_active_mode_trigger").is_some()
        || body.get("last_active_mode_change_utc_ms").is_some()
    {
        return ApiResponse::bad_request("Mode fields belong in /api/mode");
    }
    if body.get("profiles").is_some() {
        return ApiResponse::bad_request("Profiles moved to /api/profiles and /api/config");
    }
    if body.get("mode_transitions").is_some() {
        return ApiResponse::bad_request("Use transitions in /api/transitions");
    }
    let mode_transitions = match body.get("transitions").cloned() {
        Some(value) => {
            match serde_json::from_value::<Vec<rhythm_core::ModeTransitionConfig>>(value) {
                Ok(transitions) => Some(transitions),
                Err(_) => return ApiResponse::bad_request("Invalid transitions"),
            }
        }
        None => None,
    };

    match commands::do_transitions_set(state, mode_transitions) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_post_transition_trigger(state: &SharedState, transition_id: &str) -> ApiResponse {
    match commands::do_trigger_transition(state, transition_id) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) if e.to_string().contains("Unknown transition") => {
            ApiResponse::bad_request(&e.to_string())
        }
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_profiles(state: &SharedState) -> ApiResponse {
    match commands::build_profiles(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_light_profile(state: &SharedState, body: &Value) -> ApiResponse {
    let _ = (state, body);
    ApiResponse::bad_request(
        "Global light-profile selection has been replaced by settings.active_mode",
    )
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
            let hub_connected = state
                .lock()
                .map(|s| s.has_any_connected_hub())
                .unwrap_or(false);
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
        let room_id = commands::resolve_node_id(state, raw_room_id);
        let action = match item.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return ApiResponse::bad_request("Missing action"),
        };

        let per_item_persist = persist && !batch;
        match commands::do_node_action(state, &room_id, action, per_item_persist) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Dispatch node action(s). Accepts single object or array.
pub fn handle_node_action(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let items: Vec<Value> = if body.is_array() {
        body.as_array().cloned().unwrap_or_default()
    } else {
        vec![body.clone()]
    };

    let mut results = Vec::new();
    let batch = items.len() > 1;

    for item in &items {
        let raw_node_id = match item.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing node_id"),
        };
        let node_id = commands::resolve_node_id(state, raw_node_id);
        let action = match item.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return ApiResponse::bad_request("Missing action"),
        };

        let per_item_persist = persist && !batch;
        if let Err(e) = commands::do_node_action(state, &node_id, action, per_item_persist) {
            return ApiResponse::server_error(e);
        }
        match commands::build_node_state(state, &node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    match serde_json::to_string(&NodesResponse { nodes: results }) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
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
        let room_id = commands::resolve_node_id(state, raw_room_id);
        let brightness = match item.get("brightness").and_then(|v| v.as_u64()) {
            Some(b) => b as u8,
            None => return ApiResponse::bad_request("Missing brightness"),
        };

        let per_item_persist = persist && !batch;
        match commands::do_set_node_brightness(state, &room_id, brightness, per_item_persist) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Set node brightness. Accepts single object or array.
pub fn handle_set_node_brightness(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let items: Vec<Value> = if body.is_array() {
        body.as_array().cloned().unwrap_or_default()
    } else {
        vec![body.clone()]
    };

    let mut results = Vec::new();
    let batch = items.len() > 1;

    for item in &items {
        let raw_node_id = match item.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing node_id"),
        };
        let node_id = commands::resolve_node_id(state, raw_node_id);
        let brightness = match item.get("brightness").and_then(|v| v.as_u64()) {
            Some(b) => b as u8,
            None => return ApiResponse::bad_request("Missing brightness"),
        };

        let per_item_persist = persist && !batch;
        if let Err(e) =
            commands::do_set_node_brightness(state, &node_id, brightness, per_item_persist)
        {
            return ApiResponse::server_error(e);
        }
        match commands::build_node_state(state, &node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    match serde_json::to_string(&NodesResponse { nodes: results }) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
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
        let room_id = commands::resolve_node_id(state, raw_room_id);
        let time_offset = match item.get("time_offset").and_then(|v| v.as_f64()) {
            Some(t) => t as f32,
            None => return ApiResponse::bad_request("Missing time_offset"),
        };

        let per_item_persist = persist && !batch;
        match commands::do_set_node_time_offset(state, &room_id, time_offset, per_item_persist) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Set node time offset. Accepts single object or array.
pub fn handle_set_node_time_offset(
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
        let raw_node_id = match item.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing node_id"),
        };
        let node_id = commands::resolve_node_id(state, raw_node_id);
        let time_offset = match item.get("time_offset").and_then(|v| v.as_f64()) {
            Some(t) => t as f32,
            None => return ApiResponse::bad_request("Missing time_offset"),
        };

        let per_item_persist = persist && !batch;
        if let Err(e) =
            commands::do_set_node_time_offset(state, &node_id, time_offset, per_item_persist)
        {
            return ApiResponse::server_error(e);
        }
        match commands::build_node_state(state, &node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    match serde_json::to_string(&NodesResponse { nodes: results }) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
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
        let room_id = commands::resolve_node_id(state, raw_room_id);
        let rhythm_enabled = item.get("rhythm_enabled").and_then(|v| v.as_bool());
        let disabled = item.get("disabled").and_then(|v| v.as_bool());
        let room_state = match item.get("state").cloned() {
            Some(value) => match serde_json::from_value::<rhythm_core::RoomModeState>(value) {
                Ok(state) => Some(state),
                Err(_) => return ApiResponse::bad_request("Invalid room state"),
            },
            None => None,
        };
        let room_profile =
            match parse_profile_settings_patch(item.get("room_profile"), "room_profile") {
                Ok(patch) => patch,
                Err(e) => return ApiResponse::bad_request(&e),
            };

        let per_item_persist = persist && !batch;
        match commands::do_node_preferences_set(
            state,
            &room_id,
            rhythm_enabled,
            disabled,
            room_state,
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

/// Update node preferences. Accepts single object or array.
pub fn handle_put_node_preferences(
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
        let raw_node_id = match item.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing node_id"),
        };
        let node_id = commands::resolve_node_id(state, raw_node_id);
        let rhythm_enabled = item.get("rhythm_enabled").and_then(|v| v.as_bool());
        let disabled = item.get("disabled").and_then(|v| v.as_bool());
        let room_state = match item.get("state").cloned() {
            Some(value) => match serde_json::from_value::<rhythm_core::RoomModeState>(value) {
                Ok(state) => Some(state),
                Err(_) => return ApiResponse::bad_request("Invalid node state"),
            },
            None => None,
        };
        let room_profile =
            match parse_profile_settings_patch(item.get("profile_settings"), "profile_settings") {
                Ok(patch) => patch,
                Err(e) => return ApiResponse::bad_request(&e),
            };

        let per_item_persist = persist && !batch;
        if let Err(e) = commands::do_node_preferences_set(
            state,
            &node_id,
            rhythm_enabled,
            disabled,
            room_state,
            room_profile.as_ref(),
            per_item_persist,
        ) {
            return ApiResponse::server_error(e);
        }
        match commands::build_node_state(state, &node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    match serde_json::to_string(&NodesResponse { nodes: results }) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
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

    log::info!(
        target: "pair",
        "Pairing request: hub_type={}, params={}",
        request.hub_type,
        logging::summarize_json_for_log(&request.params)
    );

    match start_fn(state, &request.hub_type, &request.params) {
        Ok(session) => {
            log::info!(target: "pair", "Pairing result: status={:?} error={:?}", session.status, session.error);
            if session.status == crate::pairing::PairingStatus::Complete {
                // Persist canonical registry changes made during pairing
                if let Ok(s) = state.lock() {
                    commands::persist_canonical(&s);
                }
                // Persist hub registry updates if the integration created any
                commands::persist_registry(state);
                // Notify SSE clients
                #[cfg(feature = "desktop")]
                {
                    commands::emit_triage_changed(state);
                    crate::state::emit_server_event(
                        state,
                        crate::server_event::ServerEvent::NodesChanged,
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
    match perform_unpair_device(state, request) {
        Ok(result) => match serde_json::to_string(&result) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(e) => ApiResponse::server_error(e),
        },
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
    let room_id = body
        .get("room_id")
        .and_then(|v| v.as_str())
        .map(|raw_room_id| commands::resolve_node_id(state, raw_room_id));
    match commands::do_canonical_assign_room(state, device_id, room_id.as_deref()) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_device_parent(state: &SharedState, device_id: &str, body: &Value) -> ApiResponse {
    let parent_id = body
        .get("parent_id")
        .and_then(|v| v.as_str())
        .map(|raw_parent_id| commands::resolve_node_id(state, raw_parent_id));
    match commands::do_canonical_assign_room(state, device_id, parent_id.as_deref()) {
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

pub fn handle_put_triage_room(state: &SharedState, entry_id: &str, body: &Value) -> ApiResponse {
    let room_id = match body.get("room_id").and_then(|v| v.as_str()) {
        Some(room_id) => room_id,
        None => return ApiResponse::bad_request("Missing room_id"),
    };
    match commands::do_triage_assign_room(state, entry_id, room_id) {
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

pub fn handle_get_topology_nodes(state: &SharedState) -> ApiResponse {
    match commands::build_topology_nodes(state) {
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

pub fn handle_delete_topology_room(state: &SharedState, room_id: &str) -> ApiResponse {
    match commands::do_topology_delete_room(state, room_id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) if e.to_string().contains("Room not found") => {
            ApiResponse::bad_request("Room not found")
        }
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

pub fn handle_put_topology_node_control(
    state: &SharedState,
    source_id: &str,
    kind: &str,
    body: &Value,
) -> ApiResponse {
    let kind = match parse_node_control_kind(kind) {
        Ok(kind) => kind,
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let target_id = if body.get("target_id").is_some_and(|value| value.is_null()) {
        None
    } else {
        match body.get("target_id").and_then(|value| value.as_str()) {
            Some(target_id) => Some(target_id),
            None => return ApiResponse::bad_request("Missing target_id"),
        }
    };

    match commands::do_topology_set_control_target(state, source_id, kind, target_id) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) => ApiResponse::server_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
    use crate::canonical::registry::ResolveResult;
    use crate::factory_default_config::{
        factory_default_configuration_bundle, factory_default_light_profile_config,
    };
    use crate::hub::{ActiveHub, HubType};
    use crate::pairing::{PairingStatus, UnpairingRequest, UnpairingResult};
    use crate::registry::HubDeviceRegistry;
    use crate::state::AppState;
    use crate::topology::HubRoomBinding;
    use rhythm_core::runtime::hub_registry::DeviceType;
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

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

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(prefix: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "rhythm-os-{}-{}-{}",
                prefix,
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn state_with_matter_capture_dir() -> (SharedState, TestDir, PathBuf) {
        let state = test_state();
        let temp_dir = TestDir::new("matter-capture");
        let capture_dir = temp_dir.path().join("matter").join("captures");
        fs::create_dir_all(&capture_dir).unwrap();
        state.lock().unwrap().data_dir = temp_dir.path().display().to_string();
        (state, temp_dir, capture_dir)
    }

    #[test]
    fn put_motion_timeout_missing_node_id() {
        let state = test_state();
        let r = handle_put_motion_timeout(&state, &json!({"timeout_secs": 60}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("node_id"));
    }

    #[test]
    fn put_motion_timeout_missing_timeout() {
        let state = test_state();
        let r = handle_put_motion_timeout(&state, &json!({"node_id": "r"}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("timeout_secs"));
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

    #[test]
    fn get_matter_captures_returns_sorted_capture_summaries() {
        let (state, _temp_dir, capture_dir) = state_with_matter_capture_dir();
        fs::write(
            capture_dir.join("matter-100.json"),
            serde_json::to_string_pretty(&json!({
                "device_id": "matter-100",
                "source": "pair",
                "captured_at_unix_ms": 100,
                "commissioned": {
                    "node_id": 100,
                    "vendor_name": "GE",
                    "product_name": "Cync",
                    "vendor_id": 1,
                    "product_id": 2,
                    "light_endpoint": 1,
                    "color_modes": ["color_temperature"]
                },
                "derived_quirks": ["needs_xy_not_ct"]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            capture_dir.join("matter-102.json"),
            serde_json::to_string_pretty(&json!({
                "device_id": "matter-102",
                "source": "on_demand_probe",
                "captured_at_unix_ms": 200,
                "commissioned": {
                    "node_id": 102,
                    "vendor_name": "Shenzen",
                    "product_name": "Bulb",
                    "vendor_id": 4921,
                    "product_id": 171,
                    "light_endpoint": 1,
                    "color_modes": ["xy", "color_temperature"]
                },
                "derived_quirks": ["needs_explicit_on"]
            }))
            .unwrap(),
        )
        .unwrap();

        let r = handle_get_matter_captures(&state);
        assert_eq!(r.status, 200);

        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        let captures = parsed["captures"].as_array().unwrap();
        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0]["id"], "matter-102");
        assert_eq!(captures[0]["vendor_id"], 4921);
        assert_eq!(captures[0]["derived_quirks"][0], "needs_explicit_on");
        assert_eq!(captures[1]["id"], "matter-100");
    }

    #[test]
    fn get_matter_capture_returns_full_capture_json() {
        let (state, _temp_dir, capture_dir) = state_with_matter_capture_dir();
        fs::write(
            capture_dir.join("matter-102.json"),
            serde_json::to_string_pretty(&json!({
                "device_id": "matter-102",
                "source": "pair",
                "captured_at_unix_ms": 200,
                "commissioned": {
                    "node_id": 102,
                    "vendor_name": "Shenzen",
                    "product_name": "Bulb",
                    "vendor_id": 4921,
                    "product_id": 171,
                    "light_endpoint": 1,
                    "color_modes": ["xy"]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let r = handle_get_matter_capture(&state, "matter-102");
        assert_eq!(r.status, 200);

        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["device_id"], "matter-102");
        assert_eq!(parsed["commissioned"]["vendor_id"], 4921);
    }

    #[test]
    fn get_matter_capture_rejects_invalid_id() {
        let state = test_state();
        let r = handle_get_matter_capture(&state, "../matter-102");
        assert_eq!(r.status, 400);
        assert!(r.body.contains("Invalid Matter capture id"));
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
            fn set_mode_configs(&self, _: Vec<rhythm_core::ModeConfig>) -> anyhow::Result<()> {
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
            fn restore_room_state(&self, _: &str, _: rhythm_core::RestoredRoomState) {}
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
                Ok(())
            }
            fn lights_off_room(&self, _: &str, _: Option<u32>) -> anyhow::Result<()> {
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
                    kind: rhythm_core::LightNodeKind::Room,
                    parent_id: None,
                    rhythm_enabled: true,
                    disabled: false,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                    hard_off: false,
                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                },
                RoomSnapshot {
                    id: "room2".into(),
                    name: "Room 2".into(),
                    kind: rhythm_core::LightNodeKind::Room,
                    parent_id: None,
                    rhythm_enabled: false,
                    disabled: false,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                    hard_off: false,
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

    fn handler_state_with_canonical_light_for_hub(
        hub_type_name: &str,
        native_id: &str,
    ) -> (
        SharedState,
        Arc<Mutex<HubDeviceRegistry>>,
        String,
        String,
        HubKey,
    ) {
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::with_options(true)));
        let mut app = AppState::default();
        let hub_type = HubType::new(hub_type_name);
        let hub_key = HubKey::new(hub_type.clone(), "local");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key: hub_key.clone(),
                runtime: None,
                hub_data: Box::new(()),
                registry: Some(registry.clone()),
                discovery: None,
                shutdown: Default::default(),
            },
        );

        let room_id = app.topology.create_room("Office");
        let identity = DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: native_id.to_string(),
            room_name: "Office Lamp".to_string(),
            name: "Office Lamp".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("100")],
            manufacturer: None,
            model: None,
        };
        let canonical_id = match app.canonical_registry.resolve(&identity, &hub_key, 1) {
            ResolveResult::AlreadyKnown { canonical_id }
            | ResolveResult::ReApproved { canonical_id }
            | ResolveResult::Created { canonical_id } => canonical_id,
            ResolveResult::Queued { .. } => panic!("unexpected triage for test device"),
        };
        app.canonical_registry
            .assign_room(&canonical_id, Some(&room_id));
        let _ = app
            .topology
            .attach_device_hub_default(&room_id, &canonical_id);
        app.topology
            .get_mut(&room_id)
            .unwrap()
            .upsert_hub_room_binding(HubRoomBinding {
                hub_key: hub_key.clone(),
                hub_room_id: native_id.to_string(),
                control_id: native_id.to_string(),
                light_device_ids: vec![native_id.to_string()],
            });

        registry.lock().unwrap().upsert_room(
            native_id,
            "Office Lamp",
            native_id,
            &[native_id.to_string()],
        );

        (
            Arc::new(Mutex::new(app)),
            registry,
            canonical_id,
            room_id,
            hub_key,
        )
    }

    fn handler_state_with_canonical_light() -> (
        SharedState,
        Arc<Mutex<HubDeviceRegistry>>,
        String,
        String,
        HubKey,
    ) {
        handler_state_with_canonical_light_for_hub("mock", "device-1")
    }

    // -- Void mutations return 204 --

    #[test]
    fn delete_topology_room_returns_204() {
        let (state, _registry, canonical_id, room_id, _hub_key) =
            handler_state_with_canonical_light();
        let r = handle_delete_topology_room(&state, &room_id);
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());

        let state = state.lock().unwrap();
        assert!(state.topology.get(&room_id).is_none());
        assert_eq!(
            state
                .canonical_registry
                .get(&canonical_id)
                .and_then(|device| device.room_id.clone()),
            None
        );
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
        let _ = handle_put_config(
            &state,
            Some(rhythm_core::RHYTHM_PROFILE_ID),
            &json!({
                "id": "rhythm",
                "name": "Modified Day",
                "curve": { "type": "super-gaussian" },
                "min_brightness": 17,
                "max_brightness": 83,
                "min_color_temp": 2200,
                "max_color_temp": 5000,
                "max_dim_steps": 4,
                "fade_ms": { "mode": "fixed", "value": 999 }
            }),
        );
        let r = handle_reset_config(&state, None);
        assert_eq!(r.status, 200);
        let parsed: rhythm_core::LightProfileConfig = serde_json::from_str(&r.body).unwrap();
        assert_eq!(
            parsed,
            factory_default_light_profile_config(rhythm_core::RHYTHM_PROFILE_ID).unwrap()
        );
    }

    #[test]
    fn get_factory_default_configuration_returns_factory_default_bundle() {
        let r = handle_get_factory_default_configuration();
        assert_eq!(r.status, 200);
        let parsed: crate::bundle::ConfigurationBundle = serde_json::from_str(&r.body).unwrap();
        assert_eq!(
            parsed.name.as_deref(),
            Some(
                factory_default_configuration_bundle()
                    .name
                    .as_deref()
                    .unwrap_or("")
            )
        );
        assert_eq!(
            parsed.configuration.profiles,
            factory_default_configuration_bundle()
                .configuration
                .profiles
        );
    }

    #[test]
    fn post_configuration_reset_restores_factory_default_bundle() {
        let state = handler_state_with_runtime();
        let _ = handle_put_configuration(
            &state,
            &json!({
                "configuration": {
                    "power_save": true,
                    "active_mode": "day",
                    "profiles": [{
                        "id": "focus",
                        "name": "Focus",
                        "curve": { "type": "super-gaussian" },
                        "min_brightness": 10,
                        "max_brightness": 40,
                        "min_color_temp": 1800,
                        "max_color_temp": 4000,
                        "max_dim_steps": 4
                    }],
                    "mode_configs": [{
                        "mode": "day",
                        "active_profile_id": "focus"
                    }],
                    "mode_transitions": [],
                    "rooms": []
                }
            }),
        );

        let r = handle_post_configuration_reset(&state);
        assert_eq!(r.status, 200);
        let parsed: crate::bundle::ConfigurationBundle = serde_json::from_str(&r.body).unwrap();
        let factory_default = factory_default_configuration_bundle();
        let mut parsed_profiles = parsed.configuration.profiles.clone();
        parsed_profiles.sort_by(|left, right| left.id.cmp(&right.id));
        let mut expected_profiles = factory_default.configuration.profiles.clone();
        expected_profiles.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(
            parsed.configuration.power_save,
            factory_default.configuration.power_save
        );
        assert_eq!(
            parsed.configuration.active_mode,
            factory_default.configuration.active_mode
        );
        assert_eq!(
            parsed.configuration.mode_configs,
            factory_default.configuration.mode_configs
        );
        assert_eq!(
            parsed.configuration.mode_transitions,
            factory_default.configuration.mode_transitions
        );
        assert_eq!(parsed_profiles, expected_profiles);
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
        assert_eq!(parsed["min_brightness"], 20);
        assert_eq!(parsed["max_brightness"], 20);
        assert_eq!(parsed["curve"]["type"], "constant");
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
        let factory_profiles =
            crate::factory_default_config::factory_default_light_profile_config_map();
        assert_eq!(
            state
                .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
                .unwrap()
                .min_brightness,
            factory_profiles[rhythm_core::RHYTHM_PROFILE_ID].min_brightness
        );
    }

    #[test]
    fn put_config_day_idle_default_fifteen_artifact_normalizes_to_one_percent() {
        let state = handler_state_with_runtime();
        let r = handle_put_config(
            &state,
            Some(rhythm_core::DAY_IDLE_PROFILE_ID),
            &json!({
                "id": "day_idle",
                "name": "Day Idle",
                "curve": {
                    "type": "constant",
                    "brightness": 15,
                    "color_temp": 0,
                    "direct_color": {
                        "xy": { "x": 0.2041, "y": 0.2444 },
                        "rgb": { "r": 38, "g": 191, "b": 255 }
                    }
                },
                "min_brightness": 15,
                "max_brightness": 15,
                "min_color_temp": 0,
                "max_color_temp": 0,
                "max_dim_steps": 1,
                "fade_ms": { "mode": "auto" },
                "motion_timeout_secs": { "mode": "auto" },
                "rhythm_interval_secs": { "mode": "auto" }
            }),
        );
        assert_eq!(r.status, 204);

        let stored = state
            .lock()
            .unwrap()
            .light_profile_config(rhythm_core::DAY_IDLE_PROFILE_ID)
            .unwrap()
            .clone();
        assert_eq!(stored.min_brightness, 1);
        assert_eq!(stored.max_brightness, 1);
        assert!(matches!(
            stored.curve,
            rhythm_core::LightCurveShape::Constant {
                brightness: 1.0,
                ..
            }
        ));
    }

    #[test]
    fn put_config_day_idle_custom_brightness_is_preserved() {
        let state = handler_state_with_runtime();
        let r = handle_put_config(
            &state,
            Some(rhythm_core::DAY_IDLE_PROFILE_ID),
            &json!({
                "id": "day_idle",
                "name": "Day Idle",
                "curve": {
                    "type": "constant",
                    "brightness": 1.0,
                    "color_temp": 0,
                    "direct_color": {
                        "xy": { "x": 0.2041, "y": 0.2444 },
                        "rgb": { "r": 38, "g": 191, "b": 255 }
                    }
                },
                "min_brightness": 20,
                "max_brightness": 20,
                "min_color_temp": 0,
                "max_color_temp": 0,
                "max_dim_steps": 1,
                "fade_ms": { "mode": "auto" },
                "motion_timeout_secs": { "mode": "auto" },
                "rhythm_interval_secs": { "mode": "auto" }
            }),
        );
        assert_eq!(r.status, 204);

        let stored = state
            .lock()
            .unwrap()
            .light_profile_config(rhythm_core::DAY_IDLE_PROFILE_ID)
            .unwrap()
            .clone();
        assert_eq!(stored.min_brightness, 20);
        assert_eq!(stored.max_brightness, 20);
        assert!(matches!(
            stored.curve,
            rhythm_core::LightCurveShape::Constant {
                brightness: 1.0,
                ..
            }
        ));
    }

    #[test]
    fn reset_config_for_day_idle_returns_inherit_active_default() {
        let state = handler_state_with_runtime();
        let r = handle_reset_config(&state, Some(rhythm_core::DAY_IDLE_PROFILE_ID));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["id"], rhythm_core::DAY_IDLE_PROFILE_ID);
        assert_eq!(parsed["curve"]["type"], "inherit-active");
        assert_eq!(parsed["min_brightness"], 1);
    }

    #[test]
    fn reset_config_for_sleep_idle_returns_inherit_active_default() {
        let state = handler_state_with_runtime();
        let r = handle_reset_config(&state, Some(rhythm_core::SLEEP_IDLE_PROFILE_ID));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["id"], rhythm_core::SLEEP_IDLE_PROFILE_ID);
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
        let r = handle_put_motion_timeout(&state, &json!({"node_id": "room1", "timeout_secs": 60}));
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());
    }

    #[test]
    fn put_device_parent_returns_204() {
        let state = handler_state_with_runtime();
        let (canonical_id, target_room_id) = {
            let mut state = state.lock().unwrap();
            let hub_key = state.hubs.keys().next().cloned().unwrap();
            let room_id = state.topology.create_room("Office");
            let identity = DiscoveredIdentity {
                native_id: "device-1".to_string(),
                room_id: "device-1".to_string(),
                room_name: "Office Lamp".to_string(),
                name: "Office Lamp".to_string(),
                device_type: DeviceType::Light,
                hardware_ids: vec![HardwareId::matter("100")],
                manufacturer: None,
                model: None,
            };
            let canonical_id = match state.canonical_registry.resolve(&identity, &hub_key, 1) {
                ResolveResult::AlreadyKnown { canonical_id }
                | ResolveResult::ReApproved { canonical_id }
                | ResolveResult::Created { canonical_id } => canonical_id,
                ResolveResult::Queued { .. } => panic!("unexpected triage result"),
            };
            state
                .canonical_registry
                .assign_room(&canonical_id, Some(&room_id));
            assert!(state
                .topology
                .attach_device_user_override(&room_id, &canonical_id));
            let target_room_id = state.topology.create_room("Desk");
            (canonical_id, target_room_id)
        };

        let r = handle_put_device_parent(
            &state,
            &canonical_id,
            &json!({"parent_id": target_room_id.clone()}),
        );
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());

        let state = state.lock().unwrap();
        assert_eq!(
            state.topology.device_parent_room_id(&canonical_id),
            Some(target_room_id.as_str())
        );
    }

    #[test]
    fn put_topology_node_control_returns_204() {
        let state = test_state();
        let (source_id, target_id) = {
            let mut state = state.lock().unwrap();
            let source_id = state.topology.create_room("Source");
            let target_id = state.topology.create_room("Target");
            (source_id, target_id)
        };

        let r = handle_put_topology_node_control(
            &state,
            &source_id,
            "motion",
            &json!({"target_id": target_id.clone()}),
        );
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());

        let state = state.lock().unwrap();
        assert_eq!(
            state
                .topology
                .explicit_control_target(&source_id, &crate::topology::NodeControlKind::Motion),
            Some(target_id.as_str())
        );
    }

    #[test]
    fn put_topology_node_control_invalid_kind_returns_400() {
        let state = test_state();
        let source_id = {
            let mut state = state.lock().unwrap();
            state.topology.create_room("Source")
        };

        let r = handle_put_topology_node_control(
            &state,
            &source_id,
            "unknown",
            &json!({"target_id": null}),
        );
        assert_eq!(r.status, 400);
        assert!(r.body.contains("Invalid control kind"));
    }

    #[test]
    fn delete_device_hard_removes_canonical_topology_and_registry_entries() {
        let (state, registry, canonical_id, room_id, hub_key) =
            handler_state_with_canonical_light();

        let r = handle_delete_device(&state, "device-1");
        assert_eq!(r.status, 204);

        let s = state.lock().unwrap();
        assert!(s.canonical_registry.get(&canonical_id).is_none());
        let room = s.topology.get(&room_id).unwrap();
        assert!(!room.devices.iter().any(|d| d.device_id == canonical_id));
        assert!(!room
            .hub_room_bindings
            .iter()
            .any(|t| t.hub_key == hub_key && t.hub_room_id == "device-1"));
        drop(s);

        let reg = registry.lock().unwrap();
        assert!(reg.get_light_entities("device-1").is_empty());
        assert!(!reg.rooms().iter().any(|room| room.id == "device-1"));
    }

    #[test]
    fn delete_device_on_embedded_matter_uses_unpairing() {
        let (state, registry, canonical_id, room_id, hub_key) =
            handler_state_with_canonical_light_for_hub("matter", "matter-100");
        let calls = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
        {
            let calls = calls.clone();
            let mut s = state.lock().unwrap();
            s.platform_type = "embedded";
            s.platform_context = "rpiz";
            s.start_unpairing_fn = Some(Arc::new(move |_, hub_type, params| {
                calls
                    .lock()
                    .unwrap()
                    .push((hub_type.to_string(), params.clone()));
                Ok(UnpairingResult {
                    hub_type: hub_type.to_string(),
                    status: PairingStatus::Complete,
                    device_id: Some("matter-100".to_string()),
                    error: None,
                })
            }));
        }

        let r = handle_delete_device(&state, &canonical_id);
        assert_eq!(r.status, 204);

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "matter");
        assert_eq!(recorded[0].1["device_id"], "matter-100");
        drop(recorded);

        let s = state.lock().unwrap();
        assert!(s.canonical_registry.get(&canonical_id).is_none());
        let room = s.topology.get(&room_id).unwrap();
        assert!(!room.devices.iter().any(|d| d.device_id == canonical_id));
        assert!(!room
            .hub_room_bindings
            .iter()
            .any(|t| t.hub_key == hub_key && t.hub_room_id == "matter-100"));
        drop(s);

        let reg = registry.lock().unwrap();
        assert!(reg.get_light_entities("matter-100").is_empty());
        assert!(!reg.rooms().iter().any(|room| room.id == "matter-100"));
    }

    #[test]
    fn unpair_completion_uses_hard_remove_cleanup() {
        let (state, registry, canonical_id, room_id, hub_key) =
            handler_state_with_canonical_light();
        {
            let mut s = state.lock().unwrap();
            s.start_unpairing_fn = Some(Arc::new(|_, _, _| {
                Ok(UnpairingResult {
                    hub_type: "mock".to_string(),
                    status: PairingStatus::Complete,
                    device_id: Some("device-1".to_string()),
                    error: None,
                })
            }));
        }

        let r = handle_unpair_device(
            &state,
            &UnpairingRequest {
                hub_type: "mock".to_string(),
                params: json!({ "device_id": "device-1", "force": true }),
            },
        );
        assert_eq!(r.status, 200);

        let s = state.lock().unwrap();
        assert!(s.canonical_registry.get(&canonical_id).is_none());
        let room = s.topology.get(&room_id).unwrap();
        assert!(!room.devices.iter().any(|d| d.device_id == canonical_id));
        assert!(!room
            .hub_room_bindings
            .iter()
            .any(|t| t.hub_key == hub_key && t.hub_room_id == "device-1"));
        drop(s);

        let reg = registry.lock().unwrap();
        assert!(reg.get_light_entities("device-1").is_empty());
        assert!(!reg.rooms().iter().any(|room| room.id == "device-1"));
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
        assert!(parsed.get("mode").is_none());
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn get_settings_returns_raw_settings() {
        let state = handler_state_with_runtime();
        let r = handle_get_settings(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed["power_save"].is_boolean());
        assert!(parsed.get("mode").is_none());
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn put_settings_rejects_mode_payload() {
        let state = handler_state_with_runtime();
        let r = handle_put_settings(&state, &json!({"mode": {"active": "sleep"}}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("Mode fields moved to /api/mode"));
    }

    #[test]
    fn put_settings_rejects_transitions_payload() {
        let state = handler_state_with_runtime();
        let r = handle_put_settings(&state, &json!({"transitions": []}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("Transitions moved to /api/transitions"));
    }

    #[test]
    fn get_mode_returns_raw_mode() {
        let state = handler_state_with_runtime();
        let r = handle_get_mode(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed["active"].is_string());
        assert!(parsed["configs"].is_array());
        assert!(parsed.get("transitions").is_none());
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn put_mode_returns_raw_mode() {
        let state = handler_state_with_runtime();
        let r = handle_put_mode(&state, &json!({"active": "sleep"}));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["active"], "sleep");
        assert!(parsed["last_change"].is_object());
    }

    #[test]
    fn put_mode_rejects_read_only_last_change() {
        let state = handler_state_with_runtime();
        let r = handle_put_mode(&state, &json!({"last_change": {"trigger": "manual"}}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("last_change is read-only"));
    }

    #[test]
    fn put_mode_rejects_transitions_payload() {
        let state = handler_state_with_runtime();
        let r = handle_put_mode(&state, &json!({"transitions": []}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("Use /api/transitions"));
    }

    #[test]
    fn put_mode_rejects_runtime_only_room_default_states() {
        let state = handler_state_with_runtime();
        let r = handle_put_mode(
            &state,
            &json!({
                "configs": [{
                    "mode": "sleep",
                    "active_profile_id": "sleep",
                    "room_defaults": [{
                        "room_id": "office",
                        "state": "warning"
                    }]
                }]
            }),
        );
        assert_eq!(r.status, 400);
        assert!(r.body.contains("runtime-only state"));
    }

    #[test]
    fn get_transitions_returns_wrapper() {
        let state = handler_state_with_runtime();
        let r = handle_get_transitions(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed["transitions"].is_array());
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn put_transitions_returns_wrapper() {
        let state = handler_state_with_runtime();
        let r = handle_put_transitions(&state, &json!({"transitions": []}));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["transitions"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn put_transitions_accepts_auto_duration() {
        let state = handler_state_with_runtime();
        let r = handle_put_transitions(
            &state,
            &json!({
                "transitions": [{
                    "from_mode": "sleep",
                    "to_mode": "day",
                    "trigger": {"kind": "solar", "event": "sunrise"},
                    "duration_ms": {"mode": "auto"}
                }]
            }),
        );
        assert_eq!(r.status, 200);

        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["transitions"][0]["duration_ms"]["mode"], "auto");
    }

    #[test]
    fn put_transitions_accepts_scheduled_trigger() {
        let state = handler_state_with_runtime();
        let r = handle_put_transitions(
            &state,
            &json!({
                "transitions": [{
                    "from_mode": "day",
                    "to_mode": "sleep",
                    "trigger": {"kind": "scheduled", "time": "22:00"},
                    "duration_ms": {"mode": "fixed", "value": 5000}
                }]
            }),
        );
        assert_eq!(r.status, 200);

        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["transitions"][0]["trigger"]["kind"], "scheduled");
        assert_eq!(parsed["transitions"][0]["trigger"]["time"], "22:00");
        assert_eq!(parsed["transitions"][0]["duration_ms"]["mode"], "fixed");
        assert_eq!(parsed["transitions"][0]["duration_ms"]["value"], 5000);
    }

    #[test]
    fn put_transitions_rejects_legacy_mode_transitions_payload() {
        let state = handler_state_with_runtime();
        let r = handle_put_transitions(&state, &json!({"mode_transitions": []}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("Use transitions in /api/transitions"));
    }

    #[test]
    fn get_profiles_returns_profiles_wrapper() {
        let state = handler_state_with_runtime();
        let r = handle_get_profiles(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed["profiles"].is_array());
        assert!(!parsed["profiles"].as_array().unwrap().is_empty());
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
