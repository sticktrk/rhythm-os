//! Reqwest-based implementation of [`HueTransport`] for server-class targets.
//!
//! Provides a ready-to-use transport so current binaries get Hue
//! communication without writing platform-specific code.

use std::mem::ManuallyDrop;
use std::time::Duration;

use crate::api_types::{HueV2GroupedLight, HueV2Light, HueV2Response};
use crate::transport::HueTransport;
use anyhow::Result;

const HUE_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HUE_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const HUE_IDENTIFY_ACTION: &str = "identify";

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
            let mirek = ((1_000_000.0_f32 / k as f32).round() as u32).clamp(153, 500);
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

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(anyhow::anyhow!(
                "PUT scene/{} recall failed with status {}: {}",
                scene_id,
                status,
                body
            ));
        }

        Ok(())
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

        let envelope: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
            anyhow::anyhow!("PUT room/{} returned invalid JSON: {}", room_id, error)
        })?;
        if let Some(errors) = envelope.get("errors").and_then(|value| value.as_array()) {
            if !errors.is_empty() {
                return Err(anyhow::anyhow!(
                    "PUT room/{} returned Hue errors: {}",
                    room_id,
                    serde_json::Value::Array(errors.clone())
                ));
            }
        }
        Ok(())
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
    fn reqwest_hue_transport_new_builds_https_base_url_without_network_io() {
        let transport = ReqwestHueTransport::new("192.0.2.10").unwrap();

        assert_eq!(transport.base_url(), "https://192.0.2.10");
    }
}
