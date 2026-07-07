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

use std::collections::BTreeMap;
use std::fmt::Display;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use rhythm_runtime_api::{RuntimeExtensionRequest, RuntimeExtensionResponse, RuntimeHttpMethod};

use crate::api_types::{HubCredentialsResponse, NodesResponse, SyncResponse};
use crate::commands::{self};
use crate::logging;
use crate::state::SharedState;
use crate::topology::{InputBinding, InputBindingPreset, NodeControlKind};
use rhythm_core::{ButtonAction, LightProfileNodeOverride, Rgb, XyColor};

type ProfileOverridesPatch = Option<Option<BTreeMap<String, Option<LightProfileNodeOverride>>>>;

fn mutation_items(body: &Value) -> Result<Vec<Value>, String> {
    if let Some(items) = body.get("items").and_then(|v| v.as_array()) {
        return Ok(items.clone());
    }
    if let Some(items) = body.get("nodes").and_then(|v| v.as_array()) {
        return Ok(items.clone());
    }
    if let Some(items) = body.get("rooms").and_then(|v| v.as_array()) {
        return Ok(items.clone());
    }
    if body.is_array() {
        return Ok(body.as_array().cloned().unwrap_or_default());
    }
    Ok(vec![body.clone()])
}

fn dispatch_spacing_from_body(body: &Value) -> Result<Duration, String> {
    let Some(value) = body.get("dispatch_spacing_ms") else {
        return Ok(commands::default_http_batch_dispatch_spacing());
    };
    let Some(ms) = value.as_u64() else {
        return Err("dispatch_spacing_ms must be a non-negative integer".to_string());
    };
    if ms > 60_000 {
        return Err("dispatch_spacing_ms must be <= 60000".to_string());
    }
    Ok(Duration::from_millis(ms))
}

fn parse_rgb(value: Option<&Value>) -> Result<Rgb, String> {
    let value = value.ok_or_else(|| "Missing rgb".to_string())?;
    let body = value
        .as_object()
        .ok_or_else(|| "rgb must be an object".to_string())?;

    let channel = |key: &str| -> Result<u8, String> {
        let raw = body
            .get(key)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("rgb.{key} must be an integer"))?;
        if raw > u8::MAX as u64 {
            return Err(format!("rgb.{key} must be <= 255"));
        }
        Ok(raw as u8)
    };

    Ok(Rgb::new(channel("r")?, channel("g")?, channel("b")?))
}

fn parse_xy(value: Option<&Value>) -> Result<Option<XyColor>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let body = value
        .as_object()
        .ok_or_else(|| "xy must be an object or null".to_string())?;
    let x = body
        .get("x")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "xy.x must be a number".to_string())? as f32;
    let y = body
        .get("y")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "xy.y must be a number".to_string())? as f32;
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err("xy values must be between 0 and 1".to_string());
    }
    Ok(Some(XyColor { x, y }))
}

fn parse_optional_u8(value: Option<&Value>, key: &str) -> Result<Option<u8>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let raw = value
        .as_u64()
        .ok_or_else(|| format!("{key} must be an integer or null"))?;
    Ok(Some(raw.clamp(1, 100) as u8))
}

fn parse_node_curve_modifier(item: &Value) -> Result<commands::NodeCurveModifier, String> {
    let brightness = item.get("brightness").and_then(|v| v.as_u64());
    let color_temperature = item
        .get("color_temperature")
        .or_else(|| item.get("color_temp_kelvin"))
        .or_else(|| item.get("kelvin"))
        .and_then(|v| v.as_u64());

    match (brightness, color_temperature) {
        (Some(_), Some(_)) => {
            Err("Specify exactly one curve modifier: brightness or color_temperature".to_string())
        }
        (Some(brightness), None) => Ok(commands::NodeCurveModifier::Brightness(
            brightness.clamp(1, 100) as u8,
        )),
        (None, Some(kelvin)) => {
            let preserve_brightness = item
                .get("preserve_brightness")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            Ok(commands::NodeCurveModifier::ColorTemperature {
                kelvin: kelvin.clamp(500, 25_000) as u16,
                preserve_brightness,
            })
        }
        (None, None) => Err("Missing curve modifier: brightness or color_temperature".to_string()),
    }
}

fn parse_optional_u32(value: Option<&Value>, key: &str) -> Result<Option<u32>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let raw = value
        .as_u64()
        .ok_or_else(|| format!("{key} must be an integer or null"))?;
    if raw > u32::MAX as u64 {
        return Err(format!("{key} must be <= {}", u32::MAX));
    }
    Ok(Some(raw as u32))
}

fn parse_color_scope(value: Option<&Value>) -> Result<commands::NodeColorScope, String> {
    let Some(value) = value else {
        return Ok(commands::NodeColorScope::Auto);
    };
    let raw = value
        .as_str()
        .ok_or_else(|| "scope must be a string".to_string())?;
    match raw {
        "auto" => Ok(commands::NodeColorScope::Auto),
        "preview" => Ok(commands::NodeColorScope::Preview),
        "mood" => Ok(commands::NodeColorScope::Mood),
        _ => Err(format!("Invalid color scope: {raw}")),
    }
}

fn nodes_response(
    results: Vec<crate::api_types::NodeStateDto>,
    queued: bool,
    spacing: Duration,
) -> ApiResponse {
    let dispatch_count = queued.then_some(results.len());
    nodes_response_with_dispatch_count(results, dispatch_count, spacing)
}

