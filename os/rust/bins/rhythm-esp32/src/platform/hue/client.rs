//! Philips Hue HTTP client for ESP32.
//!
//! Provides Hue V2 REST API for room control via grouped_light resources.
//! Implements `rhythm_hue::HueTransport` for the ESP32 platform.

use std::sync::Mutex;

use anyhow::Result;
use embedded_svc::http::client::Client;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use log::{info, warn};
use rhythm_hue::api_types::{HueV2GroupedLight, HueV2Response};
use serde::Deserialize;

// ============================================================================
// HueClient
// ============================================================================

/// Hue HTTP client for ESP32.
///
/// Maintains a persistent HTTPS connection for V2 API calls to avoid the
/// ~15-20KB heap cost of a fresh TLS handshake on every request. The Hue
/// bridge supports HTTP Keep-Alive, so the underlying TCP+TLS session is
/// reused across sequential PUT/GET calls.
pub struct HueClient {
    bridge_ip: String,
    /// Persistent HTTPS client for V2 API. Checked out by `take_v2_client()`,
    /// returned by `return_v2_client()`. Dropped on request error so the next
    /// call creates a fresh connection.
    v2_conn: Mutex<Option<Client<EspHttpConnection>>>,
}

// Safety: The `EspHttpConnection` inside `v2_conn` contains a raw pointer
// (`*mut esp_http_client`) which is `!Send` by default. Access is fully
// serialized through a `Mutex`, and the esp-idf HTTP client handle is safe
// to use from any single thread at a time.
unsafe impl Send for HueClient {}
unsafe impl Sync for HueClient {}

impl HueClient {
    /// Create a new Hue client for the specified bridge IP.
    pub fn new(bridge_ip: String) -> Self {
        Self {
            bridge_ip,
            v2_conn: Mutex::new(None),
        }
    }

    /// Test the connection with stored credentials (V1 HTTP, no TLS).
    pub fn test_connection(&self, username: &str) -> Result<bool> {
        let config = Configuration::default();
        let mut client = Client::wrap(EspHttpConnection::new(&config)?);

        let url = format!("http://{}/api/{}/config", self.bridge_ip, username);
        let request = client.get(&url)?;
        let response = request.submit()?;

        Ok(response.status() == 200)
    }

    // ========================================================================
    // V2 API — persistent HTTPS connection
    // ========================================================================

    /// Eagerly create the TLS connection so the first API call doesn't pay
    /// the 1-3s handshake cost. Called during ensure_runtime().
    pub fn warmup_tls(&self) -> Result<()> {
        let client = Self::create_v2_client()?;
        self.return_v2_client(client);
        info!(target: "cmd", "TLS connection to Hue bridge warmed up");
        Ok(())
    }

    /// Create a fresh HTTPS client for the Hue bridge (self-signed cert).
    fn create_v2_client() -> Result<Client<EspHttpConnection>> {
        let config = Configuration {
            use_global_ca_store: false,
            crt_bundle_attach: None,
            timeout: Some(core::time::Duration::from_secs(10)),
            ..Default::default()
        };
        Ok(Client::wrap(EspHttpConnection::new(&config)?))
    }

    /// Check out the persistent V2 client (or create one if absent).
    fn take_v2_client(&self) -> Result<Client<EspHttpConnection>> {
        let mut guard = self
            .v2_conn
            .lock()
            .map_err(|_| anyhow::anyhow!("v2_conn lock poisoned"))?;
        match guard.take() {
            Some(client) => Ok(client),
            None => {
                info!(target: "cmd", "Creating new TLS connection to Hue bridge");
                Self::create_v2_client()
            }
        }
    }

    /// Return the V2 client for reuse by future requests.
    fn return_v2_client(&self, client: Client<EspHttpConnection>) {
        if let Ok(mut guard) = self.v2_conn.lock() {
            *guard = Some(client);
        }
    }

