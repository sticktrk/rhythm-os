//! Hub-owned preferences for the free Rhythm Pro preview.
use crate::{
    auth::{ApiAuthRequestInfo, ApiTokenRole},
    state::SharedState,
};
use axum::{
    extract::{Extension, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProSettings {
    pub bulb_profile_updates: bool,
    pub remote_mcp: bool,
}

impl Default for ProSettings {
    fn default() -> Self {
        // Existing installations already refresh the catalog. Remote agents
        // require an explicit local choice independently of app tunnel access.
        Self {
            bulb_profile_updates: true,
            remote_mcp: false,
        }
    }
}

impl ProSettings {
    pub fn for_platform(platform: &str) -> Self {
        Self {
            bulb_profile_updates: platform != "ha_addon",
            ..Self::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    bulb_profile_updates: Option<bool>,
    remote_mcp: Option<bool>,
}

fn require_local_owner(info: Option<Extension<ApiAuthRequestInfo>>) -> Result<(), StatusCode> {
    if info.is_some_and(|Extension(i)| i.role == Some(ApiTokenRole::Owner) && !i.via_remote_access)
    {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

pub async fn get(
    State(state): State<SharedState>,
    info: Option<Extension<ApiAuthRequestInfo>>,
) -> Result<Json<ProSettings>, StatusCode> {
    require_local_owner(info)?;
    Ok(Json(
        state
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .pro_settings,
    ))
}

pub async fn put(
    State(state): State<SharedState>,
    info: Option<Extension<ApiAuthRequestInfo>>,
    Json(patch): Json<Patch>,
) -> Result<Json<ProSettings>, StatusCode> {
    require_local_owner(info)?;
    if patch.bulb_profile_updates.is_none() && patch.remote_mcp.is_none() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut app = state
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut next = app.pro_settings;
    if let Some(value) = patch.remote_mcp {
        next.remote_mcp = value;
    }
    if let Some(value) = patch.bulb_profile_updates {
        next.bulb_profile_updates = value;
    }
    app.storage
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?
        .save_pro_settings(&next)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if app.pro_settings.bulb_profile_updates != next.bulb_profile_updates {
        app.bulb_catalog_generation += 1;
        app.bulb_catalog_wake.notify_one();
    }
    app.pro_settings = next;
    Ok(Json(next))
}
