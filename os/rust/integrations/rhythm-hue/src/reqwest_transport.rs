//! Reqwest-based implementation of [`HueTransport`] for server-class targets.
//!
//! Provides a ready-to-use transport so current binaries get Hue
//! communication without writing platform-specific code.

use std::collections::BTreeSet;
use std::mem::ManuallyDrop;
use std::time::Duration;

use crate::api_types::{HueV2GroupedLight, HueV2Light, HueV2Response};
use crate::transport::{
    HueBridgeSearchLight, HueCreatedResource, HueCreatedRoom, HueRoomDefinition, HueTransport,
};
use anyhow::Result;

const HUE_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HUE_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const HUE_IDENTIFY_ACTION: &str = "identify";
// Hue's current vendor envelope spans 50..=1000 mirek (20,000K..=1,000K).
// Per-node capability projection keeps ordinary/unknown bulbs in their
// narrower safe range before commands reach this transport.
const HUE_VENDOR_MIN_MIRED: u32 = 50;
const HUE_VENDOR_MAX_MIRED: u32 = 1000;
const HUE_BRIDGE_SEARCH_TIMEOUT: Duration = Duration::from_secs(45);
const HUE_BRIDGE_SEARCH_POLL_INTERVAL: Duration = Duration::from_secs(1);
const HUE_BRIDGE_REMOVE_TIMEOUT: Duration = Duration::from_secs(15);
const HUE_BRIDGE_REMOVE_POLL_INTERVAL: Duration = Duration::from_millis(500);

fn identify_light_body() -> serde_json::Value {
    serde_json::json!({
        "identify": { "action": HUE_IDENTIFY_ACTION }
    })
}

fn scene_recall_body(transition_ms: Option<u32>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "recall": { "action": "active" }
    });
    if let Some(duration) = transition_ms {
        body["recall"]["duration"] = serde_json::json!(duration);
    }
    body
}

fn room_definition_body(definition: &HueRoomDefinition) -> serde_json::Value {
    let children = definition
        .device_ids
        .iter()
        .map(|device_id| serde_json::json!({"rid": device_id, "rtype": "device"}))
        .collect::<Vec<_>>();
    serde_json::json!({
        "children": children,
        "metadata": {
            "name": definition.name,
            "archetype": definition.archetype,
        }
    })
}

fn room_rename_body(name: &str) -> serde_json::Value {
    serde_json::json!({"metadata": {"name": name}})
}

fn validate_hue_v2_resource_write_response(
    operation: &str,
    body: &str,
    expected_resource_id: &str,
    expected_resource_type: &str,
) -> Result<()> {
    let envelope: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| anyhow::anyhow!("{operation} returned invalid JSON: {error}"))?;
    let data = validate_hue_v2_envelope(operation, &envelope)?;
    match data {
        [receipt]
            if receipt.get("rid").and_then(serde_json::Value::as_str)
                == Some(expected_resource_id)
                && receipt.get("rtype").and_then(serde_json::Value::as_str)
                    == Some(expected_resource_type) =>
        {
            Ok(())
        }
        _ => anyhow::bail!("{operation} did not confirm the expected Hue resource identity"),
    }
}

fn created_hue_v2_resource(
    operation: &str,
    expected_resource_type: &str,
    body: &str,
) -> Result<HueCreatedResource> {
    let envelope: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| anyhow::anyhow!("{operation} returned invalid JSON: {error}"))?;
    let data = validate_hue_v2_envelope(operation, &envelope)?;
    match data {
        [receipt]
            if receipt.get("rtype").and_then(serde_json::Value::as_str)
                == Some(expected_resource_type)
                && receipt
                    .get("rid")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|resource_id| !resource_id.trim().is_empty()) =>
        {
            Ok(HueCreatedResource {
                resource_id: receipt["rid"].as_str().unwrap().to_string(),
                resource_type: expected_resource_type.to_string(),
            })
        }
        _ => anyhow::bail!(
            "{operation} did not return exactly one created {expected_resource_type} identity"
        ),
    }
}

fn normalized_hue_api_path(path: &str) -> Result<&str> {
    let normalized = path.trim_matches('/');
    if normalized.is_empty()
        || normalized.split('/').any(|component| {
            component.is_empty()
                || component == "."
                || component == ".."
                || component.contains(['?', '#', '\\'])
        })
    {
        anyhow::bail!("Invalid Hue API path");
    }
    Ok(normalized)
}

fn hue_v1_error_description(error: &serde_json::Value) -> String {
    let error_type = error
        .get("type")
        .map(serde_json::Value::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    format!("type {error_type}")
}

fn hue_v1_errors(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| entry.get("error"))
            .map(hue_v1_error_description)
            .collect(),
        serde_json::Value::Object(_) => value
            .get("error")
            .map(hue_v1_error_description)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn validate_hue_v1_response(operation: &str, value: &serde_json::Value) -> Result<()> {
    let errors = hue_v1_errors(value);
    if !errors.is_empty() {
        anyhow::bail!("{operation} returned Hue errors: {}", errors.join("; "));
    }
    Ok(())
}

fn validated_v1_write_response(
    method: &str,
    response: reqwest::blocking::Response,
) -> Result<serde_json::Value> {
    let status = response.status();
    let response_body = response.text().unwrap_or_default();
    let operation = format!("{method} Hue V1 resource");
    if !status.is_success() {
        anyhow::bail!("{operation} failed with HTTP status {status}");
    }
    let value = serde_json::from_str(&response_body)
        .map_err(|error| anyhow::anyhow!("{operation} returned invalid JSON: {error}"))?;
    validate_hue_v1_response(&operation, &value)?;
    Ok(value)
}

fn validate_hue_v1_update_success(
    operation: &str,
    value: &serde_json::Value,
    path: &str,
    body: &serde_json::Value,
) -> Result<()> {
    let fields = body
        .as_object()
        .filter(|fields| !fields.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{operation} was given an invalid update body"))?;
    for field in fields.keys() {
        validate_hue_v1_success(operation, value, &format!("/{path}/{field}"))?;
    }
    Ok(())
}

fn validate_hue_v1_post_success(
    operation: &str,
    value: &serde_json::Value,
    path: &str,
) -> Result<()> {
    validate_hue_v1_response(operation, value)?;
    let entries = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("{operation} returned an invalid non-array response"))?;
    let expected_prefix = format!("/{path}/");
    let confirmed = entries.iter().any(|entry| {
        entry
            .get("success")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|success| {
                success
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| !id.trim().is_empty())
                    || success.keys().any(|address| {
                        address == &format!("/{path}") || address.starts_with(&expected_prefix)
                    })
            })
    });
    if confirmed {
        Ok(())
    } else {
        anyhow::bail!("{operation} did not contain a valid Hue success receipt")
    }
}

