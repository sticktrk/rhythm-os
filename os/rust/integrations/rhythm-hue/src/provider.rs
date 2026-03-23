//! Generic Hue hub provider — shared credential validation and state setup.
//!
//! Thin wrapper around `rhythm_os::lifecycle::configure_hub` with Hue-specific
//! credential parsing. Also provides credential factory and accessor functions
//! so rhythm-os stays hub-agnostic.

use std::sync::mpsc::Receiver;

use anyhow::Result;
use log::info;

use rhythm_os::hub::{ActiveHub, HubCredentials, HubEvent, HubType};
use rhythm_os::state::SharedState;

/// Create Hue credentials.
pub fn hue_credentials(bridge_ip: &str, username: &str) -> HubCredentials {
    HubCredentials::new(
        HubType::HUE,
        bridge_ip,
        serde_json::json!({ "username": username }),
    )
}

/// Extract Hue username from credentials data.
pub fn hue_username(creds: &HubCredentials) -> Option<&str> {
    creds.get_str("username")
}

/// Shared credential validation and state setup for Hue hubs.
///
/// Delegates to `rhythm_os::lifecycle::configure_hub` with Hue-specific
/// credential parsing and same-credentials detection.
pub fn configure_hue_hub<F>(
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
            struct HueCreds {
                username: String,
            }
            let creds: HueCreds = serde_json::from_str(creds_json)
                .map_err(|e| anyhow::anyhow!("Invalid Hue credentials: {}", e))?;
            Ok(hue_credentials(address, &creds.username))
        },
        // already_configured
        |state, new_creds| {
            let s = match state.lock() {
                Ok(s) => s,
                Err(_) => return false,
            };
            let existing = s
                .hub_credentials
                .values()
                .find(|c| c.hub_type.as_ref().is_some_and(|t| t.as_str() == "hue"));
            if let Some(existing) = existing {
                if existing.address == new_creds.address
                    && hue_username(existing) == hue_username(new_creds)
                {
                    info!(target: "sys", "Hue hub already configured with same credentials, skipping");
                    return true;
                }
            }
            false
        },
        // connect_fn
        connect_fn,
    )
}
