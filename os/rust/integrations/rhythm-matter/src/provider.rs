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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc, Mutex};

    use rhythm_os::canonical::identity::HubKey;
    use rhythm_os::hub::HubType;

    fn active_hub(address: &str) -> (ActiveHub, Receiver<HubEvent>) {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let hub_type = HubType::new("matter");
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
    fn matter_credentials_round_trip_fabric_id() {
        let creds = matter_credentials("local", "fabric-a");

        assert_eq!(
            creds.hub_type.as_ref().map(|ty| ty.as_str()),
            Some("matter")
        );
        assert_eq!(creds.address, "local");
        assert_eq!(matter_fabric_id(&creds), Some("fabric-a"));
    }

    #[test]
    fn configure_matter_hub_defaults_fabric_and_skips_same_credentials() {
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        let calls = Arc::new(Mutex::new(0usize));
        let first_calls = calls.clone();

        configure_matter_hub("local", "not valid json", &state, move |_| {
            *first_calls.lock().unwrap() += 1;
            Ok(active_hub("local"))
        })
        .unwrap();

        assert_eq!(*calls.lock().unwrap(), 1);
        let key = HubKey::new(HubType::new("matter"), "local");
        assert_eq!(
            state
                .lock()
                .unwrap()
                .hub_credentials
                .get(&key)
                .and_then(matter_fabric_id),
            Some("default")
        );

        configure_matter_hub("local", r#"{"fabric_id":"default"}"#, &state, |_| {
            panic!("same Matter credentials should short-circuit")
        })
        .unwrap();
    }

    #[test]
    fn configure_matter_hub_reconfigures_when_fabric_changes() {
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));

        configure_matter_hub("local", r#"{"fabric_id":"fabric-a"}"#, &state, |_| {
            Ok(active_hub("local"))
        })
        .unwrap();
        configure_matter_hub("local", r#"{"fabric_id":"fabric-b"}"#, &state, |_| {
            Ok(active_hub("local"))
        })
        .unwrap();

        let key = HubKey::new(HubType::new("matter"), "local");
        assert_eq!(
            state
                .lock()
                .unwrap()
                .hub_credentials
                .get(&key)
                .and_then(matter_fabric_id),
            Some("fabric-b")
        );
    }
}
