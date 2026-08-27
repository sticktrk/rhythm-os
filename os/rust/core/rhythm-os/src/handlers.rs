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

fn correlation_id_from_body(body: &Value) -> Option<String> {
    let correlation_id = body.get("correlation_id")?.as_str()?.trim();
    (!correlation_id.is_empty() && correlation_id.len() <= 128).then(|| correlation_id.to_string())
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
    pub fn json_status(status: u16, body: String) -> Self {
        Self {
            status,
            body,
            content_type: "application/json",
        }
    }

    pub fn json_ok(body: String) -> Self {
        Self::json_status(200, body)
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

    pub fn conflict(msg: &str) -> Self {
        Self {
            status: 409,
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

pub fn handle_get_matter_setup_code(state: &SharedState, device_id: &str) -> ApiResponse {
    let loader = match state.lock() {
        Ok(state) => state.load_pairing_recovery_fn.clone(),
        Err(_) => return ApiResponse::server_error("lock"),
    };
    let Some(loader) = loader else {
        return ApiResponse::server_error("Matter setup code recovery is unavailable");
    };
    match loader(state, "matter", device_id) {
        Ok(Some(secret)) => match serde_json::to_string(&secret) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(error) => ApiResponse::server_error(error),
        },
        Ok(None) => ApiResponse::not_found("No saved Matter setup code is available"),
        Err(error) => ApiResponse::server_error(error),
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
    let (appliance_reset, post_barrier_recovery) = state
        .lock()
        .map(|state| {
            (
                state.platform_type == "appliance",
                state.factory_reset_recovery_fn.clone(),
            )
        })
        .unwrap_or((false, None));
    let mut reset_reservation = if appliance_reset {
        match try_acquire_factory_reset_guard(state) {
            Ok(guard) => Some(guard),
            Err(response) => return response,
        }
    } else {
        None
    };
    match commands::do_factory_reset(state) {
        Ok(json) => {
            if let Some(callback) = state
                .lock()
                .ok()
                .and_then(|state| state.after_factory_reset_fn.clone())
            {
                if let Err(error) = callback(state) {
                    // Shared state has already crossed the destructive reset
                    // barrier.  Never reopen Matter/Hue/local-BLE pairing in
                    // the reboot grace period, even when platform cleanup
                    // fails: a new bond or association could recreate state
                    // that this reset just removed.
                    if let Some(guard) = reset_reservation.as_mut() {
                        guard.keep_reserved();
                    }
                    if let Some(recover) = post_barrier_recovery.as_ref() {
                        recover();
                    }
                    return ApiResponse::server_error(format!(
                        "Factory reset cleared Rhythm state, but platform cleanup failed: {error:#}"
                    ));
                }
            }
            // Once both shared reset and synchronous platform cleanup succeed,
            // retain both pairing reservations until the scheduled reboot.
            // Releasing either after the HTTP response would allow a new
            // Matter association or Bluetooth bond in the reboot grace period.
            if let Some(guard) = reset_reservation.as_mut() {
                guard.keep_reserved();
            }
            ApiResponse::json_ok(json)
        }
        Err(e) if commands::factory_reset_error_is_post_barrier(&e) => {
            // Reset began and may already have removed credentials/state. Keep
            // both shared pairing slots poisoned until the recovery reboot so
            // a concurrent pairing request cannot repopulate either one.
            if let Some(guard) = reset_reservation.as_mut() {
                guard.keep_reserved();
            }
            if let Some(recover) = post_barrier_recovery.as_ref() {
                recover();
            }
            ApiResponse::server_error(format!(
                "Factory reset could not be confirmed after reset began: {e:#}"
            ))
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
    let archive = request
        .params
        .get("archive")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let requested_device_id = request
        .params
        .get("device_id")
        .and_then(serde_json::Value::as_str);
    let requested_hub_address = request
        .params
        .get("hub_address")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| {
            if request.hub_type == crate::hub::HubType::LOCAL_BLE {
                "default"
            } else {
                "local"
            }
        });
    let requested_hub_key = crate::canonical::identity::HubKey::new(
        crate::hub::HubType::new(&request.hub_type),
        requested_hub_address,
    );
    if archive {
        let device_id = requested_device_id
            .ok_or_else(|| anyhow::anyhow!("Archive request is missing device_id"))?;
        commands::validate_device_archive(state, device_id, &requested_hub_key)?;
    } else if let Some(device_id) = requested_device_id {
        commands::validate_active_device_removal_target(state, device_id, &requested_hub_key)?;
    }
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
        logging::summarize_pairing_params_for_log(&request.params)
    );

    let result = match start_fn(state, &request.hub_type, &request.params) {
        Ok(result) => result,
        Err(e) => {
            crate::pairing::record_pairing_history(
                state,
                crate::pairing::pairing_history_entry_for_unpair(
                    &request.hub_type,
                    &request.params,
                    &crate::pairing::PairingStatus::Failed,
                    None,
                    Some(&format!("{e:#}")),
                ),
            );
            return Err(e);
        }
    };
    log::info!(
        target: "pair",
        "Unpairing result: status={:?} scope={:?} warning={:?} error={:?}",
        result.status,
        result.completion_scope,
        result.warning,
        result.error
    );
    if result.status == crate::pairing::PairingStatus::Complete {
        if archive && result.device_id.is_none() {
            let error = anyhow::anyhow!("Completed archive did not identify the removed device");
            crate::pairing::record_pairing_history(
                state,
                crate::pairing::pairing_history_entry_for_unpair(
                    &request.hub_type,
                    &request.params,
                    &crate::pairing::PairingStatus::Failed,
                    None,
                    Some(&error.to_string()),
                ),
            );
            return Err(error);
        }
        if let Some(device_id) = &result.device_id {
            let hub_address = result
                .hub_address
                .as_deref()
                .or_else(|| {
                    request
                        .params
                        .get("hub_address")
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or_else(|| {
                    if result.hub_type == crate::hub::HubType::LOCAL_BLE {
                        "default"
                    } else {
                        "local"
                    }
                });
            let hub_key = crate::canonical::identity::HubKey::new(
                crate::hub::HubType::new(&result.hub_type),
                hub_address,
            );
            let cleanup = if archive {
                commands::do_device_archive(state, device_id, &hub_key)
            } else {
                commands::do_device_endpoint_remove(state, device_id, &hub_key)
            };
            if let Err(error) = cleanup {
                let error_text = format!("{error:#}");
                crate::pairing::record_pairing_history(
                    state,
                    crate::pairing::pairing_history_entry_for_unpair(
                        &request.hub_type,
                        &request.params,
                        &crate::pairing::PairingStatus::Failed,
                        Some(device_id),
                        Some(&error_text),
                    ),
                );
                return Err(error);
            }
        }
    }

    crate::pairing::record_pairing_history(
        state,
        crate::pairing::pairing_history_entry_for_unpair(
            &request.hub_type,
            &request.params,
            &result.status,
            result.device_id.as_deref(),
            result.error.as_deref(),
        ),
    );

    Ok(result)
}

enum ApplianceDeleteUnpairDecision {
    NotApplicable,
    Request(crate::pairing::UnpairingRequest),
    EndpointSelectionRequired(String),
}

fn appliance_delete_unpair_request(
    state: &SharedState,
    id: &str,
) -> anyhow::Result<ApplianceDeleteUnpairDecision> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.platform_type != "appliance" {
        return Ok(ApplianceDeleteUnpairDecision::NotApplicable);
    }

    let mut supported_unpair_types = s
        .hub_capabilities
        .iter()
        .filter(|capability| capability.supports_unpairing)
        .map(|capability| capability.hub_type.as_str())
        .collect::<std::collections::HashSet<_>>();
    // Keep legacy appliance behavior while capabilities hydrate, and support
    // direct IDs in tests/minimal runtimes that install pairing callbacks
    // without publishing the full state DTO.
    supported_unpair_types.insert("matter");
    supported_unpair_types.insert("hue_ble");
    supported_unpair_types.insert("local_ble");
    let direct_prefix = id
        .starts_with("matter-")
        .then_some("matter")
        .or_else(|| id.starts_with("hue-ble-").then_some("hue_ble"))
        .or_else(|| id.starts_with("local-ble-").then_some("local_ble"));
    let canonical = s.canonical_registry.get(id).or_else(|| {
        direct_prefix.and_then(|hub_type| {
            let address = if hub_type == crate::hub::HubType::LOCAL_BLE {
                "default"
            } else {
                "local"
            };
            let key = crate::canonical::identity::HubKey::new(
                crate::hub::HubType::new(hub_type),
                address,
            );
            s.canonical_registry.find_by_native_id(&key, id)
        })
    });
    if let Some(device) = canonical {
        if device.endpoints.len() > 1 {
            return Ok(ApplianceDeleteUnpairDecision::EndpointSelectionRequired(
                format!(
                    "Device has multiple endpoints ({}); use endpoint-specific POST /unpair with hub_type and hub_address",
                    device
                        .endpoints
                        .iter()
                        .map(|endpoint| endpoint.hub_key.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
    let mut endpoints = if let Some(hub_type) = direct_prefix {
        let address = if hub_type == crate::hub::HubType::LOCAL_BLE {
            "default"
        } else {
            "local"
        };
        vec![(hub_type.to_string(), address.to_string(), id.to_string())]
    } else {
        canonical
            .map(|device| {
                device
                    .active_endpoints()
                    .filter(|endpoint| {
                        supported_unpair_types.contains(endpoint.hub_key.hub_type.as_str())
                    })
                    .map(|endpoint| {
                        (
                            endpoint.hub_key.hub_type.as_str().to_string(),
                            endpoint.hub_key.address.clone(),
                            endpoint.native_id.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    endpoints.sort();
    endpoints.dedup();

    match endpoints.as_slice() {
        [] => Ok(ApplianceDeleteUnpairDecision::NotApplicable),
        [(hub_type, hub_address, device_id)] => Ok(ApplianceDeleteUnpairDecision::Request(
            crate::pairing::UnpairingRequest {
                hub_type: hub_type.clone(),
                params: serde_json::json!({
                    "device_id": device_id,
                    "hub_address": hub_address,
                    "force": false
                }),
            },
        )),
        _ => Ok(ApplianceDeleteUnpairDecision::EndpointSelectionRequired(
            format!(
                "Device has multiple removable endpoints ({}); use endpoint-specific POST /unpair with hub_type and hub_address",
                endpoints
                    .iter()
                    .map(|(hub_type, hub_address, _)| format!("{hub_type}@{hub_address}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

pub fn handle_delete_device(state: &SharedState, id: &str) -> ApiResponse {
    match appliance_delete_unpair_request(state, id) {
        Ok(ApplianceDeleteUnpairDecision::Request(request)) => {
            let _adapter_guard = match try_acquire_unpairing_guard(state, &request.hub_type) {
                Ok(guard) => guard,
                Err(response) => return response,
            };
            match perform_unpair_device(state, &request) {
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
            }
        }
        Ok(ApplianceDeleteUnpairDecision::EndpointSelectionRequired(message)) => {
            return ApiResponse::conflict(&message);
        }
        Ok(ApplianceDeleteUnpairDecision::NotApplicable) => {}
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

fn parse_expected_profile_overrides_value(
    body: &serde_json::Map<String, Value>,
    field_name: &str,
) -> Result<Option<BTreeMap<String, LightProfileNodeOverride>>, String> {
    let Some(value) = body.get("expected_profile_overrides") else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| format!("{field_name}.expected_profile_overrides must be an object"))?;
    let mut overrides = BTreeMap::new();
    for (profile_id, value) in object {
        if profile_id.trim().is_empty() {
            return Err(format!(
                "{field_name}.expected_profile_overrides keys must be non-empty profile IDs"
            ));
        }
        let profile_override: LightProfileNodeOverride = serde_json::from_value(value.clone())
            .map_err(|e| {
                format!(
                    "Invalid {field_name}.expected_profile_overrides.{}: {}",
                    profile_id, e
                )
            })?;
        overrides.insert(profile_id.clone(), profile_override);
    }
    Ok(Some(overrides))
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

    let motion_activation_enabled = match body.get("motion_activation_enabled") {
        None => None,
        Some(v) if v.is_null() => Some(None),
        Some(v) => Some(Some(v.as_bool().ok_or_else(|| {
            format!("{field_name}.motion_activation_enabled must be a boolean or null")
        })?)),
    };

    let room_schedule = match body.get("room_schedule") {
        None => None,
        Some(v) if v.is_null() => Some(None),
        Some(v) => {
            let schedule: rhythm_core::RoomScheduleConfig = serde_json::from_value(v.clone())
                .map_err(|e| format!("Invalid {field_name}.room_schedule: {e}"))?;
            if schedule.wake_time == schedule.sleep_time {
                return Err(format!(
                    "{field_name}.room_schedule wake_time and sleep_time must differ"
                ));
            }
            Some(Some(schedule))
        }
    };

    Ok(Some(commands::RoomProfileSettingsPatch {
        clear_all: false,
        profile_id,
        mood_enabled,
        mood_profile_id,
        mood_scene_id,
        fade_ms: parse_timer_patch_value(body, "fade_ms")?,
        motion_timeout_secs: parse_timer_patch_value(body, "motion_timeout_secs")?,
        motion_activation_enabled,
        room_schedule,
        profile_overrides: parse_profile_overrides_patch_value(body, field_name)?,
        expected_effective_profile_overrides: None,
        replace_profile_overrides: body
            .get("replace_profile_overrides")
            .and_then(Value::as_bool)
            .unwrap_or(false),
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
    handle_put_config_with_options_and_precondition(
        state,
        profile_id,
        body,
        apply_outputs,
        None,
        None,
    )
}

pub fn handle_put_config_with_options_and_precondition(
    state: &SharedState,
    profile_id: Option<&str>,
    body: &Value,
    apply_outputs: bool,
    expected_server_instance_id: Option<&str>,
    expected_resource_sha256: Option<&str>,
) -> ApiResponse {
    let precondition = match (expected_server_instance_id, expected_resource_sha256) {
        (Some(server_instance_id), Some(resource_sha256)) => {
            Some((server_instance_id, resource_sha256))
        }
        (None, None) => None,
        _ => {
            return ApiResponse::bad_request(
                "Guarded config writes require both expected server identity and resource hash",
            )
        }
    };
    let mut config: rhythm_core::LightProfileConfig = match serde_json::from_value(body.clone()) {
        Ok(c) => c,
        Err(e) => return ApiResponse::bad_request(&format!("Invalid config: {}", e)),
    };
    if let Some(id) = profile_id {
        config.id = id.to_string();
    }
    match commands::do_config_set_with_options_if_matches(
        state,
        config,
        apply_outputs,
        precondition,
    ) {
        Ok(()) => ApiResponse::no_content(),
        Err(e) if e.to_string().starts_with("config precondition failed:") => {
            ApiResponse::conflict(&e.to_string())
        }
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

pub fn handle_get_scenes(state: &SharedState, target_id: Option<&str>) -> ApiResponse {
    match commands::build_scenes_for_target(state, target_id) {
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
    let correlation_id = correlation_id_from_body(body);
    let request: crate::scenes::SceneApplyRequest = match serde_json::from_value(body.clone()) {
        Ok(request) => request,
        Err(e) => return ApiResponse::bad_request(&format!("Invalid scene apply request: {}", e)),
    };
    let target_id = request.target_id.clone();
    match commands::do_scene_apply(state, scene_id, request) {
        Ok(json) => {
            let mut record = crate::activity::LightActivityRecord::app(&target_id, "apply_scene");
            record.payload = Some(json!({"scene_id": scene_id}));
            record.correlation_id = correlation_id;
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

pub fn handle_get_hue_authority(state: &SharedState) -> ApiResponse {
    match commands::build_hue_authority(state)
        .and_then(|response| serde_json::to_string(&response).map_err(anyhow::Error::from))
    {
        Ok(json) => ApiResponse::json_ok(json),
        Err(error) => ApiResponse::server_error(error),
    }
}

pub fn handle_put_hue_authority(state: &SharedState, body: &Value) -> ApiResponse {
    let request =
        match serde_json::from_value::<crate::api_types::HueAuthorityUpdateRequest>(body.clone()) {
            Ok(request) => request,
            Err(error) => {
                return ApiResponse::bad_request(&format!("Invalid Hue authority request: {error}"))
            }
        };
    match commands::do_hue_authority_update(state, request)
        .and_then(|response| serde_json::to_string(&response).map_err(anyhow::Error::from))
    {
        Ok(json) => ApiResponse::json_ok(json),
        Err(error) if error.to_string().contains("changed while") => {
            ApiResponse::conflict(&error.to_string())
        }
        Err(error)
            if error.to_string().contains("required")
                || error.to_string().contains("not configured")
                || error.to_string().contains("stale or incomplete")
                || error.to_string().contains("No rooms") =>
        {
            ApiResponse::bad_request(&error.to_string())
        }
        Err(error) => ApiResponse::server_error(error),
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
    let correlation_id = correlation_id_from_body(body);
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
            record.correlation_id = correlation_id.clone();
            record.fanout_of = correlation_id.clone();
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
        record.correlation_id = correlation_id.clone();
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
    let correlation_id = correlation_id_from_body(body);
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

    let batch = updates.len() > 1;
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
        record.correlation_id = correlation_id.clone();
        if batch {
            record.fanout_of = correlation_id.clone();
        }
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

/// Apply the per-node motion admission preference synchronously.
///
/// Unlike the general preference endpoint, this mutation has no integration
/// light command to pace. Returning the post-apply node state gives clients an
/// authoritative acknowledgement instead of treating queue acceptance as
/// success.
pub fn handle_put_node_motion_activation(
    state: &SharedState,
    body: &Value,
    persist: bool,
) -> ApiResponse {
    let Some(raw_node_id) = body.get("node_id").and_then(Value::as_str) else {
        return ApiResponse::bad_request("Missing node_id");
    };
    let Some(enabled) = body.get("enabled").and_then(Value::as_bool) else {
        return ApiResponse::bad_request("enabled must be a boolean");
    };
    let node_id = commands::resolve_node_id(state, raw_node_id);
    let correlation_id = body
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        })
        .map(str::to_string)
        .unwrap_or_else(|| crate::logging::next_command_id("motion-activation"));

    let before = state
        .lock()
        .ok()
        .and_then(|locked| locked.hub_runtime())
        .and_then(|runtime| runtime.engine_node_snapshot(&node_id))
        .map(|snapshot| snapshot.profile_settings.motion_activation_enabled());
    let patch = commands::RoomProfileSettingsPatch {
        motion_activation_enabled: Some(Some(enabled)),
        ..Default::default()
    };

    let result = commands::do_node_preferences_set(
        state,
        &node_id,
        None,
        None,
        None,
        None,
        Some(&patch),
        persist,
    );

    let mut record = crate::activity::LightActivityRecord::app(&node_id, "set_motion_activation");
    record.correlation_id = Some(correlation_id.clone());
    record.change = Some(crate::activity::LightValueChange {
        axis: Some("motion_activation_enabled".to_string()),
        before: before.map(Value::Bool),
        after: result.as_ref().ok().map(|_| Value::Bool(enabled)),
    });
    record.payload = Some(json!({
        "requested_enabled": enabled,
        "status": if result.is_ok() { "applied" } else { "failed" },
        "error": result.as_ref().err().map(ToString::to_string),
    }));
    crate::activity::record_light_activity(state, record);

    match result {
        Ok(_) => {
            tracing::info!(
                target: "cmd",
                event = "motion_activation_set",
                node_id = %node_id,
                correlation_id = %correlation_id,
                enabled,
                status = "applied",
                "Motion activation preference applied"
            );
            match commands::build_node_state(state, &node_id) {
                Ok(node) => nodes_response(vec![node], false, Duration::ZERO),
                Err(error) => ApiResponse::server_error(error),
            }
        }
        Err(error) => {
            tracing::warn!(
                target: "cmd",
                event = "motion_activation_set",
                node_id = %node_id,
                correlation_id = %correlation_id,
                enabled,
                status = "failed",
                error = %error,
                "Motion activation preference failed"
            );
            ApiResponse::server_error(error)
        }
    }
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

    if items.len() == 1 {
        if let Some(mode_value) = items[0].get("schedule_test") {
            let mode = match serde_json::from_value::<rhythm_core::RhythmMode>(mode_value.clone()) {
                Ok(mode) => mode,
                Err(_) => return ApiResponse::bad_request("schedule_test must be day or sleep"),
            };
            let Some(raw_node_id) = items[0].get("node_id").and_then(Value::as_str) else {
                return ApiResponse::bad_request("Missing node_id");
            };
            let node_id = commands::resolve_node_id(state, raw_node_id);
            let request_id = items[0]
                .get("request_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= 128)
                .map(str::to_string)
                .unwrap_or_else(|| crate::logging::next_command_id("room-schedule-test"));
            let result = commands::do_room_schedule_test(state, &node_id, mode);
            let mut record = crate::activity::LightActivityRecord::app(
                &node_id,
                if mode == rhythm_core::RhythmMode::Day {
                    "room_schedule_test_wake"
                } else {
                    "room_schedule_test_sleep"
                },
            );
            record.correlation_id = Some(request_id);
            record.payload = Some(json!({
                "status": if result.is_ok() { "applied" } else { "failed" },
                "failure_stage": result.as_ref().err().map(|_| "output_apply"),
            }));
            crate::activity::record_light_activity(state, record);
            return match result {
                Ok(node) => ApiResponse::json_ok(format!(r#"{{"nodes":[{}]}}"#, node)),
                Err(error) => ApiResponse::server_error(error),
            };
        }
    }

    // Room schedules require an authoritative acknowledgement. Keep this
    // additive shape on the SDK-owned preferences route, but apply it
    // synchronously so a 2xx response means the canonical room state was
    // persisted rather than merely admitted to the dispatch queue.
    if items.len() == 1
        && items[0]
            .get("profile_settings")
            .and_then(Value::as_object)
            .is_some_and(|settings| settings.contains_key("room_schedule"))
    {
        let Some(raw_node_id) = items[0].get("node_id").and_then(Value::as_str) else {
            return ApiResponse::bad_request("Missing node_id");
        };
        let node_id = commands::resolve_node_id(state, raw_node_id);
        let previous_schedule = match state
            .lock()
            .ok()
            .and_then(|locked| locked.hub_runtime())
            .and_then(|runtime| runtime.engine_node_snapshot(&node_id))
        {
            Some(snapshot)
                if snapshot.kind.is_light_addressable() && snapshot.parent_id.is_none() =>
            {
                snapshot.profile_settings.room_schedule
            }
            Some(_) => {
                return ApiResponse::bad_request(
                    "Schedules require a room or unassigned light node",
                )
            }
            None => return ApiResponse::bad_request("Schedule target was not found"),
        };
        let patch = match parse_profile_settings_patch(
            items[0].get("profile_settings"),
            "profile_settings",
        ) {
            Ok(Some(patch)) => patch,
            Ok(None) => return ApiResponse::bad_request("Missing room schedule"),
            Err(error) => return ApiResponse::bad_request(&error),
        };
        let Some(schedule) = patch
            .room_schedule
            .as_ref()
            .and_then(Option::as_ref)
            .copied()
        else {
            return ApiResponse::bad_request("Room schedule cannot be cleared");
        };
        let source = if schedule.follows_time() {
            "follow_time"
        } else {
            "wake_sleep_presets"
        };
        let request_id = items[0]
            .get("request_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .map(str::to_string)
            .or_else(|| Some(crate::logging::next_command_id("room-schedule-save")));
        let accepted_node = match commands::do_node_preferences_set(
            state,
            &node_id,
            None,
            None,
            None,
            None,
            Some(&patch),
            persist,
        ) {
            Ok(node) => node,
            Err(error) => {
                let mut record = crate::activity::LightActivityRecord::app(
                    &node_id,
                    "room_schedule_config_updated",
                );
                record.correlation_id = request_id;
                record.payload = Some(json!({
                    "source": source,
                    "status": "failed",
                    "failure_stage": "persistence",
                }));
                crate::activity::record_light_activity(state, record);
                return ApiResponse::server_error(error);
            }
        };
        let output_result = commands::apply_room_schedule_configuration(
            state,
            &node_id,
            previous_schedule,
            schedule,
        );
        if let Err(error) = &output_result {
            tracing::warn!(
                target: "cmd",
                event = "room_schedule_config_updated",
                node_id = %node_id,
                status = "failed",
                failure_stage = "output_apply",
                error = %error,
                "Room schedule was accepted but immediate output application failed"
            );
        }
        let mut record =
            crate::activity::LightActivityRecord::app(&node_id, "room_schedule_config_updated");
        record.correlation_id = request_id;
        record.payload = Some(json!({
            "source": source,
            "status": match &output_result {
                Ok(true) => "applied",
                Ok(false) => "accepted",
                Err(_) => "failed",
            },
            "failure_stage": output_result.as_ref().err().map(|_| "output_apply"),
        }));
        crate::activity::record_light_activity(state, record);
        let authoritative_node = commands::build_node_state(state, &node_id)
            .and_then(|node| serde_json::to_string(&node).map_err(Into::into))
            .unwrap_or(accepted_node);
        return ApiResponse::json_ok(format!(r#"{{"nodes":[{}]}}"#, authoritative_node));
    }

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
        if room_profile
            .as_ref()
            .is_some_and(|patch| patch.room_schedule.is_some())
        {
            return ApiResponse::bad_request(
                "Schedule updates must target one room or unassigned light node per request",
            );
        }
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
                .room_profile
                .as_ref()
                .filter(|patch| patch.room_schedule.is_some())
                .map(|_| "room_schedule_config_updated".to_string())
                .or_else(|| {
                    update
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
                })
                .unwrap_or_else(|| "set_light_preferences".to_string());
            let mut record = crate::activity::LightActivityRecord::app(&update.node_id, action_id);
            record.payload = Some(json!({
                "rhythm_enabled": update.rhythm_enabled,
                "standby_enabled": update.standby_enabled,
                "state": update.target_state.map(|state| state.as_api_str()),
                "profile_settings_touched": update.room_profile.is_some(),
                "room_schedule_source": update.room_profile.as_ref()
                    .and_then(|patch| patch.room_schedule.as_ref())
                    .and_then(|schedule| schedule.as_ref())
                    .map(|schedule| if schedule.follows_time() { "follow_time" } else { "wake_sleep_presets" }),
                "status": "accepted",
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
    handle_put_node_profile_overrides_with_precondition(state, body, persist, None, None)
}

pub fn handle_put_node_profile_overrides_with_precondition(
    state: &SharedState,
    body: &Value,
    persist: bool,
    expected_server_instance_id: Option<&str>,
    expected_resource_sha256: Option<&str>,
) -> ApiResponse {
    let guarded = match (expected_server_instance_id, expected_resource_sha256) {
        (Some(server_instance_id), Some(resource_sha256)) => {
            let live_server_instance_id = match state.lock() {
                Ok(locked) => locked.server_instance_id.clone(),
                Err(_) => return ApiResponse::server_error("lock"),
            };
            if live_server_instance_id != server_instance_id {
                return ApiResponse::conflict(
                    "node profile override precondition failed: live server identity changed",
                );
            }
            let live_resource_sha256 = match commands::nodes_state_resource_sha256(state) {
                Ok(hash) => hash,
                Err(error) => return ApiResponse::server_error(error),
            };
            if live_resource_sha256 != resource_sha256 {
                return ApiResponse::conflict(
                    "node profile override precondition failed: live nodes-state hash changed",
                );
            }
            true
        }
        (Some(server_instance_id), None) => {
            let live_server_instance_id = match state.lock() {
                Ok(locked) => locked.server_instance_id.clone(),
                Err(_) => return ApiResponse::server_error("lock"),
            };
            if live_server_instance_id != server_instance_id {
                return ApiResponse::conflict(
                    "node profile override precondition failed: live server identity changed",
                );
            }
            true
        }
        (None, None) => false,
        (None, Some(_)) => {
            return ApiResponse::bad_request(
                "Guarded node profile override writes require expected server identity",
            )
        }
    };
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
        let replace_profile_overrides = body
            .get("replace")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let expected_effective_profile_overrides =
            match parse_expected_profile_overrides_value(body, "body") {
                Ok(value) => value,
                Err(e) => return ApiResponse::bad_request(&e),
            };
        if guarded && expected_effective_profile_overrides.is_none() {
            return ApiResponse::bad_request(
                "Guarded node profile override writes require expected_profile_overrides",
            );
        }
        updates.push(commands::QueuedNodePreferencesPatch {
            node_id: commands::resolve_node_id(state, raw_node_id),
            rhythm_enabled: None,
            disabled: None,
            standby_enabled: None,
            target_state: None,
            room_profile: Some(commands::RoomProfileSettingsPatch {
                profile_overrides: Some(profile_overrides),
                expected_effective_profile_overrides,
                replace_profile_overrides,
                ..Default::default()
            }),
        });
    }

    if guarded {
        for update in &updates {
            let Some(expected) = update
                .room_profile
                .as_ref()
                .and_then(|profile| profile.expected_effective_profile_overrides.as_ref())
            else {
                return ApiResponse::bad_request(
                    "Guarded node profile override writes require expected_profile_overrides",
                );
            };
            let current = match commands::effective_node_profile_overrides(state, &update.node_id) {
                Ok(current) => current,
                Err(error) => return ApiResponse::server_error(error),
            };
            if current != *expected {
                return ApiResponse::conflict(
                    "node profile override precondition failed: live effective overrides changed",
                );
            }
        }
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
        let mut record = crate::activity::LightActivityRecord::app(
            &update.node_id,
            "set_light_profile_overrides",
        );
        record.correlation_id = correlation_id_from_body(body);
        let profile_override_keys = update
            .room_profile
            .as_ref()
            .and_then(|profile| profile.profile_overrides.as_ref())
            .and_then(|profile_overrides| profile_overrides.as_ref())
            .map(|overrides| overrides.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut profile_override_fields = update
            .room_profile
            .as_ref()
            .and_then(|profile| profile.profile_overrides.as_ref())
            .and_then(|profile_overrides| profile_overrides.as_ref())
            .into_iter()
            .flat_map(|overrides| overrides.values())
            .filter_map(Option::as_ref)
            .flat_map(LightProfileNodeOverride::field_names)
            .collect::<Vec<_>>();
        profile_override_fields.sort_unstable();
        profile_override_fields.dedup();
        let clear_profile_overrides = update
            .room_profile
            .as_ref()
            .and_then(|profile| profile.profile_overrides.as_ref())
            .is_some_and(|profile_overrides| profile_overrides.is_none());
        record.payload = Some(json!({
            "profile_overrides_touched": true,
            "profile_override_keys": profile_override_keys,
            "profile_override_fields": profile_override_fields,
            "clear_profile_overrides": clear_profile_overrides,
            "replace_profile_overrides": update
                .room_profile
                .as_ref()
                .is_some_and(|profile| profile.replace_profile_overrides),
            "status": "accepted",
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

struct PairingAttemptGuard {
    state: SharedState,
    hub_type: String,
    reservations: Vec<PairingReservation>,
    resource_activity_fn: Option<crate::state::PairingResourceActivityFn>,
    release_on_drop: bool,
}

struct PairingReservation {
    pairing_slot: String,
    resource_activity: bool,
}

impl Drop for PairingAttemptGuard {
    fn drop(&mut self) {
        if !self.release_on_drop {
            return;
        }
        for reservation in self.reservations.iter().rev() {
            if reservation.resource_activity {
                if let Some(callback) = &self.resource_activity_fn {
                    if let Err(error) = callback(&self.hub_type, &reservation.pairing_slot, false) {
                        log::error!(
                            target: "sys",
                            "Could not release pairing resource {} for {}; keeping the slot reserved: {error:#}",
                            reservation.pairing_slot,
                            self.hub_type
                        );
                        continue;
                    }
                }
            }
            if let Ok(mut state) = self.state.lock() {
                state.finish_pairing(&reservation.pairing_slot);
            }
        }
    }
}

impl PairingAttemptGuard {
    fn keep_reserved(&mut self) {
        self.release_on_drop = false;
    }
}

fn matter_pairing_uses_bluetooth(params: &serde_json::Value) -> bool {
    match params.get("rendezvous").and_then(serde_json::Value::as_str) {
        Some("on_network") => false,
        Some(_) => true,
        None => params
            .get("setup_payload")
            .and_then(serde_json::Value::as_str)
            .map(|payload| {
                payload
                    .trim()
                    .get(..3)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("MT:"))
            })
            // Invalid or legacy requests without enough information stay on
            // the conservative shared-adapter path until integration parsing.
            .unwrap_or(true),
    }
}

fn pairing_slots(
    platform_type: &str,
    hub_type: &str,
    params: &serde_json::Value,
) -> Vec<PairingReservation> {
    if platform_type != "appliance" {
        return vec![PairingReservation {
            pairing_slot: hub_type.to_string(),
            resource_activity: false,
        }];
    }

    match hub_type {
        crate::hub::HubType::MATTER => {
            let mut reservations = vec![PairingReservation {
                // Commissioner serialization is independent from whether the
                // selected rendezvous consumes the Bluetooth adapter.
                pairing_slot: crate::hub::HubType::MATTER.to_string(),
                resource_activity: false,
            }];
            if matter_pairing_uses_bluetooth(params) {
                reservations.push(PairingReservation {
                    pairing_slot: "appliance_bluetooth_adapter".to_string(),
                    resource_activity: true,
                });
            }
            reservations
        }
        crate::hub::HubType::HUE_BLE | crate::hub::HubType::LOCAL_BLE => {
            vec![PairingReservation {
                pairing_slot: "appliance_bluetooth_adapter".to_string(),
                resource_activity: false,
            }]
        }
        _ => vec![PairingReservation {
            pairing_slot: hub_type.to_string(),
            resource_activity: false,
        }],
    }
}

fn pairing_conflict_message(reservation: &PairingReservation, hub_type: &str) -> String {
    if reservation.pairing_slot == "appliance_bluetooth_adapter" {
        "Bluetooth pairing is already in progress on this appliance".to_string()
    } else {
        format!("Pairing already in progress for {hub_type}")
    }
}

fn try_acquire_pairing_guard(
    state: &SharedState,
    hub_type: &str,
    params: &serde_json::Value,
) -> Result<PairingAttemptGuard, ApiResponse> {
    try_acquire_pairing_guard_with(state, hub_type, |platform_type| {
        pairing_slots(platform_type, hub_type, params)
    })
}

fn try_acquire_factory_reset_guard(
    state: &SharedState,
) -> Result<PairingAttemptGuard, ApiResponse> {
    try_acquire_pairing_guard_with(state, crate::hub::HubType::MATTER, |platform_type| {
        if platform_type == "appliance" {
            vec![
                PairingReservation {
                    pairing_slot: crate::hub::HubType::MATTER.to_string(),
                    resource_activity: false,
                },
                PairingReservation {
                    pairing_slot: "appliance_bluetooth_adapter".to_string(),
                    resource_activity: false,
                },
            ]
        } else {
            Vec::new()
        }
    })
}

fn try_acquire_pairing_guard_with(
    state: &SharedState,
    hub_type: &str,
    reservation_plan: impl FnOnce(&str) -> Vec<PairingReservation>,
) -> Result<PairingAttemptGuard, ApiResponse> {
    let mut state_guard = match state.lock() {
        Ok(state) => state,
        Err(_) => return Err(ApiResponse::server_error("lock")),
    };
    let reservations = reservation_plan(state_guard.platform_type);
    if let Some(conflict) = reservations.iter().find(|reservation| {
        state_guard
            .pairing_in_progress
            .contains(&reservation.pairing_slot)
    }) {
        return Err(ApiResponse::conflict(&pairing_conflict_message(
            conflict, hub_type,
        )));
    }
    for reservation in &reservations {
        let acquired = state_guard.begin_pairing(&reservation.pairing_slot);
        debug_assert!(acquired);
    }
    let resource_activity_fn = state_guard.pairing_resource_activity_fn.clone();
    drop(state_guard);

    let mut guard = PairingAttemptGuard {
        state: state.clone(),
        hub_type: hub_type.to_string(),
        reservations,
        resource_activity_fn,
        release_on_drop: true,
    };
    for reservation in &guard.reservations {
        if !reservation.resource_activity {
            continue;
        }
        let Some(callback) = &guard.resource_activity_fn else {
            continue;
        };
        if let Err(error) = callback(hub_type, &reservation.pairing_slot, true) {
            // The platform did not acknowledge the external reservation.
            // Release every logical slot acquired atomically above so an
            // adapter admission failure cannot poison Matter serialization.
            if let Err(rollback_error) = callback(hub_type, &reservation.pairing_slot, false) {
                log::error!(
                    target: "sys",
                    "Could not roll back unacknowledged pairing resource {} for {}: {rollback_error:#}",
                    reservation.pairing_slot,
                    hub_type
                );
            }
            if let Ok(mut state_guard) = guard.state.lock() {
                for acquired in &guard.reservations {
                    state_guard.finish_pairing(&acquired.pairing_slot);
                }
            }
            guard.release_on_drop = false;
            return Err(ApiResponse::server_error(format!(
                "Could not reserve pairing resource for {hub_type}: {error:#}"
            )));
        }
    }
    Ok(guard)
}

fn try_acquire_unpairing_guard(
    state: &SharedState,
    hub_type: &str,
) -> Result<Option<PairingAttemptGuard>, ApiResponse> {
    if !matches!(
        hub_type,
        crate::hub::HubType::HUE_BLE | crate::hub::HubType::LOCAL_BLE
    ) {
        return Ok(None);
    }
    try_acquire_pairing_guard(state, hub_type, &serde_json::Value::Null).map(Some)
}

pub fn handle_pair_device(
    state: &SharedState,
    request: &crate::pairing::PairingRequest,
) -> ApiResponse {
    handle_pair_device_with_context(
        state,
        request,
        crate::pairing::PairingRequestContext::accepted_now(),
    )
}

pub fn handle_pair_device_with_context(
    state: &SharedState,
    request: &crate::pairing::PairingRequest,
    request_context: crate::pairing::PairingRequestContext,
) -> ApiResponse {
    if request.hub_type == crate::hub::HubType::LOCAL_BLE && request.session_id.is_none() {
        return ApiResponse::bad_request("Local Bluetooth pairing requires a session ID");
    }
    if let Some(session_id) = request.session_id.as_deref() {
        if let Err(error) = crate::pairing::validate_pairing_session_id(session_id) {
            return ApiResponse::bad_request(error);
        }
    }

    let durable_reconciliation = request.hub_type == crate::hub::HubType::LOCAL_BLE
        || (request.hub_type == crate::hub::HubType::MATTER
            && request.session_id.is_some()
            && state
                .lock()
                .map(|state| state.storage.is_some())
                .unwrap_or(false));
    if durable_reconciliation {
        let reconcile = match state.lock() {
            Ok(state) => state.reconcile_pairing_results_fn.clone(),
            Err(_) => return ApiResponse::server_error("lock"),
        };
        if let Some(reconcile) = reconcile {
            if let Err(error) = reconcile(state, Some(&request.hub_type)) {
                return ApiResponse::server_error(error);
            }
        }
    }

    let request_fingerprint = match (durable_reconciliation, request.session_id.as_deref()) {
        (true, Some(_)) => {
            match crate::pairing::pairing_request_fingerprint_for_state(
                state,
                &request.hub_type,
                &request.params,
            ) {
                Ok(fingerprint) => Some(fingerprint),
                Err(error) => return ApiResponse::server_error(error),
            }
        }
        _ => None,
    };

    let start_pairing = {
        let Ok(s) = state.lock() else {
            return ApiResponse::server_error("lock");
        };
        s.start_pairing_fn.clone()
    };

    let Some(start_fn) = start_pairing else {
        return ApiResponse::server_error("No pairing support configured");
    };

    let mut pairing_result_lease = None;
    if durable_reconciliation {
        let session_id = request
            .session_id
            .as_deref()
            .expect("durable pairing requires a session ID");
        let fingerprint = request_fingerprint
            .as_deref()
            .expect("session-bound pairing request has a fingerprint");
        match crate::pairing::begin_pairing_result(
            state,
            session_id,
            &request.hub_type,
            fingerprint,
        ) {
            Ok(crate::pairing::BeginPairingResult::Started(lease)) => {
                pairing_result_lease = Some(lease);
            }
            Ok(crate::pairing::BeginPairingResult::Pending(status)) => {
                return match serde_json::to_string(&status) {
                    Ok(json) => ApiResponse::json_status(202, json),
                    Err(error) => ApiResponse::server_error(error),
                };
            }
            Ok(crate::pairing::BeginPairingResult::Terminal(result)) => {
                return match serde_json::to_string(&result) {
                    Ok(json) => ApiResponse::json_ok(json),
                    Err(error) => ApiResponse::server_error(error),
                };
            }
            Ok(crate::pairing::BeginPairingResult::HubTypeConflict) => {
                return ApiResponse::conflict(
                    "Pairing session ID is already used by another hub type",
                );
            }
            Ok(crate::pairing::BeginPairingResult::RequestConflict) => {
                return ApiResponse::conflict(
                    "Pairing session ID is already used by another request",
                );
            }
            Ok(crate::pairing::BeginPairingResult::Cancelled) => {
                return ApiResponse::conflict(
                    "Pairing session ID was already closed before this request started",
                );
            }
            Err(error) => return ApiResponse::server_error(error),
        }
    }

    let _pairing_guard = match try_acquire_pairing_guard(state, &request.hub_type, &request.params)
    {
        Ok(guard) => guard,
        Err(response) => {
            if let (Some(session_id), Some(fingerprint), Some(_)) = (
                request.session_id.as_deref(),
                request_fingerprint.as_deref(),
                pairing_result_lease.as_ref(),
            ) {
                if let Err(error) = crate::pairing::fail_pairing_result_before_start(
                    state,
                    session_id,
                    &request.hub_type,
                    fingerprint,
                    &response.body,
                ) {
                    log::error!(target: "pair", "Could not close unstarted pairing reservation: {error:#}");
                    drop(pairing_result_lease.take());
                    if let Ok(Some(status)) =
                        crate::pairing::lookup_pairing_result(state, session_id)
                    {
                        return match serde_json::to_string(&status) {
                            Ok(json) => ApiResponse::json_status(202, json),
                            Err(error) => ApiResponse::server_error(error),
                        };
                    }
                    return ApiResponse::server_error(error);
                }
                if request.hub_type == crate::hub::HubType::LOCAL_BLE {
                    drop(pairing_result_lease.take());
                    return match crate::pairing::lookup_pairing_result(state, session_id) {
                        Ok(Some(status)) => match status.result {
                            Some(result) => match serde_json::to_string(&result) {
                                Ok(json) => ApiResponse::json_ok(json),
                                Err(error) => ApiResponse::server_error(error),
                            },
                            None => ApiResponse::server_error(
                                "Local Bluetooth pairing admission did not close terminally",
                            ),
                        },
                        Ok(None) => ApiResponse::server_error(
                            "Local Bluetooth pairing admission result disappeared",
                        ),
                        Err(error) => ApiResponse::server_error(error),
                    };
                }
            }
            return response;
        }
    };

    log::info!(
        target: "pair",
        "Pairing request: hub_type={}, params={}",
        request.hub_type,
        logging::summarize_pairing_params_for_log(&request.params)
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
            object.insert(
                "session_id".to_string(),
                serde_json::Value::String(session_id.to_string()),
            );
            if let Some(request_fingerprint) = request_fingerprint.as_deref() {
                object.insert(
                    "request_fingerprint".to_string(),
                    serde_json::Value::String(request_fingerprint.to_string()),
                );
            }
            params_with_session_id = serde_json::Value::Object(object);
            &params_with_session_id
        } else {
            &request.params
        }
    } else {
        &request.params
    };

    match start_fn(state, &request.hub_type, params, request_context) {
        Ok(mut session) => {
            if request.hub_type == crate::hub::HubType::LOCAL_BLE {
                if !matches!(
                    session.status,
                    crate::pairing::PairingStatus::Complete | crate::pairing::PairingStatus::Failed
                ) {
                    return ApiResponse::server_error(
                        "Local Bluetooth pairing returned a non-terminal result",
                    );
                }
                session = match crate::pairing::sanitized_terminal_session_for_delivery(
                    &session,
                    &request.hub_type,
                ) {
                    Ok(session) => session,
                    Err(error) => {
                        log::error!(target: "pair", "Local Bluetooth integration returned an invalid terminal result: {error:#}");
                        // A successful store activation still owns an outbox;
                        // the status path can reconstruct a valid result from
                        // that authority without exposing malformed output.
                        return ApiResponse::server_error(
                            "Local Bluetooth pairing returned an invalid final result",
                        );
                    }
                };
            }
            log::info!(target: "pair", "Pairing result: status={:?} error={:?}", session.status, session.error);
            if session.status == crate::pairing::PairingStatus::Complete {
                // Persist canonical registry changes made during pairing
                if let Ok(s) = state.lock() {
                    commands::persist_canonical(&s);
                }
                // Persist hub registry updates if the integration created any
                commands::persist_registry(state);
            }
            let mut terminal_persistence_error = None;
            if matches!(
                session.status,
                crate::pairing::PairingStatus::Complete | crate::pairing::PairingStatus::Failed
            ) {
                if let (Some(session_id), Some(request_fingerprint)) = (
                    request.session_id.as_deref(),
                    request_fingerprint.as_deref(),
                ) {
                    if let Err(error) = crate::pairing::complete_pairing_result_with_fingerprint(
                        state,
                        session_id,
                        &request.hub_type,
                        request_fingerprint,
                        &session,
                    ) {
                        log::error!(target: "pair", "Could not persist terminal pairing result: {error:#}");
                        terminal_persistence_error = Some(error.to_string());
                        if request.hub_type == crate::hub::HubType::LOCAL_BLE {
                            let warning =
                                "Pairing finished, but its durable confirmation could not be saved"
                                    .to_string();
                            if session.warnings.len() < 32 {
                                session.warnings.push(warning);
                            } else if let Some(last) = session.warnings.last_mut() {
                                *last = warning;
                            }
                        }
                    }
                }
            }
            if request.hub_type == crate::hub::HubType::LOCAL_BLE
                && session.status == crate::pairing::PairingStatus::Failed
                && terminal_persistence_error.is_some()
            {
                // A successful activation has an integration-owned outbox and
                // can be repaired by GET. A failed attempt has no device
                // commit receipt, so never expose an unqueryable terminal SSE
                // or 200 response. The client treats this 500 as ambiguous and
                // retains its recovery pointer until the pending ledger ages
                // into an explicit interrupted failure.
                return ApiResponse::server_error(
                    "Pairing stopped, but its final status could not be saved",
                );
            }
            crate::pairing::record_pairing_history(
                state,
                crate::pairing::pairing_history_entry_for_pair(
                    &request.hub_type,
                    &request.params,
                    &session,
                ),
            );
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
            crate::pairing::emit_pairing_progress_with_devices_and_failure(
                state,
                &session.hub_type,
                request.session_id.as_deref(),
                session.status.clone(),
                stage,
                message,
                session.device.clone(),
                session.devices.clone(),
                session.warnings.clone(),
                session.error.clone(),
                session.failure_stage,
            );
            if session.status == crate::pairing::PairingStatus::Complete {
                commands::emit_triage_changed(state);
                crate::state::emit_server_event(
                    state,
                    crate::server_event::ServerEvent::NodesChanged,
                );
            }
            match serde_json::to_string(&session) {
                Ok(json) => ApiResponse::json_ok(json),
                Err(e) => ApiResponse::server_error(e),
            }
        }
        Err(e) => {
            let failure_stage = e
                .downcast_ref::<crate::pairing::PairingFailure>()
                .map(crate::pairing::PairingFailure::stage);
            if request.hub_type == crate::hub::HubType::LOCAL_BLE {
                log::error!(
                    target: "pair",
                    "Local Bluetooth pairing failed at stage={}",
                    failure_stage.map_or("unknown", crate::pairing::PairingFailureStage::as_str)
                );
            } else {
                log::error!(target: "pair", "Pairing failed: {}", e);
            }
            let mut failed_session = crate::pairing::PairingSession {
                hub_type: request.hub_type.clone(),
                status: crate::pairing::PairingStatus::Failed,
                device: None,
                devices: Vec::new(),
                error: Some(e.to_string()),
                failure_stage,
                warnings: Vec::new(),
                details: None,
            };
            if request.hub_type == crate::hub::HubType::LOCAL_BLE {
                failed_session = match crate::pairing::sanitized_terminal_session_for_delivery(
                    &failed_session,
                    &request.hub_type,
                ) {
                    Ok(session) => session,
                    Err(_) => crate::pairing::PairingSession {
                        hub_type: request.hub_type.clone(),
                        status: crate::pairing::PairingStatus::Failed,
                        device: None,
                        devices: Vec::new(),
                        error: Some("Local Bluetooth pairing failed".to_string()),
                        failure_stage,
                        warnings: Vec::new(),
                        details: None,
                    },
                };
            }
            let mut terminal_persisted = true;
            if let (Some(session_id), Some(request_fingerprint)) = (
                request.session_id.as_deref(),
                request_fingerprint.as_deref(),
            ) {
                if let Err(persist_error) = crate::pairing::complete_pairing_result_with_fingerprint(
                    state,
                    session_id,
                    &request.hub_type,
                    request_fingerprint,
                    &failed_session,
                ) {
                    log::error!(target: "pair", "Could not persist failed pairing result: {persist_error:#}");
                    terminal_persisted = false;
                }
            }
            crate::pairing::record_pairing_history(
                state,
                crate::pairing::pairing_history_entry_for_pair(
                    &request.hub_type,
                    &request.params,
                    &failed_session,
                ),
            );
            if request.hub_type != crate::hub::HubType::LOCAL_BLE || terminal_persisted {
                crate::pairing::emit_pairing_progress_with_devices_and_failure(
                    state,
                    &request.hub_type,
                    request.session_id.as_deref(),
                    crate::pairing::PairingStatus::Failed,
                    crate::pairing::PairingStage::Failed,
                    "Pairing failed",
                    None,
                    Vec::new(),
                    Vec::new(),
                    failed_session.error.clone(),
                    failed_session.failure_stage,
                );
            }
            if request.hub_type == crate::hub::HubType::LOCAL_BLE {
                ApiResponse::server_error("Local Bluetooth pairing failed")
            } else {
                ApiResponse::server_error(e)
            }
        }
    }
}

pub fn handle_get_pair_device(state: &SharedState, session_id: &str) -> ApiResponse {
    if let Err(error) = crate::pairing::validate_pairing_session_id(session_id) {
        return ApiResponse::bad_request(error);
    }
    let reconcile = match state.lock() {
        Ok(state) => state.reconcile_pairing_results_fn.clone(),
        Err(_) => return ApiResponse::server_error("lock"),
    };
    if let Some(reconcile) = reconcile {
        if let Err(error) = reconcile(state, None) {
            return ApiResponse::server_error(error);
        }
    }
    let status = match crate::pairing::lookup_or_tombstone_pairing_result(state, session_id) {
        Ok(Some(status)) => status,
        Ok(None) => crate::pairing::PairingResultStatus {
            session_id: session_id.to_string(),
            state: crate::pairing::PairingResultState::NotFound,
            hub_type: None,
            result: None,
        },
        Err(error) => return ApiResponse::server_error(error),
    };
    let http_status = if status.state == crate::pairing::PairingResultState::NotFound {
        404
    } else {
        200
    };
    match serde_json::to_string(&status) {
        Ok(json) => ApiResponse::json_status(http_status, json),
        Err(error) => ApiResponse::server_error(error),
    }
}

pub fn handle_delete_pair_device(state: &SharedState, session_id: &str) -> ApiResponse {
    if let Err(error) = crate::pairing::validate_pairing_session_id(session_id) {
        return ApiResponse::bad_request(error);
    }
    let reconcile = match state.lock() {
        Ok(state) => state.reconcile_pairing_results_fn.clone(),
        Err(_) => return ApiResponse::server_error("lock"),
    };
    if let Some(reconcile) = reconcile {
        if let Err(error) = reconcile(state, None) {
            return ApiResponse::server_error(error);
        }
    }
    match crate::pairing::acknowledge_pairing_result(state, session_id) {
        Ok(crate::pairing::AcknowledgePairingResult::Acknowledged) => ApiResponse::no_content(),
        Ok(crate::pairing::AcknowledgePairingResult::Pending) => {
            ApiResponse::conflict("Pairing result is still pending")
        }
        Ok(crate::pairing::AcknowledgePairingResult::NotFound) => {
            ApiResponse::not_found("Pairing result was not found")
        }
        Err(error) => ApiResponse::server_error(error),
    }
}

// ---------------------------------------------------------------------------
// Device unpairing handler
// ---------------------------------------------------------------------------

pub fn handle_unpair_device(
    state: &SharedState,
    request: &crate::pairing::UnpairingRequest,
) -> ApiResponse {
    let _adapter_guard = match try_acquire_unpairing_guard(state, &request.hub_type) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
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

pub fn handle_get_removed_devices(state: &SharedState) -> ApiResponse {
    match commands::build_removed_devices(state) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_delete_removed_device(
    state: &SharedState,
    id: &str,
    correlation_id: Option<&str>,
) -> ApiResponse {
    let has_matter_endpoint = {
        let Ok(s) = state.lock() else {
            return ApiResponse::server_error("lock");
        };
        let Some(device) = s.canonical_registry.get(id) else {
            return ApiResponse::not_found("Removed device not found");
        };
        if !device.is_removed()
            || device.device_type != rhythm_core::runtime::hub_registry::DeviceType::Light
        {
            return ApiResponse::bad_request("Only archived lights can be permanently deleted");
        }
        device
            .endpoints
            .iter()
            .any(|endpoint| endpoint.hub_key.hub_type.as_str() == "matter")
    };

    // A Matter retry can reactivate the same canonical tombstone. Hold the
    // commissioner slot across the secret purge and archived-only deletion so
    // a successful retry cannot be hard-deleted by an older request.
    let _matter_pairing_guard = if has_matter_endpoint {
        match try_acquire_pairing_guard(
            state,
            crate::hub::HubType::MATTER,
            &serde_json::Value::Null,
        ) {
            Ok(guard) => Some(guard),
            Err(response) => return response,
        }
    } else {
        None
    };

    let (device, purge_recovery) = {
        let Ok(s) = state.lock() else {
            return ApiResponse::server_error("lock");
        };
        let Some(device) = s.canonical_registry.get(id).cloned() else {
            return ApiResponse::not_found("Removed device not found");
        };
        if !device.is_removed()
            || device.device_type != rhythm_core::runtime::hub_registry::DeviceType::Light
        {
            return ApiResponse::bad_request("Only archived lights can be permanently deleted");
        }
        (device, s.purge_pairing_recovery_fn.clone())
    };

    for endpoint in &device.endpoints {
        if endpoint.hub_key.hub_type.as_str() != "matter" {
            continue;
        }
        let Some(purge) = purge_recovery.as_ref() else {
            return ApiResponse::server_error("Matter recovery purge is unavailable");
        };
        if let Err(error) = purge(state, "matter", &endpoint.native_id) {
            return ApiResponse::server_error(error);
        }
    }

    if let Err(error) = commands::do_archived_device_hard_remove(state, id) {
        return ApiResponse::server_error(error);
    }

    let hub_type = device
        .endpoints
        .first()
        .map(|endpoint| endpoint.hub_key.hub_type.as_str())
        .unwrap_or("unknown");
    let mut params = serde_json::json!({
        "device_id": id,
        "device_type": "light",
    });
    if let Some(correlation_id) = correlation_id {
        params["correlation_id"] = serde_json::Value::String(correlation_id.to_string());
    }
    crate::pairing::record_pairing_history(
        state,
        crate::pairing::pairing_history_entry_for_purge(hub_type, &params, id),
    );
    ApiResponse::no_content()
}

pub fn handle_get_canonical_device(state: &SharedState, id: &str) -> ApiResponse {
    match commands::build_canonical_device(state, id) {
        Ok(json) => ApiResponse::json_ok(json),
        Err(e) if e.to_string().contains("Device not found") => {
            ApiResponse::not_found("Device not found")
        }
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
    match commands::do_canonical_assign_room_with_outcome(state, device_id, room_id.as_deref()) {
        Ok(outcome) => match serde_json::to_string(&outcome) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(error) => ApiResponse::server_error(error),
        },
        Err(e) => ApiResponse::server_error(e),
    }
}

pub fn handle_put_device_parent(state: &SharedState, device_id: &str, body: &Value) -> ApiResponse {
    let parent_id = body
        .get("parent_id")
        .and_then(|v| v.as_str())
        .map(|raw_parent_id| commands::resolve_node_id(state, raw_parent_id));
    match commands::do_canonical_assign_room_with_outcome(state, device_id, parent_id.as_deref()) {
        Ok(outcome) => match serde_json::to_string(&outcome) {
            Ok(json) => ApiResponse::json_ok(json),
            Err(error) => ApiResponse::server_error(error),
        },
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

fn assistant_json_response<T: serde::Serialize>(value: &T) -> ApiResponse {
    match serde_json::to_string(value) {
        Ok(body) => ApiResponse::json_ok(body),
        Err(error) => ApiResponse::server_error(error),
    }
}

fn assistant_error_response(error: crate::assistant::LightAssistantError) -> ApiResponse {
    match serde_json::to_string(&error.envelope()) {
        Ok(body) => ApiResponse::json_status(error.status, body),
        Err(serialization_error) => ApiResponse::server_error(serialization_error),
    }
}

pub fn handle_get_assistant_contract(state: &SharedState) -> ApiResponse {
    match crate::assistant::build_light_assistant_contract(state) {
        Ok(contract) => assistant_json_response(&contract),
        Err(error) => assistant_error_response(error),
    }
}

pub fn handle_get_assistant_topology(state: &SharedState) -> ApiResponse {
    match crate::assistant::build_light_assistant_topology_snapshot(state) {
        Ok(snapshot) => assistant_json_response(&snapshot),
        Err(error) => assistant_error_response(error),
    }
}

pub fn handle_post_assistant_move_plan(state: &SharedState, body: &Value) -> ApiResponse {
    let request = match serde_json::from_value(body.clone()) {
        Ok(request) => request,
        Err(_) => {
            return assistant_error_response(crate::assistant::LightAssistantError {
                status: 400,
                code: "invalid_request",
                message: "The move plan request does not match the advertised contract".to_string(),
                retryable: false,
                mutation_may_have_applied: false,
            })
        }
    };
    match crate::assistant::plan_light_assistant_device_room_move(state, request) {
        Ok(plan) => assistant_json_response(&plan),
        Err(error) => assistant_error_response(error),
    }
}

pub fn handle_post_assistant_move_apply(state: &SharedState, body: &Value) -> ApiResponse {
    let request = match serde_json::from_value(body.clone()) {
        Ok(request) => request,
        Err(_) => {
            return assistant_error_response(crate::assistant::LightAssistantError {
                status: 400,
                code: "invalid_request",
                message: "The apply request does not match the advertised contract".to_string(),
                retryable: false,
                mutation_may_have_applied: false,
            })
        }
    };
    match crate::assistant::apply_light_assistant_device_room_move(state, request) {
        Ok(receipt) => assistant_json_response(&receipt),
        Err(error) => assistant_error_response(error),
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
    match commands::do_topology_rename_room(state, room_id, name) {
        Ok(()) => ApiResponse::no_content(),
        Err(error) if error.to_string().contains("Room not found") => {
            ApiResponse::bad_request("Room not found")
        }
        Err(error) => ApiResponse::server_error(error),
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
    let target_ids: Vec<&str> = if let Some(value) = body.get("target_ids") {
        let Some(values) = value.as_array() else {
            return ApiResponse::bad_request("target_ids must be an array");
        };
        let mut targets = Vec::with_capacity(values.len());
        for value in values {
            let Some(target_id) = value.as_str() else {
                return ApiResponse::bad_request("target_ids must contain strings");
            };
            targets.push(target_id);
        }
        targets
    } else if body.get("target_id").is_some_and(|value| value.is_null()) {
        Vec::new()
    } else {
        match body.get("target_id").and_then(|value| value.as_str()) {
            Some(target_id) => vec![target_id],
            None => return ApiResponse::bad_request("Missing target_id"),
        }
    };

    match commands::do_topology_set_control_targets(state, source_id, kind, &target_ids) {
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
        PairedDeviceInfo, PairingRecoverySecret, PairingRequest, PairingSession, PairingStage,
        PairingStatus, UnpairingRequest, UnpairingResult,
    };
    use crate::registry::HubDeviceRegistry;
    use crate::state::{AppState, ObservedPowerSource, ObservedPowerState, WorkItem};
    use crate::topology::HubRoomBinding;
    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_core::{RhythmMode, RoomModeDefault, RoomModeState};
    use rhythm_runtime_api::{
        LightRuntime, RuntimeCapabilities, RuntimeEvent, RuntimeManifest, RuntimePlan,
        RuntimeResult, RuntimeSnapshot,
    };
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn pairing_test_state(label: &str) -> (SharedState, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "rhythm-handler-pairing-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut app = AppState::default();
        app.storage = Some(Arc::new(
            crate::storage::FileStorage::new(path.to_str().unwrap()).unwrap(),
        ));
        (Arc::new(Mutex::new(app)), path)
    }

    // ---- ApiResponse construction ----

    #[test]
    fn api_response_json_ok() {
        let r = ApiResponse::json_ok("{}".to_string());
        assert_eq!(r.status, 200);
        assert_eq!(r.content_type, "application/json");
    }

    #[test]
    fn matter_setup_code_handler_returns_only_integration_owned_secret() {
        let state = test_state();
        state.lock().unwrap().load_pairing_recovery_fn =
            Some(Arc::new(|_, hub_type, native_device_id| {
                assert_eq!(hub_type, "matter");
                assert_eq!(native_device_id, "matter-42-2");
                Ok(Some(PairingRecoverySecret {
                    payload_kind: "qr_code".to_string(),
                    setup_payload: "MT:HANDLER-SECRET".to_string(),
                    captured_at: "2026-08-11T12:00:00Z".to_string(),
                }))
            }));

        let response = handle_get_matter_setup_code(&state, "matter-42-2");

        assert_eq!(response.status, 200);
        let json: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(json["payload_kind"], "qr_code");
        assert_eq!(json["setup_payload"], "MT:HANDLER-SECRET");
        assert_eq!(json["captured_at"], "2026-08-11T12:00:00Z");
    }

    #[test]
    fn matter_setup_code_handler_fails_closed_without_integration_callback() {
        let response = handle_get_matter_setup_code(&test_state(), "matter-42");
        assert_eq!(response.status, 500);
        assert!(!response.body.contains("MT:"));
    }

    fn archived_matter_device(state: &SharedState) -> String {
        let hub_key = HubKey::new(HubType::new("matter"), "local");
        let identity = DiscoveredIdentity {
            native_id: "matter-42-2".to_string(),
            room_id: None,
            room_name: None,
            name: "Archived lamp".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("vid-1-pid-2-node-42")],
            manufacturer: Some("Acme".to_string()),
            model: Some("Lamp".to_string()),
        };
        let mut app = state.lock().unwrap();
        let id = match app.canonical_registry.resolve(&identity, &hub_key, 1) {
            ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("expected new canonical device, got {other:?}"),
        };
        assert!(app.canonical_registry.soft_remove(&id, 2));
        id
    }

    #[test]
    fn removed_devices_report_recovery_availability_without_exposing_secret() {
        let state = test_state();
        let id = archived_matter_device(&state);
        state.lock().unwrap().load_pairing_recovery_fn =
            Some(Arc::new(|_, hub_type, native_device_id| {
                assert_eq!(hub_type, "matter");
                assert_eq!(native_device_id, "matter-42-2");
                Ok(Some(PairingRecoverySecret {
                    payload_kind: "qr_code".to_string(),
                    setup_payload: "MT:PRIVATE-ARCHIVE-SECRET".to_string(),
                    captured_at: "2026-08-27T00:00:00Z".to_string(),
                }))
            }));

        let available = handle_get_removed_devices(&state);
        assert_eq!(available.status, 200);
        let available: Value = serde_json::from_str(&available.body).unwrap();
        assert_eq!(available[0]["id"], id);
        assert_eq!(available[0]["recovery_available"], true);
        assert!(!available.to_string().contains("PRIVATE-ARCHIVE-SECRET"));

        state.lock().unwrap().load_pairing_recovery_fn = Some(Arc::new(|_, _, _| Ok(None)));
        let unavailable: Value =
            serde_json::from_str(&handle_get_removed_devices(&state).body).unwrap();
        assert_eq!(unavailable[0]["recovery_available"], false);
    }

    #[test]
    fn permanent_delete_purges_recovery_before_removing_tombstone() {
        let state = test_state();
        let id = archived_matter_device(&state);
        let purged = Arc::new(AtomicBool::new(false));
        let purged_for_callback = purged.clone();
        state.lock().unwrap().purge_pairing_recovery_fn =
            Some(Arc::new(move |_, hub_type, native_device_id| {
                assert_eq!(hub_type, "matter");
                assert_eq!(native_device_id, "matter-42-2");
                purged_for_callback.store(true, Ordering::SeqCst);
                Ok(())
            }));

        let response = handle_delete_removed_device(&state, &id, Some("purge-journey"));

        assert_eq!(response.status, 204);
        assert!(purged.load(Ordering::SeqCst));
        assert!(state.lock().unwrap().canonical_registry.get(&id).is_none());
    }

    #[test]
    fn permanent_delete_keeps_tombstone_when_recovery_purge_fails() {
        let state = test_state();
        let id = archived_matter_device(&state);
        state.lock().unwrap().purge_pairing_recovery_fn = Some(Arc::new(|_, _, _| {
            Err(anyhow::anyhow!("private store unavailable"))
        }));

        let response = handle_delete_removed_device(&state, &id, None);

        assert_eq!(response.status, 500);
        assert!(state.lock().unwrap().canonical_registry.get(&id).is_some());
    }

    #[test]
    fn permanent_delete_refuses_concurrent_matter_retry() {
        let state = test_state();
        let id = archived_matter_device(&state);
        let purge_called = Arc::new(AtomicBool::new(false));
        let purge_called_for_callback = purge_called.clone();
        {
            let mut app = state.lock().unwrap();
            assert!(app.begin_pairing("matter"));
            app.purge_pairing_recovery_fn = Some(Arc::new(move |_, _, _| {
                purge_called_for_callback.store(true, Ordering::SeqCst);
                Ok(())
            }));
        }

        let response = handle_delete_removed_device(&state, &id, None);

        assert_eq!(response.status, 409);
        assert!(!purge_called.load(Ordering::SeqCst));
        assert!(state.lock().unwrap().canonical_registry.get(&id).is_some());
    }

    #[test]
    fn permanent_delete_does_not_remove_reactivated_identity() {
        let state = test_state();
        let id = archived_matter_device(&state);
        let id_for_callback = id.clone();
        state.lock().unwrap().purge_pairing_recovery_fn = Some(Arc::new(move |state, _, _| {
            state
                .lock()
                .unwrap()
                .canonical_registry
                .get_mut(&id_for_callback)
                .unwrap()
                .removed_at = None;
            Ok(())
        }));

        let response = handle_delete_removed_device(&state, &id, None);

        assert_eq!(response.status, 500);
        let state = state.lock().unwrap();
        let device = state.canonical_registry.get(&id).unwrap();
        assert!(!device.is_removed());
        assert!(response.body.contains("no longer archived"));
    }

    #[test]
    fn ordinary_unpair_refuses_archived_tombstone_before_integration_io() {
        let state = test_state();
        let id = archived_matter_device(&state);
        let integration_called = Arc::new(AtomicBool::new(false));
        let integration_called_for_callback = integration_called.clone();
        state.lock().unwrap().start_unpairing_fn = Some(Arc::new(move |_, _, _| {
            integration_called_for_callback.store(true, Ordering::SeqCst);
            anyhow::bail!("archived target reached integration I/O")
        }));

        let response = handle_unpair_device(
            &state,
            &UnpairingRequest {
                hub_type: "matter".to_string(),
                params: serde_json::json!({
                    "device_id": "matter-42-2",
                    "force": false,
                }),
            },
        );

        assert_eq!(response.status, 500);
        assert!(response.body.contains("owner-only removed-device deletion"));
        assert!(!integration_called.load(Ordering::SeqCst));
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .get(&id)
            .is_some_and(|device| device.is_removed()));
    }

    #[test]
    fn legacy_delete_refuses_archived_tombstone_before_integration_io() {
        let state = test_state();
        let id = archived_matter_device(&state);
        let integration_called = Arc::new(AtomicBool::new(false));
        let integration_called_for_callback = integration_called.clone();
        {
            let mut app = state.lock().unwrap();
            app.platform_type = "appliance";
            app.start_unpairing_fn = Some(Arc::new(move |_, _, _| {
                integration_called_for_callback.store(true, Ordering::SeqCst);
                anyhow::bail!("archived target reached integration I/O")
            }));
        }

        let response = handle_delete_device(&state, &id);

        assert_eq!(response.status, 500);
        assert!(response.body.contains("owner-only removed-device deletion"));
        assert!(!integration_called.load(Ordering::SeqCst));
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .get(&id)
            .is_some_and(|device| device.is_removed()));
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
            s.start_pairing_fn = Some(Arc::new(move |_, hub_type, params, _| {
                assert_eq!(hub_type, "matter");
                *captured_params.lock().unwrap() = Some(params.clone());
                let device = PairedDeviceInfo {
                    device_id: "matter-100".to_string(),
                    name: "Test Matter Bulb".to_string(),
                    device_type: DeviceType::Light,
                    manufacturer: Some("Test".to_string()),
                    model: Some("T100".to_string()),
                };
                Ok(PairingSession {
                    hub_type: hub_type.to_string(),
                    status: PairingStatus::Complete,
                    device: Some(device.clone()),
                    devices: vec![device],
                    error: None,
                    failure_stage: None,
                    warnings: vec!["A second candidate was out of range".to_string()],
                    details: None,
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
                devices,
                warnings,
                ..
            } => {
                assert_eq!(session_id.as_deref(), Some("pair-123"));
                assert_eq!(status, PairingStatus::Complete);
                assert_eq!(stage, PairingStage::Complete);
                assert_eq!(
                    device.as_ref().map(|device| device.device_id.as_str()),
                    Some("matter-100")
                );
                assert_eq!(devices.len(), 1);
                assert_eq!(warnings, ["A second candidate was out of range"]);
            }
            other => panic!("expected pairing completion event, got {:?}", other),
        }

        assert!(state.lock().unwrap().pairing_in_progress.is_empty());
    }

    #[test]
    fn local_ble_pairing_requires_a_reconcilable_session_id() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "local_ble".to_string(),
                session_id: None,
                params: json!({"setup": {"secret": "must-not-run"}}),
            },
        );
        assert_eq!(response.status, 400);
        assert_eq!(
            response.body,
            "Local Bluetooth pairing requires a session ID"
        );
    }

    #[test]
    fn structured_local_ble_failure_survives_status_history_and_sse() {
        let (state, path) = pairing_test_state("structured-local-ble-failure");
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        {
            let mut app = state.lock().unwrap();
            app.event_tx = Some(tx);
            app.start_pairing_fn = Some(Arc::new(|_, _, _, _| {
                Err(anyhow::Error::new(crate::pairing::PairingFailure::new(
                    crate::pairing::PairingFailureStage::CandidateConnect,
                    "Device found, but the Rhythm Box could not connect. Reset it into pairing mode, keep it close, and try again.",
                )))
            }));
        }

        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "local_ble".to_string(),
                session_id: Some("local-pair-structured-failure".to_string()),
                params: json!({"profile_id": "orein.oc02001.button.v1"}),
            },
        );

        // Preserve the existing POST failure contract while the correlated
        // terminal status carries actionable, additive diagnostics.
        assert_eq!(response.status, 500);
        assert_eq!(response.body, "Local Bluetooth pairing failed");
        let status = handle_get_pair_device(&state, "local-pair-structured-failure");
        assert_eq!(status.status, 200);
        let status: crate::pairing::PairingResultStatus =
            serde_json::from_str(&status.body).unwrap();
        let result = status.result.unwrap();
        assert_eq!(
            result.failure_stage,
            Some(crate::pairing::PairingFailureStage::CandidateConnect)
        );
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Reset it into pairing mode")));

        let requested = rx.try_recv().unwrap();
        assert!(matches!(
            requested,
            crate::server_event::ServerEvent::PairingProgress {
                stage: PairingStage::Requested,
                ..
            }
        ));
        let failed = rx.try_recv().unwrap();
        assert!(matches!(
            failed,
            crate::server_event::ServerEvent::PairingProgress {
                status: PairingStatus::Failed,
                failure_stage: Some(crate::pairing::PairingFailureStage::CandidateConnect),
                ..
            }
        ));

        let storage = state.lock().unwrap().storage.clone().unwrap();
        let history = crate::storage::Storage::load_pairing_history(storage.as_ref())
            .unwrap()
            .unwrap();
        assert_eq!(
            history.entries.last().unwrap().failure_stage,
            Some(crate::pairing::PairingFailureStage::CandidateConnect)
        );
        fs::remove_dir_all(path).ok();
    }

    #[test]
    fn terminal_pairing_post_is_idempotent_and_get_is_restart_safe() {
        let (state, path) = pairing_test_state("idempotent");
        let calls = Arc::new(AtomicUsize::new(0));
        {
            let calls = calls.clone();
            state.lock().unwrap().start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                let device = PairedDeviceInfo {
                    device_id: "local-ble-opaque".to_string(),
                    name: "Button".to_string(),
                    device_type: DeviceType::Button,
                    manufacturer: Some("Orein".to_string()),
                    model: Some("OC02001".to_string()),
                };
                Ok(PairingSession {
                    hub_type: "local_ble".to_string(),
                    status: PairingStatus::Complete,
                    device: Some(device.clone()),
                    devices: vec![device],
                    error: None,
                    failure_stage: None,
                    warnings: vec![
                        "Storage acknowledgement is degraded".to_string(),
                        "EA:84:C2:50:A8:65 needed a retry".to_string(),
                    ],
                    details: Some(json!({"setup_payload": "must-not-persist"})),
                })
            }));
        }
        let request = PairingRequest {
            hub_type: "local_ble".to_string(),
            session_id: Some("local-pair-idempotent".to_string()),
            params: json!({"setup": {"ble_identity": "0A0B0C0D0E0F"}}),
        };

        let first_response = handle_pair_device(&state, &request);
        assert_eq!(first_response.status, 200);
        assert!(!first_response.body.contains("must-not-persist"));
        assert!(!first_response.body.contains("EA:84:C2:50:A8:65"));
        let first_body: serde_json::Value = serde_json::from_str(&first_response.body).unwrap();
        assert!(first_body.get("details").is_none());
        assert_eq!(first_body["warnings"][1], "<redacted> needed a retry");
        assert_eq!(handle_pair_device(&state, &request).status, 200);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let restarted = {
            let mut app = AppState::default();
            app.storage = Some(Arc::new(
                crate::storage::FileStorage::new(path.to_str().unwrap()).unwrap(),
            ));
            Arc::new(Mutex::new(app))
        };
        let status = handle_get_pair_device(&restarted, "local-pair-idempotent");
        assert_eq!(status.status, 200);
        let body: serde_json::Value = serde_json::from_str(&status.body).unwrap();
        assert_eq!(body["state"], "terminal");
        assert_eq!(body["result"]["status"], "complete");
        assert_eq!(
            body["result"]["warnings"][0],
            "Storage acknowledgement is degraded"
        );
        assert!(body["result"].get("details").is_none());

        let unknown = handle_get_pair_device(&restarted, "unknown-valid-session");
        assert_eq!(unknown.status, 404);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&unknown.body).unwrap()["state"],
            "not_found"
        );
        assert_eq!(
            handle_get_pair_device(&restarted, "bad/session").status,
            400
        );

        assert_eq!(
            handle_delete_pair_device(&restarted, "local-pair-idempotent").status,
            204
        );
        assert_eq!(
            handle_delete_pair_device(&restarted, "local-pair-idempotent").status,
            204,
            "terminal acknowledgement is idempotent while its tombstone lives"
        );
        assert_eq!(
            handle_get_pair_device(&restarted, "local-pair-idempotent").status,
            404
        );
        fs::remove_dir_all(path).ok();
    }

    #[test]
    fn matter_terminal_pairing_result_is_idempotent_and_queryable() {
        let (state, path) = pairing_test_state("matter-idempotent");
        let calls = Arc::new(AtomicUsize::new(0));
        {
            let calls = calls.clone();
            state.lock().unwrap().start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(PairingSession {
                    hub_type: "matter".to_string(),
                    status: PairingStatus::Failed,
                    device: None,
                    devices: Vec::new(),
                    error: Some("On-network Matter pairing could not reach local IPv6".to_string()),
                    failure_stage: None,
                    warnings: Vec::new(),
                    details: None,
                })
            }));
        }
        let request = PairingRequest {
            hub_type: "matter".to_string(),
            session_id: Some("matter-pair-idempotent".to_string()),
            params: json!({
                "setup_payload": "34970112332",
                "rendezvous": "on_network",
            }),
        };

        let first = handle_pair_device(&state, &request);
        let duplicate = handle_pair_device(&state, &request);

        assert_eq!(first.status, 200);
        assert_eq!(duplicate.status, 200);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let status = handle_get_pair_device(&state, "matter-pair-idempotent");
        assert_eq!(status.status, 200);
        let status: crate::pairing::PairingResultStatus =
            serde_json::from_str(&status.body).unwrap();
        assert_eq!(status.state, crate::pairing::PairingResultState::Terminal);
        assert_eq!(status.result.unwrap().status, PairingStatus::Failed);

        fs::remove_dir_all(path).ok();
    }

    #[test]
    fn an_overtaking_get_prevents_the_later_post_from_starting_work() {
        let (state, path) = pairing_test_state("overtaken-get");
        let calls = Arc::new(AtomicUsize::new(0));
        {
            let calls = calls.clone();
            state.lock().unwrap().start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("integration must not run after a causal 404")
            }));
        }

        assert_eq!(
            handle_get_pair_device(&state, "overtaken-local-session").status,
            404
        );
        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "local_ble".to_string(),
                session_id: Some("overtaken-local-session".to_string()),
                params: json!({"profile_id": "orein.oc02001.button.v1"}),
            },
        );
        assert_eq!(response.status, 409);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        fs::remove_dir_all(path).ok();
    }

    #[test]
    fn acknowledgement_reconciles_authoritative_completion_before_pending_check() {
        let (state, path) = pairing_test_state("ack-reconcile");
        let session_id = "local-ack-reconcile";
        let fingerprint =
            crate::pairing::pairing_request_fingerprint_for_state(&state, "local_ble", &json!({}))
                .unwrap();
        let lease = match crate::pairing::begin_pairing_result(
            &state,
            session_id,
            "local_ble",
            &fingerprint,
        )
        .unwrap()
        {
            crate::pairing::BeginPairingResult::Started(lease) => lease,
            result => panic!("unexpected reservation result: {result:?}"),
        };
        let fingerprint_for_reconcile = fingerprint.clone();
        state.lock().unwrap().reconcile_pairing_results_fn = Some(Arc::new(move |state, _| {
            let device = PairedDeviceInfo {
                device_id: "local-ble-ack-reconcile".to_string(),
                name: "Button".to_string(),
                device_type: DeviceType::Button,
                manufacturer: Some("Orein".to_string()),
                model: Some("OC02001".to_string()),
            };
            crate::pairing::complete_pairing_result_with_fingerprint(
                state,
                session_id,
                "local_ble",
                &fingerprint_for_reconcile,
                &PairingSession {
                    hub_type: "local_ble".to_string(),
                    status: PairingStatus::Complete,
                    device: Some(device.clone()),
                    devices: vec![device],
                    error: None,
                    failure_stage: None,
                    warnings: Vec::new(),
                    details: None,
                },
            )
        }));

        assert_eq!(handle_delete_pair_device(&state, session_id).status, 204);
        drop(lease);
        assert!(crate::pairing::lookup_pairing_result(&state, session_id)
            .unwrap()
            .is_none());
        fs::remove_dir_all(path).ok();
    }

    #[test]
    fn pending_pairing_get_and_duplicate_post_do_not_start_work_twice() {
        let (state, path) = pairing_test_state("pending");
        let fingerprint =
            crate::pairing::pairing_request_fingerprint_for_state(&state, "local_ble", &json!({}))
                .unwrap();
        let _lease = match crate::pairing::begin_pairing_result(
            &state,
            "local-pair-pending",
            "local_ble",
            &fingerprint,
        )
        .unwrap()
        {
            crate::pairing::BeginPairingResult::Started(lease) => lease,
            result => panic!("unexpected reservation result: {result:?}"),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        {
            let calls = calls.clone();
            state.lock().unwrap().start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("must not run")
            }));
        }

        let status = handle_get_pair_device(&state, "local-pair-pending");
        assert_eq!(status.status, 200);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&status.body).unwrap()["state"],
            "pending"
        );
        let duplicate = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "local_ble".to_string(),
                session_id: Some("local-pair-pending".to_string()),
                params: json!({}),
            },
        );
        assert_eq!(duplicate.status, 202);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        fs::remove_dir_all(path).ok();
    }

    #[test]
    fn simultaneous_duplicate_pairing_post_observes_the_same_pending_operation() {
        let (state, path) = pairing_test_state("concurrent-duplicate");
        let calls = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Arc::new(Mutex::new(release_rx));
        {
            let calls = calls.clone();
            let release_rx = release_rx.clone();
            state.lock().unwrap().start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                Ok(PairingSession {
                    hub_type: "local_ble".to_string(),
                    status: PairingStatus::Failed,
                    device: None,
                    devices: Vec::new(),
                    error: Some("fixture complete".to_string()),
                    failure_stage: None,
                    warnings: Vec::new(),
                    details: None,
                })
            }));
        }
        let request = PairingRequest {
            hub_type: "local_ble".to_string(),
            session_id: Some("local-pair-concurrent".to_string()),
            params: json!({"profile_id": "orein.oc02001.button.v1"}),
        };
        let first_state = state.clone();
        let first_request = request.clone();
        let first = std::thread::spawn(move || handle_pair_device(&first_state, &first_request));
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first pairing entered integration");

        let duplicate = handle_pair_device(&state, &request);
        assert_eq!(duplicate.status, 202);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&duplicate.body).unwrap()["state"],
            "pending"
        );

        release_tx.send(()).unwrap();
        assert_eq!(first.join().unwrap().status, 200);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        fs::remove_dir_all(path).ok();
    }

    #[test]
    fn pairing_session_id_cannot_be_reused_for_different_parameters() {
        let (state, path) = pairing_test_state("request-conflict");
        state.lock().unwrap().start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
            unreachable!("a conflicting idempotency key must not start integration work")
        }));
        let first_params = json!({"profile_id": "orein.oc02001.button.v1"});
        let fingerprint = crate::pairing::pairing_request_fingerprint_for_state(
            &state,
            "local_ble",
            &first_params,
        )
        .unwrap();
        let _lease = match crate::pairing::begin_pairing_result(
            &state,
            "local-pair-request-conflict",
            "local_ble",
            &fingerprint,
        )
        .unwrap()
        {
            crate::pairing::BeginPairingResult::Started(lease) => lease,
            result => panic!("unexpected reservation result: {result:?}"),
        };

        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "local_ble".to_string(),
                session_id: Some("local-pair-request-conflict".to_string()),
                params: json!({"profile_id": "different.profile.v1"}),
            },
        );
        assert_eq!(response.status, 409);
        assert_eq!(
            response.body,
            "Pairing session ID is already used by another request"
        );
        fs::remove_dir_all(path).ok();
    }

    #[test]
    fn pair_device_rejects_concurrent_attempt_for_same_hub_type() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let called = Arc::new(AtomicBool::new(false));

        {
            let called = called.clone();
            let mut s = state.lock().unwrap();
            s.pairing_in_progress.insert("matter".to_string());
            s.start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                called.store(true, Ordering::SeqCst);
                Ok(PairingSession {
                    hub_type: "matter".to_string(),
                    status: PairingStatus::Failed,
                    device: None,
                    devices: Vec::new(),
                    error: Some("should not be called".to_string()),
                    failure_stage: None,
                    warnings: Vec::new(),
                    details: None,
                })
            }));
        }

        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "matter".to_string(),
                session_id: Some("pair-busy".to_string()),
                params: json!({ "setup_payload": "34970112332" }),
            },
        );

        assert_eq!(response.status, 409);
        assert_eq!(response.body, "Pairing already in progress for matter");
        assert!(!called.load(Ordering::SeqCst));
        assert!(state.lock().unwrap().pairing_in_progress.contains("matter"));
    }

    #[test]
    fn appliance_serializes_every_ble_pairing_on_the_shared_adapter() {
        let (state, path) = pairing_test_state("shared-adapter");
        let called = Arc::new(AtomicBool::new(false));

        {
            let called = called.clone();
            let mut s = state.lock().unwrap();
            s.platform_type = "appliance";
            s.pairing_in_progress
                .insert("appliance_bluetooth_adapter".to_string());
            s.start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                called.store(true, Ordering::SeqCst);
                unreachable!("shared adapter guard must reject the integration call")
            }));
        }

        for hub_type in ["matter", "hue_ble", "local_ble"] {
            let response = handle_pair_device(
                &state,
                &PairingRequest {
                    hub_type: hub_type.to_string(),
                    session_id: Some(format!("{hub_type}-busy")),
                    params: json!({}),
                },
            );

            if hub_type == "local_ble" {
                assert_eq!(response.status, 200);
                let result: PairingSession = serde_json::from_str(&response.body).unwrap();
                assert_eq!(result.status, PairingStatus::Failed);
                assert_eq!(
                    result.error.as_deref(),
                    Some("Bluetooth pairing is already in progress on this appliance")
                );
            } else {
                assert_eq!(response.status, 409);
                assert_eq!(
                    response.body,
                    "Bluetooth pairing is already in progress on this appliance"
                );
            }
        }
        assert!(!called.load(Ordering::SeqCst));
        assert!(state
            .lock()
            .unwrap()
            .pairing_in_progress
            .contains("appliance_bluetooth_adapter"));
        let status = handle_get_pair_device(&state, "local_ble-busy");
        assert_eq!(status.status, 200);
        let status: crate::pairing::PairingResultStatus =
            serde_json::from_str(&status.body).unwrap();
        assert_eq!(
            status.result.unwrap().status,
            crate::pairing::PairingStatus::Failed,
            "a request observed as pending must close terminally when adapter admission fails"
        );
        fs::remove_dir_all(path).ok();
    }

    #[test]
    fn appliance_on_network_matter_pairing_does_not_reserve_bluetooth() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let activity = Arc::new(Mutex::new(Vec::<(String, String, bool)>::new()));
        {
            let activity_for_hook = activity.clone();
            let state_during_pair = state.clone();
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state
                .pairing_in_progress
                .insert("appliance_bluetooth_adapter".to_string());
            state.pairing_resource_activity_fn =
                Some(Arc::new(move |hub_type, pairing_slot, active| {
                    activity_for_hook.lock().unwrap().push((
                        hub_type.to_string(),
                        pairing_slot.to_string(),
                        active,
                    ));
                    Ok(())
                }));
            state.start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                let state = state_during_pair.lock().unwrap();
                assert!(state.pairing_in_progress.contains("matter"));
                assert!(state
                    .pairing_in_progress
                    .contains("appliance_bluetooth_adapter"));
                drop(state);
                Ok(PairingSession {
                    hub_type: "matter".to_string(),
                    status: PairingStatus::Failed,
                    device: None,
                    devices: Vec::new(),
                    error: Some("test failure".to_string()),
                    failure_stage: None,
                    warnings: Vec::new(),
                    details: None,
                })
            }));
        }

        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "matter".to_string(),
                session_id: None,
                params: json!({
                    "setup_payload": "34970112332",
                    "rendezvous": "on_network",
                }),
            },
        );

        assert_eq!(response.status, 200);
        assert!(activity.lock().unwrap().is_empty());
        let state = state.lock().unwrap();
        assert!(!state.pairing_in_progress.contains("matter"));
        assert!(state
            .pairing_in_progress
            .contains("appliance_bluetooth_adapter"));
    }

    #[test]
    fn appliance_pairing_guard_bridges_resource_activity_for_external_owner() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let activity = Arc::new(Mutex::new(Vec::<(String, String, bool)>::new()));
        {
            let activity_for_hook = activity.clone();
            let activity_during_pair = activity.clone();
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.pairing_resource_activity_fn =
                Some(Arc::new(move |hub_type, pairing_slot, active| {
                    activity_for_hook.lock().unwrap().push((
                        hub_type.to_string(),
                        pairing_slot.to_string(),
                        active,
                    ));
                    Ok(())
                }));
            state.start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                assert_eq!(
                    activity_during_pair.lock().unwrap().as_slice(),
                    &[(
                        "matter".to_string(),
                        "appliance_bluetooth_adapter".to_string(),
                        true,
                    )]
                );
                Ok(PairingSession {
                    hub_type: "matter".to_string(),
                    status: PairingStatus::Failed,
                    device: None,
                    devices: Vec::new(),
                    error: Some("test failure".to_string()),
                    failure_stage: None,
                    warnings: Vec::new(),
                    details: None,
                })
            }));
        }

        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "matter".to_string(),
                session_id: Some("resource-test".to_string()),
                params: json!({
                    "setup_payload": "redacted",
                    "rendezvous": "ble",
                }),
            },
        );
        assert_eq!(response.status, 200);
        assert_eq!(
            activity.lock().unwrap().as_slice(),
            &[
                (
                    "matter".to_string(),
                    "appliance_bluetooth_adapter".to_string(),
                    true,
                ),
                (
                    "matter".to_string(),
                    "appliance_bluetooth_adapter".to_string(),
                    false,
                ),
            ]
        );
    }

    #[test]
    fn pairing_resource_reservation_failure_rolls_back_slot() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        {
            let calls = calls.clone();
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.pairing_resource_activity_fn = Some(Arc::new(move |_, _, active| {
                calls.lock().unwrap().push(active);
                if active {
                    anyhow::bail!("scanner did not stop")
                }
                Ok(())
            }));
            state.start_pairing_fn = Some(Arc::new(|_, _, _, _| {
                panic!("integration must not start without the adapter reservation")
            }));
        }

        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "matter".to_string(),
                session_id: None,
                params: json!({}),
            },
        );
        assert_eq!(response.status, 500);
        assert!(response.body.contains("scanner did not stop"));
        assert_eq!(calls.lock().unwrap().as_slice(), &[true, false]);
        assert!(state.lock().unwrap().pairing_in_progress.is_empty());
    }

    #[test]
    fn pairing_resource_double_failure_still_releases_unacknowledged_slot() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        {
            let calls = calls.clone();
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.pairing_resource_activity_fn = Some(Arc::new(move |_, _, active| {
                calls.lock().unwrap().push(active);
                if active {
                    anyhow::bail!("adapter runtime did not initialize")
                } else {
                    anyhow::bail!("adapter rollback also failed")
                }
            }));
            state.start_pairing_fn = Some(Arc::new(|_, _, _, _| {
                panic!("integration must not start without the adapter reservation")
            }));
        }

        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "matter".to_string(),
                session_id: None,
                params: json!({}),
            },
        );

        assert_eq!(response.status, 500);
        assert_eq!(calls.lock().unwrap().as_slice(), &[true, false]);
        assert!(state.lock().unwrap().pairing_in_progress.is_empty());
    }

    #[test]
    fn pairing_resource_release_failure_keeps_slot_reserved() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.pairing_resource_activity_fn = Some(Arc::new(|_, _, active| {
                if !active {
                    anyhow::bail!("scanner did not resume")
                }
                Ok(())
            }));
            state.start_pairing_fn = Some(Arc::new(|_, _, _, _| {
                Ok(PairingSession {
                    hub_type: "matter".to_string(),
                    status: PairingStatus::Failed,
                    device: None,
                    devices: Vec::new(),
                    error: Some("test failure".to_string()),
                    failure_stage: None,
                    warnings: Vec::new(),
                    details: None,
                })
            }));
        }

        let response = handle_pair_device(
            &state,
            &PairingRequest {
                hub_type: "matter".to_string(),
                session_id: None,
                params: json!({}),
            },
        );
        assert_eq!(response.status, 200);
        assert!(state
            .lock()
            .unwrap()
            .pairing_in_progress
            .contains("appliance_bluetooth_adapter"));
    }

    #[test]
    fn appliance_direct_local_ble_delete_targets_vendor_neutral_hub() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().platform_type = "appliance";

        let decision =
            appliance_delete_unpair_request(&state, "local-ble-0123456789abcdef0123456789abcdef")
                .unwrap();
        let ApplianceDeleteUnpairDecision::Request(request) = decision else {
            panic!("direct local BLE ID should resolve to an unpair request");
        };
        assert_eq!(request.hub_type, HubType::LOCAL_BLE);
        assert_eq!(request.params["hub_address"], "default");
        assert_eq!(
            request.params["device_id"],
            "local-ble-0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn pair_device_releases_in_progress_after_integration_error() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let call_count = Arc::new(AtomicUsize::new(0));

        {
            let call_count = call_count.clone();
            state.lock().unwrap().start_pairing_fn = Some(Arc::new(move |_, _, _, _| {
                call_count.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::anyhow!("commissioning failed"))
            }));
        }

        let request = PairingRequest {
            hub_type: "matter".to_string(),
            session_id: Some("pair-fail".to_string()),
            params: json!({ "setup_payload": "34970112332" }),
        };

        let first = handle_pair_device(&state, &request);
        assert_eq!(first.status, 500);
        assert_eq!(first.body, "commissioning failed");
        assert!(state.lock().unwrap().pairing_in_progress.is_empty());

        let second = handle_pair_device(&state, &request);
        assert_eq!(second.status, 500);
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        assert!(state.lock().unwrap().pairing_in_progress.is_empty());
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
                "motion_activation_enabled": false,
                "room_schedule": {
                    "source": "follow_time",
                    "wake_time": "07:15",
                    "sleep_time": "23:45"
                },
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
        assert_eq!(patch.motion_activation_enabled, Some(Some(false)));
        assert_eq!(
            patch.room_schedule.unwrap().unwrap(),
            rhythm_core::RoomScheduleConfig {
                source: rhythm_core::RoomScheduleSource::FollowTime,
                wake_time: rhythm_core::ModeTransitionTime::parse("07:15").unwrap(),
                sleep_time: rhythm_core::ModeTransitionTime::parse("23:45").unwrap(),
            }
        );
        let profile_overrides = patch.profile_overrides.unwrap().unwrap();
        assert_eq!(
            profile_overrides
                .get("rhythm")
                .and_then(|override_patch| override_patch.as_ref())
                .and_then(|override_patch| override_patch.motion_timeout_secs.as_ref()),
            Some(&rhythm_core::TimerSetting::Fixed { value: 300 })
        );
        assert!(parse_profile_settings_patch(
            Some(&json!({
                "room_schedule": {
                    "source": "follow_time",
                    "wake_time": "07:15",
                    "sleep_time": "07:15"
                }
            })),
            "room_profile"
        )
        .unwrap_err()
        .contains("must differ"));
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
        assert_eq!(
            parse_profile_settings_patch(
                Some(&json!({"motion_activation_enabled": "no"})),
                "room_profile"
            )
            .unwrap_err(),
            "room_profile.motion_activation_enabled must be a boolean or null"
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
            &json!({
                "correlation_id": "global-room-123",
                "nodes": [
                    {"node_id": "room1", "action": "on"},
                    {"node_id": "room2", "action": "on"}
                ]
            }),
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
        let state = state.lock().unwrap();
        assert_eq!(state.light_activity.len(), 2);
        assert!(state.light_activity.iter().all(|event| {
            event.correlation_id.as_deref() == Some("global-room-123")
                && event.fanout_of.as_deref() == Some("global-room-123")
        }));
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
                "correlation_id": "global-room-brightness-123",
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
        let state = state.lock().unwrap();
        assert_eq!(state.light_activity.len(), 2);
        assert!(state.light_activity.iter().all(|event| {
            event.correlation_id.as_deref() == Some("global-room-brightness-123")
                && event.fanout_of.as_deref() == Some("global-room-brightness-123")
        }));
    }

    #[test]
    fn scene_apply_records_app_correlation_for_cloud_activity() {
        let state = handler_state_with_runtime();
        commands::do_scene_upsert(
            &state,
            crate::scenes::SceneDefinition {
                id: "analytics-scene".to_string(),
                name: "Analytics Scene".to_string(),
                description: None,
                source: crate::scenes::SceneSource::User,
                light: Some(crate::scenes::LightSceneLayer {
                    default_transition_ms: None,
                    default_output: Some(crate::scenes::LightSceneOutput {
                        power: crate::scenes::LightScenePower::On,
                        brightness: 50,
                        color: Some(crate::scenes::LightSceneColor::Kelvin { kelvin: 3000 }),
                        transition_ms: None,
                    }),
                    palette: Vec::new(),
                    entries: Vec::new(),
                }),
                extensions: BTreeMap::new(),
            },
        )
        .unwrap();

        let response = handle_post_scene_apply(
            &state,
            "analytics-scene",
            &json!({
                "target_id": "room1",
                "correlation_id": "mood-scene-123"
            }),
        );

        assert_eq!(response.status, 200, "{}", response.body);
        let state = state.lock().unwrap();
        let activity = state.light_activity.first().unwrap();
        assert_eq!(activity.action_id, "apply_scene");
        assert_eq!(activity.correlation_id.as_deref(), Some("mood-scene-123"));
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

    #[test]
    fn motion_activation_returns_authoritative_state_and_correlated_activity() {
        let state = handler_state_with_runtime();
        let response = handle_put_node_motion_activation(
            &state,
            &json!({
                "node_id": "room1",
                "enabled": false,
                "request_id": "motion-request-123"
            }),
            false,
        );

        assert_eq!(response.status, 200);
        let parsed: Value = serde_json::from_str(&response.body).unwrap();
        assert!(parsed.get("queued").is_none());
        assert_eq!(
            parsed["nodes"][0]["profile_settings"]["motion_activation_enabled"],
            false
        );

        let state = state.lock().unwrap();
        let activity = state.light_activity.first().unwrap();
        assert_eq!(activity.action_id, "set_motion_activation");
        assert_eq!(
            activity.correlation_id.as_deref(),
            Some("motion-request-123")
        );
        assert_eq!(activity.change.as_ref().unwrap().before, Some(json!(true)));
        assert_eq!(activity.change.as_ref().unwrap().after, Some(json!(false)));
        assert_eq!(
            activity.payload.as_ref().unwrap()["requested_enabled"],
            false
        );
        assert_eq!(activity.payload.as_ref().unwrap()["status"], "applied");
    }

    #[test]
    fn motion_activation_rejects_missing_or_invalid_values() {
        let state = handler_state_with_runtime();
        let missing =
            handle_put_node_motion_activation(&state, &json!({"node_id": "room1"}), false);
        assert_eq!(missing.status, 400);
        assert!(missing.body.contains("enabled must be a boolean"));

        let invalid = handle_put_node_motion_activation(
            &state,
            &json!({"node_id": "room1", "enabled": "no"}),
            false,
        );
        assert_eq!(invalid.status, 400);
        assert!(invalid.body.contains("enabled must be a boolean"));
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
    fn room_schedule_preference_returns_authoritative_state_without_queue() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let response = handle_put_node_preferences(
            &state,
            &json!({
                "node_id": "room1",
                "profile_settings": {
                    "room_schedule": {
                        "source": "follow_time",
                        "wake_time": "07:15",
                        "sleep_time": "23:45"
                    }
                },
                "request_id": "schedule-request-1"
            }),
            false,
        );

        assert_eq!(response.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(
            parsed["nodes"][0]["profile_settings"]["room_schedule"]["source"],
            "follow_time"
        );
        assert!(rx.recv_timeout(Duration::from_millis(25)).is_err());
    }

    #[test]
    fn unassigned_light_schedule_returns_authoritative_state_without_queue() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let response = handle_put_node_preferences(
            &state,
            &json!({
                "node_id": "standalone-light",
                "profile_settings": {
                    "room_schedule": {
                        "source": "follow_time",
                        "wake_time": "07:15",
                        "sleep_time": "23:45"
                    }
                }
            }),
            false,
        );

        assert_eq!(response.status, 200, "{}", response.body);
        let parsed: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(
            parsed["nodes"][0]["profile_settings"]["room_schedule"]["wake_time"],
            "07:15"
        );
        assert!(rx.recv_timeout(Duration::from_millis(25)).is_err());
    }

    #[test]
    fn room_schedule_output_failure_keeps_accepted_state_authoritative() {
        let state = handler_state_with_runtime_options(true);
        let temp_dir = TestDir::new("room-schedule-output-failure");
        let storage: Arc<dyn crate::storage::Storage> =
            Arc::new(crate::storage::FileStorage::new(temp_dir.path().to_str().unwrap()).unwrap());
        state.lock().unwrap().storage = Some(storage.clone());

        let response = handle_put_node_preferences(
            &state,
            &json!({
                "node_id": "room1",
                "profile_settings": {
                    "room_schedule": {
                        "source": "follow_time",
                        "wake_time": "07:15",
                        "sleep_time": "23:45"
                    }
                },
                "request_id": "room-schedule-save-journey-1"
            }),
            true,
        );

        assert_eq!(response.status, 200, "{}", response.body);
        let immediate: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(
            immediate["nodes"][0]["profile_settings"]["room_schedule"]["wake_time"],
            "07:15"
        );

        let persisted = storage.load_rooms().unwrap();
        let persisted_schedule = persisted
            .get("room1")
            .and_then(|room| room.profile_settings.room_schedule)
            .unwrap();
        assert_eq!(
            persisted_schedule.wake_time,
            rhythm_core::ModeTransitionTime::parse("07:15").unwrap()
        );

        let reconnect = commands::build_node_state(&state, "room1").unwrap();
        assert_eq!(
            reconnect.profile_settings.room_schedule.unwrap().wake_time,
            persisted_schedule.wake_time
        );
        let state = state.lock().unwrap();
        let activity = state
            .light_activity
            .iter()
            .find(|event| event.action_id == "room_schedule_config_updated")
            .unwrap();
        assert_eq!(
            activity.correlation_id.as_deref(),
            Some("room-schedule-save-journey-1")
        );
        assert_eq!(activity.payload.as_ref().unwrap()["status"], "failed");
        assert_eq!(
            activity.payload.as_ref().unwrap()["failure_stage"],
            "output_apply"
        );
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
    fn hue_authority_requires_fresh_complete_room_consent_before_reconcile() {
        let state = test_state();
        let key = HubKey::new(HubType::new(HubType::HUE), "bridge.local");
        let reconcile_calls = Arc::new(AtomicUsize::new(0));
        {
            let mut s = state.lock().unwrap();
            let mut room = crate::topology::TopologyRoom::new("room-office", "Office");
            room.upsert_hub_room_binding(HubRoomBinding {
                hub_key: key.clone(),
                hub_room_id: "hue-office".to_string(),
                control_id: "hue-office-group".to_string(),
                light_device_ids: Vec::new(),
            });
            s.topology.insert_room(room);
            let calls = reconcile_calls.clone();
            s.reconcile_external_controller_authority_fn = Some(Arc::new(move |state, _| {
                // Production integration callbacks own this transaction lock.
                // The API policy lock must be distinct or this try_lock would
                // expose a non-reentrant deadlock.
                let topology_transaction = state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("lock"))?
                    .external_topology_transaction_lock
                    .clone();
                let _topology_transaction = topology_transaction
                    .try_lock()
                    .map_err(|_| anyhow::anyhow!("topology transaction remained locked"))?;
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
        }

        let initial = handle_get_hue_authority(&state);
        assert_eq!(initial.status, 200);
        let initial: Value = serde_json::from_str(&initial.body).unwrap();
        assert_eq!(initial["bridges"][0]["topology_sync_enabled"], false);
        assert_eq!(initial["bridges"][0]["topology_sync_status"], "disabled");
        assert_eq!(initial["bridges"][0]["rooms"][0]["owner"], "unreviewed");
        let revision = initial["bridges"][0]["revision"]
            .as_str()
            .unwrap()
            .to_string();

        let hue_owned = handle_put_hue_authority(
            &state,
            &json!({
                "address": "bridge.local",
                "revision": revision,
                "correlation_id": "hue-authority-test-1",
                "topology_sync_enabled": true,
                "rooms": [{"room_id": "room-office", "owner": "hue"}]
            }),
        );
        assert_eq!(hue_owned.status, 200);
        let hue_owned: Value = serde_json::from_str(&hue_owned.body).unwrap();
        assert_eq!(hue_owned["bridges"][0]["topology_sync_enabled"], true);
        assert_eq!(hue_owned["bridges"][0]["topology_sync_status"], "blocked");
        assert_eq!(reconcile_calls.load(Ordering::SeqCst), 0);

        let stale = handle_put_hue_authority(
            &state,
            &json!({
                "address": "bridge.local",
                "revision": revision,
                "correlation_id": "hue-authority-test-2",
                "rooms": [{"room_id": "room-office", "owner": "rhythm"}]
            }),
        );
        assert_eq!(stale.status, 409);

        let current = handle_get_hue_authority(&state);
        let current: Value = serde_json::from_str(&current.body).unwrap();
        let current_revision = current["bridges"][0]["revision"].as_str().unwrap();
        let rhythm_owned = handle_put_hue_authority(
            &state,
            &json!({
                "address": "bridge.local",
                "revision": current_revision,
                "correlation_id": "hue-authority-test-3",
                "rooms": [{"room_id": "room-office", "owner": "rhythm"}]
            }),
        );
        assert_eq!(rhythm_owned.status, 200);
        let rhythm_owned: Value = serde_json::from_str(&rhythm_owned.body).unwrap();
        assert_eq!(rhythm_owned["bridges"][0]["topology_sync_enabled"], true);
        assert_eq!(
            rhythm_owned["bridges"][0]["topology_sync_status"],
            "pending"
        );
        assert_eq!(reconcile_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn mixed_hue_authority_reconciles_and_applies_effective_owner_per_room() {
        let state = test_state();
        let key = HubKey::new(HubType::new(HubType::HUE), "bridge.local");
        let reconcile_calls = Arc::new(AtomicUsize::new(0));
        let release_calls = Arc::new(AtomicUsize::new(0));
        {
            let mut s = state.lock().unwrap();
            for (room_id, name) in [("room-office", "Office"), ("room-hall", "Hall")] {
                let mut room = crate::topology::TopologyRoom::new(room_id, name);
                room.upsert_hub_room_binding(HubRoomBinding {
                    hub_key: key.clone(),
                    hub_room_id: format!("hue-{room_id}"),
                    control_id: format!("group-{room_id}"),
                    light_device_ids: Vec::new(),
                });
                s.topology.insert_room(room);
            }
            let calls = reconcile_calls.clone();
            s.reconcile_external_controller_authority_fn = Some(Arc::new(move |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
            let calls = release_calls.clone();
            s.release_external_controller_authority_fn = Some(Arc::new(move |_, _, reason| {
                assert_eq!(
                    reason,
                    crate::hub::ExternalControllerReleaseReason::RoomAuthorityChanged
                );
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
        }

        let initial: Value = serde_json::from_str(&handle_get_hue_authority(&state).body).unwrap();
        let mixed = handle_put_hue_authority(
            &state,
            &json!({
                "address": "bridge.local",
                "revision": initial["bridges"][0]["revision"],
                "correlation_id": "hue-authority-mixed",
                "rooms": [
                    {"room_id": "room-office", "owner": "rhythm"},
                    {"room_id": "room-hall", "owner": "hue"}
                ]
            }),
        );
        assert_eq!(mixed.status, 200);
        let mixed: Value = serde_json::from_str(&mixed.body).unwrap();
        assert_eq!(reconcile_calls.load(Ordering::SeqCst), 1);
        assert_eq!(release_calls.load(Ordering::SeqCst), 1);
        assert_eq!(mixed["bridges"][0]["takeover_scope"], "room");
        let mixed_rooms = mixed["bridges"][0]["rooms"].as_array().unwrap();
        assert!(mixed_rooms
            .iter()
            .find(|room| room["room_id"] == "room-office")
            .is_some_and(|room| room["rhythm_automation_enabled"] == true));
        assert!(mixed_rooms
            .iter()
            .find(|room| room["room_id"] == "room-hall")
            .is_some_and(|room| room["rhythm_automation_enabled"] == false));

        let all_rhythm = handle_put_hue_authority(
            &state,
            &json!({
                "address": "bridge.local",
                "revision": mixed["bridges"][0]["revision"],
                "correlation_id": "hue-authority-all-rhythm",
                "rooms": [
                    {"room_id": "room-office", "owner": "rhythm"},
                    {"room_id": "room-hall", "owner": "rhythm"}
                ]
            }),
        );
        assert_eq!(all_rhythm.status, 200);
        assert_eq!(reconcile_calls.load(Ordering::SeqCst), 2);

        let all_rhythm: Value = serde_json::from_str(&all_rhythm.body).unwrap();
        let back_to_hue = handle_put_hue_authority(
            &state,
            &json!({
                "address": "bridge.local",
                "revision": all_rhythm["bridges"][0]["revision"],
                "correlation_id": "hue-authority-back-to-hue",
                "rooms": [
                    {"room_id": "room-office", "owner": "hue"},
                    {"room_id": "room-hall", "owner": "hue"}
                ]
            }),
        );
        assert_eq!(back_to_hue.status, 200);
        assert_eq!(release_calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn failed_reacquisition_never_reopens_an_existing_rhythm_policy() {
        let state = test_state();
        let key = HubKey::new(HubType::new(HubType::HUE), "bridge.local");
        let release_calls = Arc::new(AtomicUsize::new(0));
        let room_id = "room-office".to_string();
        {
            let mut s = state.lock().unwrap();
            let mut room = crate::topology::TopologyRoom::new(room_id.clone(), "Office");
            room.upsert_hub_room_binding(HubRoomBinding {
                hub_key: key.clone(),
                hub_room_id: "hue-office".to_string(),
                control_id: "hue-office-group".to_string(),
                light_device_ids: Vec::new(),
            });
            s.topology.insert_room(room);
            s.topology
                .replace_external_room_automation_decisions(
                    &key,
                    &[(
                        room_id.clone(),
                        crate::topology::ExternalRoomAutomationOwner::Rhythm,
                    )],
                )
                .unwrap();
            s.reconcile_external_controller_authority_fn = Some(Arc::new(|_, _| {
                Err(anyhow::anyhow!("simulated Hue takeover failure"))
            }));
            let calls = release_calls.clone();
            s.release_external_controller_authority_fn = Some(Arc::new(move |_, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
        }

        let initial: Value = serde_json::from_str(&handle_get_hue_authority(&state).body).unwrap();
        let response = handle_put_hue_authority(
            &state,
            &json!({
                "address": "bridge.local",
                "revision": initial["bridges"][0]["revision"],
                "correlation_id": "hue-authority-reacquire-failure",
                "rooms": [{"room_id": room_id, "owner": "rhythm"}]
            }),
        );

        assert_eq!(response.status, 500);
        assert_eq!(release_calls.load(Ordering::SeqCst), 1);
        let s = state.lock().unwrap();
        assert!(!s.topology.external_hub_has_full_rhythm_consent(&key));
        assert!(!s.rhythm_automation_allowed_for_node("room-office"));
        assert!(s.external_controller_authority_is_ready(&key));
    }

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
        struct EmptyDiscovery;

        impl crate::discovery::HubDiscovery for EmptyDiscovery {
            fn discover_rooms(&self) -> anyhow::Result<Vec<crate::discovery::DiscoveredRoom>> {
                Ok(Vec::new())
            }

            fn discover_devices(&self) -> anyhow::Result<Vec<crate::discovery::DiscoveredDevice>> {
                Ok(Vec::new())
            }
        }

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
                        discovery: Some(Arc::new(EmptyDiscovery)),
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
        handler_state_with_runtime_options(false)
    }

    fn handler_state_with_runtime_options(fail_schedule_output_apply: bool) -> SharedState {
        use crate::hub::{ActiveHub, HubType};
        use rhythm_core::{
            ButtonAction, InputEvent, LightProfileConfig, RoomSnapshot, RuntimeHandle,
        };

        struct HandlerMockRuntime {
            snapshots: std::sync::Mutex<Vec<RoomSnapshot>>,
            fail_schedule_output_apply: bool,
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
                self.snapshots
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|s| s.id == room_id)
                    .cloned()
            }
            fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
                self.snapshots.lock().unwrap().clone()
            }
            fn restore_room_state(&self, room_id: &str, restored: rhythm_core::RestoredRoomState) {
                if let Some(snapshot) = self
                    .snapshots
                    .lock()
                    .unwrap()
                    .iter_mut()
                    .find(|snapshot| snapshot.id == room_id)
                {
                    snapshot.rhythm_enabled = restored.rhythm_enabled;
                    snapshot.disabled = restored.disabled;
                    snapshot.time_offset_minutes = restored.time_offset_minutes;
                    snapshot.brightness_offset = restored.brightness_offset;
                    snapshot.soft_off = restored.soft_off;
                    snapshot.mood_active = restored.mood_active;
                    snapshot.standby_enabled = restored.standby_enabled;
                    snapshot.hard_off = restored.hard_off;
                    snapshot.profile_settings = restored.profile_settings;
                }
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
                if self.fail_schedule_output_apply {
                    anyhow::bail!("simulated schedule output failure");
                }
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
            fail_schedule_output_apply,
            snapshots: std::sync::Mutex::new(vec![
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
            ]),
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
        handler_state_with_canonical_light_for_hub_at(hub_type_name, "local", native_id)
    }

    fn handler_state_with_canonical_light_for_hub_at(
        hub_type_name: &str,
        hub_address: &str,
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
        let hub_key = HubKey::new(hub_type.clone(), hub_address);
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
        let (state, _registry, canonical_id, room_id, hub_key) =
            handler_state_with_canonical_light();
        assert!(state.lock().unwrap().topology.upsert_managed_room_binding(
            &room_id,
            HubRoomBinding {
                hub_key,
                hub_room_id: "device-1".to_string(),
                control_id: "device-1".to_string(),
                light_device_ids: vec!["device-1".to_string()],
            },
        ));
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
                Ok(())
            })
        });

        let r = handle_post_factory_reset(&state);
        assert_eq!(r.status, 200);
        assert!(invoked.load(Ordering::SeqCst));
    }

    #[test]
    fn post_barrier_factory_reset_failure_invokes_platform_recovery() {
        let state = handler_state_with_runtime();
        let temp_dir = TestDir::new("factory-reset-recovery");
        // remove_file on a directory fails after do_hub_disconnect has crossed
        // the shared reset safety barrier.
        fs::create_dir(temp_dir.path().join("rooms.json")).unwrap();
        let recovery_invoked = Arc::new(AtomicBool::new(false));
        let follow_up_invoked = Arc::new(AtomicBool::new(false));
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.storage = Some(Arc::new(
                crate::storage::FileStorage::new(temp_dir.path().to_str().unwrap()).unwrap(),
            ));
            state.factory_reset_recovery_fn = Some({
                let recovery_invoked = recovery_invoked.clone();
                Arc::new(move || recovery_invoked.store(true, Ordering::SeqCst))
            });
            state.after_factory_reset_fn = Some({
                let follow_up_invoked = follow_up_invoked.clone();
                Arc::new(move |_| {
                    follow_up_invoked.store(true, Ordering::SeqCst);
                    Ok(())
                })
            });
        }

        let response = handle_post_factory_reset(&state);

        assert_eq!(response.status, 500);
        assert!(response.body.contains("after reset began"));
        assert!(recovery_invoked.load(Ordering::SeqCst));
        assert!(
            !follow_up_invoked.load(Ordering::SeqCst),
            "normal platform cleanup must not run after an incomplete shared reset"
        );
        assert!(state
            .lock()
            .unwrap()
            .pairing_in_progress
            .contains("appliance_bluetooth_adapter"));
        assert!(state.lock().unwrap().pairing_in_progress.contains("matter"));
    }

    #[test]
    fn appliance_factory_reset_rejects_an_in_flight_bluetooth_lifecycle() {
        let state = handler_state_with_runtime();
        let invoked = Arc::new(AtomicBool::new(false));
        {
            let invoked = invoked.clone();
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state
                .pairing_in_progress
                .insert("appliance_bluetooth_adapter".to_string());
            state.before_factory_reset_fn = Some(Arc::new(move |_| {
                invoked.store(true, Ordering::SeqCst);
                Ok(())
            }));
        }

        let response = handle_post_factory_reset(&state);

        assert_eq!(response.status, 409);
        assert!(!invoked.load(Ordering::SeqCst));
        assert!(state
            .lock()
            .unwrap()
            .pairing_in_progress
            .contains("appliance_bluetooth_adapter"));
    }

    #[test]
    fn appliance_factory_reset_rejects_an_in_flight_on_network_matter_pairing() {
        let state = handler_state_with_runtime();
        let invoked = Arc::new(AtomicBool::new(false));
        {
            let invoked = invoked.clone();
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.pairing_in_progress.insert("matter".to_string());
            state.before_factory_reset_fn = Some(Arc::new(move |_| {
                invoked.store(true, Ordering::SeqCst);
                Ok(())
            }));
        }

        let response = handle_post_factory_reset(&state);

        assert_eq!(response.status, 409);
        assert_eq!(response.body, "Pairing already in progress for matter");
        assert!(!invoked.load(Ordering::SeqCst));
        assert!(state.lock().unwrap().pairing_in_progress.contains("matter"));
    }

    #[test]
    fn appliance_factory_reset_retains_pairing_reservations_until_reboot() {
        let state = handler_state_with_runtime();
        let before_invoked = Arc::new(AtomicBool::new(false));
        let after_invoked = Arc::new(AtomicBool::new(false));
        {
            let before_invoked = before_invoked.clone();
            let after_invoked = after_invoked.clone();
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.before_factory_reset_fn = Some(Arc::new(move |_| {
                before_invoked.store(true, Ordering::SeqCst);
                Ok(())
            }));
            state.after_factory_reset_fn = Some(Arc::new(move |_| {
                after_invoked.store(true, Ordering::SeqCst);
                Ok(())
            }));
        }

        let response = handle_post_factory_reset(&state);

        assert_eq!(response.status, 200);
        assert!(before_invoked.load(Ordering::SeqCst));
        assert!(after_invoked.load(Ordering::SeqCst));
        assert!(state
            .lock()
            .unwrap()
            .pairing_in_progress
            .contains("appliance_bluetooth_adapter"));
        assert!(state.lock().unwrap().pairing_in_progress.contains("matter"));
        let on_network = match try_acquire_pairing_guard(
            &state,
            crate::hub::HubType::MATTER,
            &json!({
                "setup_payload": "34970112332",
                "rendezvous": "on_network",
            }),
        ) {
            Ok(_) => panic!("reboot grace must exclude new on-network Matter pairing"),
            Err(response) => response,
        };
        assert_eq!(on_network.status, 409);
    }

    #[test]
    fn appliance_factory_reset_cleanup_failure_retains_pairing_reservations() {
        let state = handler_state_with_runtime();
        let recovery_invoked = Arc::new(AtomicBool::new(false));
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state.before_factory_reset_fn = Some(Arc::new(|_| Ok(())));
            state.factory_reset_recovery_fn = Some({
                let recovery_invoked = recovery_invoked.clone();
                Arc::new(move || recovery_invoked.store(true, Ordering::SeqCst))
            });
            state.after_factory_reset_fn = Some(Arc::new(|_| {
                anyhow::bail!("persistent Bluetooth bond cleanup failed")
            }));
        }

        let response = handle_post_factory_reset(&state);

        assert_eq!(response.status, 500);
        assert!(response.body.contains("platform cleanup failed"));
        assert!(recovery_invoked.load(Ordering::SeqCst));
        assert!(state
            .lock()
            .unwrap()
            .pairing_in_progress
            .contains("appliance_bluetooth_adapter"));
        assert!(state.lock().unwrap().pairing_in_progress.contains("matter"));
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
                "replace": true,
                "correlation_id": "room-light-settings-123",
                "profile_overrides": {
                    "rhythm": {
                        "min_brightness": 8,
                        "max_brightness": 72,
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
        assert_eq!(event.action_id, "set_light_profile_overrides");
        assert_eq!(
            event.correlation_id.as_deref(),
            Some("room-light-settings-123")
        );
        let payload = event.payload.as_ref().unwrap();
        assert_eq!(payload["profile_overrides_touched"], true);
        assert_eq!(payload["profile_override_keys"], json!(["rhythm"]));
        assert_eq!(
            payload["profile_override_fields"],
            json!(["max_brightness", "min_brightness", "motion_timeout_secs"])
        );
        assert_eq!(payload["clear_profile_overrides"], false);
        assert_eq!(payload["replace_profile_overrides"], true);
        assert_eq!(payload["status"], "accepted");
    }

    #[test]
    fn guarded_node_profile_override_accepts_the_live_nodes_hash_and_queues() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let server_instance_id = state.lock().unwrap().server_instance_id.clone();
        let resource_sha256 = commands::nodes_state_resource_sha256(&state).unwrap();

        let response = handle_put_node_profile_overrides_with_precondition(
            &state,
            &json!({
                "node_id": "standalone-light",
                "replace": true,
                "expected_profile_overrides": {},
                "profile_overrides": {
                    "rhythm": {"min_brightness": 8}
                }
            }),
            false,
            Some(&server_instance_id),
            Some(&resource_sha256),
        );

        assert_eq!(response.status, 200);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::SetNodePreferences { .. }
        ));
    }

    #[test]
    fn target_guarded_node_profile_override_ignores_unrelated_nodes_state_changes() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let server_instance_id = state.lock().unwrap().server_instance_id.clone();
        let stale_resource_sha256 = commands::nodes_state_resource_sha256(&state).unwrap();
        state.lock().unwrap().room_observed_power.insert(
            "standalone-light".to_string(),
            ObservedPowerState::new(true, ObservedPowerSource::Command),
        );
        assert_ne!(
            commands::nodes_state_resource_sha256(&state).unwrap(),
            stale_resource_sha256,
        );

        let response = handle_put_node_profile_overrides_with_precondition(
            &state,
            &json!({
                "node_id": "standalone-light",
                "replace": true,
                "expected_profile_overrides": {},
                "profile_overrides": {
                    "rhythm": {"min_brightness": 8}
                }
            }),
            false,
            Some(&server_instance_id),
            None,
        );

        assert_eq!(response.status, 200);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            WorkItem::SetNodePreferences { .. }
        ));
    }

    #[test]
    fn target_guarded_node_profile_override_rejects_stale_target_state() {
        let state = handler_state_with_runtime();
        let server_instance_id = state.lock().unwrap().server_instance_id.clone();

        let response = handle_put_node_profile_overrides_with_precondition(
            &state,
            &json!({
                "node_id": "standalone-light",
                "replace": true,
                "expected_profile_overrides": {
                    "rhythm": {"min_brightness": 22}
                },
                "profile_overrides": {
                    "rhythm": {"min_brightness": 8}
                }
            }),
            false,
            Some(&server_instance_id),
            None,
        );

        assert_eq!(response.status, 409);
        assert!(response.body.contains("live effective overrides changed"));
    }

    #[test]
    fn queued_node_profile_override_rejects_changed_effective_overrides_at_worker_admission() {
        let state = handler_state_with_runtime();
        let rx = attach_work_queue(&state);
        let queued = handle_put_node_profile_overrides(
            &state,
            &json!({
                "node_id": "standalone-light",
                "replace": true,
                "expected_profile_overrides": {},
                "profile_overrides": {
                    "rhythm": {"min_brightness": 8}
                }
            }),
            false,
        );
        assert_eq!(queued.status, 200);

        let competing_patch = commands::RoomProfileSettingsPatch {
            profile_overrides: Some(Some(BTreeMap::from([(
                "rhythm".to_string(),
                Some(LightProfileNodeOverride {
                    min_brightness: Some(22),
                    ..Default::default()
                }),
            )]))),
            replace_profile_overrides: true,
            ..Default::default()
        };
        commands::do_node_preferences_set(
            &state,
            "standalone-light",
            None,
            None,
            None,
            None,
            Some(&competing_patch),
            false,
        )
        .unwrap();

        let result = match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            WorkItem::SetNodePreferences {
                node_id,
                rhythm_enabled,
                disabled,
                standby_enabled,
                target_state,
                room_profile,
                persist_after,
                ..
            } => commands::do_node_preferences_set(
                &state,
                &node_id,
                rhythm_enabled,
                disabled,
                standby_enabled,
                target_state,
                room_profile.as_ref(),
                persist_after,
            ),
            _ => panic!("expected queued node-preferences work"),
        };

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("node profile override precondition failed"));
        let runtime = state.lock().unwrap().hub_runtime().unwrap();
        let current = runtime.engine_node_snapshot("standalone-light").unwrap();
        assert_eq!(
            current.profile_settings.profile_overrides["rhythm"].min_brightness,
            Some(22)
        );
    }

    #[test]
    fn put_device_parent_reports_canonical_and_projection_outcomes() {
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
        assert_eq!(r.status, 200);
        let outcome: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(outcome["schema_version"], 1);
        assert_eq!(outcome["canonical_committed"], true);
        assert_eq!(outcome["projection_status"], "not_applicable");

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
    fn put_topology_node_control_accepts_target_ids() {
        let state = test_state();
        let (source_id, target_a, target_b) = {
            let mut state = state.lock().unwrap();
            let source_id = state.topology.create_room("Source");
            let target_a = state.topology.create_room("Target A");
            let target_b = state.topology.create_room("Target B");
            (source_id, target_a, target_b)
        };

        let r = handle_put_topology_node_control(
            &state,
            &source_id,
            "motion",
            &json!({"target_ids": [target_a.clone(), target_b.clone()]}),
        );
        assert_eq!(r.status, 204);
        assert!(r.body.is_empty());

        let state = state.lock().unwrap();
        let mut targets = state
            .topology
            .explicit_control_targets(&source_id, &crate::topology::NodeControlKind::Motion);
        targets.sort();
        let mut expected = vec![target_a.as_str(), target_b.as_str()];
        expected.sort();
        assert_eq!(targets, expected);
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
        {
            let mut s = state.lock().unwrap();
            s.mode_configs
                .get_mut(&RhythmMode::Day)
                .unwrap()
                .room_defaults
                .push(RoomModeDefault {
                    room_id: canonical_id.clone(),
                    state: RoomModeState::Active,
                });
            s.mode_configs
                .get_mut(&RhythmMode::Sleep)
                .unwrap()
                .room_defaults
                .push(RoomModeDefault {
                    room_id: canonical_id.clone(),
                    state: RoomModeState::HardOff,
                });
        }

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
        assert!(s.mode_configs().into_iter().all(|config| config
            .room_defaults
            .iter()
            .all(|default| default.room_id != canonical_id)));
        drop(s);

        let reg = registry.lock().unwrap();
        assert!(reg.get_light_entities("device-1").is_empty());
        assert!(!reg.rooms().iter().any(|room| room.id == "device-1"));
    }

    #[test]
    fn delete_device_on_appliance_matter_gracefully_unpairs_before_local_cleanup() {
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
                    hub_address: Some("local".to_string()),
                    status: PairingStatus::Complete,
                    device_id: Some("matter-100".to_string()),
                    error: None,
                    completion_scope: None,
                    warning: None,
                })
            }));
        }

        let r = handle_delete_device(&state, &canonical_id);
        assert_eq!(r.status, 204);

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "matter");
        assert_eq!(recorded[0].1["device_id"], "matter-100");
        assert_eq!(recorded[0].1["hub_address"], "local");
        assert_eq!(recorded[0].1["force"], false);
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
    fn delete_device_on_appliance_routes_bridge_only_endpoint_to_exact_hub() {
        let (state, _registry, canonical_id, _room_id, hub_key) =
            handler_state_with_canonical_light_for_hub_at("hue", "192.0.2.10", "hue-device-a");
        let calls = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
        {
            let calls = calls.clone();
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state
                .hub_capabilities
                .push(crate::hub::HubIntegrationCapability {
                    hub_type: "hue".to_string(),
                    configurable: true,
                    device_onboarding_methods: Vec::new(),
                    device_profiles: Vec::new(),
                    supports_unpairing: true,
                    unpairable_device_types: vec!["light".to_string()],
                    supports_roomless_devices: true,
                    blocks_room_readiness: true,
                });
            state.start_unpairing_fn = Some(Arc::new(move |_, hub_type, params| {
                calls
                    .lock()
                    .unwrap()
                    .push((hub_type.to_string(), params.clone()));
                Ok(UnpairingResult {
                    hub_type: "hue".to_string(),
                    hub_address: Some("192.0.2.10".to_string()),
                    status: PairingStatus::Complete,
                    device_id: Some("hue-device-a".to_string()),
                    error: None,
                    completion_scope: None,
                    warning: None,
                })
            }));
        }

        let response = handle_delete_device(&state, &canonical_id);
        assert_eq!(response.status, 204);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.as_slice().len(), 1);
        assert_eq!(calls[0].0, "hue");
        assert_eq!(calls[0].1["device_id"], "hue-device-a");
        assert_eq!(calls[0].1["hub_address"], "192.0.2.10");
        drop(calls);
        assert!(state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key, "hue-device-a")
            .is_none());
    }

    #[test]
    fn delete_device_on_appliance_rejects_ambiguous_merged_endpoints() {
        let (state, _registry, canonical_id, _room_id, hue_key) =
            handler_state_with_canonical_light_for_hub_at("hue", "192.0.2.10", "hue-device-a");
        let ble_key = HubKey::new(HubType::new("hue_ble"), "local");
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state
                .hub_capabilities
                .push(crate::hub::HubIntegrationCapability {
                    hub_type: "hue".to_string(),
                    configurable: true,
                    device_onboarding_methods: Vec::new(),
                    device_profiles: Vec::new(),
                    supports_unpairing: true,
                    unpairable_device_types: vec!["light".to_string()],
                    supports_roomless_devices: true,
                    blocks_room_readiness: true,
                });
            state
                .canonical_registry
                .get_mut(&canonical_id)
                .unwrap()
                .upsert_endpoint(ble_key.clone(), "hue-ble-a".to_string(), 2, None);
            state.start_unpairing_fn = Some(Arc::new(|_, _, _| {
                panic!("ambiguous generic delete must not pick an endpoint")
            }));
        }

        let response = handle_delete_device(&state, &canonical_id);
        assert_eq!(response.status, 409);
        assert!(response.body.contains("endpoint-specific POST /unpair"));
        let state = state.lock().unwrap();
        let device = state.canonical_registry.get(&canonical_id).unwrap();
        assert!(device.endpoint_for_hub(&hue_key).is_some());
        assert!(device.endpoint_for_hub(&ble_key).is_some());
    }

    #[test]
    fn delete_device_rejects_hue_plus_non_unpairable_endpoint() {
        let (state, _registry, canonical_id, _room_id, hue_key) =
            handler_state_with_canonical_light_for_hub_at("hue", "192.0.2.10", "hue-device-a");
        let ha_key = HubKey::new(HubType::new("homeassistant"), "ha.local");
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state
                .hub_capabilities
                .push(crate::hub::HubIntegrationCapability {
                    hub_type: "hue".to_string(),
                    configurable: true,
                    device_onboarding_methods: Vec::new(),
                    device_profiles: Vec::new(),
                    supports_unpairing: true,
                    unpairable_device_types: vec!["light".to_string()],
                    supports_roomless_devices: true,
                    blocks_room_readiness: true,
                });
            state
                .canonical_registry
                .get_mut(&canonical_id)
                .unwrap()
                .upsert_endpoint(ha_key.clone(), "ha-light-a".to_string(), 2, None);
            state.start_unpairing_fn = Some(Arc::new(|_, _, _| {
                panic!("generic delete must not partially remove a merged device")
            }));
        }

        let response = handle_delete_device(&state, &canonical_id);

        assert_eq!(response.status, 409);
        assert!(response.body.contains("endpoint-specific POST /unpair"));
        let state = state.lock().unwrap();
        let device = state.canonical_registry.get(&canonical_id).unwrap();
        assert!(device.endpoint_for_hub(&hue_key).is_some());
        assert!(device.endpoint_for_hub(&ha_key).is_some());
    }

    #[test]
    fn delete_device_rejects_an_inactive_second_endpoint() {
        let (state, _registry, canonical_id, _room_id, hue_key) =
            handler_state_with_canonical_light_for_hub_at("hue", "192.0.2.10", "hue-device-a");
        let inactive_key = HubKey::new(HubType::new("hue_ble"), "local");
        {
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            let device = state.canonical_registry.get_mut(&canonical_id).unwrap();
            device.upsert_endpoint(inactive_key.clone(), "hue-ble-a".to_string(), 2, None);
            device
                .endpoints
                .iter_mut()
                .find(|endpoint| endpoint.hub_key == inactive_key)
                .unwrap()
                .active = false;
            state.start_unpairing_fn = Some(Arc::new(|_, _, _| {
                panic!("generic delete must require explicit inactive endpoint handling")
            }));
        }

        let response = handle_delete_device(&state, &canonical_id);

        assert_eq!(response.status, 409);
        let state = state.lock().unwrap();
        let device = state.canonical_registry.get(&canonical_id).unwrap();
        assert!(device.endpoint_for_hub(&hue_key).is_some());
        assert!(device.endpoint_for_hub(&inactive_key).is_some());
    }

    #[test]
    fn unpair_completion_removes_only_the_exact_endpoint_from_a_merged_device() {
        let (state, _registry, canonical_id, _room_id, hue_key) =
            handler_state_with_canonical_light_for_hub_at("hue", "192.0.2.10", "hue-device-a");
        let ble_key = HubKey::new(HubType::new("hue_ble"), "local");
        {
            let mut state = state.lock().unwrap();
            state
                .canonical_registry
                .get_mut(&canonical_id)
                .unwrap()
                .upsert_endpoint(ble_key.clone(), "hue-ble-a".to_string(), 2, None);
            state.start_unpairing_fn = Some(Arc::new(|_, _, _| {
                Ok(UnpairingResult {
                    hub_type: "hue".to_string(),
                    hub_address: Some("192.0.2.10".to_string()),
                    status: PairingStatus::Complete,
                    device_id: Some("hue-device-a".to_string()),
                    error: None,
                    completion_scope: None,
                    warning: None,
                })
            }));
        }

        let response = handle_unpair_device(
            &state,
            &UnpairingRequest {
                hub_type: "hue".to_string(),
                params: json!({
                    "device_id": canonical_id,
                    "hub_address": "192.0.2.10"
                }),
            },
        );
        assert_eq!(response.status, 200);
        let state = state.lock().unwrap();
        let device = state.canonical_registry.get(&canonical_id).unwrap();
        assert!(device.endpoint_for_hub(&hue_key).is_none());
        assert_eq!(
            device
                .endpoint_for_hub(&ble_key)
                .map(|endpoint| endpoint.native_id.as_str()),
            Some("hue-ble-a")
        );
    }

    #[test]
    fn appliance_rejects_hue_ble_unpair_while_matter_owns_adapter_slot() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let called = Arc::new(AtomicBool::new(false));
        {
            let called = called.clone();
            let mut state = state.lock().unwrap();
            state.platform_type = "appliance";
            state
                .pairing_in_progress
                .insert("appliance_bluetooth_adapter".to_string());
            state.start_unpairing_fn = Some(Arc::new(move |_, _, _| {
                called.store(true, Ordering::SeqCst);
                unreachable!("shared adapter guard must reject Hue BLE removal")
            }));
        }

        let response = handle_unpair_device(
            &state,
            &UnpairingRequest {
                hub_type: "hue_ble".to_string(),
                params: json!({"device_id": "hue-ble-a", "force": false}),
            },
        );

        assert_eq!(response.status, 409);
        assert_eq!(
            response.body,
            "Bluetooth pairing is already in progress on this appliance"
        );
        assert!(!called.load(Ordering::SeqCst));
        assert!(state
            .lock()
            .unwrap()
            .pairing_in_progress
            .contains("appliance_bluetooth_adapter"));
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
                    hub_address: Some("local".to_string()),
                    status: PairingStatus::Complete,
                    device_id: Some("device-1".to_string()),
                    error: None,
                    completion_scope: None,
                    warning: None,
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

    #[test]
    fn archive_commit_failure_records_failed_instead_of_archived() {
        let (state, _registry, canonical_id, _room_id, _hub_key) =
            handler_state_with_canonical_light();
        let path = std::env::temp_dir().join(format!(
            "rhythm-handler-archive-failure-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = Arc::new(crate::storage::FileStorage::new(path.to_str().unwrap()).unwrap());
        std::fs::create_dir(path.join("authority_state.json")).unwrap();
        {
            let mut app = state.lock().unwrap();
            app.storage = Some(storage.clone());
            app.start_unpairing_fn = Some(Arc::new(|_, _, _| {
                Ok(UnpairingResult {
                    hub_type: "mock".to_string(),
                    hub_address: Some("local".to_string()),
                    status: PairingStatus::Complete,
                    device_id: Some("device-1".to_string()),
                    error: None,
                    completion_scope: None,
                    warning: None,
                })
            }));
        }

        let response = handle_unpair_device(
            &state,
            &UnpairingRequest {
                hub_type: "mock".to_string(),
                params: json!({
                    "device_id": "device-1",
                    "force": true,
                    "archive": true
                }),
            },
        );

        assert_eq!(response.status, 500);
        assert!(!state
            .lock()
            .unwrap()
            .canonical_registry
            .get(&canonical_id)
            .unwrap()
            .is_removed());
        let history = crate::storage::Storage::load_pairing_history(storage.as_ref())
            .unwrap()
            .unwrap();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].status, "failed");
        assert_ne!(history.entries[0].status, "archived");

        let _ = std::fs::remove_dir_all(path);
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
