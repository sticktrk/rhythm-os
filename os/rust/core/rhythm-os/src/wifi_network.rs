//! Owner-only, uncached device observation. Never persists network names.
use crate::{handlers::ApiResponse, state::SharedState};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WifiNetworkStatus {
    Connected,
    Offline,
    Unsupported,
    Unavailable,
    Busy,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WifiNetwork {
    pub status: WifiNetworkStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssid: Option<String>,
}
impl std::fmt::Debug for WifiNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WifiNetwork")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
impl WifiNetwork {
    pub fn unknown(status: WifiNetworkStatus) -> Self {
        Self { status, ssid: None }
    }
}
pub type ReadWifiNetworkFn = Arc<dyn Fn(&SharedState, &str) -> Result<WifiNetwork> + Send + Sync>;

pub fn handle_read(state: &SharedState, device_id: &str) -> ApiResponse {
    if device_id.is_empty() || device_id.len() > 100 {
        return ApiResponse::bad_request("A registered Matter device is required");
    }
    let callback = state
        .lock()
        .ok()
        .and_then(|s| s.read_wifi_network_fn.clone());
    let result = callback
        .and_then(|read| read(state, device_id).ok())
        .unwrap_or_else(|| WifiNetwork::unknown(WifiNetworkStatus::Unavailable));
    let mut body = serde_json::to_value(&result).unwrap();
    if result.status == WifiNetworkStatus::Connected && result.ssid.is_some() {
        body["observed_at_ms"] = serde_json::json!(crate::state::current_epoch_ms());
    } else {
        body.as_object_mut().unwrap().remove("ssid");
    }
    ApiResponse::json_ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use std::sync::Mutex;
    #[test]
    fn only_a_successful_observation_exposes_the_network_and_never_persists_it() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        assert!(handle_read(&state, "").status == 400);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&handle_read(&state, "matter-1").body)
                .unwrap()["status"],
            "unavailable"
        );
        state.lock().unwrap().read_wifi_network_fn = Some(Arc::new(|_, id| {
            assert_eq!(id, "matter-1");
            Ok(WifiNetwork {
                status: WifiNetworkStatus::Connected,
                ssid: Some("Fixture actual network".into()),
            })
        }));
        let observed: serde_json::Value =
            serde_json::from_str(&handle_read(&state, "matter-1").body).unwrap();
        assert_eq!(observed["ssid"], "Fixture actual network");
        assert!(observed["observed_at_ms"].as_u64().unwrap() > 0);
        for status in [
            WifiNetworkStatus::Offline,
            WifiNetworkStatus::Unsupported,
            WifiNetworkStatus::Busy,
        ] {
            state.lock().unwrap().read_wifi_network_fn = Some(Arc::new(move |_, _| {
                Ok(WifiNetwork {
                    status,
                    ssid: Some("stale network must not escape".into()),
                })
            }));
            let body = handle_read(&state, "matter-1").body;
            assert!(!body.contains("ssid"));
            assert!(!body.contains("observed_at_ms"));
        }
        let private = WifiNetwork {
            status: WifiNetworkStatus::Connected,
            ssid: Some("private fixture".into()),
        };
        assert!(!format!("{private:?}").contains("private fixture"));
    }
}
