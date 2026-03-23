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