    /// Execute a V2 GET, retrying once on connection error (stale keepalive).
    ///
    /// Connection errors (from connect/submit) trigger a retry with a fresh
    /// TLS session. Content errors (HTTP status, JSON parse) do NOT retry —
    /// the connection is fine and returned for reuse.
    fn v2_get_one<T: for<'de> Deserialize<'de>>(
        &self,
        username: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<T> {
        let mut client = self.take_v2_client()?;

        let url = format!(
            "https://{}/clip/v2/resource/{}/{}",
            self.bridge_ip, resource_type, resource_id
        );
        let headers = [("hue-application-key", username)];

        // Connection phase: connect + read raw response. Retry on stale keepalive.
        let (status, body) = match Self::v2_connect_get(&mut client, &url, &headers) {
            Ok(raw) => {
                self.return_v2_client(client);
                raw
            }
            Err(conn_err) => {
                drop(client);
                info!(target: "cmd", "V2 GET conn retry: {}", conn_err);
                let mut client = Self::create_v2_client()?;
                let raw = Self::v2_connect_get(&mut client, &url, &headers)?;
                self.return_v2_client(client);
                raw
            }
        };

        // Content phase: parse response. Connection already returned for reuse.
        if status != 200 {
            return Err(anyhow::anyhow!(
                "V2 GET {}/{} failed with status {}",
                resource_type,
                resource_id,
                status
            ));
        }
        let body_str = std::str::from_utf8(&body)?;
        let envelope: HueV2Response<T> = serde_json::from_str(body_str)?;
        if !envelope.errors.is_empty() {
            warn!(target: "hub", "V2 GET {}/{} returned {} error(s): {:?}",
                resource_type, resource_id, envelope.errors.len(), envelope.errors);
        }
        envelope.data.into_iter().next().ok_or_else(|| {
            anyhow::anyhow!(
                "No data in V2 response for {}/{}",
                resource_type,
                resource_id
            )
        })
    }

    /// Connection-level GET: connect, submit, read body. No parsing.
    fn v2_connect_get(
        client: &mut Client<EspHttpConnection>,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>)> {
        let request = client.get_with_headers(url, headers)?;
        let mut response = request.submit()?;
        let status = response.status();
        let body = Self::read_response_body(&mut response)?;
        Ok((status, body))
    }

    /// Execute a V2 PUT, retrying once on connection error (stale keepalive).
    ///
    /// Connection errors trigger a retry. HTTP errors (non-200) do NOT retry —
    /// the connection is returned for reuse.
    fn v2_put(
        &self,
        username: &str,
        resource_type: &str,
        resource_id: &str,
        body: &str,
    ) -> Result<()> {
        let mut client = self.take_v2_client()?;

        let url = format!(
            "https://{}/clip/v2/resource/{}/{}",
            self.bridge_ip, resource_type, resource_id
        );
        let content_length = body.len().to_string();
        let headers = [
            ("hue-application-key", username),
            ("Content-Type", "application/json"),
            ("Content-Length", &content_length),
        ];
        log::debug!(target: "cmd", "V2 PUT {}/{} body={}", resource_type, resource_id, body);

        // Connection phase: connect + write + read response. Retry on stale keepalive.
        let (status, resp_body) = match Self::v2_connect_put(&mut client, &url, &headers, body) {
            Ok(raw) => {
                self.return_v2_client(client);
                raw
            }
            Err(conn_err) => {
                drop(client);
                info!(target: "cmd", "V2 PUT conn retry: {}", conn_err);
                let mut client = Self::create_v2_client()?;
                let raw = Self::v2_connect_put(&mut client, &url, &headers, body)?;
                self.return_v2_client(client);
                raw
            }
        };

        // Content phase: check status. Connection already returned for reuse.
        if status != 200 {
            crate::diag::vitals_cmd_result(false);
            return Err(anyhow::anyhow!(
                "V2 PUT {}/{} failed with status {}: {}",
                resource_type,
                resource_id,
                status,
                resp_body
            ));
        }

        crate::diag::vitals_cmd_result(true);
        Ok(())
    }

    /// Connection-level PUT: connect, write body, read response.
    fn v2_connect_put(
        client: &mut Client<EspHttpConnection>,
        url: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Result<(u16, String)> {
        let mut request = client.put(url, headers)?;
        embedded_svc::io::Write::write_all(&mut request, body.as_bytes())?;
        let mut response = request.submit()?;
        let status = response.status();
        // Always drain response body so the connection is clean for reuse
        let resp_body = Self::read_response_body(&mut response)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        Ok((status, resp_body))
    }

    /// Read HTTP response body into a Vec.
    fn read_response_body(
        response: &mut embedded_svc::http::client::Response<&mut EspHttpConnection>,
    ) -> Result<Vec<u8>> {
        let mut buf = [0u8; 2048];
        let mut body = Vec::new();
        loop {
            let read = embedded_svc::io::Read::read(response, &mut buf)?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read]);
        }
        Ok(body)
    }

    // ========================================================================
    // V2 Light Control
    // ========================================================================

