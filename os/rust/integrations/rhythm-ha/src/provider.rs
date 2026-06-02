//! Generic HA hub provider — shared credential validation and state setup.
//!
//! Thin wrapper around `rhythm_os::lifecycle::configure_hub` with HA-specific
//! credential parsing. Keeps `config_from_credentials` and `parse_ha_address`
//! which are HA-specific helpers. Also provides credential factory and accessor
//! functions so rhythm-os stays hub-agnostic.

use std::sync::mpsc::Receiver;

use anyhow::Result;
use log::info;

use rhythm_os::hub::{ActiveHub, HubCredentials, HubEvent};
use rhythm_os::state::SharedState;

use crate::transport::HaConnectionConfig;

/// Create Home Assistant credentials.
pub fn ha_credentials(address: &str, token: &str) -> HubCredentials {
    HubCredentials::new(
        "homeassistant",
        address,
        serde_json::json!({ "token": token }),
    )
}

/// Extract HA long-lived access token from credentials data.
pub fn ha_token(creds: &HubCredentials) -> Option<&str> {
    creds.get_str("token")
}

/// Shared credential validation and state setup for HA hubs.
///
/// Delegates to `rhythm_os::lifecycle::configure_hub` with HA-specific
/// credential parsing and same-credentials detection.
pub fn configure_ha_hub<F>(
    address: &str,
    credentials_json: &str,
    state: &SharedState,
    connect_fn: F,
) -> Result<()>
where
    F: FnOnce(&SharedState) -> Result<(ActiveHub, Receiver<HubEvent>)>,
{
    rhythm_os::lifecycle::configure_hub(
        state,
        address,
        credentials_json,
        // build_credentials
        |address, creds_json| {
            #[derive(serde::Deserialize)]
            struct HaCreds {
                token: String,
            }
            let creds: HaCreds = serde_json::from_str(creds_json)
                .map_err(|e| anyhow::anyhow!("Invalid HA credentials: {}", e))?;
            Ok(ha_credentials(address, &creds.token))
        },
        // already_configured
        |state, new_creds| {
            let s = match state.lock() {
                Ok(s) => s,
                Err(_) => return false,
            };
            let existing = s.hub_credentials.values().find(|c| {
                c.hub_type
                    .as_ref()
                    .is_some_and(|t| t.as_str() == "homeassistant")
            });
            if let Some(existing) = existing {
                if existing.address == new_creds.address
                    && ha_token(existing) == ha_token(new_creds)
                {
                    info!(target: "sys", "HA hub already configured with same credentials, skipping");
                    return true;
                }
            }
            false
        },
        // connect_fn
        connect_fn,
    )
}

/// Parse an `HaConnectionConfig` from the stored hub credentials.
pub fn config_from_credentials(
    address: &str,
    credentials: &HubCredentials,
) -> Result<HaConnectionConfig> {
    let token = ha_token(credentials)
        .ok_or_else(|| anyhow::anyhow!("HA token not found in credentials"))?
        .to_string();

    // Parse address — may be "host:port" or just "host"
    let (host, port, use_ssl) = parse_ha_address(address);

    Ok(HaConnectionConfig {
        host,
        port,
        token,
        use_ssl,
    })
}

/// Parse an HA address string into (host, port, use_ssl).
///
/// Supports: "host", "host:port", "http://host:port", "https://host:port"
fn parse_ha_address(address: &str) -> (String, u16, bool) {
    let (scheme, rest) = if let Some(stripped) = address.strip_prefix("https://") {
        (true, stripped)
    } else if let Some(stripped) = address.strip_prefix("http://") {
        (false, stripped)
    } else {
        (false, address)
    };

    if let Some((host, port_str)) = rest.rsplit_once(':') {
        if let Ok(port) = port_str.parse::<u16>() {
            return (host.to_string(), port, scheme);
        }
    }

    (rest.to_string(), 8123, scheme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc, Mutex};

    use rhythm_os::canonical::identity::HubKey;
    use rhythm_os::hub::{ActiveHub, HubType};

    fn active_hub(address: &str) -> (ActiveHub, Receiver<HubEvent>) {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let hub_type = HubType::new("homeassistant");
        let hub_key = HubKey::new(hub_type.clone(), address);
        (
            ActiveHub {
                hub_type,
                hub_key,
                runtime: None,
                hub_data: Box::new(()),
                registry: None,
                discovery: None,
                shutdown: Arc::new(AtomicBool::new(false)),
            },
            rx,
        )
    }

    #[test]
    fn ha_credentials_round_trip_and_config_parse_addresses() {
        let creds = ha_credentials("https://ha.local:9443", "token-123");
        assert_eq!(ha_token(&creds), Some("token-123"));

        let config = config_from_credentials(&creds.address, &creds).unwrap();
        assert_eq!(config.host, "ha.local");
        assert_eq!(config.port, 9443);
        assert!(config.use_ssl);
        assert_eq!(config.token, "token-123");

        assert_eq!(
            parse_ha_address("ha.local"),
            ("ha.local".to_string(), 8123, false)
        );
        assert_eq!(
            parse_ha_address("http://ha.local:8124"),
            ("ha.local".to_string(), 8124, false)
        );
        assert_eq!(
            parse_ha_address("https://ha.local"),
            ("ha.local".to_string(), 8123, true)
        );
        assert_eq!(
            parse_ha_address("ha.local:notaport"),
            ("ha.local:notaport".to_string(), 8123, false)
        );
    }

    #[test]
    fn config_from_credentials_rejects_missing_token() {
        let creds = HubCredentials::new("homeassistant", "ha.local", serde_json::json!({}));

        let error = config_from_credentials("ha.local", &creds).unwrap_err();

        assert_eq!(error.to_string(), "HA token not found in credentials");
    }

    #[test]
    fn configure_ha_hub_stores_credentials_and_skips_same_credentials() {
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        let calls = Arc::new(Mutex::new(0usize));
        let first_calls = calls.clone();

        configure_ha_hub("ha.local", r#"{"token":"token-123"}"#, &state, move |_| {
            *first_calls.lock().unwrap() += 1;
            Ok(active_hub("ha.local"))
        })
        .unwrap();

        assert_eq!(*calls.lock().unwrap(), 1);
        let key = HubKey::new(HubType::new("homeassistant"), "ha.local");
        assert_eq!(
            state
                .lock()
                .unwrap()
                .hub_credentials
                .get(&key)
                .and_then(ha_token),
            Some("token-123")
        );

        configure_ha_hub("ha.local", r#"{"token":"token-123"}"#, &state, |_| {
            panic!("same HA credentials should short-circuit")
        })
        .unwrap();
    }

    #[test]
    fn configure_ha_hub_rejects_invalid_credentials_json() {
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));

        let error = configure_ha_hub("ha.local", "{}", &state, |_| {
            panic!("invalid credentials should not connect")
        })
        .unwrap_err();

        assert!(error.to_string().contains("Invalid HA credentials:"));
    }
}
