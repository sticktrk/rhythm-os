//! Owner-only saved networks shared by the Box connection, accessory
//! provisioning and Matter network changes. Never include this document in
//! snapshots, debug bundles or backups.
//!
//! One catalog carries two independent roles. `box_profile_id` names the
//! network the Box itself joins and is written only by the Box connection path.
//! `default_id` names the network new accessories receive and is chosen by the
//! owner. The legacy top-level fields mirror the Box network when one is known,
//! so a previous-version reader restores the Box to the right network.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{handlers::ApiResponse, provisioning::WifiCredentials, state::SharedState};

pub const MAX_PROFILES: usize = 16;

#[derive(Clone, Serialize, Deserialize)]
pub struct WifiProfile {
    pub id: String,
    pub ssid: String,
    pub password: String,
}

impl WifiProfile {
    fn credentials(&self) -> WifiCredentials {
        WifiCredentials {
            ssid: self.ssid.clone(),
            password: self.password.clone(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WifiProfileStore {
    /// Kept for previous-version readers; see the module documentation.
    pub ssid: String,
    pub password: String,
    pub profile_schema: u32,
    pub revision: u64,
    pub default_id: Option<String>,
    #[serde(default)]
    pub box_profile_id: Option<String>,
    pub profiles: Vec<WifiProfile>,
    /// False for a catalog converted from a legacy credential that has not
    /// been written yet; its generated identities are not stable until saved.
    #[serde(skip)]
    pub persisted: bool,
}

/// Result of recording the network the Box joined.
#[derive(Debug, PartialEq, Eq)]
pub enum BoxNetworkUpdate {
    Unchanged,
    Stored,
    /// The credential could not be represented; the stale Box pointer was
    /// cleared so startup recovery never rejoins the previous network.
    Dropped,
}

impl WifiProfileStore {
    /// A legacy credential was both the Box network and the provisioning default.
    pub fn from_legacy(wifi: Option<WifiCredentials>) -> Self {
        let profiles = wifi
            .filter(|wifi| validate_stored(wifi).is_ok())
            .map(|wifi| {
                vec![WifiProfile {
                    id: uuid::Uuid::new_v4().to_string(),
                    ssid: wifi.ssid,
                    password: wifi.password,
                }]
            })
            .unwrap_or_default();
        let seeded = profiles.first().map(|p| p.id.clone());
        let mut store = Self {
            ssid: String::new(),
            password: String::new(),
            profile_schema: 1,
            revision: 0,
            default_id: seeded.clone(),
            box_profile_id: seeded,
            profiles,
            persisted: false,
        };
        store.sync_legacy();
        store
    }

    fn find(&self, id: Option<&str>) -> Option<&WifiProfile> {
        id.and_then(|id| self.profiles.iter().find(|p| p.id == id))
    }

    fn legacy_mirror(&self) -> Option<&WifiProfile> {
        self.find(self.box_profile_id.as_deref())
            .or_else(|| self.find(self.default_id.as_deref()))
    }

    fn sync_legacy(&mut self) {
        let mirror = self.legacy_mirror().map(WifiProfile::credentials);
        self.ssid = mirror.as_ref().map(|w| w.ssid.clone()).unwrap_or_default();
        self.password = mirror.map(|w| w.password).unwrap_or_default();
    }

    fn bump(&mut self) -> Result<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("saved network revision exhausted"))?;
        self.sync_legacy();
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.profile_schema != 1 || self.profiles.len() > MAX_PROFILES {
            bail!("unsupported profile document");
        }
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for profile in &self.profiles {
            if uuid::Uuid::parse_str(&profile.id).is_err() || !ids.insert(&profile.id) {
                bail!("invalid profile identity");
            }
            if !names.insert(&profile.ssid) {
                bail!("duplicate saved network");
            }
            validate_stored(&profile.credentials())?;
        }
        if self.box_profile_id.is_some() && self.find(self.box_profile_id.as_deref()).is_none() {
            bail!("invalid Box network");
        }
        if self.default_id.is_some() == self.profiles.is_empty()
            || (self.default_id.is_some() && self.find(self.default_id.as_deref()).is_none())
        {
            bail!("invalid default");
        }
        let mirror = self.legacy_mirror();
        if mirror.map(|p| p.ssid.as_str()).unwrap_or("") != self.ssid
            || mirror.map(|p| p.password.as_str()).unwrap_or("") != self.password
        {
            bail!("invalid legacy credential mirror");
        }
        Ok(())
    }

    pub fn public_view(&self) -> Value {
        json!({"revision": self.revision, "default_id": self.default_id,
            "box_profile_id": self.box_profile_id,
            "profiles": self.profiles.iter().map(|p| json!({"id": p.id, "ssid": p.ssid})).collect::<Vec<_>>()})
    }

    /// Accessory credentials: an explicit profile or the provisioning default.
    pub fn credentials(&self, id: Option<&str>) -> Result<Option<WifiCredentials>> {
        match id.or(self.default_id.as_deref()) {
            Some(id) => self
                .find(Some(id))
                .map(|p| Some(p.credentials()))
                .ok_or_else(|| anyhow::anyhow!("Saved network no longer exists")),
            None => Ok(None),
        }
    }

    /// The network the Box itself restores at startup. Never the provisioning
    /// default: choosing where accessories go must not move the Box.
    pub fn box_credentials(&self) -> Option<WifiCredentials> {
        self.find(self.box_profile_id.as_deref())
            .map(WifiProfile::credentials)
    }

    /// Record the network the Box joined. A provisioning default that was
    /// following the Box keeps following it; an owner-chosen default stays.
    pub fn set_box_network(&mut self, wifi: &WifiCredentials) -> Result<BoxNetworkUpdate> {
        let before = (
            self.box_profile_id.clone(),
            self.default_id.clone(),
            self.box_credentials(),
        );
        let follows = self.default_id.is_none() || self.default_id == self.box_profile_id;
        let existing = self.profiles.iter().position(|p| p.ssid == wifi.ssid);
        let id = if validate_stored(wifi).is_err() {
            None
        } else if let Some(index) = existing {
            self.profiles[index].password = wifi.password.clone();
            Some(self.profiles[index].id.clone())
        } else if self.profiles.len() < MAX_PROFILES {
            let id = uuid::Uuid::new_v4().to_string();
            self.profiles.push(WifiProfile {
                id: id.clone(),
                ssid: wifi.ssid.clone(),
                password: wifi.password.clone(),
            });
            Some(id)
        } else {
            None
        };
        let stored = id.is_some();
        if follows && stored {
            self.default_id = id.clone();
        }
        self.box_profile_id = id;
        if before
            == (
                self.box_profile_id.clone(),
                self.default_id.clone(),
                self.box_credentials(),
            )
        {
            return Ok(if stored {
                BoxNetworkUpdate::Unchanged
            } else {
                BoxNetworkUpdate::Dropped
            });
        }
        self.bump()?;
        Ok(if stored {
            BoxNetworkUpdate::Stored
        } else {
            BoxNetworkUpdate::Dropped
        })
    }

    /// The Box forgot its network. Saved networks and the default remain.
    pub fn clear_box_network(&mut self) -> Result<bool> {
        if self.box_profile_id.take().is_none() {
            return Ok(false);
        }
        self.bump()?;
        Ok(true)
    }
}

/// What the document may hold. The Box connection is recorded as the Box
/// joined it, including security modes an accessory cannot be given, so only
/// structure is checked. Owner-entered networks use [`validate_credentials`].
fn validate_stored(wifi: &WifiCredentials) -> Result<()> {
    if wifi.ssid.is_empty()
        || wifi.ssid.len() > 32
        || wifi.password.len() > 64
        || wifi.ssid.contains('\0')
        || wifi.password.contains('\0')
    {
        bail!("invalid saved network");
    }
    Ok(())
}

/// What an accessory can be provisioned with.
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

type Provider = Option<std::sync::Arc<dyn Fn() -> Result<Option<WifiCredentials>> + Send + Sync>>;

fn storage_and_provider(
    state: &SharedState,
) -> Result<(std::sync::Arc<dyn crate::storage::Storage>, Provider)> {
    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state unavailable"))?;
    Ok((
        state
            .storage
            .clone()
            .ok_or_else(|| anyhow::anyhow!("storage unavailable"))?,
        state.commissioning_wifi_credentials_provider.clone(),
    ))
}

/// An absent document seeds from the platform's current connection.
fn seeded(current: Option<WifiProfileStore>, provider: &Provider) -> Result<WifiProfileStore> {
    match current {
        Some(store) => Ok(store),
        None => Ok(WifiProfileStore::from_legacy(match provider {
            Some(provider) => provider()?,
            None => None,
        })),
    }
}

pub fn selected_credentials(state: &SharedState, id: &str) -> Result<WifiCredentials> {
    let (storage, _) = storage_and_provider(state)?;
    storage
        .load_wifi_profiles()?
        .ok_or_else(|| anyhow::anyhow!("No saved network"))?
        .credentials(Some(id))?
        .ok_or_else(|| anyhow::anyhow!("No saved network"))
}

pub fn handle_list(state: &SharedState) -> ApiResponse {
    let mut view = None;
    let result = storage_and_provider(state).and_then(|(storage, provider)| {
        storage.mutate_wifi_profiles(&mut |current| {
            let store = seeded(current, &provider)?;
            view = Some(store.public_view());
            // Identities handed to a client must survive the next read.
            Ok((!store.persisted).then_some(store))
        })
    });
    match (result, view) {
        (Ok(()), Some(view)) => ApiResponse::json_ok(view.to_string()),
        _ => unavailable(),
    }
}

fn unavailable() -> ApiResponse {
    ApiResponse::server_error("Saved networks are unavailable")
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProfileAction {
    Save,
    Remove,
    Default,
}

impl ProfileAction {
    fn outcome(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Remove => "remove",
            Self::Default => "default",
        }
    }
}

#[derive(Deserialize)]
struct ProfileUpdate {
    revision: u64,
    correlation_id: String,
    action: ProfileAction,
    id: Option<String>,
    ssid: Option<String>,
    /// Omitted keeps the secret during an edit; empty means an open network.
    password: Option<String>,
}

enum Rejection {
    Stale,
    Missing,
    Invalid(String),
}

const BOX_MANAGED: &str =
    "This network is the Rhythm Box connection. Change it from the Box network settings";

impl ProfileUpdate {
    fn apply(&self, store: &mut WifiProfileStore) -> std::result::Result<(), Rejection> {
        if self.revision != store.revision {
            return Err(Rejection::Stale);
        }
        let index = match &self.id {
            Some(id) => Some(
                store
                    .profiles
                    .iter()
                    .position(|p| &p.id == id)
                    .ok_or(Rejection::Missing)?,
            ),
            None => None,
        };
        let is_box = self.id.is_some() && self.id == store.box_profile_id;
        match (self.action, index) {
            (ProfileAction::Save, index) => {
                // The Box proved this credential by joining with it. Editing it
                // here would retarget startup recovery without moving the Box.
                if is_box {
                    return Err(Rejection::Invalid(BOX_MANAGED.into()));
                }
                if index.is_none() && store.profiles.len() >= MAX_PROFILES {
                    return Err(Rejection::Invalid(
                        "Remove a saved network before adding another".into(),
                    ));
                }
                let wifi =
                    WifiCredentials {
                        ssid: self.ssid.clone().unwrap_or_default(),
                        password: match (&self.password, index) {
                            (Some(password), _) => password.clone(),
                            (None, Some(index)) => store.profiles[index].password.clone(),
                            (None, None) => return Err(Rejection::Invalid(
                                "A password is required; use an empty string for an open network"
                                    .into(),
                            )),
                        },
                    };
                validate_credentials(&wifi).map_err(|e| Rejection::Invalid(e.to_string()))?;
                if store
                    .profiles
                    .iter()
                    .enumerate()
                    .any(|(i, p)| Some(i) != index && p.ssid == wifi.ssid)
                {
                    return Err(Rejection::Invalid(
                        "This network is already saved; edit the existing entry".into(),
                    ));
                }
                match index {
                    Some(index) => {
                        store.profiles[index].ssid = wifi.ssid;
                        store.profiles[index].password = wifi.password;
                    }
                    None => {
                        let id = uuid::Uuid::new_v4().to_string();
                        if store.default_id.is_none() {
                            store.default_id = Some(id.clone());
                        }
                        store.profiles.push(WifiProfile {
                            id,
                            ssid: wifi.ssid,
                            password: wifi.password,
                        });
                    }
                }
            }
            (ProfileAction::Remove, Some(index)) => {
                if is_box {
                    return Err(Rejection::Invalid(BOX_MANAGED.into()));
                }
                // Never pick the network new accessories join on the owner's behalf.
                if self.id == store.default_id && store.profiles.len() > 1 {
                    return Err(Rejection::Invalid(
                        "Choose another default network before removing this one".into(),
                    ));
                }
                store.profiles.remove(index);
                if self.id == store.default_id {
                    store.default_id = None;
                }
            }
            (ProfileAction::Default, Some(_)) => store.default_id = self.id.clone(),
            (_, None) => return Err(Rejection::Missing),
        }
        store.bump().map_err(|e| Rejection::Invalid(e.to_string()))
    }
}

/// One revision-checked owner edit. Nothing here moves the Box.
pub fn handle_update(state: &SharedState, body: &Value) -> ApiResponse {
    let correlation = body
        .get("correlation_id")
        .and_then(Value::as_str)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok());
    let Some(correlation) = correlation else {
        return ApiResponse::bad_request("A UUID correlation_id is required");
    };
    let (response, outcome) = match serde_json::from_value::<ProfileUpdate>(body.clone()) {
        Ok(update) => update_profile(state, &update),
        Err(_) => (
            ApiResponse::bad_request("Invalid saved network request"),
            "rejected",
        ),
    };
    record_outcome(state, "wifi_profile", correlation, outcome);
    response
}

fn update_profile(state: &SharedState, update: &ProfileUpdate) -> (ApiResponse, &'static str) {
    debug_assert!(uuid::Uuid::parse_str(&update.correlation_id).is_ok());
    let mut applied = None;
    let result = storage_and_provider(state).and_then(|(storage, provider)| {
        storage.mutate_wifi_profiles(&mut |current| {
            let mut store = seeded(current, &provider)?;
            let result = update.apply(&mut store);
            let save = result.is_ok().then(|| store.clone());
            applied = Some(result.map(|()| store.public_view()));
            Ok(save)
        })
    });
    match (result, applied) {
        (Ok(()), Some(Ok(view))) => (
            ApiResponse::json_ok(view.to_string()),
            update.action.outcome(),
        ),
        (Ok(()), Some(Err(Rejection::Stale))) => (
            ApiResponse::conflict("Saved networks changed; reload before editing"),
            "conflict",
        ),
        (Ok(()), Some(Err(Rejection::Missing))) => (
            ApiResponse::not_found("Saved network no longer exists"),
            "rejected",
        ),
        (Ok(()), Some(Err(Rejection::Invalid(message)))) => {
            (ApiResponse::bad_request(&message), "rejected")
        }
        _ => (unavailable(), "rejected"),
    }
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