    /// Control a grouped_light (room-level control) via V2 API.
    ///
    /// Uses `color_temperature.mirek` for color temp (no xy conversion needed).
    /// Brightness is 0-100 (V2 API uses percentage directly via `dimming.brightness`).
    pub fn set_grouped_light(
        &self,
        username: &str,
        grouped_light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        fade_ms: Option<u16>,
    ) -> Result<()> {
        let mut parts = Vec::new();
        parts.push(format!(r#""on":{{"on":{}}}"#, on));

        if on {
            if let Some(bri) = brightness {
                let bri_pct = (bri as f32).clamp(1.0, 100.0);
                parts.push(format!(r#""dimming":{{"brightness":{:.1}}}"#, bri_pct));
            }

            if let Some(k) = kelvin {
                let mirek = ((1_000_000.0_f32 / k as f32).round() as u32).clamp(153, 500) as u16;
                parts.push(format!(r#""color_temperature":{{"mirek":{}}}"#, mirek));
            }

            if let Some(ms) = fade_ms {
                parts.push(format!(r#""dynamics":{{"duration":{}}}"#, ms));
            }
        }

        let body = format!("{{{}}}", parts.join(","));
        self.v2_put(username, "grouped_light", grouped_light_id, &body)
    }

    /// Check if a grouped_light (room) has any lights on.
    pub fn is_grouped_light_on(&self, username: &str, grouped_light_id: &str) -> Result<bool> {
        let gl =
            self.v2_get_one::<HueV2GroupedLight>(username, "grouped_light", grouped_light_id)?;
        Ok(gl.on.map(|s| s.on).unwrap_or(false))
    }

    /// Fetch all resources of a given type from the Hue V2 API.
    ///
    /// Returns the raw JSON response (typically `{"data": [...], "errors": [...]}`).
    pub fn get_resources(&self, username: &str, resource_type: &str) -> Result<serde_json::Value> {
        let mut client = self.take_v2_client()?;

        let url = format!(
            "https://{}/clip/v2/resource/{}",
            self.bridge_ip, resource_type
        );
        let headers = [("hue-application-key", username)];

        let (status, body) = match Self::v2_connect_get(&mut client, &url, &headers) {
            Ok(raw) => {
                self.return_v2_client(client);
                raw
            }
            Err(conn_err) => {
                drop(client);
                info!(target: "cmd", "V2 GET resources/{} conn retry: {}", resource_type, conn_err);
                let mut client = Self::create_v2_client()?;
                let raw = Self::v2_connect_get(&mut client, &url, &headers)?;
                self.return_v2_client(client);
                raw
            }
        };

        if status != 200 {
            return Err(anyhow::anyhow!(
                "V2 GET resource/{} failed with status {}",
                resource_type,
                status
            ));
        }

        let body_str = std::str::from_utf8(&body)?;
        let value: serde_json::Value = serde_json::from_str(body_str)?;
        Ok(value)
    }
}

// ============================================================================
// HueTransport implementation
// ============================================================================

impl rhythm_hue::transport::HueTransport for HueClient {
    fn test_connection(&self, username: &str) -> Result<bool> {
        self.test_connection(username)
    }

    fn warmup_tls(&self) -> Result<()> {
        self.warmup_tls()
    }

    fn set_grouped_light(
        &self,
        username: &str,
        grouped_light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        fade_ms: Option<u16>,
    ) -> Result<()> {
        self.set_grouped_light(username, grouped_light_id, on, brightness, kelvin, fade_ms)
    }

    fn is_grouped_light_on(&self, username: &str, grouped_light_id: &str) -> Result<bool> {
        self.is_grouped_light_on(username, grouped_light_id)
    }

    fn get_resources(&self, username: &str, resource_type: &str) -> Result<serde_json::Value> {
        self.get_resources(username, resource_type)
    }

    fn release_connection(&self) {
        if let Ok(mut guard) = self.v2_conn.lock() {
            if guard.is_some() {
                *guard = None;
                info!(target: "cmd", "Released discovery TLS connection");
            }
        }
    }
}

// ============================================================================
// Extension trait for GET with custom headers
// ============================================================================

/// Extension trait to add `get_with_headers` to `Client<EspHttpConnection>`.
trait ClientExt {
    fn get_with_headers<'a>(
        &'a mut self,
        url: &'a str,
        headers: &'a [(&'a str, &'a str)],
    ) -> Result<embedded_svc::http::client::Request<&'a mut EspHttpConnection>>;
}

impl ClientExt for Client<EspHttpConnection> {
    fn get_with_headers<'a>(
        &'a mut self,
        url: &'a str,
        headers: &'a [(&'a str, &'a str)],
    ) -> Result<embedded_svc::http::client::Request<&'a mut EspHttpConnection>> {
        use embedded_svc::http::Method;
        Ok(self.request(Method::Get, url, headers)?)
    }
}
