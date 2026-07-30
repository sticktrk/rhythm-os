//! Reqwest-based implementation of [`HueTransport`] for server-class targets.
//!
//! Provides a ready-to-use transport so current binaries get Hue
//! communication without writing platform-specific code.

use std::collections::BTreeSet;
use std::mem::ManuallyDrop;
use std::time::Duration;

use crate::api_types::{HueV2GroupedLight, HueV2Light, HueV2Response};
use crate::transport::{HueBridgeSearchLight, HueTransport};
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

fn validate_hue_v2_write_response(operation: &str, body: &str) -> Result<()> {
    let envelope: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| anyhow::anyhow!("{operation} returned invalid JSON: {error}"))?;
    if let Some(errors) = envelope.get("errors").and_then(|value| value.as_array()) {
        if !errors.is_empty() {
            return Err(anyhow::anyhow!(
                "{operation} returned Hue errors: {}",
                serde_json::Value::Array(errors.clone())
            ));
        }
    }
    Ok(())
}

fn hue_v1_error_description(error: &serde_json::Value) -> String {
    let error_type = error
        .get("type")
        .map(serde_json::Value::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    let address = error
        .get("address")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    let description = error
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no description>");
    format!("type {error_type} at {address}: {description}")
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
    anyhow::bail!("{operation} did not confirm success for {expected_address}: {value}")
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
    anyhow::bail!("{operation} did not confirm deletion of {expected_address}: {value}")
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
            "{operation} returned Hue errors: {}",
            serde_json::Value::Array(errors.clone())
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
        0 => anyhow::bail!(
            "Hue V2 device {v2_device_id} has no usable zigbee_connectivity MAC address"
        ),
        1 => {}
        count => anyhow::bail!(
            "Hue V2 device {v2_device_id} has {count} distinct zigbee_connectivity MAC addresses; refusing ambiguous V1 deletion"
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
            .send()?;
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
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(anyhow::anyhow!(
                "PUT grouped_light/{} failed with status {}: {}",
                grouped_light_id,
                status,
                body
            ));
        }

        Ok(())
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
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(anyhow::anyhow!(
                "PUT light/{} failed with status {}: {}",
                light_id,
                status,
                body
            ));
        }

        Ok(())
    }

    fn identify_light(&self, username: &str, light_id: &str) -> Result<()> {
        let url = format!("{}/clip/v2/resource/light/{}", self.base_url(), light_id);
        let body = identify_light_body();

        let resp = self
            .client
            .put(&url)
            .header("hue-application-key", username)
            .json(&body)
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(anyhow::anyhow!(
                "PUT light/{} identify failed with status {}: {}",
                light_id,
                status,
                body
            ));
        }

        Ok(())
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
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "GET grouped_light/{} failed with status {}",
                grouped_light_id,
                status
            ));
        }

        let envelope: HueV2Response<HueV2GroupedLight> = resp.json()?;
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
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "GET light/{} failed with status {}",
                light_id,
                status
            ));
        }

        let envelope: HueV2Response<HueV2Light> = resp.json()?;
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
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(anyhow::anyhow!(
                "GET resource/{} failed with status {}: {}",
                resource_type,
                status,
                body
            ));
        }

        Ok(resp.json()?)
    }

    fn search_new_lights(&self, username: &str, serial: &str) -> Result<Vec<HueBridgeSearchLight>> {
        let url = format!("{}/api/{}/lights", self.base_url(), username);
        let status_url = format!("{}/new", url);
        let response = self.client.get(&status_url).send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!(
                "GET lights/new before search failed with status {}: {}",
                status,
                body
            );
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
            .send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("POST lights search failed with status {}: {}", status, body);
        }
        let value: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
            anyhow::anyhow!("POST lights search returned invalid JSON: {error}")
        })?;
        validate_hue_v1_success("POST lights search", &value, "/lights")?;

        let deadline = std::time::Instant::now() + HUE_BRIDGE_SEARCH_TIMEOUT;
        let mut saw_active = false;
        loop {
            let response = self.client.get(&status_url).send()?;
            let status = response.status();
            let body = response.text().unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("GET lights/new failed with status {}: {}", status, body);
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
        let response = self.client.get(&lights_url).send()?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GET lights failed with status {}: {}", status, body);
        }
        let value: serde_json::Value = serde_json::from_str(&body)
            .map_err(|error| anyhow::anyhow!("GET lights returned invalid JSON: {error}"))?;
        let legacy_ids = legacy_light_ids_for_macs(&value, &macs)?;

        for (index, legacy_id) in legacy_ids.iter().enumerate() {
            let delete_url = format!("{lights_url}/{legacy_id}");
            let response = self.client.delete(&delete_url).send()?;
            let status = response.status();
            let body = response.text().unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!(
                    "DELETE light/{legacy_id} failed with status {}: {}",
                    status,
                    body
                );
            }
            let value: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
                anyhow::anyhow!("DELETE light/{legacy_id} returned invalid JSON: {error}")
            })?;
            let expected_address = format!("/lights/{legacy_id}");
            if let Err(error) = validate_hue_v1_delete_success(
                &format!("DELETE light/{legacy_id}"),
                &value,
                &expected_address,
            ) {
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
                        "Hue V1 light {legacy_id} disappeared during multi-light deletion, but V2 device {v2_device_id} is still present"
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
                    "Hue Bridge still reports V2 device {v2_device_id} after deleting V1 lights {}",
                    legacy_ids.join(", ")
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
            .send()?;

        let status = resp.status();
        let response_body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "PUT scene/{} recall failed with status {}: {}",
                scene_id,
                status,
                response_body
            ));
        }

        validate_hue_v2_write_response(&format!("PUT scene/{scene_id} recall"), &response_body)
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
            .send()?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "PUT room/{} failed with status {}: {}",
                room_id,
                status,
                body
            ));
        }

        validate_hue_v2_write_response(&format!("PUT room/{room_id}"), &body)
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
        validate_hue_v2_write_response(
            "PUT scene/native-1 recall",
            r#"{"data":[{"rid":"native-1","rtype":"scene"}],"errors":[]}"#,
        )
        .unwrap();

        let error = validate_hue_v2_write_response(
            "PUT scene/native-1 recall",
            r#"{"data":[],"errors":[{"description":"scene unavailable"}]}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("scene unavailable"));
    }

    #[test]
    fn hue_write_response_rejects_invalid_json() {
        let error =
            validate_hue_v2_write_response("PUT scene/native-1 recall", "not-json").unwrap_err();
        assert!(error.to_string().contains("invalid JSON"));
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
        assert!(message.contains("type 901 at /lights: internal error"));
        assert!(message.contains("type 3 at /lights/7: missing resource"));

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
