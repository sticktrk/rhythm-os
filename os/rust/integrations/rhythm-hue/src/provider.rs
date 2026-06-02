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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc, Mutex};

    use rhythm_os::hub::ActiveHub;

    fn active_hub(address: &str) -> (ActiveHub, Receiver<HubEvent>) {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let hub_type = HubType::new(HubType::HUE);
        let hub_key = rhythm_os::canonical::identity::HubKey::new(hub_type.clone(), address);
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
    fn hue_credentials_round_trip_username() {
        let creds = hue_credentials("192.0.2.10", "user-123");

        assert_eq!(
            creds.hub_type.as_ref().map(|ty| ty.as_str()),
            Some(HubType::HUE)
        );
        assert_eq!(creds.address, "192.0.2.10");
        assert_eq!(hue_username(&creds), Some("user-123"));
    }

    #[test]
    fn configure_hue_hub_stores_credentials_and_skips_same_credentials() {
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        let calls = Arc::new(Mutex::new(0usize));
        let first_calls = calls.clone();

        configure_hue_hub(
            "192.0.2.10",
            r#"{"username":"user-123"}"#,
            &state,
            move |_| {
                *first_calls.lock().unwrap() += 1;
                Ok(active_hub("192.0.2.10"))
            },
        )
        .unwrap();

        assert_eq!(*calls.lock().unwrap(), 1);
        let key =
            rhythm_os::canonical::identity::HubKey::new(HubType::new(HubType::HUE), "192.0.2.10");
        assert_eq!(
            state
                .lock()
                .unwrap()
                .hub_credentials
                .get(&key)
                .and_then(hue_username),
            Some("user-123")
        );

        configure_hue_hub("192.0.2.10", r#"{"username":"user-123"}"#, &state, |_| {
            panic!("same credentials should short-circuit")
        })
        .unwrap();
    }

    #[test]
    fn configure_hue_hub_rejects_invalid_credentials_json() {
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));

        let error = configure_hue_hub("192.0.2.10", "{}", &state, |_| {
            panic!("invalid credentials should not connect")
        })
        .unwrap_err();

        assert!(error.to_string().contains("Invalid Hue credentials:"));
    }
}