fn validate_hue_v1_success(
    operation: &str,
    value: &serde_json::Value,
    expected_address: &str,
) -> Result<()> {
    validate_hue_v1_response(operation, value)?;
    let entries = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("{operation} returned an invalid non-array response"))?;
    if entries.iter().any(|entry| {
        entry
            .get("success")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|success| success.contains_key(expected_address))
    }) {
        return Ok(());
    }
    anyhow::bail!("{operation} did not confirm the expected success address")
}

fn validate_hue_v1_delete_success(
    operation: &str,
    value: &serde_json::Value,
    expected_address: &str,
) -> Result<()> {
    validate_hue_v1_response(operation, value)?;
    let entries = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("{operation} returned an invalid non-array response"))?;
    let expected_text = format!("{expected_address} deleted");
    if entries.iter().any(|entry| {
        let Some(success) = entry.get("success") else {
            return false;
        };
        success
            .as_object()
            .is_some_and(|object| object.contains_key(expected_address))
            || success
                .as_str()
                .is_some_and(|message| message.trim().trim_end_matches('.').trim() == expected_text)
    }) {
        return Ok(());
    }
    anyhow::bail!("{operation} did not confirm the expected deletion address")
}

fn hue_v1_is_exact_missing_resource(value: &serde_json::Value, expected_address: &str) -> bool {
    let errors = match value {
        serde_json::Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| entry.get("error"))
            .collect::<Vec<_>>(),
        serde_json::Value::Object(_) => value.get("error").into_iter().collect(),
        _ => Vec::new(),
    };
    !errors.is_empty()
        && errors.iter().all(|error| {
            let missing_type = error
                .get("type")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|error_type| error_type == 3)
                || error
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|error_type| error_type == "3");
            missing_type
                && error.get("address").and_then(serde_json::Value::as_str)
                    == Some(expected_address)
        })
}

fn can_tolerate_missing_v1_light(
    value: &serde_json::Value,
    expected_address: &str,
    had_successful_delete: bool,
    v2_device_exists: bool,
) -> bool {
    had_successful_delete
        && !v2_device_exists
        && hue_v1_is_exact_missing_resource(value, expected_address)
}

fn bridge_serial_search_body(serial: &str) -> serde_json::Value {
    serde_json::json!({ "deviceid": [serial] })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NewLightsSnapshot {
    active: bool,
    generation: Option<String>,
    lights: Vec<HueBridgeSearchLight>,
}

fn parse_new_lights(value: &serde_json::Value) -> Result<NewLightsSnapshot> {
    validate_hue_v1_response("GET lights/new", value)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("GET lights/new returned an invalid response"))?;
    let generation = object
        .get("lastscan")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let active = generation
        .as_deref()
        .is_some_and(|lastscan| lastscan.eq_ignore_ascii_case("active"));

    let mut lights = object
        .iter()
        .filter(|(id, _)| id.as_str() != "lastscan")
        .map(|(id, light)| HueBridgeSearchLight {
            legacy_id: id.clone(),
            name: light
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("Hue light {id}")),
        })
        .collect::<Vec<_>>();
    lights.sort_by(|left, right| left.legacy_id.cmp(&right.legacy_id));
    Ok(NewLightsSnapshot {
        active,
        generation,
        lights,
    })
}

fn completed_new_scan(
    before: &NewLightsSnapshot,
    current: &NewLightsSnapshot,
    saw_active: bool,
) -> bool {
    !current.active && (saw_active || current.generation != before.generation)
}

fn validate_hue_v2_envelope<'a>(
    operation: &str,
    value: &'a serde_json::Value,
) -> Result<&'a [serde_json::Value]> {
    let errors = value
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("{operation} returned an invalid V2 response"))?;
    if !errors.is_empty() {
        anyhow::bail!(
            "{operation} returned {} Hue application error(s)",
            errors.len()
        );
    }
    value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow::anyhow!("{operation} returned a V2 response without data"))
}

fn normalized_zigbee_mac(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    (normalized.len() == 16).then_some(normalized)
}

fn hue_uniqueid_mac(value: &str) -> Option<String> {
    if let Some((mac, endpoint)) = value.rsplit_once('-') {
        if endpoint.len() == 2
            && endpoint
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            if let Some(mac) = normalized_zigbee_mac(mac) {
                return Some(mac);
            }
        }
    }
    normalized_zigbee_mac(value)
}

