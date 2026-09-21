//! Add-on-owned entity opt-in, outside general profile imports.
use std::collections::BTreeSet;
use std::path::Path;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rhythm_os::state::SharedState;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub entities: BTreeSet<String>,
}

pub fn load(dir: &str) -> anyhow::Result<BTreeSet<String>> {
    let path = Path::new(dir).join("managed-ha-lights.json");
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let selection: Selection = serde_json::from_slice(&std::fs::read(path)?)?;
    anyhow::ensure!(
        valid(&selection.entities),
        "Invalid managed HA light selection"
    );
    Ok(selection.entities)
}

fn valid(ids: &BTreeSet<String>) -> bool {
    ids.len() <= 4096
        && ids.iter().all(|id| {
            id.len() <= 256
                && id.strip_prefix("light.").is_some_and(|s| {
                    !s.is_empty()
                        && s.bytes()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
                })
        })
}

fn catalog(s: &rhythm_os::state::AppState) -> Vec<serde_json::Value> {
    let mut lights = std::collections::BTreeMap::new();
    for device in s.canonical_registry.devices() {
        for ep in &device.endpoints {
            if ep.hub_key.hub_type.as_str() == "homeassistant" && ep.native_id.starts_with("light.")
            {
                lights.insert(
                    ep.native_id.clone(),
                    json!({"entity_id": ep.native_id, "name": device.name}),
                );
            }
        }
    }
    lights.into_values().collect()
}

pub async fn get(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.lock().expect("state lock");
    Json(json!({"entities": s.managed_ha_lights, "available": catalog(&s)}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    entities: BTreeSet<String>,
    expected_entities: BTreeSet<String>,
}

pub async fn put(State(state): State<SharedState>, Json(update): Json<Update>) -> Response {
    let mut s = state.lock().expect("state lock");
    if s.managed_ha_lights.as_ref() != Some(&update.expected_entities) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "Light selection changed. Reload before saving."})),
        )
            .into_response();
    }
    let known: BTreeSet<String> = catalog(&s)
        .into_iter()
        .filter_map(|v| v["entity_id"].as_str().map(str::to_owned))
        .collect();
    if !valid(&update.entities)
        || !update
            .entities
            .difference(&update.expected_entities)
            .all(|id| known.contains(id))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Select discovered Home Assistant lights."})),
        )
            .into_response();
    }
    // Require pausing before changing ownership. The UI performs this explicitly.
    if s.light_breaker_enabled {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "Pause Rhythm before changing managed lights."})),
        )
            .into_response();
    }
    let result = (|| -> anyhow::Result<()> {
        use std::io::Write;
        let dir = Path::new(&s.data_dir);
        let tmp = dir.join("managed-ha-lights.json.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&serde_json::to_vec(&Selection {
            entities: update.entities.clone(),
        })?)?;
        file.sync_all()?;
        std::fs::rename(tmp, dir.join("managed-ha-lights.json"))?;
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Could not persist selection. Reload before retrying."})),
        )
            .into_response();
    }
    s.managed_ha_lights = Some(update.entities);
    Json(json!({"entities": s.managed_ha_lights})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_empty_on_first_boot_and_rejects_non_lights() {
        assert!(load("/nonexistent-rhythm-test-state").unwrap().is_empty());
        assert!(valid(&["light.kitchen".into()].into()));
        for id in ["switch.fan", "light.", "light.all,light.other", "light.*"] {
            assert!(!valid(&[id.into()].into()));
        }
    }
}