fn nodes_response_with_dispatch_count(
    results: Vec<crate::api_types::NodeStateDto>,
    dispatch_count: Option<usize>,
    spacing: Duration,
) -> ApiResponse {
    let queued = dispatch_count.is_some_and(|count| count > 0);
    let body = NodesResponse {
        nodes: results,
        queued: queued.then_some(true),
        dispatch_count: queued.then_some(dispatch_count.unwrap_or_default()),
        dispatch_spacing_ms: queued.then_some(spacing.as_millis() as u64),
        estimated_dispatch_ms: queued.then_some(
            commands::estimated_dispatch_duration(dispatch_count.unwrap_or_default(), spacing)
                .as_millis() as u64,
        ),
    };
    match serde_json::to_string(&body) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

fn node_time_offset_error_response(e: anyhow::Error) -> ApiResponse {
    let message = e.to_string();
    if message.contains("not found in engine") {
        ApiResponse::not_found(&message)
    } else if message.contains("Time offsets can only be set") {
        ApiResponse::bad_request(&message)
    } else {
        ApiResponse::server_error(message)
    }
}

fn light_runtime_error_response(e: anyhow::Error) -> ApiResponse {
    let message = e.to_string();
    if message.contains("unsupported runtime extension endpoint") {
        ApiResponse::not_found(&message)
    } else if message.contains("unknown light runtime")
        || message.contains("invalid runtime extension request")
    {
        ApiResponse::bad_request(&message)
    } else if message.contains("is not active") {
        ApiResponse {
            status: 409,
            body: message,
            content_type: "text/plain",
        }
    } else {
        ApiResponse::server_error(message)
    }
}

fn runtime_extension_api_response(response: RuntimeExtensionResponse) -> ApiResponse {
    if response.status == 204 {
        return ApiResponse::no_content();
    }
    match serde_json::to_string(&response.body) {
        Ok(body) => ApiResponse {
            status: response.status,
            body,
            content_type: "application/json",
        },
        Err(e) => ApiResponse::server_error(e),
    }
}

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

    pub fn forbidden(msg: &str) -> Self {
        Self {
            status: 403,
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

pub fn handle_matter_bulb_test_run(state: &SharedState, body: &Value) -> ApiResponse {
    let run_test = {
        let Ok(s) = state.lock() else {
            return ApiResponse::server_error("lock");
        };
        s.run_device_test_fn.clone()
    };

    let Some(run_fn) = run_test else {
        return ApiResponse::server_error("No device test support configured");
    };

    match run_fn(state, "matter", body) {
        Ok(result) => match serde_json::to_string(&result) {
            Ok(body) => ApiResponse::json_ok(body),
            Err(err) => ApiResponse::server_error(err),
        },
        Err(err) => ApiResponse::server_error(err),
    }
}

pub fn handle_matter_bulb_test_report(state: &SharedState, body: &Value) -> ApiResponse {
    let save_report = {
        let Ok(s) = state.lock() else {
            return ApiResponse::server_error("lock");
        };
        s.save_device_test_report_fn.clone()
    };

    let Some(save_fn) = save_report else {
        return ApiResponse::server_error("No device test report support configured");
    };

    match save_fn(state, "matter", body) {
        Ok(result) => match serde_json::to_string(&result) {
            Ok(body) => ApiResponse::json_ok(body),
            Err(err) => ApiResponse::server_error(err),
        },
        Err(err) => ApiResponse::server_error(err),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub fn handle_health() -> ApiResponse {
    ApiResponse::json_ok(r#"{"status":"healthy"}"#.to_string())
}

pub fn handle_get_state_with_options(state: &SharedState, authoritative: bool) -> ApiResponse {
    if authoritative {
        if let Err(e) = commands::refresh_observed_power_authoritatively(state) {
            return ApiResponse::server_error(e);
        }
    }

    match commands::build_state_snapshot(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_state(state: &SharedState) -> ApiResponse {
    handle_get_state_with_options(state, false)
}

pub fn handle_get_profile_bundle(state: &SharedState) -> ApiResponse {
    match commands::build_profile_bundle(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_profile_bundle(state: &SharedState, body: &Value) -> ApiResponse {
    let payload: crate::bundle::ProfileBundleImportPayload =
        match serde_json::from_value(body.clone()) {
            Ok(payload) => payload,
            Err(e) => {
                return ApiResponse::bad_request(&format!("Invalid profile bundle: {}", e));
            }
        };

    match commands::do_profile_bundle_import(state, payload) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_get_factory_default_profile_bundle() -> ApiResponse {
    match commands::build_factory_default_profile_bundle() {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_post_profile_bundle_reset(state: &SharedState) -> ApiResponse {
    match commands::do_profile_bundle_reset(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_post_factory_reset(state: &SharedState) -> ApiResponse {
    match commands::do_factory_reset(state) {
        Ok(json) => {
            if let Some(callback) = state
                .lock()
                .ok()
                .and_then(|state| state.after_factory_reset_fn.clone())
            {
                callback(state);
            }
            ApiResponse::json_ok(json)
        }
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

pub fn handle_get_history(
    state: &SharedState,
    params: &std::collections::HashMap<String, String>,
) -> ApiResponse {
    let query = crate::activity::LightActivityQuery {
        limit: params
            .get("limit")
            .and_then(|value| value.parse::<usize>().ok()),
        area: params.get("area").cloned(),
        source: params.get("source").cloned(),
        action: params.get("action").cloned(),
    };
    match crate::activity::build_light_activity_history(state, query) {
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

fn appliance_delete_unpair_request(
    state: &SharedState,
    id: &str,
) -> anyhow::Result<Option<crate::pairing::UnpairingRequest>> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.platform_type != "appliance" {
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
        params: serde_json::json!({ "device_id": device_id, "force": true }),
    }))
}

pub fn handle_delete_device(state: &SharedState, id: &str) -> ApiResponse {
    match appliance_delete_unpair_request(state, id) {
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

fn parse_profile_overrides_patch_value(
    body: &serde_json::Map<String, Value>,
    field_name: &str,
) -> Result<ProfileOverridesPatch, String> {
    match body.get("profile_overrides") {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(Some(None)),
        Some(v) => {
            let object = v.as_object().ok_or_else(|| {
                format!("{field_name}.profile_overrides must be an object or null")
            })?;
            let mut overrides = BTreeMap::new();
            for (profile_id, value) in object {
                if profile_id.trim().is_empty() {
                    return Err(format!(
                        "{field_name}.profile_overrides keys must be non-empty profile IDs"
                    ));
                }
                if value.is_null() {
                    overrides.insert(profile_id.clone(), None);
                    continue;
                }
                let profile_override: LightProfileNodeOverride =
                    serde_json::from_value(value.clone()).map_err(|e| {
                        format!(
                            "Invalid {field_name}.profile_overrides.{}: {}",
                            profile_id, e
                        )
                    })?;
                overrides.insert(profile_id.clone(), Some(profile_override));
            }
            Ok(Some(Some(overrides)))
        }
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

    let mood_enabled = match body.get("mood_enabled") {
        None => None,
        Some(v) if v.is_null() => Some(None),
        Some(v) => Some(Some(v.as_bool().ok_or_else(|| {
            format!("{field_name}.mood_enabled must be a boolean or null")
        })?)),
    };

    let mood_profile_value = body
        .get("mood_profile_id")
        .or_else(|| body.get("idle_profile_id"));
    let mood_profile_id = match mood_profile_value {
        None => None,
        Some(v) if v.is_null() => Some(None),
        Some(v) => Some(Some(
            v.as_str()
                .ok_or_else(|| format!("{field_name}.mood_profile_id must be a string or null"))?
                .to_string(),
        )),
    };

    let mood_scene_value = body
        .get("mood_scene_id")
        .or_else(|| body.get("active_light_scene_id"));
    let mood_scene_id = match mood_scene_value {
        None => None,
        Some(v) if v.is_null() => Some(None),
        Some(v) => Some(Some(
            v.as_str()
                .ok_or_else(|| format!("{field_name}.mood_scene_id must be a string or null"))?
                .to_string(),
        )),
    };

    Ok(Some(commands::RoomProfileSettingsPatch {
        clear_all: false,
        profile_id,
        mood_enabled,
        mood_profile_id,
        mood_scene_id,
        fade_ms: parse_timer_patch_value(body, "fade_ms")?,
        motion_timeout_secs: parse_timer_patch_value(body, "motion_timeout_secs")?,
        profile_overrides: parse_profile_overrides_patch_value(body, field_name)?,
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
    handle_put_config_with_options(state, profile_id, body, false)
}

pub fn handle_put_config_with_options(
    state: &SharedState,
    profile_id: Option<&str>,
    body: &Value,
    apply_outputs: bool,
) -> ApiResponse {
    let mut config: rhythm_core::LightProfileConfig = match serde_json::from_value(body.clone()) {
        Ok(c) => c,
        Err(e) => return ApiResponse::bad_request(&format!("Invalid config: {}", e)),
    };
    if let Some(id) = profile_id {
        config.id = id.to_string();
    }
    match commands::do_config_set_with_options(state, config, apply_outputs) {
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
        Some(v) if v.is_finite() && (-90.0..=90.0).contains(&v) => v as f32,
        Some(_) => return ApiResponse::bad_request("lat must be between -90 and 90"),
        None => return ApiResponse::bad_request("Missing lat"),
    };
    let lon = match body.get("lon").and_then(|v| v.as_f64()) {
        Some(v) if v.is_finite() && (-180.0..=180.0).contains(&v) => v as f32,
        Some(_) => return ApiResponse::bad_request("lon must be between -180 and 180"),
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

pub fn handle_get_light_breaker(state: &SharedState) -> ApiResponse {
    match commands::build_light_breaker(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_light_runtime(state: &SharedState) -> ApiResponse {
    match commands::build_light_runtime(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_light_runtime(state: &SharedState, body: &Value) -> ApiResponse {
    let runtime_id = match body.get("runtime_id") {
        Some(Value::String(value)) => value,
        Some(_) => return ApiResponse::bad_request("runtime_id must be a string"),
        None => return ApiResponse::bad_request("Missing runtime_id"),
    };

    let runtime_kind = match crate::light_runtime::parse_light_runtime_id(state, runtime_id) {
        Ok(kind) => kind,
        Err(e) => return ApiResponse::bad_request(&e.to_string()),
    };

    match commands::do_light_runtime_set(state, runtime_kind) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_light_runtime_manifests(state: &SharedState) -> ApiResponse {
    let manifests = match crate::light_runtime::light_runtime_manifests(state) {
        Ok(manifests) => manifests,
        Err(e) => return ApiResponse::server_error(e),
    };
    match serde_json::to_string(&json!({
        "runtimes": manifests,
    })) {
        Ok(body) => ApiResponse::json_ok(body),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_light_runtime_manifest(state: &SharedState, runtime_id: &str) -> ApiResponse {
    let manifest = match crate::light_runtime::light_runtime_manifest(state, runtime_id) {
        Ok(manifest) => manifest,
        Err(e) => return ApiResponse::bad_request(&e.to_string()),
    };
    match serde_json::to_string(&manifest) {
        Ok(body) => ApiResponse::json_ok(body),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_light_runtime_extension(
    state: &SharedState,
    runtime_id: &str,
    method: RuntimeHttpMethod,
    path: &str,
    query: BTreeMap<String, String>,
    body: Value,
) -> ApiResponse {
    let request = RuntimeExtensionRequest {
        method,
        path: format!("/{}", path.trim_start_matches('/')),
        query,
        body,
    };

    match crate::light_runtime::run_light_runtime_extension(state, runtime_id, request) {
        Ok(response) => runtime_extension_api_response(response),
        Err(e) => light_runtime_error_response(e),
    }
}

pub fn handle_put_light_breaker(state: &SharedState, body: &Value) -> ApiResponse {
    let enabled = if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
        enabled
    } else if let Some(enabled) = body.as_bool() {
        enabled
    } else {
        return ApiResponse::bad_request("Missing enabled");
    };

    match commands::do_light_breaker_set(state, enabled) {
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
    if body.get("light_breaker_enabled").is_some() || body.get("light_breaker").is_some() {
        return ApiResponse::bad_request("Light breaker moved to /api/light-breaker");
    }
    if body.get("power_save").is_some() {
        return ApiResponse::bad_request("power_save has been removed; off is hard_off only");
    }
    let auto_update = body.get("auto_update").and_then(|v| v.as_bool());
    let update_channel = match body.get("update_channel") {
        Some(Value::String(value)) => match crate::state::UpdateChannel::parse(value) {
            Some(channel) => Some(channel),
            None => {
                return ApiResponse::bad_request("update_channel must be \"beta\" or \"stable\"")
            }
        },
        Some(_) => {
            return ApiResponse::bad_request("update_channel must be \"beta\" or \"stable\"")
        }
        None => None,
    };
    let light_runtime = match body.get("light_runtime") {
        Some(Value::String(value)) => {
            match crate::light_runtime::parse_light_runtime_id(state, value) {
                Ok(kind) => Some(kind),
                Err(e) => return ApiResponse::bad_request(&e.to_string()),
            }
        }
        Some(_) => return ApiResponse::bad_request("light_runtime must be a string"),
        None => None,
    };

    if auto_update.is_some() || update_channel.is_some() {
        if let Err(e) =
            commands::do_settings_set(state, None, None, None, None, auto_update, update_channel)
        {
            return ApiResponse::server_error(e);
        }
    }

    match light_runtime {
        Some(kind) => match commands::do_light_runtime_settings_set(state, kind) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(e) => ApiResponse::server_error(e),
        },
        None => match commands::build_settings(state) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(e) => ApiResponse::server_error(e),
        },
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
        return ApiResponse::bad_request("power_save has been removed; off is hard_off only");
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
    let previous_mode = state.lock().ok().map(|s| s.active_mode);
    match commands::do_mode_set(state, active_mode, modes) {
        Ok(json) => {
            if let Some(next_mode) = active_mode {
                let mut record = crate::activity::LightActivityRecord::app("global", "set_mode");
                record.change = Some(crate::activity::LightValueChange {
                    axis: Some("mode".to_string()),
                    before: previous_mode.map(|mode| json!(mode)),
                    after: Some(json!(next_mode)),
                });
                record.payload = Some(json!({
                    "previous_mode": previous_mode,
                    "next_mode": next_mode,
                }));
                crate::activity::record_light_activity(state, record);
            }
            ApiResponse::json_ok(json)
        }
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
        return ApiResponse::bad_request("power_save has been removed; off is hard_off only");
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
        Ok(json) => {
            let mut record =
                crate::activity::LightActivityRecord::app("global", "trigger_transition");
            record.payload = Some(json!({"transition_id": transition_id}));
            crate::activity::record_light_activity(state, record);
            ApiResponse::json_ok(json)
        }
        Err(e) if e.to_string().contains("Unknown transition") => {
            ApiResponse::bad_request(&e.to_string())
        }
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_input_bindings(state: &SharedState) -> ApiResponse {
    match commands::build_input_bindings(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

fn parse_optional_button_action(body: &Value) -> Result<Option<ButtonAction>, String> {
    let Some(value) = body.get("button_action") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(action) = value.as_str() {
        if action == "on" {
            return Ok(Some(ButtonAction::OnPress));
        }
        if action == "off" {
            return Ok(Some(ButtonAction::OffPress));
        }
        if action == "toggle" {
            return Ok(Some(ButtonAction::Toggle));
        }
        if let Some(action) = ButtonAction::from_service_name(action) {
            return Ok(Some(action));
        }
    }
    serde_json::from_value::<ButtonAction>(value.clone())
        .map(Some)
        .map_err(|_| "Invalid button_action".to_string())
}

fn parse_input_binding_preset(body: &Value) -> Result<Option<InputBindingPreset>, String> {
    let Some(value) = body.get("preset") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value::<InputBindingPreset>(value.clone())
        .map(Some)
        .map_err(|_| "Invalid input binding preset".to_string())
}

fn input_binding_source_node_id(body: &Value) -> Result<&str, ApiResponse> {
    match body.get("source_node_id").and_then(|value| value.as_str()) {
        Some(source_node_id) if !source_node_id.is_empty() => Ok(source_node_id),
        _ => Err(ApiResponse::bad_request("Missing source_node_id")),
    }
}

fn input_binding_enabled(body: &Value) -> bool {
    body.get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}

fn input_binding_button_action(body: &Value) -> Result<Option<ButtonAction>, ApiResponse> {
    parse_optional_button_action(body).map_err(|e| ApiResponse::bad_request(&e))
}

pub fn handle_post_input_binding(state: &SharedState, body: &Value) -> ApiResponse {
    if body.get("action").is_some() || body.get("trigger").is_some() {
        let Some(binding_id) = body.get("id").and_then(|value| value.as_str()) else {
            return ApiResponse::bad_request("Generic input bindings require id");
        };
        if binding_id.is_empty() {
            return ApiResponse::bad_request("Generic input binding id cannot be empty");
        }
        let binding = match serde_json::from_value::<InputBinding>(body.clone()) {
            Ok(binding) => binding,
            Err(e) => return ApiResponse::bad_request(&format!("Invalid input binding: {}", e)),
        };
        return match commands::do_input_binding_set(state, binding) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(e) => ApiResponse::bad_request(&e.to_string()),
        };
    }

    let preset = match parse_input_binding_preset(body) {
        Ok(Some(preset)) => preset,
        Ok(None) => return ApiResponse::bad_request("Missing input binding preset"),
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let source_node_id = match input_binding_source_node_id(body) {
        Ok(source_node_id) => source_node_id,
        Err(response) => return response,
    };
    let button_action = match input_binding_button_action(body) {
        Ok(action) => action,
        Err(response) => return response,
    };

    match commands::do_preset_input_binding_create(
        state,
        preset,
        source_node_id,
        button_action,
        input_binding_enabled(body),
    ) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_put_input_binding(
    state: &SharedState,
    binding_id: &str,
    body: &Value,
) -> ApiResponse {
    if body.get("action").is_some() || body.get("trigger").is_some() {
        let mut binding_body = body.clone();
        let Some(object) = binding_body.as_object_mut() else {
            return ApiResponse::bad_request("Invalid input binding: expected object");
        };
        object.insert("id".to_string(), Value::String(binding_id.to_string()));
        let binding = match serde_json::from_value::<InputBinding>(binding_body) {
            Ok(binding) => binding,
            Err(e) => return ApiResponse::bad_request(&format!("Invalid input binding: {}", e)),
        };
        return match commands::do_input_binding_set(state, binding) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(e) => ApiResponse::bad_request(&e.to_string()),
        };
    }

    let preset = match parse_input_binding_preset(body) {
        Ok(Some(preset)) => preset,
        Ok(None) => {
            return ApiResponse::bad_request(
                "Preset input bindings require a preset field; generic bindings require trigger and action fields",
            )
        }
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let source_node_id = match input_binding_source_node_id(body) {
        Ok(source_node_id) => source_node_id,
        Err(response) => return response,
    };
    let button_action = match input_binding_button_action(body) {
        Ok(action) => action,
        Err(response) => return response,
    };

    match commands::do_preset_input_binding_set(
        state,
        binding_id,
        preset,
        source_node_id,
        button_action,
        input_binding_enabled(body),
    ) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_delete_input_binding(state: &SharedState, binding_id: &str) -> ApiResponse {
    match commands::do_input_binding_delete(state, binding_id) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_profiles(state: &SharedState) -> ApiResponse {
    match commands::build_profiles(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_get_scenes(state: &SharedState) -> ApiResponse {
    match commands::build_scenes(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_post_scene(state: &SharedState, body: &Value) -> ApiResponse {
    let scene: crate::scenes::SceneDefinition = match serde_json::from_value(body.clone()) {
        Ok(scene) => scene,
        Err(e) => return ApiResponse::bad_request(&format!("Invalid scene: {}", e)),
    };
    match commands::do_scene_upsert(state, scene) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_put_scene(state: &SharedState, scene_id: &str, body: &Value) -> ApiResponse {
    let mut scene: crate::scenes::SceneDefinition = match serde_json::from_value(body.clone()) {
        Ok(scene) => scene,
        Err(e) => return ApiResponse::bad_request(&format!("Invalid scene: {}", e)),
    };
    scene.id = scene_id.to_string();
    match commands::do_scene_upsert(state, scene) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_delete_scene(state: &SharedState, scene_id: &str) -> ApiResponse {
    match commands::do_scene_delete(state, scene_id) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_post_scene_apply(state: &SharedState, scene_id: &str, body: &Value) -> ApiResponse {
    let request: crate::scenes::SceneApplyRequest = match serde_json::from_value(body.clone()) {
        Ok(request) => request,
        Err(e) => return ApiResponse::bad_request(&format!("Invalid scene apply request: {}", e)),
    };
    let target_id = request.target_id.clone();
    match commands::do_scene_apply(state, scene_id, request) {
        Ok(json) => {
            let mut record = crate::activity::LightActivityRecord::app(&target_id, "apply_scene");
            record.payload = Some(json!({"scene_id": scene_id}));
            crate::activity::record_light_activity(state, record);
            ApiResponse::json_ok(json)
        }
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_post_scene_preview(state: &SharedState, scene_id: &str, body: &Value) -> ApiResponse {
    let request: crate::scenes::ScenePreviewRequest = match serde_json::from_value(body.clone()) {
        Ok(request) => request,
        Err(e) => {
            return ApiResponse::bad_request(&format!("Invalid scene preview request: {}", e));
        }
    };
    let duration_ms = request.duration_ms;
    match commands::do_scene_preview(state, scene_id, request) {
        Ok(json) => {
            if let Ok(response) = serde_json::from_str::<crate::scenes::SceneApplyResponse>(&json) {
                let mut record =
                    crate::activity::LightActivityRecord::app(&response.target_id, "preview_scene");
                record.payload = Some(json!({
                    "scene_id": response.scene_id,
                    "preview_id": response.preview_id,
                    "affected_node_ids": response.affected_node_ids,
                    "duration_ms": duration_ms,
                }));
                crate::activity::record_light_activity(state, record);
            }
            ApiResponse::json_ok(json)
        }
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_post_scene_draft_preview(state: &SharedState, body: &Value) -> ApiResponse {
    let request: crate::scenes::SceneDraftPreviewRequest =
        match serde_json::from_value(body.clone()) {
            Ok(request) => request,
            Err(e) => {
                return ApiResponse::bad_request(&format!(
                    "Invalid scene draft preview request: {}",
                    e
                ));
            }
        };
    let duration_ms = request.duration_ms;
    match commands::do_scene_draft_preview(state, request) {
        Ok(json) => {
            if let Ok(response) = serde_json::from_str::<crate::scenes::SceneApplyResponse>(&json) {
                let mut record = crate::activity::LightActivityRecord::app(
                    &response.target_id,
                    "preview_draft_scene",
                );
                record.payload = Some(json!({
                    "scene_id": response.scene_id,
                    "preview_id": response.preview_id,
                    "affected_node_ids": response.affected_node_ids,
                    "duration_ms": duration_ms,
                }));
                crate::activity::record_light_activity(state, record);
            }
            ApiResponse::json_ok(json)
        }
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_post_scene_preview_commit(state: &SharedState, preview_id: &str) -> ApiResponse {
    match commands::do_scene_preview_commit(state, preview_id) {
        Ok(json) => {
            if let Ok(response) = serde_json::from_str::<crate::scenes::SceneApplyResponse>(&json) {
                let mut record = crate::activity::LightActivityRecord::app(
                    &response.target_id,
                    "commit_scene_preview",
                );
                record.payload = Some(json!({
                    "scene_id": response.scene_id,
                    "preview_id": preview_id,
                    "affected_node_ids": response.affected_node_ids,
                }));
                crate::activity::record_light_activity(state, record);
            }
            ApiResponse::json_ok(json)
        }
        Err(e) => ApiResponse::bad_request(&e.to_string()),
    }
}

pub fn handle_post_scene_preview_cancel(state: &SharedState, preview_id: &str) -> ApiResponse {
    let preview = state
        .lock()
        .ok()
        .and_then(|s| s.light_scene_previews.get(preview_id).cloned());
    match commands::do_scene_preview_cancel(state, preview_id) {
        Ok(json) => {
            if let Some(preview) = preview {
                let mut record = crate::activity::LightActivityRecord::app(
                    &preview.target_node_id,
                    "cancel_scene_preview",
                );
                record.payload = Some(json!({
                    "scene_id": preview.scene_id,
                    "preview_id": preview_id,
                    "affected_node_ids": preview.affected_node_ids,
                }));
                crate::activity::record_light_activity(state, record);
            }
            ApiResponse::json_ok(json)
        }
        Err(e) => ApiResponse::bad_request(&e.to_string()),
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
            let hub_key = crate::canonical::identity::HubKey::new(
                crate::hub::HubType::new(hub_type),
                address,
            );
            let hub_connected = state
                .lock()
                .map(|s| s.hub_is_connected(&hub_key))
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

pub fn handle_post_hub_retry(state: &SharedState, body: &Value) -> ApiResponse {
    let hub_type = match body.get("hub_type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return ApiResponse::bad_request("Missing hub_type"),
    };
    let address = match body.get("address").and_then(|v| v.as_str()) {
        Some(a) => a,
        None => return ApiResponse::bad_request("Missing address"),
    };

    match commands::do_retry_hub_connect(state, hub_type, address) {
        Ok(()) => ApiResponse::no_content(),
        Err(e)
            if e.to_string().contains("Unknown hub type")
                || e.to_string().contains("No stored credentials")
                || e.to_string().contains("not connectable")
                || e.to_string().contains("not supported") =>
        {
            ApiResponse::bad_request(&e.to_string())
        }
        Err(e) => ApiResponse::server_error(e),
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

/// Dispatch node action(s). Batch actions are queued and paced on the
/// background dispatch worker so HTTP cannot fan out directly to hubs.
pub fn handle_node_action(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let items = match mutation_items(body) {
        Ok(items) => items,
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let dispatch_spacing = match dispatch_spacing_from_body(body) {
        Ok(spacing) => spacing,
        Err(e) => return ApiResponse::bad_request(&e),
    };

    let batch = items.len() > 1;
    let mut actions = Vec::with_capacity(items.len());

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
        actions.push((node_id, action.to_string()));
    }

    if batch {
        let mut results = Vec::with_capacity(actions.len());
        for (node_id, _) in &actions {
            match commands::build_node_state(state, node_id) {
                Ok(node) => results.push(node),
                Err(e) => return ApiResponse::server_error(e),
            }
        }
        if let Err(e) =
            commands::queue_node_action_batch(state, actions.clone(), persist, dispatch_spacing)
        {
            return ApiResponse::server_error(e);
        }
        for (node_id, action) in &actions {
            let mut record = crate::activity::LightActivityRecord::app(
                node_id,
                crate::activity::http_action_id(action),
            );
            record.payload = Some(json!({"request_action": action}));
            crate::activity::record_light_activity(state, record);
        }
        return nodes_response(results, batch, dispatch_spacing);
    }

    let mut results = Vec::new();
    for (node_id, action) in &actions {
        if let Err(e) = commands::do_node_action(state, node_id, action, persist) {
            return ApiResponse::server_error(e);
        }
        match commands::build_node_state(state, node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
        let mut record = crate::activity::LightActivityRecord::app(
            node_id,
            crate::activity::http_action_id(action),
        );
        record.payload = Some(json!({"request_action": action}));
        crate::activity::record_light_activity(state, record);
    }

    nodes_response(results, batch, dispatch_spacing)
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
            // Clamp, don't truncate: `256 as u8` wraps to 0 = lights off.
            Some(b) => b.min(100) as u8,
            None => return ApiResponse::bad_request("Missing brightness"),
        };

        let per_item_persist = persist && !batch;
        match commands::do_set_node_brightness(state, &room_id, brightness, per_item_persist) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
        let mut record = crate::activity::LightActivityRecord::app(&room_id, "set_brightness");
        record.brightness = Some(brightness.clamp(1, 100));
        record.change = Some(crate::activity::LightValueChange {
            axis: Some("brightness".to_string()),
            before: None,
            after: Some(json!(brightness.clamp(1, 100))),
        });
        record.payload = Some(json!({"brightness": brightness.clamp(1, 100)}));
        crate::activity::record_light_activity(state, record);
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Set node brightness. Batch updates are queued and paced on the background
/// dispatch worker so HTTP cannot fan out directly to hubs.
pub fn handle_set_node_brightness(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let items = match mutation_items(body) {
        Ok(items) => items,
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let dispatch_spacing = match dispatch_spacing_from_body(body) {
        Ok(spacing) => spacing,
        Err(e) => return ApiResponse::bad_request(&e),
    };

    let batch = items.len() > 1;
    let mut updates = Vec::with_capacity(items.len());

    for item in &items {
        let raw_node_id = match item.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing node_id"),
        };
        let node_id = commands::resolve_node_id(state, raw_node_id);
        let brightness = match item.get("brightness").and_then(|v| v.as_u64()) {
            // Clamp, don't truncate: `256 as u8` wraps to 0 = lights off.
            Some(b) => b.min(100) as u8,
            None => return ApiResponse::bad_request("Missing brightness"),
        };
        updates.push((node_id, brightness));
    }

    if batch {
        let mut results = Vec::with_capacity(updates.len());
        for (node_id, _) in &updates {
            match commands::build_node_state(state, node_id) {
                Ok(node) => results.push(node),
                Err(e) => return ApiResponse::server_error(e),
            }
        }
        if let Err(e) = commands::queue_set_node_brightness_batch(
            state,
            updates.clone(),
            persist,
            dispatch_spacing,
        ) {
            return ApiResponse::server_error(e);
        }
        for (node_id, brightness) in &updates {
            let mut record = crate::activity::LightActivityRecord::app(node_id, "set_brightness");
            record.brightness = Some((*brightness).clamp(1, 100));
            record.change = Some(crate::activity::LightValueChange {
                axis: Some("brightness".to_string()),
                before: None,
                after: Some(json!((*brightness).clamp(1, 100))),
            });
            record.payload = Some(json!({"brightness": (*brightness).clamp(1, 100)}));
            crate::activity::record_light_activity(state, record);
        }
        return nodes_response(results, batch, dispatch_spacing);
    }

    let mut results = Vec::new();
    for (node_id, brightness) in &updates {
        if let Err(e) = commands::do_set_node_brightness(state, node_id, *brightness, persist) {
            return ApiResponse::server_error(e);
        }
        match commands::build_node_state(state, node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
        let mut record = crate::activity::LightActivityRecord::app(node_id, "set_brightness");
        record.brightness = Some((*brightness).clamp(1, 100));
        record.change = Some(crate::activity::LightValueChange {
            axis: Some("brightness".to_string()),
            before: None,
            after: Some(json!((*brightness).clamp(1, 100))),
        });
        record.payload = Some(json!({"brightness": (*brightness).clamp(1, 100)}));
        crate::activity::record_light_activity(state, record);
    }

    nodes_response(results, batch, dispatch_spacing)
}

/// Set live curve modifiers for light-addressable nodes.
///
/// Accepts `brightness` or `color_temperature` (Kelvin). Color-temperature
/// targets move the node along the active curve, not to a one-shot hardware
/// color state.
pub fn handle_set_node_curve(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let items = match mutation_items(body) {
        Ok(items) => items,
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let dispatch_spacing = match dispatch_spacing_from_body(body) {
        Ok(spacing) => spacing,
        Err(e) => return ApiResponse::bad_request(&e),
    };

    let mut updates = Vec::with_capacity(items.len());

    for item in &items {
        let raw_node_id = match item.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing node_id"),
        };
        let node_id = commands::resolve_node_id(state, raw_node_id);
        let modifier = match parse_node_curve_modifier(item) {
            Ok(modifier) => modifier,
            Err(e) => return ApiResponse::bad_request(&e),
        };
        updates.push((node_id, modifier));
    }

    let mut results = Vec::with_capacity(updates.len());
    for (node_id, _) in &updates {
        match commands::build_node_state(state, node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
    }
    if let Err(e) = commands::queue_set_node_curve_modifier_batch(
        state,
        updates.clone(),
        persist,
        dispatch_spacing,
    ) {
        return ApiResponse::server_error(e);
    }
    for (node_id, modifier) in &updates {
        let (axis, value) = match *modifier {
            commands::NodeCurveModifier::Brightness(brightness) => {
                ("brightness", json!(brightness.clamp(1, 100)))
            }
            commands::NodeCurveModifier::ColorTemperature { kelvin, .. } => {
                ("color_temperature", json!(kelvin.clamp(500, 25_000)))
            }
        };
        let mut record = crate::activity::LightActivityRecord::app(node_id, "set_curve");
        if axis == "brightness" {
            record.brightness = value.as_u64().map(|value| value as u8);
        } else {
            record.kelvin = value.as_u64().map(|value| value as u16);
        }
        record.change = Some(crate::activity::LightValueChange {
            axis: Some(axis.to_string()),
            before: None,
            after: Some(value.clone()),
        });
        record.payload = Some(json!({"axis": axis, "value": value}));
        crate::activity::record_light_activity(state, record);
    }

    nodes_response(results, true, dispatch_spacing)
}

/// Set node color. `scope=mood` updates this node's Mood profile and enters Mood.
pub fn handle_set_node_color(state: &SharedState, body: &Value, persist: bool) -> ApiResponse {
    let items = match mutation_items(body) {
        Ok(items) => items,
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let dispatch_spacing = match dispatch_spacing_from_body(body) {
        Ok(spacing) => spacing,
        Err(e) => return ApiResponse::bad_request(&e),
    };

    let mut results = Vec::with_capacity(items.len());

    for item in &items {
        let raw_node_id = match item.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing node_id"),
        };
        let node_id = commands::resolve_node_id(state, raw_node_id);
        let parsed_update: Result<commands::NodeColorUpdate, String> = (|| {
            Ok(commands::NodeColorUpdate {
                rgb: parse_rgb(item.get("rgb"))?,
                xy: parse_xy(item.get("xy"))?,
                brightness: parse_optional_u8(item.get("brightness"), "brightness")?,
                transition_ms: parse_optional_u32(item.get("transition_ms"), "transition_ms")?,
                scope: parse_color_scope(item.get("scope"))?,
            })
        })();
        let update = match parsed_update {
            Ok(update) => update,
            Err(e) => return ApiResponse::bad_request(&e),
        };

        if let Err(e) = commands::do_set_node_color(state, &node_id, update, persist) {
            return ApiResponse::server_error(e);
        }
        match commands::build_node_state(state, &node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
        let mut record = crate::activity::LightActivityRecord::app(
            &node_id,
            match update.scope {
                commands::NodeColorScope::Mood => "set_mood_color",
                commands::NodeColorScope::Preview => "preview_color",
                commands::NodeColorScope::Auto => "set_color",
            },
        );
        record.brightness = update.brightness.map(|brightness| brightness.clamp(1, 100));
        record.payload = Some(json!({
            "rgb": {"r": update.rgb.r, "g": update.rgb.g, "b": update.rgb.b},
            "brightness": update.brightness.map(|brightness| brightness.clamp(1, 100)),
            "transition_ms": update.transition_ms,
            "scope": match update.scope {
                commands::NodeColorScope::Mood => "mood",
                commands::NodeColorScope::Preview => "preview",
                commands::NodeColorScope::Auto => "auto",
            },
        }));
        crate::activity::record_light_activity(state, record);
    }

    nodes_response(results, false, dispatch_spacing)
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
            Some(t) if t.is_finite() && t.abs() <= 1440.0 => t as f32,
            Some(_) => {
                return ApiResponse::bad_request("time_offset must be within \u{b1}1440 minutes")
            }
            None => return ApiResponse::bad_request("Missing time_offset"),
        };

        let per_item_persist = persist && !batch;
        match commands::do_set_node_time_offset(state, &room_id, time_offset, per_item_persist) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
        let mut record = crate::activity::LightActivityRecord::app(&room_id, "set_curve_position");
        record.change = Some(crate::activity::LightValueChange {
            axis: Some("time_offset_minutes".to_string()),
            before: None,
            after: Some(json!(time_offset)),
        });
        record.payload = Some(json!({"time_offset_minutes": time_offset}));
        crate::activity::record_light_activity(state, record);
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Set node time offset. Batch previews collapse to periodic-style live
/// dispatch targets instead of queueing every submitted child device.
pub fn handle_set_node_time_offset(
    state: &SharedState,
    body: &Value,
    persist: bool,
) -> ApiResponse {
    if !body.is_object() {
        return ApiResponse::bad_request("Expected object body");
    }
    let dispatch_spacing = match dispatch_spacing_from_body(body) {
        Ok(spacing) => spacing,
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let time_offset = match body.get("time_offset").and_then(|v| v.as_f64()) {
        Some(t) => t as f32,
        None => return ApiResponse::bad_request("Missing time_offset"),
    };

    let updates = match body.get("nodes") {
        Some(Value::Array(nodes)) if !nodes.is_empty() => {
            let mut seen = std::collections::HashSet::new();
            let mut updates = Vec::with_capacity(nodes.len());
            for node in nodes {
                let raw_node_id = match node.as_str() {
                    Some(id) if !id.is_empty() => id,
                    _ => return ApiResponse::bad_request("nodes must be an array of node ids"),
                };
                let node_id = commands::resolve_node_id(state, raw_node_id);
                if seen.insert(node_id.clone()) {
                    updates.push((node_id, time_offset));
                }
            }
            updates
        }
        Some(Value::Array(_)) | None => {
            match commands::default_node_time_offset_updates(state, time_offset) {
                Ok(updates) => updates,
                Err(e) => return node_time_offset_error_response(e),
            }
        }
        Some(_) => return ApiResponse::bad_request("nodes must be an array of node ids"),
    };

    let mut results = Vec::new();
    let dispatch_count = match commands::do_set_node_time_offsets_batch_with_spacing(
        state,
        &updates,
        persist,
        dispatch_spacing,
    ) {
        Ok(dispatch_count) => dispatch_count,
        Err(e) => return node_time_offset_error_response(e),
    };
    for (node_id, _) in &updates {
        match commands::build_node_state(state, node_id) {
            Ok(node) => results.push(node),
            Err(e) if e.to_string().contains("not found") => {
                return node_time_offset_error_response(e);
            }
            Err(e) => return ApiResponse::server_error(e),
        }
    }
    for (node_id, time_offset) in &updates {
        let mut record = crate::activity::LightActivityRecord::app(node_id, "set_curve_position");
        record.change = Some(crate::activity::LightValueChange {
            axis: Some("time_offset_minutes".to_string()),
            before: None,
            after: Some(json!(time_offset)),
        });
        record.payload = Some(json!({"time_offset_minutes": time_offset}));
        crate::activity::record_light_activity(state, record);
    }

    nodes_response_with_dispatch_count(results, Some(dispatch_count), dispatch_spacing)
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
        let standby_enabled = item.get("standby_enabled").and_then(|v| v.as_bool());
        let room_state = match item.get("state").cloned() {
            Some(value) => match serde_json::from_value::<rhythm_core::RoomModeState>(value) {
                Ok(state) => Some(state),
                Err(_) => return ApiResponse::bad_request("Invalid room state"),
            },
            None => None,
        };
        let room_profile = match parse_profile_settings_patch(
            item.get("profile_settings")
                .or_else(|| item.get("room_profile")),
            "profile_settings",
        ) {
            Ok(patch) => patch,
            Err(e) => return ApiResponse::bad_request(&e),
        };

        let per_item_persist = persist && !batch;
        match commands::do_node_preferences_set(
            state,
            &room_id,
            rhythm_enabled,
            disabled,
            standby_enabled,
            room_state,
            room_profile.as_ref(),
            per_item_persist,
        ) {
            Ok(json) => results.push(json),
            Err(e) => return ApiResponse::server_error(e),
        }
        if rhythm_enabled.is_some()
            || standby_enabled.is_some()
            || room_state.is_some()
            || room_profile.is_some()
        {
            let action_id = room_state
                .map(|_| "set_room_state".to_string())
                .or_else(|| {
                    rhythm_enabled.map(|enabled| {
                        if enabled {
                            "circadian_on".to_string()
                        } else {
                            "circadian_off".to_string()
                        }
                    })
                })
                .unwrap_or_else(|| "set_light_preferences".to_string());
            let mut record = crate::activity::LightActivityRecord::app(&room_id, action_id);
            record.payload = Some(json!({
                "rhythm_enabled": rhythm_enabled,
                "standby_enabled": standby_enabled,
                "state": room_state.map(|state| state.as_api_str()),
                "profile_settings_touched": room_profile.is_some(),
            }));
            crate::activity::record_light_activity(state, record);
        }
    }

    if batch && persist {
        commands::persist_rooms(state);
    }

    ApiResponse::json_ok(format!(r#"{{"rooms":[{}]}}"#, results.join(",")))
}

/// Update node preferences. All preference changes are queued and paced on
/// the background dispatch worker so HTTP requests return without waiting
/// for hub commands to complete.
pub fn handle_put_node_preferences(
    state: &SharedState,
    body: &Value,
    persist: bool,
) -> ApiResponse {
    let items = match mutation_items(body) {
        Ok(items) => items,
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let dispatch_spacing = match dispatch_spacing_from_body(body) {
        Ok(spacing) => spacing,
        Err(e) => return ApiResponse::bad_request(&e),
    };

    let mut updates = Vec::with_capacity(items.len());

    for item in &items {
        let raw_node_id = match item.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing node_id"),
        };
        let node_id = commands::resolve_node_id(state, raw_node_id);
        let rhythm_enabled = item.get("rhythm_enabled").and_then(|v| v.as_bool());
        let disabled = item.get("disabled").and_then(|v| v.as_bool());
        let standby_enabled = item.get("standby_enabled").and_then(|v| v.as_bool());
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
        updates.push(commands::QueuedNodePreferencesPatch {
            node_id,
            rhythm_enabled,
            disabled,
            standby_enabled,
            target_state: room_state,
            room_profile,
        });
    }

    let node_ids: Vec<String> = updates
        .iter()
        .map(|update| update.node_id.clone())
        .collect();
    let mut results = Vec::with_capacity(node_ids.len());
    for node_id in &node_ids {
        match commands::build_node_state(state, node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
    }
    let queue_dispatch_spacing = if updates.len() <= 1 {
        Duration::ZERO
    } else {
        dispatch_spacing
    };
    if let Err(e) = commands::queue_node_preferences_batch(
        state,
        updates.clone(),
        persist,
        queue_dispatch_spacing,
    ) {
        return ApiResponse::server_error(e);
    }
    for update in &updates {
        if update.rhythm_enabled.is_some()
            || update.standby_enabled.is_some()
            || update.target_state.is_some()
            || update.room_profile.is_some()
        {
            let action_id = update
                .target_state
                .map(|_| "set_room_state".to_string())
                .or_else(|| {
                    update.rhythm_enabled.map(|enabled| {
                        if enabled {
                            "circadian_on".to_string()
                        } else {
                            "circadian_off".to_string()
                        }
                    })
                })
                .unwrap_or_else(|| "set_light_preferences".to_string());
            let mut record = crate::activity::LightActivityRecord::app(&update.node_id, action_id);
            record.payload = Some(json!({
                "rhythm_enabled": update.rhythm_enabled,
                "standby_enabled": update.standby_enabled,
                "state": update.target_state.map(|state| state.as_api_str()),
                "profile_settings_touched": update.room_profile.is_some(),
            }));
            crate::activity::record_light_activity(state, record);
        }
    }
    nodes_response(results, true, queue_dispatch_spacing)
}

/// Patch per-profile overrides for one or more nodes.
///
/// Body shape:
/// `{ "node_id": "...", "profile_overrides": { "profile-id": { ... }, "other": null } }`
/// where a `null` profile entry removes that profile override, and
/// `profile_overrides: null` clears all profile overrides for the node.
pub fn handle_put_node_profile_overrides(
    state: &SharedState,
    body: &Value,
    persist: bool,
) -> ApiResponse {
    let items = match mutation_items(body) {
        Ok(items) => items,
        Err(e) => return ApiResponse::bad_request(&e),
    };
    let dispatch_spacing = match dispatch_spacing_from_body(body) {
        Ok(spacing) => spacing,
        Err(e) => return ApiResponse::bad_request(&e),
    };

    let mut updates = Vec::with_capacity(items.len());

    for item in &items {
        let body = match item.as_object() {
            Some(body) => body,
            None => {
                return ApiResponse::bad_request("node profile override item must be an object")
            }
        };
        let raw_node_id = match body.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return ApiResponse::bad_request("Missing node_id"),
        };
        let profile_overrides = match parse_profile_overrides_patch_value(body, "body") {
            Ok(Some(profile_overrides)) => profile_overrides,
            Ok(None) => return ApiResponse::bad_request("Missing profile_overrides"),
            Err(e) => return ApiResponse::bad_request(&e),
        };
        updates.push(commands::QueuedNodePreferencesPatch {
            node_id: commands::resolve_node_id(state, raw_node_id),
            rhythm_enabled: None,
            disabled: None,
            standby_enabled: None,
            target_state: None,
            room_profile: Some(commands::RoomProfileSettingsPatch {
                profile_overrides: Some(profile_overrides),
                ..Default::default()
            }),
        });
    }

    let node_ids: Vec<String> = updates
        .iter()
        .map(|update| update.node_id.clone())
        .collect();
    let mut results = Vec::with_capacity(node_ids.len());
    for node_id in &node_ids {
        match commands::build_node_state(state, node_id) {
            Ok(node) => results.push(node),
            Err(e) => return ApiResponse::server_error(e),
        }
    }
    let queue_dispatch_spacing = if updates.len() <= 1 {
        Duration::ZERO
    } else {
        dispatch_spacing
    };
    if let Err(e) = commands::queue_node_preferences_batch(
        state,
        updates.clone(),
        persist,
        queue_dispatch_spacing,
    ) {
        return ApiResponse::server_error(e);
    }
    for update in &updates {
        let mut record =
            crate::activity::LightActivityRecord::app(&update.node_id, "set_light_preferences");
        let profile_override_keys = update
            .room_profile
            .as_ref()
            .and_then(|profile| profile.profile_overrides.as_ref())
            .and_then(|profile_overrides| profile_overrides.as_ref())
            .map(|overrides| overrides.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let clear_profile_overrides = update
            .room_profile
            .as_ref()
            .and_then(|profile| profile.profile_overrides.as_ref())
            .is_some_and(|profile_overrides| profile_overrides.is_none());
        record.payload = Some(json!({
            "profile_overrides_touched": true,
            "profile_override_keys": profile_override_keys,
            "clear_profile_overrides": clear_profile_overrides,
        }));
        crate::activity::record_light_activity(state, record);
    }
    nodes_response(results, true, queue_dispatch_spacing)
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

    crate::pairing::emit_pairing_progress(
        state,
        &request.hub_type,
        request.session_id.as_deref(),
        crate::pairing::PairingStatus::Searching,
        crate::pairing::PairingStage::Requested,
        "Pairing request received",
        None,
        None,
    );

    let params_with_session_id;
    let params = if let Some(session_id) = request.session_id.as_deref() {
        if let Some(object) = request.params.as_object() {
            let mut object = object.clone();
            object
                .entry("session_id".to_string())
                .or_insert_with(|| serde_json::Value::String(session_id.to_string()));
            params_with_session_id = serde_json::Value::Object(object);
            &params_with_session_id
        } else {
            &request.params
        }
    } else {
        &request.params
    };

    match start_fn(state, &request.hub_type, params) {
        Ok(session) => {
            log::info!(target: "pair", "Pairing result: status={:?} error={:?}", session.status, session.error);
            let (stage, message) = match &session.status {
                crate::pairing::PairingStatus::Complete => {
                    (crate::pairing::PairingStage::Complete, "Pairing complete")
                }
                crate::pairing::PairingStatus::Failed => {
                    (crate::pairing::PairingStage::Failed, "Pairing failed")
                }
                _ => (
                    crate::pairing::PairingStage::Commissioning,
                    "Pairing status changed",
                ),
            };
            crate::pairing::emit_pairing_progress(
                state,
                &session.hub_type,
                request.session_id.as_deref(),
                session.status.clone(),
                stage,
                message,
                session.device.clone(),
                session.error.clone(),
            );
            if session.status == crate::pairing::PairingStatus::Complete {
                // Persist canonical registry changes made during pairing
                if let Ok(s) = state.lock() {
                    commands::persist_canonical(&s);
                }
                // Persist hub registry updates if the integration created any
                commands::persist_registry(state);
                // Notify SSE clients
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
            crate::pairing::emit_pairing_progress(
                state,
                &request.hub_type,
                request.session_id.as_deref(),
                crate::pairing::PairingStatus::Failed,
                crate::pairing::PairingStage::Failed,
                "Pairing failed",
                None,
                Some(e.to_string()),
            );
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

pub fn handle_put_canonical_device(
    state: &SharedState,
    device_id: &str,
    body: &Value,
) -> ApiResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(name) => name,
        None => return ApiResponse::bad_request("Missing name"),
    };
    match commands::do_canonical_rename_device(state, device_id, name) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) if e.to_string().contains("Device not found") => {
            ApiResponse::bad_request("Device not found")
        }
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

pub fn handle_post_device_flash(state: &SharedState, device_id: &str) -> ApiResponse {
    match commands::do_flash_canonical_device(state, device_id) {
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
        factory_default_light_profile_config, factory_default_profile_bundle,
    };
    use crate::hub::{ActiveHub, HubType};
    use crate::pairing::{
        PairedDeviceInfo, PairingRequest, PairingSession, PairingStage, PairingStatus,
        UnpairingRequest, UnpairingResult,
    };
    use crate::registry::HubDeviceRegistry;
    use crate::state::{AppState, ObservedPowerSource, ObservedPowerState, WorkItem};
    use crate::topology::HubRoomBinding;
    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_runtime_api::{
        LightRuntime, RuntimeCapabilities, RuntimeEvent, RuntimeManifest, RuntimePlan,
        RuntimeResult, RuntimeSnapshot,
    };
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

    #[test]
    fn api_response_not_found_and_time_offset_errors_classify_messages() {
        let r = ApiResponse::not_found("missing");
        assert_eq!(r.status, 404);
        assert_eq!(r.body, "missing");

        let r = node_time_offset_error_response(anyhow::anyhow!("room not found in engine"));
        assert_eq!(r.status, 404);
        assert!(r.body.contains("not found in engine"));

        let r = node_time_offset_error_response(anyhow::anyhow!(
            "Time offsets can only be set on light nodes"
        ));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("Time offsets can only be set"));

        let r = node_time_offset_error_response(anyhow::anyhow!("storage failed"));
        assert_eq!(r.status, 500);
        assert_eq!(r.body, "storage failed");
    }

    #[test]
    fn pair_device_emits_progress_and_passes_session_id_to_integration() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let captured_params = Arc::new(Mutex::new(None::<serde_json::Value>));

        {
            let captured_params = captured_params.clone();
            let mut s = state.lock().unwrap();
            s.event_tx = Some(tx);
            s.start_pairing_fn = Some(Arc::new(move |_, hub_type, params| {
                assert_eq!(hub_type, "matter");
                *captured_params.lock().unwrap() = Some(params.clone());
                Ok(PairingSession {
                    hub_type: hub_type.to_string(),
                    status: PairingStatus::Complete,
                    device: Some(PairedDeviceInfo {
                        device_id: "matter-100".to_string(),
                        name: "Test Matter Bulb".to_string(),
                        device_type: DeviceType::Light,
                        manufacturer: Some("Test".to_string()),
                        model: Some("T100".to_string()),
                    }),
                    error: None,
                })
            }));
        }

        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "matter".to_string(),
                session_id: Some("pair-123".to_string()),
                params: json!({ "setup_payload": "34970112332" }),
            },
        );

        assert_eq!(response.status, 200);
        let forwarded = captured_params.lock().unwrap().clone().unwrap();
        assert_eq!(forwarded["setup_payload"], "34970112332");
        assert_eq!(forwarded["session_id"], "pair-123");

        let first = rx.try_recv().expect("requested progress event");
        match first {
            crate::server_event::ServerEvent::PairingProgress {
                session_id,
                status,
                stage,
                ..
            } => {
                assert_eq!(session_id.as_deref(), Some("pair-123"));
                assert_eq!(status, PairingStatus::Searching);
                assert_eq!(stage, PairingStage::Requested);
            }
            other => panic!("expected pairing progress event, got {:?}", other),
        }

        let second = rx.try_recv().expect("completion progress event");
        match second {
            crate::server_event::ServerEvent::PairingProgress {
                session_id,
                status,
                stage,
                device,
                ..
            } => {
                assert_eq!(session_id.as_deref(), Some("pair-123"));
                assert_eq!(status, PairingStatus::Complete);
                assert_eq!(stage, PairingStage::Complete);
                assert_eq!(
                    device.as_ref().map(|device| device.device_id.as_str()),
                    Some("matter-100")
                );
            }
            other => panic!("expected pairing completion event, got {:?}", other),
        }
    }

    // ---- Handler validation (400 paths) ----

    fn test_state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    #[test]
    fn mutation_items_accepts_all_supported_batch_shapes() {
        assert_eq!(
            mutation_items(&json!({"node_id": "room1"})).unwrap(),
            vec![json!({"node_id": "room1"})]
        );
        assert_eq!(
            mutation_items(&json!([{"node_id": "a"}, {"node_id": "b"}])).unwrap(),
            vec![json!({"node_id": "a"}), json!({"node_id": "b"})]
        );
        assert_eq!(
            mutation_items(&json!({"items": [{"node_id": "a"}]})).unwrap(),
            vec![json!({"node_id": "a"})]
        );
        assert_eq!(
            mutation_items(&json!({"nodes": [{"node_id": "a"}]})).unwrap(),
            vec![json!({"node_id": "a"})]
        );
        assert_eq!(
            mutation_items(&json!({"rooms": [{"room_id": "a"}]})).unwrap(),
            vec![json!({"room_id": "a"})]
        );
    }

    #[test]
    fn dispatch_spacing_from_body_defaults_validates_and_caps() {
        assert_eq!(
            dispatch_spacing_from_body(&json!({})).unwrap(),
            commands::default_http_batch_dispatch_spacing()
        );
        assert_eq!(
            dispatch_spacing_from_body(&json!({"dispatch_spacing_ms": 42})).unwrap(),
            Duration::from_millis(42)
        );
        assert_eq!(
            dispatch_spacing_from_body(&json!({"dispatch_spacing_ms": "fast"})).unwrap_err(),
            "dispatch_spacing_ms must be a non-negative integer"
        );
        assert_eq!(
            dispatch_spacing_from_body(&json!({"dispatch_spacing_ms": 60_001})).unwrap_err(),
            "dispatch_spacing_ms must be <= 60000"
        );
    }

    #[test]
    fn color_parsers_accept_valid_payloads_and_report_precise_errors() {
        let rgb = parse_rgb(Some(&json!({"r": 1, "g": 2, "b": 3}))).unwrap();
        assert_eq!((rgb.r, rgb.g, rgb.b), (1, 2, 3));
        assert_eq!(parse_rgb(None).unwrap_err(), "Missing rgb");
        assert_eq!(
            parse_rgb(Some(&json!({"r": 256, "g": 2, "b": 3}))).unwrap_err(),
            "rgb.r must be <= 255"
        );
        assert_eq!(
            parse_rgb(Some(&json!({"r": 1, "g": "2", "b": 3}))).unwrap_err(),
            "rgb.g must be an integer"
        );
        assert_eq!(
            parse_rgb(Some(&json!([1, 2, 3]))).unwrap_err(),
            "rgb must be an object"
        );

        assert_eq!(parse_xy(None).unwrap(), None);
        assert_eq!(parse_xy(Some(&Value::Null)).unwrap(), None);
        assert_eq!(
            parse_xy(Some(&json!({"x": 0.1, "y": 0.2}))).unwrap(),
            Some(XyColor { x: 0.1, y: 0.2 })
        );
        assert_eq!(
            parse_xy(Some(&json!({"x": 1.2, "y": 0.2}))).unwrap_err(),
            "xy values must be between 0 and 1"
        );
        assert_eq!(
            parse_xy(Some(&json!({"x": 0.1}))).unwrap_err(),
            "xy.y must be a number"
        );
        assert_eq!(
            parse_xy(Some(&json!("xy"))).unwrap_err(),
            "xy must be an object or null"
        );

        assert_eq!(parse_optional_u8(None, "brightness").unwrap(), None);
        assert_eq!(
            parse_optional_u8(Some(&Value::Null), "brightness").unwrap(),
            None
        );
        assert_eq!(
            parse_optional_u8(Some(&json!(0)), "brightness").unwrap(),
            Some(1)
        );
        assert_eq!(
            parse_optional_u8(Some(&json!(500)), "brightness").unwrap(),
            Some(100)
        );
        assert_eq!(
            parse_optional_u8(Some(&json!("bright")), "brightness").unwrap_err(),
            "brightness must be an integer or null"
        );

        assert_eq!(parse_optional_u32(None, "transition_ms").unwrap(), None);
        assert_eq!(
            parse_optional_u32(Some(&Value::Null), "transition_ms").unwrap(),
            None
        );
        assert_eq!(
            parse_optional_u32(Some(&json!(123)), "transition_ms").unwrap(),
            Some(123)
        );
        assert!(parse_optional_u32(Some(&json!(u64::MAX)), "transition_ms")
            .unwrap_err()
            .contains("transition_ms must be <="));
    }

    #[test]
    fn node_curve_and_color_scope_parsers_cover_aliases_and_conflicts() {
        assert_eq!(
            parse_node_curve_modifier(&json!({"brightness": 250})).unwrap(),
            commands::NodeCurveModifier::Brightness(100)
        );
        assert_eq!(
            parse_node_curve_modifier(
                &json!({"color_temp_kelvin": 100, "preserve_brightness": false})
            )
            .unwrap(),
            commands::NodeCurveModifier::ColorTemperature {
                kelvin: 500,
                preserve_brightness: false,
            }
        );
        assert_eq!(
            parse_node_curve_modifier(&json!({"kelvin": 30_000})).unwrap(),
            commands::NodeCurveModifier::ColorTemperature {
                kelvin: 25_000,
                preserve_brightness: true,
            }
        );
        assert_eq!(
            parse_node_curve_modifier(&json!({"brightness": 50, "kelvin": 2700})).unwrap_err(),
            "Specify exactly one curve modifier: brightness or color_temperature"
        );
        assert_eq!(
            parse_node_curve_modifier(&json!({})).unwrap_err(),
            "Missing curve modifier: brightness or color_temperature"
        );

        assert_eq!(
            parse_color_scope(None).unwrap(),
            commands::NodeColorScope::Auto
        );
        assert_eq!(
            parse_color_scope(Some(&json!("preview"))).unwrap(),
            commands::NodeColorScope::Preview
        );
        assert_eq!(
            parse_color_scope(Some(&json!("mood"))).unwrap(),
            commands::NodeColorScope::Mood
        );
        assert_eq!(
            parse_color_scope(Some(&json!(true))).unwrap_err(),
            "scope must be a string"
        );
        assert_eq!(
            parse_color_scope(Some(&json!("scene"))).unwrap_err(),
            "Invalid color scope: scene"
        );
    }

    #[test]
    fn input_binding_parsers_cover_aliases_defaults_and_errors() {
        assert_eq!(parse_optional_button_action(&json!({})).unwrap(), None);
        assert_eq!(
            parse_optional_button_action(&json!({"button_action": null})).unwrap(),
            None
        );
        assert_eq!(
            parse_optional_button_action(&json!({"button_action": "on"})).unwrap(),
            Some(ButtonAction::OnPress)
        );
        assert_eq!(
            parse_optional_button_action(&json!({"button_action": "off"})).unwrap(),
            Some(ButtonAction::OffPress)
        );
        assert_eq!(
            parse_optional_button_action(&json!({"button_action": "toggle"})).unwrap(),
            Some(ButtonAction::Toggle)
        );
        assert!(
            parse_optional_button_action(&json!({"button_action": "bad"}))
                .unwrap_err()
                .contains("Invalid button_action")
        );

        assert_eq!(parse_input_binding_preset(&json!({})).unwrap(), None);
        assert_eq!(
            parse_input_binding_preset(&json!({"preset": null})).unwrap(),
            None
        );
        assert_eq!(
            parse_input_binding_preset(&json!({"preset": "day_sleep_toggle"})).unwrap(),
            Some(InputBindingPreset::DaySleepToggle)
        );
        assert_eq!(
            parse_input_binding_preset(&json!({"preset": "unknown"})).unwrap_err(),
            "Invalid input binding preset"
        );

        assert!(input_binding_source_node_id(&json!({"source_node_id": "button-1"})).is_ok());
        assert_eq!(
            input_binding_source_node_id(&json!({})).unwrap_err().status,
            400
        );
        assert!(input_binding_enabled(&json!({})));
        assert!(!input_binding_enabled(&json!({"enabled": false})));
        match input_binding_button_action(&json!({"button_action": "on"})) {
            Ok(action) => assert_eq!(action, Some(ButtonAction::OnPress)),
            Err(response) => panic!("unexpected error response {}", response.status),
        }
        assert_eq!(
            input_binding_button_action(&json!({"button_action": "bad"}))
                .unwrap_err()
                .status,
            400
        );
    }

    #[test]
    fn profile_settings_patch_parser_covers_clear_aliases_timers_and_errors() {
        assert!(parse_profile_settings_patch(None, "room_profile")
            .unwrap()
            .is_none());

        let clear = parse_profile_settings_patch(Some(&Value::Null), "room_profile")
            .unwrap()
            .unwrap();
        assert!(clear.clear_all);

        let patch = parse_profile_settings_patch(
            Some(&json!({
                "profile_id": null,
                "mood_enabled": false,
                "idle_profile_id": "legacy-idle",
                "active_light_scene_id": "scene-1",
                "fade_ms": {"mode": "fixed", "value": 250},
                "motion_timeout_secs": null,
                "profile_overrides": {
                    "rhythm": {
                        "motion_timeout_secs": {"mode": "fixed", "value": 300}
                    },
                    "sleep": null
                }
            })),
            "room_profile",
        )
        .unwrap()
        .unwrap();
        assert_eq!(patch.profile_id, Some(None));
        assert_eq!(patch.mood_enabled, Some(Some(false)));
        assert_eq!(patch.mood_profile_id, Some(Some("legacy-idle".to_string())));
        assert_eq!(patch.mood_scene_id, Some(Some("scene-1".to_string())));
        assert_eq!(
            patch.fade_ms,
            Some(Some(rhythm_core::TimerSetting::Fixed { value: 250 }))
        );
        assert_eq!(patch.motion_timeout_secs, Some(None));
        let profile_overrides = patch.profile_overrides.unwrap().unwrap();
        assert_eq!(
            profile_overrides
                .get("rhythm")
                .and_then(|override_patch| override_patch.as_ref())
                .and_then(|override_patch| override_patch.motion_timeout_secs.as_ref()),
            Some(&rhythm_core::TimerSetting::Fixed { value: 300 })
        );
        assert!(matches!(profile_overrides.get("sleep"), Some(None)));

        assert_eq!(
            parse_profile_settings_patch(Some(&json!("bad")), "room_profile").unwrap_err(),
            "room_profile must be an object or null"
        );
        assert_eq!(
            parse_profile_settings_patch(Some(&json!({"profile_id": 5})), "room_profile")
                .unwrap_err(),
            "room_profile.profile_id must be a string or null"
        );
        assert_eq!(
            parse_profile_settings_patch(Some(&json!({"mood_enabled": "yes"})), "room_profile")
                .unwrap_err(),
            "room_profile.mood_enabled must be a boolean or null"
        );
        assert_eq!(
            parse_profile_settings_patch(Some(&json!({"mood_profile_id": 5})), "room_profile")
                .unwrap_err(),
            "room_profile.mood_profile_id must be a string or null"
        );
        assert_eq!(
            parse_profile_settings_patch(Some(&json!({"mood_scene_id": false})), "room_profile")
                .unwrap_err(),
            "room_profile.mood_scene_id must be a string or null"
        );
        assert!(parse_profile_settings_patch(
            Some(&json!({"fade_ms": {"mode": "fixed", "value": "fast"}})),
            "room_profile"
        )
        .unwrap_err()
        .contains("Invalid fade_ms"));

        let empty = serde_json::Map::new();
        assert_eq!(parse_timer_patch_value(&empty, "fade_ms").unwrap(), None);
        let mut timer_body = serde_json::Map::new();
        timer_body.insert("fade_ms".to_string(), Value::Null);
        assert_eq!(
            parse_timer_patch_value(&timer_body, "fade_ms").unwrap(),
            Some(None)
        );
    }

    #[test]
    fn mode_settings_and_transition_handlers_reject_legacy_or_invalid_payloads() {
        let state = handler_state_with_runtime();

        for (body, expected) in [
            (
                json!({"rhythm_interval_secs": 30}),
                "rhythm_interval_secs now belongs in light profile config",
            ),
            (json!({"mode": "sleep"}), "Use active/configs in /api/mode"),
            (
                json!({"active_mode": "sleep"}),
                "Use active/configs in /api/mode",
            ),
            (json!({"modes": []}), "Use active/configs in /api/mode"),
            (
                json!({"power_save": true}),
                "power_save has been removed; off is hard_off only",
            ),
            (
                json!({"profiles": []}),
                "Profiles moved to /api/profiles and /api/config",
            ),
        ] {
            let response = handle_put_mode(&state, &body);
            assert_eq!(response.status, 400, "{body}");
            assert!(response.body.contains(expected), "{:?}", response.body);
        }

        let response = handle_put_mode(&state, &json!({"active": "invalid"}));
        assert_eq!(response.status, 400);
        assert!(response.body.contains("Invalid active"));

        let response = handle_put_mode(&state, &json!({"configs": "invalid"}));
        assert_eq!(response.status, 400);
        assert!(response.body.contains("Invalid configs"));

        for (body, expected) in [
            (
                json!({"rhythm_interval_secs": 30}),
                "rhythm_interval_secs now belongs in light profile config",
            ),
            (
                json!({"power_save": true}),
                "power_save has been removed; off is hard_off only",
            ),
            (json!({"active": "day"}), "Mode fields belong in /api/mode"),
            (json!({"configs": []}), "Mode fields belong in /api/mode"),
            (
                json!({"last_change": {}}),
                "Mode fields belong in /api/mode",
            ),
            (
                json!({"profiles": []}),
                "Profiles moved to /api/profiles and /api/config",
            ),
        ] {
            let response = handle_put_transitions(&state, &body);
            assert_eq!(response.status, 400, "{body}");
            assert!(response.body.contains(expected), "{:?}", response.body);
        }

        let response = handle_put_transitions(&state, &json!({"transitions": "bad"}));
        assert_eq!(response.status, 400);
        assert!(response.body.contains("Invalid transitions"));
    }

    #[test]
    fn settings_and_input_binding_handlers_cover_rejection_branches() {
        let state = handler_state_with_runtime();

        for (body, expected) in [
            (
                json!({"profiles": []}),
                "Profiles moved to /api/profiles and /api/config",
            ),
            (
                json!({"light_breaker_enabled": true}),
                "Light breaker moved to /api/light-breaker",
            ),
            (
                json!({"light_breaker": {"enabled": true}}),
                "Light breaker moved to /api/light-breaker",
            ),
        ] {
            let response = handle_put_settings(&state, &body);
            assert_eq!(response.status, 400, "{body}");
            assert!(response.body.contains(expected), "{:?}", response.body);
        }

        let response = handle_post_input_binding(
            &state,
            &json!({
                "trigger": {"kind": "button", "source_node_id": "button-1"},
                "action": {"kind": "mode_set", "mode": "day"}
            }),
        );
        assert_eq!(response.status, 400);
        assert!(response.body.contains("Generic input bindings require id"));

        let response = handle_post_input_binding(
            &state,
            &json!({
                "id": "",
                "trigger": {"kind": "button", "source_node_id": "button-1"},
                "action": {"kind": "mode_set", "mode": "day"}
            }),
        );
        assert_eq!(response.status, 400);
        assert!(response.body.contains("id cannot be empty"));

        let response = handle_post_input_binding(
            &state,
            &json!({
                "id": "binding-1",
                "trigger": {"kind": "bad"},
                "action": {"kind": "mode_set", "mode": "day"}
            }),
        );
        assert_eq!(response.status, 400);
        assert!(response.body.contains("Invalid input binding"));

        let response = handle_put_input_binding(
            &state,
            "binding-1",
            &json!({
                "trigger": {"kind": "button", "source_node_id": "button-1"},
                "action": {"kind": "unknown"}
            }),
        );
        assert_eq!(response.status, 400);
        assert!(response.body.contains("Invalid input binding"));

        let response = handle_put_input_binding(&state, "binding-1", &json!("bad"));
        assert_eq!(response.status, 400);
        assert!(response
            .body
            .contains("Preset input bindings require a preset field"));
    }

    fn add_button_node(state: &SharedState, native_id: &str) -> String {
        let hub_key = HubKey::new(HubType::new("hue"), "local");
        let identity = DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: None,
            room_name: None,
            name: native_id.to_string(),
            device_type: DeviceType::Button,
            hardware_ids: vec![HardwareId::serial(native_id)],
            manufacturer: None,
            model: None,
        };

        let mut state = state.lock().unwrap();
        let canonical_id = match state.canonical_registry.resolve(&identity, &hub_key, 1) {
            ResolveResult::AlreadyKnown { canonical_id }
            | ResolveResult::ReApproved { canonical_id }
            | ResolveResult::Created { canonical_id } => canonical_id,
            ResolveResult::Queued { .. } => panic!("unexpected triage for test button"),
        };
        state.topology.ensure_standalone_device(&canonical_id);
        canonical_id
    }

    fn attach_work_queue(state: &SharedState) -> std::sync::mpsc::Receiver<WorkItem> {
        attach_work_queue_with_capacity(state, 16)
    }

    fn attach_work_queue_with_capacity(
        state: &SharedState,
        capacity: usize,
    ) -> std::sync::mpsc::Receiver<WorkItem> {
        let (tx, rx) = std::sync::mpsc::sync_channel(capacity);
        state.lock().unwrap().work_tx = Some(tx);
        rx
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
    fn put_node_profile_overrides_missing_node_id() {
        let state = test_state();
        let r = handle_put_node_profile_overrides(
            &state,
            &json!({"profile_overrides": {"rhythm": null}}),
            false,
        );
        assert_eq!(r.status, 400);
        assert!(r.body.contains("node_id"));
    }

    #[test]
    fn put_node_profile_overrides_missing_overrides() {
        let state = test_state();
        let r = handle_put_node_profile_overrides(&state, &json!({"node_id": "r"}), false);
        assert_eq!(r.status, 400);
        assert!(r.body.contains("profile_overrides"));
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
    fn light_breaker_endpoint_toggles_global_control() {
        let state = test_state();

        let r = handle_get_light_breaker(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["enabled"], true);

        let r = handle_put_light_breaker(&state, &json!({"enabled": false}));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["enabled"], false);
        assert!(!state.lock().unwrap().light_breaker_enabled);

        let r = handle_put_light_breaker(&state, &json!(true));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["enabled"], true);
        assert!(state.lock().unwrap().light_breaker_enabled);
    }

    #[test]
    fn settings_rejects_light_breaker_mutation() {
        let state = test_state();
        let r = handle_put_settings(&state, &json!({"light_breaker_enabled": false}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("/api/light-breaker"));
    }

    #[test]
    fn settings_response_omits_light_breaker() {
        let state = test_state();
        state.lock().unwrap().light_breaker_enabled = false;

        let r = handle_get_settings(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed.get("light_breaker_enabled").is_none());
        assert!(parsed.get("light_breaker").is_none());
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

    #[test]
    fn legacy_room_brightness_records_light_activity() {
        let state = handler_state_with_runtime();
        let r = handle_set_brightness(
            &state,
            &json!({"room_id": "room1", "brightness": 50}),
            false,
        );
        assert_eq!(r.status, 200);

        let s = state.lock().unwrap();
        assert_eq!(s.light_activity.len(), 1);
        let event = &s.light_activity[0];
        assert_eq!(event.node_id, "room1");
        assert_eq!(event.action_id, "set_brightness");
        assert_eq!(event.brightness, Some(50));
        assert_eq!(
            event.target.as_ref().map(|target| target.node_id.as_str()),
            Some("room1")
        );
    }

    #[test]
    fn node_action_batch_queued() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let r = handle_node_action(
            &state,
            &json!([
                {"node_id": "room1", "action": "on"},
                {"node_id": "room2", "action": "on"}
            ]),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["queued"], true);
        assert_eq!(parsed["dispatch_spacing_ms"], 500);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::QueuedNodeAction { .. }
        ));
    }

    #[test]
    fn node_action_batch_tolerates_backpressured_queue() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue_with_capacity(&state, 1);
        let r = handle_node_action(
            &state,
            &json!([
                {"node_id": "room1", "action": "on"},
                {"node_id": "room2", "action": "on"}
            ]),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["queued"], true);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::QueuedNodeAction { .. }
        ));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::QueuedNodeAction { .. }
        ));
    }

    #[test]
    fn node_action_batch_persist_is_deferred_after_items() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let r = handle_node_action(
            &state,
            &json!([
                {"node_id": "room1", "action": "on"},
                {"node_id": "room2", "action": "on"}
            ]),
            true,
        );
        assert_eq!(r.status, 200);
        for _ in 0..2 {
            match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
                WorkItem::QueuedNodeAction { persist_after, .. } => {
                    assert!(!persist_after);
                }
                _ => panic!("expected queued node action"),
            }
        }
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::DeferredPersist { .. }
        ));
    }

    #[test]
    fn node_brightness_batch_queued() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let r = handle_set_node_brightness(
            &state,
            &json!({
                "dispatch_spacing_ms": 250,
                "nodes": [
                    {"node_id": "room1", "brightness": 50},
                    {"node_id": "room2", "brightness": 50}
                ]
            }),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["queued"], true);
        assert_eq!(parsed["dispatch_spacing_ms"], 250);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::SetNodeBrightness { .. }
        ));
    }

    #[test]
    fn node_curve_missing_modifier() {
        let state = handler_state_with_runtime();
        let r = handle_set_node_curve(&state, &json!({"node_id": "room1"}), false);
        assert_eq!(r.status, 400);
        assert!(r.body.contains("curve modifier"));
    }

    #[test]
    fn node_curve_rejects_multiple_modifiers() {
        let state = handler_state_with_runtime();
        let r = handle_set_node_curve(
            &state,
            &json!({"node_id": "room1", "brightness": 50, "color_temperature": 3200}),
            false,
        );
        assert_eq!(r.status, 400);
        assert!(r.body.contains("exactly one"));
    }

    #[test]
    fn node_curve_batch_queued() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let r = handle_set_node_curve(
            &state,
            &json!({
                "dispatch_spacing_ms": 250,
                "nodes": [
                    {"node_id": "room1", "brightness": 50},
                    {"node_id": "room2", "color_temperature": 3200}
                ]
            }),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["queued"], true);
        assert_eq!(parsed["dispatch_spacing_ms"], 250);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::SetNodeCurveModifier { .. }
        ));
    }

    #[test]
    fn node_curve_single_queued() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let r = handle_set_node_curve(
            &state,
            &json!({"node_id": "room1", "brightness": 50}),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["queued"], true);
        assert_eq!(parsed["dispatch_count"], 1);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::SetNodeCurveModifier { .. }
        ));
    }

    #[test]
    fn node_time_offset_batch_supported() {
        let state = handler_state_with_runtime();
        state.lock().unwrap().room_observed_power.insert(
            "room1".to_string(),
            ObservedPowerState::new(true, ObservedPowerSource::Command),
        );
        let rx = attach_work_queue(&state);
        let r = handle_set_node_time_offset(
            &state,
            &json!({"time_offset": -40.0, "nodes": ["room1", "room2"]}),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["queued"], true);
        assert_eq!(parsed["dispatch_count"], 1);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::PeriodicNodeTick { .. }
        ));
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn node_time_offset_defaults_to_on_periodic_sources_when_nodes_omitted() {
        let state = handler_state_with_runtime();
        state.lock().unwrap().room_observed_power.insert(
            "room1".to_string(),
            ObservedPowerState::new(true, ObservedPowerSource::Command),
        );
        let rx = attach_work_queue(&state);

        let r = handle_set_node_time_offset(&state, &json!({"time_offset": -40.0}), false);

        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["nodes"][0]["id"], "room1");
        assert_eq!(parsed["dispatch_count"], 1);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::PeriodicNodeTick { .. }
        ));
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn node_time_offset_rejects_legacy_array_body() {
        let state = handler_state_with_runtime();
        let r = handle_set_node_time_offset(
            &state,
            &json!([
                {"node_id": "room1", "time_offset": -40.0}
            ]),
            false,
        );

        assert_eq!(r.status, 400);
        assert!(r.body.contains("Expected object body"));
    }

    #[test]
    fn node_preferences_batch_queued() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let r = handle_put_node_preferences(
            &state,
            &json!([
                {"node_id": "room1", "state": "active"},
                {"node_id": "room2", "state": "active"}
            ]),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["queued"], true);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::SetNodePreferences { .. }
        ));
        let s = state.lock().unwrap();
        assert_eq!(s.light_activity.len(), 2);
        let mut activity_nodes = s
            .light_activity
            .iter()
            .map(|event| {
                assert_eq!(event.action_id, "set_room_state");
                event.node_id.as_str()
            })
            .collect::<Vec<_>>();
        activity_nodes.sort_unstable();
        assert_eq!(activity_nodes, vec!["room1", "room2"]);
    }

    /// Regression test for issue #36: rapid single-item preference taps must
    /// not block the HTTP handler on hub dispatch. The single-item path was
    /// previously synchronous, so a slow Matter device could stall up to ~30s
    /// per request, piling up taps until the iOS app gave up and disconnected.
    #[test]
    fn node_preferences_single_item_queues_dispatch() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let r = handle_put_node_preferences(
            &state,
            &json!({"node_id": "room1", "state": "active"}),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["queued"], true);
        assert_eq!(parsed["dispatch_count"], 1);
        assert_eq!(parsed["dispatch_spacing_ms"], 0);
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            WorkItem::SetNodePreferences {
                dispatch_spacing, ..
            } => assert_eq!(dispatch_spacing, Duration::ZERO),
            other => panic!("unexpected work item: {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn parse_profile_settings_patch_accepts_legacy_scene_alias() {
        let patch = parse_profile_settings_patch(
            Some(&json!({"active_light_scene_id": "icy-glow"})),
            "profile_settings",
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            patch
                .mood_scene_id
                .as_ref()
                .and_then(|value| value.as_deref()),
            Some("icy-glow")
        );
    }

    #[test]
    fn node_preferences_accepts_legacy_scene_alias() {
        let state = handler_state_with_runtime();
        state.lock().unwrap().scenes.insert(
            "icy-glow".into(),
            crate::scenes::SceneDefinition {
                id: "icy-glow".into(),
                name: "Icy Glow".into(),
                description: None,
                source: crate::scenes::SceneSource::User,
                light: None,
                extensions: Default::default(),
            },
        );
        let rx = attach_work_queue(&state);

        let r = handle_put_node_preferences(
            &state,
            &json!({
                "node_id": "room1",
                "profile_settings": {
                    "active_light_scene_id": "icy-glow"
                }
            }),
            false,
        );

        assert_eq!(r.status, 200);
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            WorkItem::SetNodePreferences {
                room_profile: Some(patch),
                ..
            } => assert_eq!(
                patch
                    .mood_scene_id
                    .as_ref()
                    .and_then(|value| value.as_deref()),
                Some("icy-glow")
            ),
            other => panic!("unexpected work item: {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn scene_draft_preview_rejects_invalid_body() {
        let state = handler_state_with_runtime();
        let r = handle_post_scene_draft_preview(
            &state,
            &json!({
                "target_id": "room1"
            }),
        );

        assert_eq!(r.status, 400);
        assert!(r.body.contains("Invalid scene draft preview request"));
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

    #[test]
    fn hub_credentials_response_reports_configured_hub_connection_status() {
        struct TestHubProvider;

        impl crate::hub::HubProvider for TestHubProvider {
            fn hub_type(&self) -> HubType {
                HubType::new("homeassistant")
            }

            fn configure(
                &self,
                address: &str,
                credentials_json: &str,
                state: &SharedState,
            ) -> anyhow::Result<()> {
                let key = HubKey::new(HubType::new("homeassistant"), address);
                let credentials = serde_json::from_str(credentials_json)?;
                let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                s.hub_credentials.insert(
                    key.clone(),
                    crate::hub::HubCredentials::new("homeassistant", address, credentials),
                );
                s.hubs.insert(
                    key.clone(),
                    ActiveHub {
                        hub_type: HubType::new("homeassistant"),
                        hub_key: key.clone(),
                        runtime: None,
                        hub_data: Box::new(()),
                        registry: None,
                        discovery: None,
                        shutdown: Arc::new(AtomicBool::new(false)),
                    },
                );
                s.set_hub_connected(&key, false);
                Ok(())
            }
        }

        static TEST_HUB_PROVIDER: TestHubProvider = TestHubProvider;

        let state = test_state();
        {
            let hue_key = HubKey::new(HubType::new("hue"), "192.168.1.20:443");
            let mut s = state.lock().unwrap();
            s.get_hub_provider_fn = Some(Arc::new(|_| &TEST_HUB_PROVIDER));
            s.hubs.insert(
                hue_key.clone(),
                ActiveHub {
                    hub_type: HubType::new("hue"),
                    hub_key: hue_key.clone(),
                    runtime: None,
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: None,
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
            s.set_hub_connected(&hue_key, true);
        }

        let r = handle_put_hub_credentials(
            &state,
            &json!({
                "hub_type": "homeassistant",
                "address": "homeassistant.local:8123",
                "credentials": {"token": "ha-token"}
            }),
        );

        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["hub_connected"], false);
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
                    mood_active: false,
                    standby_enabled: false,
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
                    mood_active: false,
                    standby_enabled: false,
                    hard_off: false,
                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                },
                RoomSnapshot {
                    id: "standalone-light".into(),
                    name: "Standalone Light".into(),
                    kind: rhythm_core::LightNodeKind::LightDevice,
                    parent_id: None,
                    rhythm_enabled: true,
                    disabled: false,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                    mood_active: false,
                    standby_enabled: false,
                    hard_off: false,
                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                },
            ],
        });
        let mut app = AppState::default();
        install_test_light_runtime_modules(&mut app);
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

    fn install_test_light_runtime_modules(app: &mut AppState) {
        crate::light_runtime::register_light_runtime_modules(
            app,
            [crate::light_runtime::LightRuntimeModule::ephemeral(
                crate::light_runtime::RHYTHM_ADAPTIVE_RUNTIME_ID,
                &["rhythm", "rhythm_adaptive"],
                test_rhythm_adaptive_manifest,
                create_test_rhythm_adaptive_runtime,
            )],
        )
        .expect("test light runtime modules should register");
    }

    fn test_rhythm_adaptive_manifest() -> RuntimeManifest {
        RuntimeManifest::new(
            crate::light_runtime::RHYTHM_ADAPTIVE_RUNTIME_ID,
            "Rhythm Adaptive",
        )
        .with_capabilities(RuntimeCapabilities::light_runtime())
    }

    fn create_test_rhythm_adaptive_runtime(
        _: Arc<dyn rhythm_core::RuntimeHandle>,
    ) -> Box<dyn LightRuntime> {
        Box::new(NoopTestLightRuntime(
            crate::light_runtime::RHYTHM_ADAPTIVE_RUNTIME_ID,
        ))
    }

    const HANDLER_EXTERNAL_RUNTIME_ID: &str = "handler-lab";
    const HANDLER_EXTERNAL_RUNTIME_ALIAS: &str = "handler_lab";

    fn register_external_handler_light_runtime_module(state: &SharedState) {
        let mut s = state.lock().unwrap();
        crate::light_runtime::register_light_runtime_module(
            &mut s,
            crate::light_runtime::LightRuntimeModule::ephemeral(
                HANDLER_EXTERNAL_RUNTIME_ID,
                &[HANDLER_EXTERNAL_RUNTIME_ALIAS],
                handler_external_runtime_manifest,
                create_handler_external_runtime,
            ),
        )
        .expect("external handler test runtime should register");
    }

    fn handler_external_runtime_manifest() -> RuntimeManifest {
        RuntimeManifest::new(HANDLER_EXTERNAL_RUNTIME_ID, "Handler Lab")
            .with_capabilities(RuntimeCapabilities::light_runtime())
    }

    fn create_handler_external_runtime(
        _: Arc<dyn rhythm_core::RuntimeHandle>,
    ) -> Box<dyn LightRuntime> {
        Box::new(NoopTestLightRuntime(HANDLER_EXTERNAL_RUNTIME_ID))
    }

    struct NoopTestLightRuntime(&'static str);

    impl LightRuntime for NoopTestLightRuntime {
        fn name(&self) -> &str {
            self.0
        }

        fn handle_event(
            &mut self,
            _snapshot: &RuntimeSnapshot,
            _event: RuntimeEvent,
        ) -> RuntimeResult<RuntimePlan> {
            Ok(RuntimePlan::noop())
        }
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
            room_id: Some(native_id.to_string()),
            room_name: Some("Office Lamp".to_string()),
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
    fn get_factory_default_profile_bundle_returns_factory_default_bundle() {
        let r = handle_get_factory_default_profile_bundle();
        assert_eq!(r.status, 200);
        let parsed: crate::bundle::ProfileBundle = serde_json::from_str(&r.body).unwrap();
        assert_eq!(
            parsed.name.as_deref(),
            Some(
                factory_default_profile_bundle()
                    .name
                    .as_deref()
                    .unwrap_or("")
            )
        );
        assert_eq!(
            parsed.profile.profiles,
            factory_default_profile_bundle().profile.profiles
        );
    }

    #[test]
    fn post_profile_bundle_reset_restores_factory_default_bundle() {
        let state = handler_state_with_runtime();
        let _ = handle_put_profile_bundle(
            &state,
            &json!({
                "profile": {
                    "power_save": true,
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
                    "mode_transitions": []
                }
            }),
        );

        let r = handle_post_profile_bundle_reset(&state);
        assert_eq!(r.status, 200);
        let parsed: crate::bundle::ProfileBundle = serde_json::from_str(&r.body).unwrap();
        let factory_default = factory_default_profile_bundle();
        let mut parsed_profiles = parsed.profile.profiles.clone();
        parsed_profiles.sort_by(|left, right| left.id.cmp(&right.id));
        let mut expected_profiles = factory_default.profile.profiles.clone();
        expected_profiles.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(
            parsed.profile.power_save,
            factory_default.profile.power_save
        );
        assert_eq!(
            parsed.profile.mode_transitions,
            factory_default.profile.mode_transitions
        );
        assert_eq!(parsed_profiles, expected_profiles);
    }

    #[test]
    fn post_factory_reset_invokes_platform_follow_up() {
        let state = handler_state_with_runtime();
        let invoked = Arc::new(AtomicBool::new(false));
        state.lock().unwrap().after_factory_reset_fn = Some({
            let invoked = invoked.clone();
            Arc::new(move |_| {
                invoked.store(true, Ordering::SeqCst);
            })
        });

        let r = handle_post_factory_reset(&state);
        assert_eq!(r.status, 200);
        assert!(invoked.load(Ordering::SeqCst));
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
        assert_eq!(parsed["min_brightness"], 1);
        assert_eq!(parsed["max_brightness"], 1);
        assert_eq!(parsed["curve"]["type"], "constant");
        assert!(parsed["curve"]["direct_color"].is_null());
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
    fn put_config_day_idle_explicit_fifteen_brightness_is_preserved() {
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
        assert_eq!(stored.min_brightness, 15);
        assert_eq!(stored.max_brightness, 15);
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
    fn put_node_profile_overrides_queues_patch() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let r = handle_put_node_profile_overrides(
            &state,
            &json!({
                "node_id": "standalone-light",
                "profile_overrides": {
                    "rhythm": {
                        "motion_timeout_secs": {"mode": "fixed", "value": 60}
                    }
                }
            }),
            false,
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["queued"], true);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::SetNodePreferences { .. }
        ));
        let s = state.lock().unwrap();
        assert_eq!(s.light_activity.len(), 1);
        let event = &s.light_activity[0];
        assert_eq!(event.node_id, "standalone-light");
        assert_eq!(event.action_id, "set_light_preferences");
        let payload = event.payload.as_ref().unwrap();
        assert_eq!(payload["profile_overrides_touched"], true);
        assert_eq!(payload["profile_override_keys"], json!(["rhythm"]));
        assert_eq!(payload["clear_profile_overrides"], false);
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
                room_id: Some("device-1".to_string()),
                room_name: Some("Office Lamp".to_string()),
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
    fn put_preset_input_binding_accepts_selected_button() {
        let state = test_state();
        let button_id = add_button_node(&state, "button-native-1");

        let r = handle_put_input_binding(
            &state,
            "bedroom_day_sleep",
            &json!({
                "preset": "day_sleep_toggle",
                "source_node_id": button_id,
                "button_action": "on"
            }),
        );

        assert_eq!(r.status, 200);
        let body: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(body["bindings"][0]["id"], "bedroom_day_sleep");
        assert_eq!(body["bindings"][0]["source_node_id"], button_id);
        assert_eq!(body["bindings"][0]["preset"], "day_sleep_toggle");
        assert_eq!(body["bindings"][0]["action"]["kind"], "mode_cycle");
        assert_eq!(body["bindings"][0]["trigger"]["button_action"], "on_press");
    }

    #[test]
    fn post_day_sleep_input_binding_generates_per_button_ids() {
        let state = test_state();
        let first_button_id = add_button_node(&state, "button-native-1");
        let second_button_id = add_button_node(&state, "button-native-2");

        let first = handle_post_input_binding(
            &state,
            &json!({
                "preset": "day_sleep_toggle",
                "source_node_id": first_button_id,
                "button_action": "on"
            }),
        );
        assert_eq!(first.status, 200);

        let second = handle_post_input_binding(
            &state,
            &json!({
                "preset": "day_sleep_toggle",
                "source_node_id": second_button_id,
                "button_action": "on"
            }),
        );
        assert_eq!(second.status, 200);

        let body: Value = serde_json::from_str(&second.body).unwrap();
        let bindings = body["bindings"].as_array().unwrap();
        assert_eq!(bindings.len(), 2);
        assert_ne!(bindings[0]["id"], bindings[1]["id"]);
        assert!(bindings
            .iter()
            .all(|binding| binding["preset"] == "day_sleep_toggle"));
    }

    #[test]
    fn put_generic_input_binding_uses_path_id() {
        let state = test_state();
        let button_id = add_button_node(&state, "button-native-2");

        let r = handle_put_input_binding(
            &state,
            "custom_sleep_button",
            &json!({
                "source_node_id": button_id,
                "trigger": {
                    "kind": "button",
                    "button_action": "off_press"
                },
                "action": {
                    "kind": "mode_cycle",
                    "modes": ["day", "sleep"],
                    "transition": { "kind": "none" }
                }
            }),
        );

        assert_eq!(r.status, 200);
        let body: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(body["bindings"][0]["id"], "custom_sleep_button");
        assert_eq!(body["bindings"][0]["source_node_id"], button_id);
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
    fn delete_device_on_appliance_matter_uses_unpairing() {
        let (state, registry, canonical_id, room_id, hub_key) =
            handler_state_with_canonical_light_for_hub("matter", "matter-100");
        let calls = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
        {
            let calls = calls.clone();
            let mut s = state.lock().unwrap();
            s.platform_type = "appliance";
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
        assert_eq!(recorded[0].1["force"], true);
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
        let r = handle_put_settings(&state, &json!({"auto_update": false}));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed.get("power_save").is_none());
        assert_eq!(parsed["auto_update"], false);
        assert_eq!(parsed["light_runtime"], "rhythm-adaptive");
        assert!(parsed.get("mode").is_none());
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn put_settings_updates_update_channel() {
        let state = handler_state_with_runtime();
        let r = handle_put_settings(&state, &json!({"update_channel": "stable"}));
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["update_channel"], "stable");
        assert_eq!(
            state.lock().unwrap().update_channel,
            Some(crate::state::UpdateChannel::Stable)
        );
    }

    #[test]
    fn put_settings_rejects_invalid_update_channel() {
        let state = handler_state_with_runtime();
        for body in [
            json!({"update_channel": "nightly"}),
            json!({"update_channel": 7}),
        ] {
            let r = handle_put_settings(&state, &body);
            assert_eq!(r.status, 400);
            assert!(r.body.contains("update_channel"), "body: {}", r.body);
        }
        assert!(state.lock().unwrap().update_channel.is_none());
    }

    #[test]
    fn put_settings_updates_light_runtime() {
        let state = handler_state_with_runtime();
        register_external_handler_light_runtime_module(&state);
        let r = handle_put_settings(
            &state,
            &json!({"light_runtime": HANDLER_EXTERNAL_RUNTIME_ALIAS}),
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["light_runtime"], HANDLER_EXTERNAL_RUNTIME_ID);
    }

    #[test]
    fn light_runtime_resource_selects_runtime() {
        let state = handler_state_with_runtime();

        let r = handle_get_light_runtime(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["runtime_id"], "rhythm-adaptive");
        assert_eq!(parsed["available_runtime_ids"], json!(["rhythm-adaptive"]));

        register_external_handler_light_runtime_module(&state);
        let r = handle_put_light_runtime(
            &state,
            &json!({"runtime_id": HANDLER_EXTERNAL_RUNTIME_ALIAS}),
        );
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["runtime_id"], HANDLER_EXTERNAL_RUNTIME_ID);
        assert_eq!(
            state.lock().unwrap().light_runtime_kind,
            crate::light_runtime::LightRuntimeKind::new(HANDLER_EXTERNAL_RUNTIME_ID)
        );
    }

    #[test]
    fn light_runtime_handlers_expose_and_select_external_registered_module_by_alias() {
        let state = handler_state_with_runtime();
        register_external_handler_light_runtime_module(&state);

        let r = handle_get_light_runtime_manifests(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed["runtimes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|manifest| manifest["id"] == HANDLER_EXTERNAL_RUNTIME_ID));

        let r = handle_get_light_runtime_manifest(&state, HANDLER_EXTERNAL_RUNTIME_ALIAS);
        assert_eq!(r.status, 200);
        let manifest: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(manifest["id"], HANDLER_EXTERNAL_RUNTIME_ID);
        assert_eq!(manifest["name"], "Handler Lab");

        let r = handle_put_light_runtime(
            &state,
            &json!({"runtime_id": HANDLER_EXTERNAL_RUNTIME_ALIAS}),
        );
        assert_eq!(r.status, 200);
        let selected: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(selected["runtime_id"], HANDLER_EXTERNAL_RUNTIME_ID);
        assert_eq!(
            state.lock().unwrap().light_runtime_kind.as_str(),
            HANDLER_EXTERNAL_RUNTIME_ID
        );

        let r = handle_put_settings(
            &state,
            &json!({"light_runtime": HANDLER_EXTERNAL_RUNTIME_ALIAS}),
        );
        assert_eq!(r.status, 200);
        let settings: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(settings["light_runtime"], HANDLER_EXTERNAL_RUNTIME_ID);
    }

    #[test]
    fn light_runtime_manifest_rejects_unknown_runtime() {
        let state = handler_state_with_runtime();

        let r = handle_get_light_runtime_manifest(&state, "not-registered");

        assert_eq!(r.status, 400);
        assert!(r.body.contains("unknown light runtime"));
    }

    #[test]
    fn light_runtime_resource_requires_runtime_id() {
        let state = handler_state_with_runtime();
        let r = handle_put_light_runtime(&state, &json!({"mode": "expert"}));

        assert_eq!(r.status, 400);
        assert!(r.body.contains("Missing runtime_id"));
    }

    #[test]
    fn put_settings_rejects_unknown_light_runtime() {
        let state = handler_state_with_runtime();
        let r = handle_put_settings(&state, &json!({"light_runtime": "unknown"}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("unknown light runtime"));
    }

    #[test]
    fn get_settings_returns_raw_settings() {
        let state = handler_state_with_runtime();
        let r = handle_get_settings(&state);
        assert_eq!(r.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(parsed.get("power_save").is_none());
        assert!(parsed["auto_update"].is_boolean());
        assert_eq!(parsed["light_runtime"], "rhythm-adaptive");
        assert!(parsed.get("mode").is_none());
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn put_settings_rejects_removed_power_save() {
        let state = handler_state_with_runtime();
        let r = handle_put_settings(&state, &json!({"power_save": false}));
        assert_eq!(r.status, 400);
        assert!(r.body.contains("power_save has been removed"));
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
                    "trigger_enabled": false,
                    "duration_ms": {"mode": "fixed", "value": 5000}
                }]
            }),
        );
        assert_eq!(r.status, 200);

        let parsed: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(parsed["transitions"][0]["trigger"]["kind"], "scheduled");
        assert_eq!(parsed["transitions"][0]["trigger"]["time"], "22:00");
        assert_eq!(parsed["transitions"][0]["trigger_enabled"], false);
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
