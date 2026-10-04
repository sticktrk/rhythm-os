//! Reviewed HA registry identity ownership, isolated from profile imports.
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rhythm_ha::light::{HaLightSelection, SELECTION_FILE};
use rhythm_os::state::SharedState;
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Legacy entity ids never become authority without fresh identity review.
/// Keep a matching rollback snapshot, then seal the old live path so an older
/// binary cannot resume writing devices from a stale entity-id selection.
pub fn load(dir: &str) -> anyhow::Result<BTreeSet<String>> {
    let dir = Path::new(dir);
    let legacy = dir.join("managed-ha-lights.json");
    if legacy.exists() {
        let bytes = std::fs::read(&legacy)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        if value.get("entities").is_some() {
            let backup = dir.join("managed-ha-lights-v1.rollback.json");
            if !backup.exists() {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&backup)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
            } else {
                anyhow::ensure!(
                    std::fs::read(&backup)? == bytes,
                    "Legacy selection differs from preserved rollback snapshot"
                );
            }
            atomic_write(
                dir,
                "managed-ha-lights.json",
                &json!({"version":2,"requires_identity_review":true}),
            )?;
        }
    }
    HaLightSelection::load(
        dir.to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid data path"))?,
    )?;
    Ok(BTreeSet::new())
}

/// Preserve enabled user intent across restart while runtime identity checks
/// keep all output fenced until fresh HA reconciliation.
pub fn has_reviewed_lights(dir: &str) -> anyhow::Result<bool> {
    Ok(!HaLightSelection::load(dir)?.lights.is_empty())
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

fn cache(
    s: &rhythm_os::state::AppState,
) -> Option<std::sync::Arc<std::sync::Mutex<rhythm_ha::hub_state::HaEventRoutingCache>>> {
    s.hubs
        .values()
        .find_map(|hub| hub.data::<rhythm_ha::hub_state::HaHubData>())
        .map(|ha| ha.event_routing_cache.clone())
}

pub async fn get(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.lock().expect("state lock");
    let cache = cache(&s);
    let cache = cache.as_ref().and_then(|cache| cache.lock().ok());
    let available: Vec<_> = cache
        .as_ref()
        .map(|cache| {
            cache.lights.iter().map(|(entity_id, entry)| json!({
        "entity_id": entity_id, "name": entry.name, "area_id": entry.area_id,
        "identity": entry.identity, "reviewable": entry.identity.is_some() && cache.lights_ready,
        "observation": entry.observation, "capabilities": entry.capabilities,
    })).collect()
        })
        .unwrap_or_default();
    let unresolved: Vec<_> = cache
        .as_ref()
        .map(|cache| {
            cache
                .reviewed
                .iter()
                .filter(|(_, proof)| {
                    !cache
                        .lights
                        .values()
                        .any(|entry| entry.identity.as_ref() == Some(proof))
                })
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default();
    Json(
        json!({"entities": s.managed_ha_lights, "available": available, "unresolved": unresolved, "ready": cache.as_ref().is_some_and(|cache| cache.lights_ready), "snapshot_revision": cache.as_ref().map(|cache| format!("{}:{}", cache.snapshot_revision, cache.generation)), "snapshot_ready": cache.as_ref().is_some_and(|cache| cache.lights_ready), "selection_version": 2}),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    entities: BTreeSet<String>,
    expected_entities: BTreeSet<String>,
    expected_snapshot_revision: Option<String>,
}

fn atomic_write(dir: &Path, name: &str, value: &impl serde::Serialize) -> anyhow::Result<()> {
    use std::io::Write;
    let tmp = dir.join(format!("{name}.tmp"));
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    std::fs::rename(tmp, dir.join(name))?;
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

pub async fn put(State(state): State<SharedState>, Json(update): Json<Update>) -> Response {
    let mut s = state.lock().expect("state lock");
    if s.managed_ha_lights.as_ref() != Some(&update.expected_entities) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"Light selection changed. Reload before saving."})),
        )
            .into_response();
    }
    if s.light_breaker_enabled {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"Pause Rhythm before changing managed lights."})),
        )
            .into_response();
    }
    let Some(cache) = cache(&s) else {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"Wait for a complete Home Assistant inventory."})),
        )
            .into_response();
    };
    let mut cache = cache.lock().expect("HA cache lock");
    let Some(expected_revision) = update.expected_snapshot_revision.as_deref() else {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            Json(json!({"error":"Reload the light catalog before saving."})),
        )
            .into_response();
    };
    if expected_revision != format!("{}:{}", cache.snapshot_revision, cache.generation) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"Home Assistant devices changed. Review the new catalog."})),
        )
            .into_response();
    }
    let removing_only = update.entities.is_subset(&update.expected_entities);
    if (!cache.lights_ready && !removing_only) || !valid(&update.entities) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"Reload after Home Assistant reconciliation completes."})),
        )
            .into_response();
    }
    let mut lights = BTreeMap::new();
    for id in &update.entities {
        let Some(identity) = cache
            .lights
            .get(id)
            .and_then(|entry| entry.identity.clone())
        else {
            return (StatusCode::BAD_REQUEST, Json(json!({"error":"Select discovered lights with proven Home Assistant registry identities."}))).into_response();
        };
        if removing_only && !cache.reviewed.values().any(|proof| proof == &identity) {
            return (StatusCode::CONFLICT, Json(json!({"error":"Selected device identity changed. Remove it before reviewing its replacement."}))).into_response();
        }
        lights.insert(id.clone(), identity);
    }
    let selection = HaLightSelection { version: 2, lights };
    let result = (|| -> anyhow::Result<()> {
        // Seal before committing authority, including clean installations.
        atomic_write(
            Path::new(&s.data_dir),
            "managed-ha-lights.json",
            &json!({"version":2,"requires_identity_review":true}),
        )?;
        atomic_write(Path::new(&s.data_dir), SELECTION_FILE, &selection)
    })();
    if result.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Could not persist selection. Reload before retrying."})),
        )
            .into_response();
    }
    cache.reviewed = selection.lights;
    cache.ownership_revision = cache.ownership_revision.wrapping_add(1);
    s.managed_ha_lights = Some(update.entities);
    Json(json!({"entities":s.managed_ha_lights})).into_response()
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

#[cfg(test)]
mod migration_tests {
    use super::*;
    fn dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ha-selection-{}",
            rhythm_os::canonical::identity::generate_uuid_public()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    #[test]
    fn legacy_selection_is_preserved_sealed_and_never_auto_approved() {
        let dir = dir();
        let original = br#"{"entities":["light.old"]}"#;
        std::fs::write(dir.join("managed-ha-lights.json"), original).unwrap();
        assert!(load(dir.to_str().unwrap()).unwrap().is_empty());
        assert_eq!(
            std::fs::read(dir.join("managed-ha-lights-v1.rollback.json")).unwrap(),
            original
        );
        assert!(!has_reviewed_lights(dir.to_str().unwrap()).unwrap());
        assert!(load(dir.to_str().unwrap()).unwrap().is_empty());
        let live: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("managed-ha-lights.json")).unwrap())
                .unwrap();
        assert_eq!(live["version"], 2);
        assert!(live.get("entities").is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn future_schema_and_corrupt_versioned_selection_fail_closed() {
        let dir = dir();
        for bytes in [br#"{"version":3,"lights":{}}"#.as_slice(), b"broken"] {
            std::fs::write(dir.join(SELECTION_FILE), bytes).unwrap();
            assert!(load(dir.to_str().unwrap()).is_err());
            assert!(has_reviewed_lights(dir.to_str().unwrap()).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