fn zigbee_macs_for_device(
    value: &serde_json::Value,
    v2_device_id: &str,
) -> Result<BTreeSet<String>> {
    let data = validate_hue_v2_envelope("GET zigbee_connectivity", value)?;
    let macs = data
        .iter()
        .filter(|entry| {
            entry
                .pointer("/owner/rtype")
                .and_then(serde_json::Value::as_str)
                == Some("device")
                && entry
                    .pointer("/owner/rid")
                    .and_then(serde_json::Value::as_str)
                    == Some(v2_device_id)
        })
        .filter_map(|entry| {
            entry
                .get("mac_address")
                .and_then(serde_json::Value::as_str)
                .and_then(normalized_zigbee_mac)
        })
        .collect::<BTreeSet<_>>();
    match macs.len() {
        0 => anyhow::bail!("Hue V2 device has no usable Zigbee connectivity MAC address"),
        1 => {}
        count => anyhow::bail!(
            "Hue V2 device has {count} distinct Zigbee connectivity MAC addresses; refusing ambiguous V1 deletion"
        ),
    }
    Ok(macs)
}

fn legacy_light_ids_for_macs(
    value: &serde_json::Value,
    macs: &BTreeSet<String>,
) -> Result<Vec<String>> {
    validate_hue_v1_response("GET lights", value)?;
    let lights = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("GET lights returned an invalid non-object response"))?;
    let mut ids = lights
        .iter()
        .filter_map(|(id, light)| {
            let uniqueid = light.get("uniqueid").and_then(serde_json::Value::as_str)?;
            let mac = hue_uniqueid_mac(uniqueid)?;
            macs.contains(&mac).then(|| id.clone())
        })
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        anyhow::bail!("No Hue V1 lights exactly matched the V2 device Zigbee MAC");
    }
    Ok(ids)
}

fn light_control_body(
    on: bool,
    brightness: Option<u8>,
    kelvin: Option<u16>,
    xy: Option<(f32, f32)>,
    fade_ms: Option<u16>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "on": { "on": on }
    });

    if on {
        if let Some(bri) = brightness {
            let bri_pct = (bri as f32).clamp(1.0, 100.0);
            body["dimming"] = serde_json::json!({ "brightness": bri_pct });
        }

        if let Some((x, y)) = xy {
            body["color"] = serde_json::json!({ "xy": { "x": x, "y": y } });
        } else if let Some(k) = kelvin {
            let mirek = ((1_000_000.0_f32 / k as f32).round() as u32)
                .clamp(HUE_VENDOR_MIN_MIRED, HUE_VENDOR_MAX_MIRED);
            body["color_temperature"] = serde_json::json!({ "mirek": mirek });
        }
    }

    if let Some(ms) = fade_ms {
        body["dynamics"] = serde_json::json!({ "duration": ms });
    }

    body
}

/// Hue bridge transport using `reqwest` with rustls.
///
/// Accepts invalid TLS certificates since Hue bridges use self-signed certs.
/// Uses `ManuallyDrop` + custom `Drop` to avoid panicking when the last `Arc`
/// reference is released on a tokio worker thread (`reqwest::blocking::Client`
/// contains an internal tokio runtime that cannot be dropped in an async context).
pub struct ReqwestHueTransport {
    client: ManuallyDrop<reqwest::blocking::Client>,
    bridge_ip: String,
}

impl Drop for ReqwestHueTransport {
    fn drop(&mut self) {
        // Safety: take ownership of the client and drop it on a dedicated thread
        // so the internal tokio runtime doesn't panic if we're on an async worker.
        let client = unsafe { ManuallyDrop::take(&mut self.client) };
        std::thread::Builder::new()
            .name("reqwest-drop".to_string())
            .spawn(move || drop(client))
            .ok();
    }
}

impl ReqwestHueTransport {
    /// Create a new transport targeting the given bridge IP.
    pub fn new(bridge_ip: &str) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .danger_accept_invalid_certs(true)
            // These blocking requests run while runtime operations hold the engine lock.
            // A dead bridge must fail fast instead of hanging buttons and /api/state.
            .connect_timeout(HUE_HTTP_CONNECT_TIMEOUT)
            .timeout(HUE_HTTP_REQUEST_TIMEOUT)
            .build()?;

        Ok(Self {
            client: ManuallyDrop::new(client),
            bridge_ip: bridge_ip.to_string(),
        })
    }

    /// The base URL for V2 API calls.
    fn base_url(&self) -> String {
        format!("https://{}", self.bridge_ip)
    }
}

impl HueTransport for ReqwestHueTransport {
    fn test_connection(&self, username: &str) -> Result<bool> {
        let url = format!("{}/clip/v2/resource/bridge", self.base_url());
        let resp = self
            .client
            .get(&url)
            .header("hue-application-key", username)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue connection test failed"))?;
        Ok(resp.status().is_success())
    }

    fn warmup_tls(&self) -> Result<()> {
        // Just make a lightweight request to establish the TLS session
        let url = format!("{}/clip/v2/resource/bridge", self.base_url());
        let _ = self.client.get(&url).send();
        Ok(())
    }

