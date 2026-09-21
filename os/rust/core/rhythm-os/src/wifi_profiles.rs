//! Owner-only provisioning profiles. Never include this document in snapshots,
//! debug bundles or backups. The legacy fields contain only the current default.

use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{handlers::ApiResponse, provisioning::WifiCredentials, state::SharedState};

pub(crate) static PROFILE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const MAX_PROFILES: usize = 16;

#[derive(Clone, Serialize, Deserialize)]
pub struct WifiProfile {
    pub id: String,
    pub ssid: String,
    pub password: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WifiProfileStore {
    /// Kept for previous-version readers. Empty when the managed catalog is empty.
    pub ssid: String,
    pub password: String,
    pub profile_schema: u32,
    pub revision: u64,
    pub default_id: Option<String>,
    pub profiles: Vec<WifiProfile>,
}

impl WifiProfileStore {
    pub fn from_legacy(wifi: Option<WifiCredentials>) -> Self {
        let profiles = wifi
            .filter(|wifi| !wifi.ssid.is_empty())
            .map(|wifi| {
                vec![WifiProfile {
                    id: uuid::Uuid::new_v4().to_string(),
                    ssid: wifi.ssid,
                    password: wifi.password,
                }]
            })
            .unwrap_or_default();
        let mut store = Self {
            ssid: String::new(),
            password: String::new(),
            profile_schema: 1,
            revision: 0,
            default_id: profiles.first().map(|p| p.id.clone()),
            profiles,
        };
        store.sync_default();
        store
    }

    fn sync_default(&mut self) {
        let selected = self
            .default_id
            .as_ref()
            .and_then(|id| self.profiles.iter().find(|p| &p.id == id));
        self.ssid = selected.map(|p| p.ssid.clone()).unwrap_or_default();
        self.password = selected.map(|p| p.password.clone()).unwrap_or_default();
    }

    pub fn validate(&self) -> Result<()> {
        if self.profile_schema != 1 || self.profiles.len() > MAX_PROFILES {
            bail!("unsupported profile document");
        }
        let mut ids = std::collections::HashSet::new();
        for profile in &self.profiles {
            if uuid::Uuid::parse_str(&profile.id).is_err() || !ids.insert(&profile.id) {
                bail!("invalid profile identity");
            }
            validate_credentials(&WifiCredentials {
                ssid: profile.ssid.clone(),
                password: profile.password.clone(),
            })?;
        }
        let default = self
            .default_id
            .as_ref()
            .and_then(|id| self.profiles.iter().find(|p| &p.id == id));
        if self.profiles.is_empty() {
            if self.default_id.is_some() || !self.ssid.is_empty() || !self.password.is_empty() {
                bail!("invalid empty catalog");
            }
        } else if default.is_none_or(|p| p.ssid != self.ssid || p.password != self.password) {
            bail!("invalid default");
        }
        Ok(())
    }

    pub fn public_view(&self) -> Value {
        json!({"revision": self.revision, "default_id": self.default_id,
            "profiles": self.profiles.iter().map(|p| json!({"id": p.id, "ssid": p.ssid})).collect::<Vec<_>>()})
    }

    pub fn credentials(&self, id: Option<&str>) -> Result<Option<WifiCredentials>> {
        let id = id.or(self.default_id.as_deref());
        match id {
            Some(id) => self
                .profiles
                .iter()
                .find(|p| p.id == id)
                .map(|p| {
                    Some(WifiCredentials {
                        ssid: p.ssid.clone(),
                        password: p.password.clone(),
                    })
                })
                .ok_or_else(|| anyhow::anyhow!("Saved network no longer exists")),
            None => Ok(None),
        }
    }
}

pub fn validate_credentials(wifi: &WifiCredentials) -> Result<()> {
    if wifi.ssid.is_empty() || wifi.ssid.len() > 32 || wifi.ssid.contains('\0') {
        bail!("Network name must contain 1–32 UTF-8 bytes");
    }
    if wifi.password.contains('\0')
        || !(wifi.password.is_empty()
            || (8..=63).contains(&wifi.password.len())
            || (wifi.password.len() == 64 && wifi.password.bytes().all(|b| b.is_ascii_hexdigit())))
    {
        bail!("Use an open network, an 8–63 byte password, or a 64-digit hexadecimal key");
    }
    Ok(())
}

fn load_or_seed(state: &SharedState) -> Result<WifiProfileStore> {
    let (storage, provider) = {
        let state = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state unavailable"))?;
        (
            state
                .storage
                .clone()
                .ok_or_else(|| anyhow::anyhow!("storage unavailable"))?,
            state.commissioning_wifi_credentials_provider.clone(),
        )
    };
    if let Some(store) = storage.load_wifi_profiles()? {
        store.validate()?;
        return Ok(store);
    }
    let legacy = storage.load_commissioning_wifi_credentials()?;
    let wifi = match (legacy, provider) {
        (Some(wifi), _) => Some(wifi),
        (None, Some(provider)) => provider()?,
        _ => None,
    };
    let store = WifiProfileStore::from_legacy(wifi);
    store.validate()?;
    storage.save_wifi_profiles(&store)?;
    Ok(store)
}

pub fn selected_credentials(state: &SharedState, id: &str) -> Result<WifiCredentials> {
    let _guard = PROFILE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("profiles unavailable"))?;
    load_or_seed(state)?
        .credentials(Some(id))?
        .ok_or_else(|| anyhow::anyhow!("No saved network"))
}

