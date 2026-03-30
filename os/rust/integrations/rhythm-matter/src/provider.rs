//! Generic Matter hub provider — shared validation and state setup.
//!
//! Thin wrapper around `rhythm_os::lifecycle::configure_hub` with
//! Matter-specific credential handling. For Matter, "credentials" are
//! the fabric identity — there's no external auth like Hue or HA.

use std::sync::mpsc::Receiver;

use anyhow::Result;
use log::info;

use rhythm_os::hub::{ActiveHub, HubCredentials, HubEvent};
use rhythm_os::state::SharedState;

/// Create Matter hub credentials.
///
/// For Matter, `address` is typically `"local"` (the server is the commissioner)
/// and the credential data stores the fabric ID.
pub fn matter_credentials(address: &str, fabric_id: &str) -> HubCredentials {
    HubCredentials::new(
        "matter",
        address,
        serde_json::json!({ "fabric_id": fabric_id }),
    )
}

/// Extract the fabric ID from Matter credentials.
pub fn matter_fabric_id(creds: &HubCredentials) -> Option<&str> {
    creds.get_str("fabric_id")
}

/// Shared credential validation and state setup for Matter.
///
/// Delegates to `rhythm_os::lifecycle::configure_hub` with Matter-specific
/// credential parsing.
pub fn configure_matter_hub<F>(
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
            struct MatterCreds {
                #[serde(default = "default_fabric_id")]
                fabric_id: String,
            }
            fn default_fabric_id() -> String {
                "default".to_string()
            }

            let creds: MatterCreds = serde_json::from_str(creds_json).unwrap_or(MatterCreds {
                fabric_id: default_fabric_id(),
            });
            Ok(matter_credentials(address, &creds.fabric_id))
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
                .find(|c| c.hub_type.as_ref().is_some_and(|t| t.as_str() == "matter"));
            if let Some(existing) = existing {
                if existing.address == new_creds.address
                    && matter_fabric_id(existing) == matter_fabric_id(new_creds)
                {
                    info!(target: "sys", "Matter hub already configured, skipping");
                    return true;
                }
            }
            false
        },
        // connect_fn
        connect_fn,
    )
}
