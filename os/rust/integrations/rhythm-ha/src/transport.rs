//! Home Assistant transport abstraction.
//!
//! Defines the `HaTransport` trait that platform-specific crates implement
//! to provide HTTP communication with Home Assistant. The trait methods
//! mirror the HA REST API for light services.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Platform-agnostic interface to the Home Assistant REST API.
///
/// Implementors provide the actual HTTP transport. On desktop this uses
/// `reqwest`; on ESP32 it could use `esp-idf-svc`'s HTTP client.
pub trait HaTransport: Send + Sync {
    /// Call a Home Assistant service.
    ///
    /// # Arguments
    ///
    /// * `domain` - Service domain (e.g., "light")
    /// * `service` - Service name (e.g., "turn_on")
    /// * `data` - Service data as JSON (includes target, fields)
    fn call_service(&self, domain: &str, service: &str, data: &Value) -> Result<()>;

    /// Get states of all entities.
    fn get_states(&self) -> Result<Vec<EntityState>>;

    /// Get state of a single entity.
    fn get_state(&self, entity_id: &str) -> Result<EntityState>;

    /// Test the connection with the configured token.
    fn test_connection(&self) -> Result<bool>;

    /// Fetch the HA instance configuration (location, timezone, etc.).
    ///
    /// Returns the raw JSON from `GET /api/config`. Used by post-connect
    /// hooks to import location and timezone from HA.
    fn get_config(&self) -> Result<Value> {
        Err(anyhow::anyhow!(
            "get_config not implemented for this transport"
        ))
    }
}

/// Simplified entity state from HA REST API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityState {
    pub entity_id: String,
    pub state: String,
    #[serde(default)]
    pub attributes: Value,
}

/// Configuration for connecting to Home Assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaConnectionConfig {
    /// HA host (IP or hostname).
    pub host: String,
    /// HA port (default 8123).
    pub port: u16,
    /// Long-lived access token.
    pub token: String,
    /// Use SSL (https/wss).
    pub use_ssl: bool,
}

impl HaConnectionConfig {
    /// Build WebSocket URL.
    ///
    /// When connecting through the HA Supervisor proxy (host == "supervisor"),
    /// the path must be `/core/api/websocket`.
    pub fn ws_url(&self) -> String {
        let scheme = if self.use_ssl { "wss" } else { "ws" };
        let prefix = if self.host == "supervisor" {
            "/core"
        } else {
            ""
        };
        format!(
            "{}://{}:{}{}/api/websocket",
            scheme, self.host, self.port, prefix
        )
    }

    /// Build REST API URL for a given path.
    ///
    /// When connecting through the HA Supervisor proxy (host == "supervisor"),
    /// the path is prefixed with `/core`.
    pub fn rest_url(&self, path: &str) -> String {
        let scheme = if self.use_ssl { "https" } else { "http" };
        let prefix = if self.host == "supervisor" {
            "/core"
        } else {
            ""
        };
        format!("{}://{}:{}{}{}", scheme, self.host, self.port, prefix, path)
    }

    /// Create config from environment variables.
    ///
    /// Reads: `HA_HOST`, `HA_PORT`, `HA_TOKEN`, `HA_USE_SSL`
    pub fn from_env() -> Result<Self> {
        let host = std::env::var("HA_HOST").unwrap_or_else(|_| "homeassistant.local".to_string());
        let port = std::env::var("HA_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8123);
        let token = std::env::var("HA_TOKEN")
            .or_else(|_| std::env::var("SUPERVISOR_TOKEN"))
            .map_err(|_| anyhow::anyhow!("Neither HA_TOKEN nor SUPERVISOR_TOKEN set"))?;
        let use_ssl = std::env::var("HA_USE_SSL")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);

        Ok(Self {
            host,
            port,
            token,
            use_ssl,
        })
    }

    /// Create config for HA Supervisor proxy mode.
    ///
    /// When running as an add-on, HA provides a local proxy at
    /// `http://supervisor/core` with the SUPERVISOR_TOKEN.
    pub fn for_supervisor() -> Result<Self> {
        let token = std::env::var("SUPERVISOR_TOKEN")
            .map_err(|_| anyhow::anyhow!("SUPERVISOR_TOKEN not set"))?;
        Ok(Self {
            host: "supervisor".to_string(),
            port: 80,
            token,
            use_ssl: false,
        })
    }
}