pub fn handle_list(state: &SharedState) -> ApiResponse {
    let Ok(_guard) = PROFILE_LOCK.get_or_init(|| Mutex::new(())).lock() else {
        return unavailable();
    };
    match load_or_seed(state) {
        Ok(store) => ApiResponse::json_ok(store.public_view().to_string()),
        Err(_) => unavailable(),
    }
}

fn unavailable() -> ApiResponse {
    ApiResponse::server_error("Saved networks are unavailable")
}

/// One revision-checked edit. Omitted password keeps the secret during an edit;
/// an explicit empty password means an open network. Removal deterministically
/// promotes the first remaining profile without moving the Box.
pub fn handle_update(state: &SharedState, body: &Value) -> ApiResponse {
    let response = update_profile(state, body);
    if response.status != 200 {
        if let Some(correlation) = body
            .get("correlation_id")
            .and_then(Value::as_str)
            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        {
            record_outcome(
                state,
                "wifi_profile",
                correlation,
                if response.status == 409 {
                    "conflict"
                } else {
                    "rejected"
                },
            );
        }
    }
    response
}

fn update_profile(state: &SharedState, body: &Value) -> ApiResponse {
    let correlation = body
        .get("correlation_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if uuid::Uuid::parse_str(correlation).is_err() {
        return ApiResponse::bad_request("A UUID correlation_id is required");
    }
    let Ok(_guard) = PROFILE_LOCK.get_or_init(|| Mutex::new(())).lock() else {
        return unavailable();
    };
    let mut store = match load_or_seed(state) {
        Ok(store) => store,
        Err(_) => return unavailable(),
    };
    if body.get("revision").and_then(Value::as_u64) != Some(store.revision) {
        return ApiResponse {
            status: 409,
            content_type: "application/json",
            body: json!({"error":"Saved networks changed; reload before editing"}).to_string(),
        };
    }
    let action = body.get("action").and_then(Value::as_str).unwrap_or("");
    let id = body.get("id").and_then(Value::as_str);
    let index = id.and_then(|id| store.profiles.iter().position(|p| p.id == id));
    match action {
        "save" => {
            if id.is_some() && index.is_none() {
                return ApiResponse::not_found("Saved network no longer exists");
            }
            if index.is_none() && store.profiles.len() >= MAX_PROFILES {
                return ApiResponse::bad_request("Remove a saved network before adding another");
            }
            let wifi = WifiCredentials {
                ssid: body
                    .get("ssid")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                password: match body.get("password") {
                    Some(Value::String(password)) => password.clone(),
                    None if index.is_some() => store.profiles[index.unwrap()].password.clone(),
                    _ => {
                        return ApiResponse::bad_request(
                            "A password is required; use an empty string for an open network",
                        )
                    }
                },
            };
            if let Err(error) = validate_credentials(&wifi) {
                return ApiResponse::bad_request(&error.to_string());
            }
            let profile = WifiProfile {
                id: id
                    .map(str::to_string)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                ssid: wifi.ssid,
                password: wifi.password,
            };
            if store.default_id.is_none() {
                store.default_id = Some(profile.id.clone());
            }
            if let Some(index) = index {
                store.profiles[index] = profile;
            } else {
                store.profiles.push(profile);
            }
        }
        "remove" => {
            let Some(index) = index else {
                return ApiResponse::not_found("Saved network no longer exists");
            };
            let removed = store.profiles.remove(index);
            if store.default_id.as_deref() == Some(&removed.id) {
                store.default_id = store.profiles.first().map(|p| p.id.clone());
            }
        }
        "default" => {
            let Some(index) = index else {
                return ApiResponse::not_found("Saved network no longer exists");
            };
            store.default_id = Some(store.profiles[index].id.clone());
        }
        _ => return ApiResponse::bad_request("Unknown saved network action"),
    }
    let Some(revision) = store.revision.checked_add(1) else {
        return unavailable();
    };
    store.revision = revision;
    store.sync_default();
    let storage = state.lock().ok().and_then(|state| state.storage.clone());
    if storage.is_none_or(|storage| storage.save_wifi_profiles(&store).is_err()) {
        return unavailable();
    }
    drop(_guard);
    record_outcome(state, "wifi_profile", correlation, action);
    ApiResponse::json_ok(store.public_view().to_string())
}

pub fn record_outcome(state: &SharedState, action: &str, correlation: &str, outcome: &str) {
    use crate::pairing::PairingHistoryEntry;
    crate::pairing::record_pairing_history(
        state,
        PairingHistoryEntry {
            at: chrono::Utc::now().to_rfc3339(),
            epoch_ms: crate::state::current_epoch_ms(),
            kind: action.to_string(),
            hub_type: "matter".into(),
            correlation_id: Some(correlation.to_string()),
            device_type: None,
            profile_id: None,
            device_id: None,
            force: None,
            rendezvous: None,
            network: None,
            status: outcome.into(),
            error: None,
            failure_stage: None,
            device: None,
            devices: Vec::new(),
            warnings: Vec::new(),
        },
    );
}
