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
/// Implementors provide the actual HTTP transport. On active platforms this
/// uses reqwest, but the trait is intentionally transport-agnostic.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct MinimalTransport;

    impl HaTransport for MinimalTransport {
        fn call_service(&self, _domain: &str, _service: &str, _data: &Value) -> Result<()> {
            Ok(())
        }

        fn get_states(&self) -> Result<Vec<EntityState>> {
            Ok(vec![EntityState {
                entity_id: "light.kitchen".to_string(),
                state: "on".to_string(),
                attributes: serde_json::json!({"friendly_name": "Kitchen"}),
            }])
        }

        fn get_state(&self, entity_id: &str) -> Result<EntityState> {
            Ok(EntityState {
                entity_id: entity_id.to_string(),
                state: "off".to_string(),
                attributes: Value::Null,
            })
        }

        fn test_connection(&self) -> Result<bool> {
            Ok(true)
        }
    }

    #[test]
    fn minimal_transport_uses_default_config_error_and_entity_state_defaults() {
        let transport = MinimalTransport;

        assert_eq!(
            transport.get_config().unwrap_err().to_string(),
            "get_config not implemented for this transport"
        );
        assert_eq!(
            transport.get_states().unwrap()[0].attributes["friendly_name"],
            "Kitchen"
        );
        assert_eq!(
            transport.get_state("light.office").unwrap().entity_id,
            "light.office"
        );
        assert!(transport.test_connection().unwrap());

        let state: EntityState =
            serde_json::from_str(r#"{"entity_id":"light.bed","state":"off"}"#).unwrap();
        assert_eq!(state.attributes, Value::Null);
    }

    #[test]
    fn connection_config_builds_supervisor_and_plain_urls() {
        let plain = HaConnectionConfig {
            host: "ha.local".to_string(),
            port: 8123,
            token: "token".to_string(),
            use_ssl: false,
        };
        assert_eq!(plain.ws_url(), "ws://ha.local:8123/api/websocket");
        assert_eq!(
            plain.rest_url("/api/states"),
            "http://ha.local:8123/api/states"
        );

        let supervisor = HaConnectionConfig {
            host: "supervisor".to_string(),
            port: 80,
            token: "token".to_string(),
            use_ssl: true,
        };
        assert_eq!(
            supervisor.ws_url(),
            "wss://supervisor:80/core/api/websocket"
        );
        assert_eq!(
            supervisor.rest_url("/api/config"),
            "https://supervisor:80/core/api/config"
        );
    }

    #[test]
    fn connection_config_reads_env_and_supervisor_fallback() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("HA_HOST", "ha.local");
        std::env::set_var("HA_PORT", "9443");
        std::env::set_var("HA_TOKEN", "ha-token");
        std::env::set_var("HA_USE_SSL", "true");
        std::env::remove_var("SUPERVISOR_TOKEN");

        let config = HaConnectionConfig::from_env().unwrap();
        assert_eq!(config.host, "ha.local");
        assert_eq!(config.port, 9443);
        assert_eq!(config.token, "ha-token");
        assert!(config.use_ssl);

        std::env::remove_var("HA_TOKEN");
        std::env::set_var("SUPERVISOR_TOKEN", "supervisor-token");
        std::env::set_var("HA_PORT", "not-a-port");
        std::env::set_var("HA_USE_SSL", "0");

        let config = HaConnectionConfig::from_env().unwrap();
        assert_eq!(config.port, 8123);
        assert_eq!(config.token, "supervisor-token");
        assert!(!config.use_ssl);

        let supervisor = HaConnectionConfig::for_supervisor().unwrap();
        assert_eq!(supervisor.host, "supervisor");
        assert_eq!(supervisor.port, 80);
        assert_eq!(supervisor.token, "supervisor-token");

        std::env::remove_var("HA_HOST");
        std::env::remove_var("HA_PORT");
        std::env::remove_var("HA_TOKEN");
        std::env::remove_var("HA_USE_SSL");
        std::env::remove_var("SUPERVISOR_TOKEN");

        assert_eq!(
            HaConnectionConfig::from_env().unwrap_err().to_string(),
            "Neither HA_TOKEN nor SUPERVISOR_TOKEN set"
        );
        assert_eq!(
            HaConnectionConfig::for_supervisor()
                .unwrap_err()
                .to_string(),
            "SUPERVISOR_TOKEN not set"
        );
    }
}