    fn set_grouped_light(
        &self,
        username: &str,
        grouped_light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> Result<()> {
        let url = format!(
            "{}/clip/v2/resource/grouped_light/{}",
            self.base_url(),
            grouped_light_id
        );

        let body = light_control_body(on, brightness, kelvin, xy, fade_ms);

        let resp = self
            .client
            .put(&url)
            .header("hue-application-key", username)
            .json(&body)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue grouped-light update request failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "Hue grouped-light update failed with HTTP status {status}"
            ));
        }
        let body = resp.text().unwrap_or_default();
        validate_hue_v2_resource_write_response(
            "Hue grouped-light update",
            &body,
            grouped_light_id,
            "grouped_light",
        )
    }

    fn set_light(
        &self,
        username: &str,
        light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> Result<()> {
        let url = format!("{}/clip/v2/resource/light/{}", self.base_url(), light_id);
        let body = light_control_body(on, brightness, kelvin, xy, fade_ms);

        let resp = self
            .client
            .put(&url)
            .header("hue-application-key", username)
            .json(&body)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue light update request failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "Hue light update failed with HTTP status {status}"
            ));
        }
        let body = resp.text().unwrap_or_default();
        validate_hue_v2_resource_write_response("Hue light update", &body, light_id, "light")
    }

    fn identify_light(&self, username: &str, light_id: &str) -> Result<()> {
        let url = format!("{}/clip/v2/resource/light/{}", self.base_url(), light_id);
        let body = identify_light_body();

        let resp = self
            .client
            .put(&url)
            .header("hue-application-key", username)
            .json(&body)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue light identify request failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "Hue light identify failed with HTTP status {status}"
            ));
        }
        let body = resp.text().unwrap_or_default();
        validate_hue_v2_resource_write_response("Hue light identify", &body, light_id, "light")
    }

    fn is_grouped_light_on(&self, username: &str, grouped_light_id: &str) -> Result<bool> {
        let url = format!(
            "{}/clip/v2/resource/grouped_light/{}",
            self.base_url(),
            grouped_light_id
        );

        let resp = self
            .client
            .get(&url)
            .header("hue-application-key", username)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue grouped-light read request failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "Hue grouped-light read failed with HTTP status {status}"
            ));
        }

        let envelope: HueV2Response<HueV2GroupedLight> = resp
            .json()
            .map_err(|_| anyhow::anyhow!("Hue grouped-light read returned invalid JSON"))?;
        Ok(envelope
            .data
            .first()
            .and_then(|gl| gl.on.as_ref())
            .map(|s| s.on)
            .unwrap_or(false))
    }

    fn is_light_on(&self, username: &str, light_id: &str) -> Result<bool> {
        let url = format!("{}/clip/v2/resource/light/{}", self.base_url(), light_id);

        let resp = self
            .client
            .get(&url)
            .header("hue-application-key", username)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue light read request failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "Hue light read failed with HTTP status {status}"
            ));
        }

        let envelope: HueV2Response<HueV2Light> = resp
            .json()
            .map_err(|_| anyhow::anyhow!("Hue light read returned invalid JSON"))?;
        Ok(envelope
            .data
            .first()
            .and_then(|light| light.on.as_ref())
            .map(|s| s.on)
            .unwrap_or(false))
    }

    fn get_resources(&self, username: &str, resource_type: &str) -> Result<serde_json::Value> {
        let url = format!("{}/clip/v2/resource/{}", self.base_url(), resource_type);

        let resp = self
            .client
            .get(&url)
            .header("hue-application-key", username)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue V2 resource read request failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "Hue V2 resource read failed with HTTP status {status}"
            ));
        }

        let value = resp
            .json()
            .map_err(|_| anyhow::anyhow!("Hue V2 resource read returned invalid JSON"))?;
        validate_hue_v2_envelope("GET Hue V2 resource", &value)?;
        Ok(value)
    }

    fn get_all_resources(&self, username: &str) -> Result<serde_json::Value> {
        let url = format!("{}/clip/v2/resource", self.base_url());
        let response = self
            .client
            .get(&url)
            .header("hue-application-key", username)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue V2 inventory read request failed"))?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Hue V2 inventory read failed with HTTP status {status}");
        }
        let value = response
            .json()
            .map_err(|_| anyhow::anyhow!("Hue V2 inventory read returned invalid JSON"))?;
        validate_hue_v2_envelope("GET Hue V2 inventory", &value)?;
        Ok(value)
    }

    fn create_resource(
        &self,
        username: &str,
        resource_type: &str,
        body: &serde_json::Value,
    ) -> Result<HueCreatedResource> {
        let resource_type = normalized_hue_api_path(resource_type)?;
        if resource_type.contains('/') {
            anyhow::bail!("Hue V2 resource type must be one path component");
        }
        let url = format!("{}/clip/v2/resource/{resource_type}", self.base_url());
        let response = self
            .client
            .post(&url)
            .header("hue-application-key", username)
            .json(body)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue V2 resource create request failed"))?;
        let status = response.status();
        let response_body = response.text().unwrap_or_default();
        let operation = format!("POST Hue V2 {resource_type}");
        if !status.is_success() {
            anyhow::bail!("{operation} failed with HTTP status {status}");
        }
        created_hue_v2_resource(&operation, resource_type, &response_body)
    }

    fn update_resource(
        &self,
        username: &str,
        resource_type: &str,
        resource_id: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        let resource_type = normalized_hue_api_path(resource_type)?;
        let resource_id = normalized_hue_api_path(resource_id)?;
        if resource_type.contains('/') || resource_id.contains('/') {
            anyhow::bail!("Hue V2 resource type and ID must each be one path component");
        }
        let url = format!(
            "{}/clip/v2/resource/{resource_type}/{resource_id}",
            self.base_url()
        );
        let response = self
            .client
            .put(&url)
            .header("hue-application-key", username)
            .json(body)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue V2 resource update request failed"))?;
        let status = response.status();
        let response_body = response.text().unwrap_or_default();
        let operation = format!("PUT Hue V2 {resource_type}");
        if !status.is_success() {
            anyhow::bail!("{operation} failed with HTTP status {status}");
        }
        validate_hue_v2_resource_write_response(
            &operation,
            &response_body,
            resource_id,
            resource_type,
        )
    }

    fn delete_resource(
        &self,
        username: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<()> {
        let resource_type = normalized_hue_api_path(resource_type)?;
        let resource_id = normalized_hue_api_path(resource_id)?;
        if resource_type.contains('/') || resource_id.contains('/') {
            anyhow::bail!("Hue V2 resource type and ID must each be one path component");
        }
        let url = format!(
            "{}/clip/v2/resource/{resource_type}/{resource_id}",
            self.base_url()
        );
        let response = self
            .client
            .delete(&url)
            .header("hue-application-key", username)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue V2 resource delete request failed"))?;
        let status = response.status();
        let response_body = response.text().unwrap_or_default();
        let operation = format!("DELETE Hue V2 {resource_type}");
        if !status.is_success() {
            anyhow::bail!("{operation} failed with HTTP status {status}");
        }
        validate_hue_v2_resource_write_response(
            &operation,
            &response_body,
            resource_id,
            resource_type,
        )
    }

    fn get_v1(&self, username: &str, path: &str) -> Result<serde_json::Value> {
        let path = normalized_hue_api_path(path)?;
        let url = format!("{}/api/{username}/{path}", self.base_url());
        let response = self
            .client
            .get(&url)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue V1 resource read request failed"))?;
        let status = response.status();
        let response_body = response.text().unwrap_or_default();
        let operation = "GET Hue V1 resource";
        if !status.is_success() {
            anyhow::bail!("{operation} failed with HTTP status {status}");
        }
        let value = serde_json::from_str(&response_body)
            .map_err(|error| anyhow::anyhow!("{operation} returned invalid JSON: {error}"))?;
        validate_hue_v1_response(&operation, &value)?;
        Ok(value)
    }

    fn post_v1(
        &self,
        username: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let path = normalized_hue_api_path(path)?;
        let url = format!("{}/api/{username}/{path}", self.base_url());
        let response = self
            .client
            .post(&url)
            .json(body)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue V1 resource create request failed"))?;
        let value = validated_v1_write_response("POST", response)?;
        validate_hue_v1_post_success("POST Hue V1 resource", &value, path)?;
        Ok(value)
    }

    fn put_v1(
        &self,
        username: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let path = normalized_hue_api_path(path)?;
        let url = format!("{}/api/{username}/{path}", self.base_url());
        let response = self
            .client
            .put(&url)
            .json(body)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue V1 resource update request failed"))?;
        let value = validated_v1_write_response("PUT", response)?;
        validate_hue_v1_update_success("PUT Hue V1 resource", &value, path, body)?;
        Ok(value)
    }

    fn delete_v1(&self, username: &str, path: &str) -> Result<serde_json::Value> {
        let path = normalized_hue_api_path(path)?;
        let url = format!("{}/api/{username}/{path}", self.base_url());
        let response = self
            .client
            .delete(&url)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue V1 resource delete request failed"))?;
        let value = validated_v1_write_response("DELETE", response)?;
        validate_hue_v1_delete_success("DELETE Hue V1 resource", &value, &format!("/{path}"))?;
        Ok(value)
    }

    fn search_new_lights(&self, username: &str, serial: &str) -> Result<Vec<HueBridgeSearchLight>> {
        let url = format!("{}/api/{}/lights", self.base_url(), username);
        let status_url = format!("{}/new", url);
        let response = self
            .client
            .get(&status_url)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue light-search status request failed"))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GET lights/new before search failed with HTTP status {status}");
        }
        let before_value: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
            anyhow::anyhow!("GET lights/new before search returned invalid JSON: {error}")
        })?;
        let before = parse_new_lights(&before_value)?;
        if before.active {
            anyhow::bail!(
                "The Hue Bridge is already searching for lights; wait for that search to finish"
            );
        }

        let response = self
            .client
            .post(&url)
            .json(&bridge_serial_search_body(serial))
            .send()
            .map_err(|_| anyhow::anyhow!("Hue light-search request failed"))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("POST lights search failed with HTTP status {status}");
        }
        let value: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
            anyhow::anyhow!("POST lights search returned invalid JSON: {error}")
        })?;
        validate_hue_v1_success("POST lights search", &value, "/lights")?;

        let deadline = std::time::Instant::now() + HUE_BRIDGE_SEARCH_TIMEOUT;
        let mut saw_active = false;
        loop {
            let response = self
                .client
                .get(&status_url)
                .send()
                .map_err(|_| anyhow::anyhow!("Hue light-search status request failed"))?;
            let status = response.status();
            let body = response.text().unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("GET lights/new failed with HTTP status {status}");
            }
            let value: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
                anyhow::anyhow!("GET lights/new returned invalid JSON: {error}")
            })?;
            let current = parse_new_lights(&value)?;
            if current.active {
                saw_active = true;
            } else if completed_new_scan(&before, &current, saw_active) {
                return Ok(current.lights);
            }
            if std::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "Hue Bridge light search timed out before a new scan generation completed"
                );
            }
            std::thread::sleep(HUE_BRIDGE_SEARCH_POLL_INTERVAL);
        }
    }

    fn device_exists(&self, username: &str, v2_device_id: &str) -> Result<bool> {
        let value = self.get_resources(username, "device")?;
        let data = validate_hue_v2_envelope("GET device", &value)?;
        Ok(data.iter().any(|device| {
            device.get("id").and_then(serde_json::Value::as_str) == Some(v2_device_id)
        }))
    }

    fn remove_light_device(&self, username: &str, v2_device_id: &str) -> Result<()> {
        if !self.device_exists(username, v2_device_id)? {
            return Ok(());
        }

        let zigbee = self.get_resources(username, "zigbee_connectivity")?;
        let macs = zigbee_macs_for_device(&zigbee, v2_device_id)?;

        let lights_url = format!("{}/api/{}/lights", self.base_url(), username);
        let response = self
            .client
            .get(&lights_url)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue light inventory request failed"))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GET lights failed with HTTP status {status}");
        }
        let value: serde_json::Value = serde_json::from_str(&body)
            .map_err(|error| anyhow::anyhow!("GET lights returned invalid JSON: {error}"))?;
        let legacy_ids = legacy_light_ids_for_macs(&value, &macs)?;

        for (index, legacy_id) in legacy_ids.iter().enumerate() {
            let delete_url = format!("{lights_url}/{legacy_id}");
            let response = self
                .client
                .delete(&delete_url)
                .send()
                .map_err(|_| anyhow::anyhow!("Hue light deletion request failed"))?;
            let status = response.status();
            let body = response.text().unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("Hue light deletion failed with HTTP status {status}");
            }
            let value: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
                anyhow::anyhow!("Hue light deletion returned invalid JSON: {error}")
            })?;
            let expected_address = format!("/lights/{legacy_id}");
            if let Err(error) =
                validate_hue_v1_delete_success("Hue light deletion", &value, &expected_address)
            {
                if index > 0 && hue_v1_is_exact_missing_resource(&value, &expected_address) {
                    let v2_device_exists = self.device_exists(username, v2_device_id)?;
                    if can_tolerate_missing_v1_light(
                        &value,
                        &expected_address,
                        true,
                        v2_device_exists,
                    ) {
                        return Ok(());
                    }
                    anyhow::bail!(
                        "A Hue V1 light disappeared during multi-light deletion while its V2 device remained present"
                    );
                }
                return Err(error);
            }
        }

        let deadline = std::time::Instant::now() + HUE_BRIDGE_REMOVE_TIMEOUT;
        loop {
            if !self.device_exists(username, v2_device_id)? {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "Hue Bridge still reports a V2 device after its V1 lights were deleted"
                );
            }
            std::thread::sleep(HUE_BRIDGE_REMOVE_POLL_INTERVAL);
        }
    }

    fn recall_scene(
        &self,
        username: &str,
        scene_id: &str,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let url = format!("{}/clip/v2/resource/scene/{}", self.base_url(), scene_id);
        let body = scene_recall_body(transition_ms);

        let resp = self
            .client
            .put(&url)
            .header("hue-application-key", username)
            .json(&body)
            .send()
            .map_err(|_| anyhow::anyhow!("Hue scene recall request failed"))?;

        let status = resp.status();
        let response_body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Hue scene recall failed with HTTP status {status}"
            ));
        }

        validate_hue_v2_resource_write_response(
            "Hue scene recall",
            &response_body,
            scene_id,
            "scene",
        )
    }

    fn update_room_children(
        &self,
        username: &str,
        room_id: &str,
        device_ids: &[String],
    ) -> Result<()> {
        let url = format!("{}/clip/v2/resource/room/{}", self.base_url(), room_id);
        let children = device_ids
            .iter()
            .map(|device_id| serde_json::json!({"rid": device_id, "rtype": "device"}))
            .collect::<Vec<_>>();
        let resp = self
            .client
            .put(&url)
            .header("hue-application-key", username)
            .json(&serde_json::json!({"children": children}))
            .send()
            .map_err(|_| anyhow::anyhow!("Hue room membership update request failed"))?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Hue room membership update failed with HTTP status {status}"
            ));
        }

        validate_hue_v2_resource_write_response(
            "Hue room membership update",
            &body,
            room_id,
            "room",
        )
    }

    fn create_room(
        &self,
        username: &str,
        definition: &HueRoomDefinition,
    ) -> Result<HueCreatedRoom> {
        let created = self.create_resource(username, "room", &room_definition_body(definition))?;
        Ok(HueCreatedRoom {
            room_id: created.resource_id,
        })
    }

    fn update_room(
        &self,
        username: &str,
        room_id: &str,
        definition: &HueRoomDefinition,
    ) -> Result<()> {
        self.update_resource(username, "room", room_id, &room_definition_body(definition))
    }

    fn rename_room(&self, username: &str, room_id: &str, name: &str) -> Result<()> {
        self.update_resource(username, "room", room_id, &room_rename_body(name))
    }

    fn delete_room(&self, username: &str, room_id: &str) -> Result<()> {
        self.delete_resource(username, "room", room_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_light_body_uses_hue_identify_action_enum() {
        assert_eq!(
            identify_light_body(),
            serde_json::json!({
                "identify": { "action": "identify" }
            })
        );
    }

    #[test]
    fn scene_recall_body_uses_active_action_and_optional_duration() {
        assert_eq!(
            scene_recall_body(None),
            serde_json::json!({"recall": {"action": "active"}})
        );
        assert_eq!(
            scene_recall_body(Some(750)),
            serde_json::json!({
                "recall": {"action": "active", "duration": 750}
            })
        );
    }

    #[test]
    fn hue_write_response_rejects_application_errors_on_success_status() {
        validate_hue_v2_resource_write_response(
            "PUT scene/native-1 recall",
            r#"{"data":[{"rid":"native-1","rtype":"scene"}],"errors":[]}"#,
            "native-1",
            "scene",
        )
        .unwrap();

        let error = validate_hue_v2_resource_write_response(
            "PUT scene/native-1 recall",
            r#"{"data":[],"errors":[{"description":"scene unavailable"}]}"#,
            "native-1",
            "scene",
        )
        .unwrap_err();
        assert!(error.to_string().contains("1 Hue application error"));
        assert!(!error.to_string().contains("scene unavailable"));
    }

    #[test]
    fn hue_write_response_rejects_invalid_json() {
        let error = validate_hue_v2_resource_write_response(
            "PUT scene/native-1 recall",
            "not-json",
            "native-1",
            "scene",
        )
        .unwrap_err();
        assert!(error.to_string().contains("invalid JSON"));
    }

    #[test]
    fn generic_v2_write_receipt_must_match_exact_resource_identity() {
        validate_hue_v2_resource_write_response(
            "Hue resource update",
            r#"{"data":[{"rid":"expected-id","rtype":"room"}],"errors":[]}"#,
            "expected-id",
            "room",
        )
        .unwrap();

        for body in [
            r#"{"data":[],"errors":[]}"#,
            r#"{"data":[{"rid":"wrong-private-id","rtype":"room"}],"errors":[]}"#,
            r#"{"data":[{"rid":"expected-id","rtype":"scene"}],"errors":[]}"#,
            r#"{"data":[{"rid":"expected-id","rtype":"room"},{"rid":"extra","rtype":"room"}],"errors":[]}"#,
        ] {
            let error = validate_hue_v2_resource_write_response(
                "Hue resource update",
                body,
                "expected-id",
                "room",
            )
            .unwrap_err();
            let message = error.to_string();
            assert!(message.contains("expected Hue resource identity"));
            assert!(!message.contains("expected-id"));
            assert!(!message.contains("wrong-private-id"));
        }
    }

    #[test]
    fn generic_v1_write_receipts_require_exact_success_addresses() {
        let update = serde_json::json!([{
            "success": {"/rules/private-rule/status": "disabled"}
        }]);
        validate_hue_v1_update_success(
            "Hue V1 update",
            &update,
            "rules/private-rule",
            &serde_json::json!({"status": "disabled"}),
        )
        .unwrap();
        let error = validate_hue_v1_update_success(
            "Hue V1 update",
            &serde_json::json!([{"success": {"/rules/other/status": "disabled"}}]),
            "rules/private-rule",
            &serde_json::json!({"status": "disabled"}),
        )
        .unwrap_err();
        assert!(!error.to_string().contains("private-rule"));

        validate_hue_v1_post_success(
            "Hue V1 create",
            &serde_json::json!([{"success": {"id": "new-private-id"}}]),
            "rules",
        )
        .unwrap();
        assert!(validate_hue_v1_post_success(
            "Hue V1 create",
            &serde_json::json!([{"success": {"unrelated": true}}]),
            "rules",
        )
        .is_err());
    }

    #[test]
    fn reqwest_hue_transport_new_builds_https_base_url_without_network_io() {
        let transport = ReqwestHueTransport::new("192.0.2.10").unwrap();

        assert_eq!(transport.base_url(), "https://192.0.2.10");
    }

    #[test]
    fn bridge_serial_search_uses_v1_deviceid_shape() {
        assert_eq!(
            bridge_serial_search_body("E277DA"),
            serde_json::json!({"deviceid": ["E277DA"]})
        );
    }

    #[test]
    fn bridge_control_preserves_extended_hue_temperature_range() {
        assert_eq!(
            light_control_body(true, Some(80), Some(20_000), None, None)["color_temperature"]
                ["mirek"],
            50
        );
        assert_eq!(
            light_control_body(true, Some(80), Some(1_000), None, None)["color_temperature"]
                ["mirek"],
            1000
        );
    }

    #[test]
    fn bridge_search_status_distinguishes_active_and_completed_results() {
        let active = parse_new_lights(&serde_json::json!({"lastscan": "active"})).unwrap();
        assert!(active.active);
        assert!(active.lights.is_empty());

        let completed = parse_new_lights(&serde_json::json!({
            "12": {"name": "Desk lamp"},
            "lastscan": "2026-07-30T00:00:00"
        }))
        .unwrap();
        assert!(!completed.active);
        assert_eq!(
            completed.lights,
            vec![HueBridgeSearchLight {
                legacy_id: "12".to_string(),
                name: "Desk lamp".to_string(),
            }]
        );
    }

    #[test]
    fn bridge_search_does_not_accept_stale_nonempty_results() {
        let before = parse_new_lights(&serde_json::json!({
            "12": {"name": "Old result"},
            "lastscan": "2026-07-29T00:00:00"
        }))
        .unwrap();
        let stale = before.clone();
        assert!(!completed_new_scan(&before, &stale, false));

        let next_generation = parse_new_lights(&serde_json::json!({
            "13": {"name": "New result"},
            "lastscan": "2026-07-30T00:00:00"
        }))
        .unwrap();
        assert!(completed_new_scan(&before, &next_generation, false));
        assert!(completed_new_scan(&before, &stale, true));
    }

    #[test]
    fn hue_v1_errors_are_collected_and_success_must_match_exact_address() {
        let error = validate_hue_v1_response(
            "POST lights search",
            &serde_json::json!([
                {"error": {"type": 901, "address": "/lights", "description": "internal error"}},
                {"error": {"type": 3, "address": "/lights/7", "description": "missing resource"}}
            ]),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("type 901"));
        assert!(message.contains("type 3"));
        assert!(!message.contains("/lights/7"));
        assert!(!message.contains("missing resource"));

        validate_hue_v1_delete_success(
            "DELETE light/7",
            &serde_json::json!([{"success": {"/lights/7": "deleted"}}]),
            "/lights/7",
        )
        .unwrap();
        validate_hue_v1_delete_success(
            "DELETE light/7",
            &serde_json::json!([{"success": "/lights/7 deleted."}]),
            "/lights/7",
        )
        .unwrap();
        assert!(validate_hue_v1_delete_success(
            "DELETE light/7",
            &serde_json::json!([{"success": "/lights/8 deleted."}]),
            "/lights/7",
        )
        .is_err());
        assert!(validate_hue_v1_delete_success(
            "DELETE light/7",
            &serde_json::json!([]),
            "/lights/7",
        )
        .is_err());
    }

    #[test]
    fn hue_application_errors_do_not_echo_names_or_secrets() {
        let secret = "customer-secret-application-key";
        let room_name = "Tim's private bedroom";
        let body = serde_json::json!({
            "data": [],
            "errors": [{
                "description": format!("could not update {room_name} with {secret}"),
                "address": "/clip/v2/resource/room/private-room-id"
            }]
        })
        .to_string();
        let error =
            validate_hue_v2_resource_write_response("PUT room", &body, "private-room-id", "room")
                .unwrap_err();
        let message = error.to_string();
        assert!(!message.contains(secret));
        assert!(!message.contains(room_name));
        assert!(!message.contains("private-room-id"));
    }

    #[test]
    fn room_control_plane_bodies_and_created_identity_are_typed() {
        let definition = HueRoomDefinition {
            name: "Office".to_string(),
            archetype: "office".to_string(),
            device_ids: vec!["device-b".to_string(), "device-a".to_string()],
        };
        assert_eq!(
            room_definition_body(&definition),
            serde_json::json!({
                "children": [
                    {"rid": "device-b", "rtype": "device"},
                    {"rid": "device-a", "rtype": "device"}
                ],
                "metadata": {"name": "Office", "archetype": "office"}
            })
        );
        assert_eq!(
            room_rename_body("Studio"),
            serde_json::json!({
                "metadata": {"name": "Studio"}
            })
        );
        assert_eq!(
            created_hue_v2_resource(
                "POST room",
                "room",
                r#"{"data":[{"rid":"room-1","rtype":"room"}],"errors":[]}"#,
            )
            .unwrap(),
            HueCreatedResource {
                resource_id: "room-1".to_string(),
                resource_type: "room".to_string(),
            }
        );
        let wrong_type = created_hue_v2_resource(
            "POST room",
            "room",
            r#"{"data":[{"rid":"private-id","rtype":"scene"}],"errors":[]}"#,
        )
        .unwrap_err();
        assert!(!wrong_type.to_string().contains("private-id"));
        assert!(normalized_hue_api_path("../../config").is_err());
    }

    #[test]
    fn bridge_removal_maps_v2_device_mac_to_all_exact_v1_light_ids() {
        let macs = zigbee_macs_for_device(
            &serde_json::json!({
                "errors": [],
                "data": [
                    {
                        "owner": {"rtype": "device", "rid": "device-a"},
                        "mac_address": "00:17:88:01:AA:BB:CC:DD"
                    },
                    {
                        "owner": {"rtype": "device", "rid": "device-a"},
                        "mac_address": "00-17-88-01-AA-BB-CC-DD"
                    },
                    {
                        "owner": {"rtype": "device", "rid": "device-b"},
                        "mac_address": "00:17:88:01:11:22:33:44"
                    }
                ]
            }),
            "device-a",
        )
        .unwrap();
        assert_eq!(macs, BTreeSet::from(["00178801aabbccdd".to_string()]));

        let ids = legacy_light_ids_for_macs(
            &serde_json::json!({
                "7": {"uniqueid": "00:17:88:01:aa:bb:cc:dd-0b"},
                "8": {"uniqueid": "00:17:88:01:AA:BB:CC:DD-02"},
                "9": {"uniqueid": "00:17:88:01:aa:bb:cc:de-0b"},
                "10": {"uniqueid": "00178801aabbccdd00"}
            }),
            &macs,
        )
        .unwrap();
        assert_eq!(ids, vec!["7".to_string(), "8".to_string()]);

        let ambiguous = zigbee_macs_for_device(
            &serde_json::json!({
                "errors": [],
                "data": [
                    {
                        "owner": {"rtype": "device", "rid": "device-a"},
                        "mac_address": "00:17:88:01:AA:BB:CC:DD"
                    },
                    {
                        "owner": {"rtype": "device", "rid": "device-a"},
                        "mac_address": "00:17:88:01:11:22:33:44"
                    }
                ]
            }),
            "device-a",
        )
        .unwrap_err();
        assert!(ambiguous.to_string().contains("2 distinct"));
    }

    #[test]
    fn multi_light_delete_only_tolerates_the_exact_missing_v1_resource() {
        let exact_missing = serde_json::json!([
            {"error": {"type": 3, "address": "/lights/8", "description": "resource not available"}}
        ]);
        assert!(hue_v1_is_exact_missing_resource(
            &exact_missing,
            "/lights/8"
        ));
        assert!(can_tolerate_missing_v1_light(
            &exact_missing,
            "/lights/8",
            true,
            false
        ));
        assert!(!can_tolerate_missing_v1_light(
            &exact_missing,
            "/lights/8",
            false,
            false
        ));
        assert!(!can_tolerate_missing_v1_light(
            &exact_missing,
            "/lights/8",
            true,
            true
        ));
        assert!(!hue_v1_is_exact_missing_resource(
            &serde_json::json!([
                {"error": {"type": 3, "address": "/lights/9", "description": "resource not available"}}
            ]),
            "/lights/8"
        ));
        assert!(!hue_v1_is_exact_missing_resource(
            &serde_json::json!([
                {"error": {"type": 901, "address": "/lights/8", "description": "internal error"}}
            ]),
            "/lights/8"
        ));
    }
}
