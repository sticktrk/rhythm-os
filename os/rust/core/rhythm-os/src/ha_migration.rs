//! Offline, non-mutating migration review. A candidate match is never ownership.
use anyhow::{ensure, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::bundle::{BackupBundle, BundleKind, ProfileBundle, ProfileBundleData};
use crate::canonical::identity::{CanonicalDevice, HardwareId};
use crate::scenes::{is_native_scene_id, SceneSource};

#[derive(Debug, Serialize)]
pub struct DeviceReview {
    pub source_device_id: String,
    pub source_room_id: Option<String>,
    pub source_integrations: Vec<String>,
    pub candidate_entities: Vec<String>,
    pub outcome: &'static str,
    pub method: &'static str,
}

#[derive(Debug, Serialize)]
pub struct MigrationPreview {
    pub format: &'static str,
    pub version: u32,
    pub input_sha256: String,
    pub phase: &'static str,
    pub light_breaker_enabled: bool,
    pub managed_entities: Vec<String>,
    pub profiles: ProfileBundle,
    pub devices: Vec<DeviceReview>,
    pub rooms_requiring_assignment: Vec<String>,
    pub excluded_native_scene_ids: Vec<String>,
    pub stripped_scene_extension_ids: Vec<String>,
    pub required_reviews: Vec<&'static str>,
}

fn hardware_address(value: &str) -> Option<String> {
    let value = value.trim();
    if !value
        .bytes()
        .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'-' | b'.'))
    {
        return None;
    }
    let hex: String = value
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>()
        .to_ascii_lowercase();
    if !matches!(hex.len(), 12 | 16)
        || hex.bytes().all(|b| b == b'0')
        || hex.bytes().all(|b| b == b'f')
    {
        return None;
    }
    Some(hex)
}

fn exact_hardware_candidate(source: &CanonicalDevice, target: &CanonicalDevice) -> bool {
    source.device_type == target.device_type && source.hardware_ids.iter().any(|id| match id {
        // Matter node IDs are fabric-scoped and are never migration evidence.
        HardwareId::MatterId(_) => false,
        HardwareId::Mac(value) => hardware_address(value).is_some_and(|address| target.hardware_ids.iter().any(|other| {
            matches!(other, HardwareId::Mac(other) if hardware_address(other).as_ref() == Some(&address))
        })),
        HardwareId::Serial(value) => !value.trim().is_empty()
            && source.manufacturer.as_ref().is_some_and(|v| !v.trim().is_empty())
            && source.manufacturer == target.manufacturer
            && source.model.as_ref().is_some_and(|v| !v.trim().is_empty())
            && source.model == target.model
            && target.hardware_ids.contains(id),
    })
}

/// Inputs are exported snapshots, never network endpoints. No credentials,
/// routing policies or mutable runtime state are carried into the output.
pub fn preview(backup_bytes: &[u8], catalog_bytes: &[u8]) -> Result<MigrationPreview> {
    ensure!(
        backup_bytes.len() <= 32 * 1024 * 1024 && catalog_bytes.len() <= 8 * 1024 * 1024,
        "Migration input exceeds supported size"
    );
    let backup: BackupBundle = serde_json::from_slice(backup_bytes)?;
    ensure!(
        backup.kind == BundleKind::BackupBundle && (1..=3).contains(&backup.schema_version),
        "Unsupported backup schema"
    );
    let targets: Vec<CanonicalDevice> = serde_json::from_slice(catalog_bytes)?;
    ensure!(targets.len() <= 4096, "Too many HA target devices");
    let mut devices = Vec::new();
    for source in backup.installation.canonical_registry.devices() {
        let mut candidate_entities: Vec<_> = targets
            .iter()
            .filter(|target| !target.is_removed() && exact_hardware_candidate(source, target))
            .flat_map(|target| target.endpoints.iter())
            .filter(|ep| ep.active && ep.hub_key.hub_type.as_str() == "homeassistant")
            .map(|ep| ep.native_id.clone())
            .collect();
        candidate_entities.sort();
        candidate_entities.dedup();
        let mut integrations: Vec<_> = source
            .endpoints
            .iter()
            .filter(|ep| ep.active)
            .map(|ep| ep.hub_key.hub_type.as_str().to_owned())
            .collect();
        integrations.sort();
        integrations.dedup();
        let method = if integrations.iter().any(|s| s == "matter") {
            "ha_multi_admin_then_verify_thread_transport"
        } else if integrations.iter().any(|s| s == "hue") {
            "configure_ha_bridge_then_verify_lights_and_inputs"
        } else if integrations.iter().all(|s| s == "homeassistant") && !integrations.is_empty() {
            "review_ha_registry_identity"
        } else {
            "verify_exact_model_ha_integration_support"
        };
        devices.push(DeviceReview {
            source_device_id: source.id.clone(),
            source_room_id: source.room_id.clone(),
            source_integrations: integrations,
            outcome: match candidate_entities.len() {
                0 => "no_proven_counterpart",
                1 => "candidate_requires_review",
                _ => "ambiguous_requires_review",
            },
            candidate_entities,
            method,
        });
    }
    devices.sort_by(|a, b| a.source_device_id.cmp(&b.source_device_id));
    let config = backup.configuration;
    let mut excluded = Vec::new();
    let mut stripped = Vec::new();
    let scenes = config
        .scenes
        .into_iter()
        .filter_map(|mut scene| {
            if is_native_scene_id(&scene.id) || matches!(scene.source, SceneSource::Imported { .. })
            {
                excluded.push(scene.id);
                None
            } else {
                if !scene.extensions.is_empty() {
                    stripped.push(scene.id.clone());
                    scene.extensions.clear();
                }
                Some(scene)
            }
        })
        .collect();
    excluded.sort();
    stripped.sort();
    let mut room_ids: Vec<_> = config.rooms.into_iter().map(|room| room.id).collect();
    room_ids.sort();
    room_ids.dedup();
    let mut digest = Sha256::new();
    digest.update((backup_bytes.len() as u64).to_le_bytes());
    digest.update(backup_bytes);
    digest.update(catalog_bytes);
    Ok(MigrationPreview {
        format: "rhythm-ha-migration-preview",
        version: 1,
        input_sha256: format!("{:x}", digest.finalize()),
        phase: "review_required",
        light_breaker_enabled: false,
        managed_entities: Vec::new(),
        profiles: ProfileBundle {
            schema_version: 1,
            kind: BundleKind::ProfileBundle,
            name: None,
            description: None,
            profile: ProfileBundleData {
                power_save: config.power_save,
                profiles: config.profiles,
                scenes,
                mode_transitions: config.mode_transitions,
                light_schedules: Some(config.light_schedules),
            },
        },
        devices,
        rooms_requiring_assignment: room_ids,
        excluded_native_scene_ids: excluded,
        stripped_scene_extension_ids: stripped,
        required_reviews: vec![
            "stop_old_writer_before_enabling_addon",
            "verify_ha_registry_identity_and_physical_control",
            "rebind_rooms_schedules_and_inputs",
            "review_scene_and_schedule_references",
            "enroll_phones_and_explicitly_handover_tunnel",
            "retain_matching_old_image_and_data_for_rollback",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn backup() -> serde_json::Value {
        json!({"schema_version":3,"kind":"backup_bundle","created_at":"2026-01-01T00:00:00Z",
            "configuration":{},"installation":{},"runtime_state":{}})
    }
    fn device(id: &str, kind: &str, hardware: serde_json::Value) -> serde_json::Value {
        json!({"id":id,"name":"Same room name", "device_type":"light","hardware_ids":hardware,
            "endpoints":[{"hub_key":{"hub_type":kind,"address":"fixture"},"native_id":"light.example",
                "preferred":true,"active":true,"last_seen":0}],"created_at":0})
    }
    fn source(backup: &mut serde_json::Value, value: serde_json::Value) {
        backup["installation"]["canonical_registry"] =
            json!({"devices":{"old":value},"triage":{"entries":[]}});
    }
    #[test]
    fn portable_conversion_is_paused_deterministic_and_excludes_credentials() {
        let mut value = backup();
        value["installation"]["hub_credentials"] =
            json!([{"address":"private-host","data":{"token":"secret-fixture"}}]);
        let bytes = serde_json::to_vec(&value).unwrap();
        let first = serde_json::to_value(preview(&bytes, b"[]").unwrap()).unwrap();
        assert_eq!(
            first,
            serde_json::to_value(preview(&bytes, b"[]").unwrap()).unwrap()
        );
        assert_eq!(first["managed_entities"], json!([]));
        assert_eq!(first["light_breaker_enabled"], false);
        assert!(!first.to_string().contains("secret-fixture"));
        assert!(!first.to_string().contains("private-host"));
    }
    #[test]
    fn names_and_fabric_scoped_matter_ids_never_match() {
        let mut value = backup();
        source(
            &mut value,
            device("old", "matter", json!([{"type":"matter_id","value":"123"}])),
        );
        let target = device(
            "new",
            "homeassistant",
            json!([{"type":"matter_id","value":"123"}]),
        );
        let result = preview(
            &serde_json::to_vec(&value).unwrap(),
            &serde_json::to_vec(&vec![target]).unwrap(),
        )
        .unwrap();
        assert_eq!(result.devices[0].outcome, "no_proven_counterpart");
    }
    #[test]
    fn exact_hardware_still_requires_review_and_does_not_select() {
        let mut value = backup();
        let hw = json!([{"type":"mac","value":"00:17:88:01:00:00:00:01"}]);
        source(&mut value, device("old", "hue", hw.clone()));
        let target = device("new", "homeassistant", hw);
        let result = preview(
            &serde_json::to_vec(&value).unwrap(),
            &serde_json::to_vec(&vec![target]).unwrap(),
        )
        .unwrap();
        assert_eq!(result.devices[0].outcome, "candidate_requires_review");
        assert!(result.managed_entities.is_empty());
    }
    #[test]
    fn malformed_and_placeholder_hardware_never_becomes_migration_evidence() {
        for (old, new) in [
            ("unknown", "missing"),
            ("a", "a"),
            ("00:17:88", "001788"),
            ("00:00:00:00:00:00", "000000000000"),
            ("ff:ff:ff:ff:ff:ff:ff:ff", "ffffffffffffffff"),
            ("00:17:88:01:00:01!", "00:17:88:01:00:01"),
        ] {
            let mut value = backup();
            source(
                &mut value,
                device("old", "hue", json!([{"type":"mac","value":old}])),
            );
            let target = device("new", "homeassistant", json!([{"type":"mac","value":new}]));
            let result = preview(
                &serde_json::to_vec(&value).unwrap(),
                &serde_json::to_vec(&vec![target]).unwrap(),
            )
            .unwrap();
            assert_eq!(
                result.devices[0].outcome, "no_proven_counterpart",
                "{old} vs {new}"
            );
        }
    }
    #[test]
    fn valid_mac_and_eui64_formats_match_after_strict_validation() {
        for (old, new) in [
            (" 00:17:88:AB:00:01 ", "0017.88ab.0001"),
            ("00:17:88:01:00:00:00:01", "00-17-88-01-00-00-00-01"),
        ] {
            let source: CanonicalDevice =
                serde_json::from_value(device("old", "hue", json!([{"type":"mac","value":old}])))
                    .unwrap();
            let target: CanonicalDevice = serde_json::from_value(device(
                "new",
                "homeassistant",
                json!([{"type":"mac","value":new}]),
            ))
            .unwrap();
            assert!(exact_hardware_candidate(&source, &target));
        }
    }
    #[test]
    fn unknown_schema_is_rejected() {
        let mut value = backup();
        value["schema_version"] = json!(4);
        assert!(preview(&serde_json::to_vec(&value).unwrap(), b"[]").is_err());
    }
    #[test]
    fn native_scenes_and_opaque_extensions_are_not_imported() {
        let mut value = backup();
        value["configuration"]["scenes"] = json!([
            {"id":"native-hue-old", "name":"Native", "source":{"kind":"imported","provider":"hue"}},
            {"id":"portable", "name":"Portable", "extensions":{"old_controller":{"token":"secret-fixture"}}}
        ]);
        let result = preview(&serde_json::to_vec(&value).unwrap(), b"[]").unwrap();
        assert_eq!(result.profiles.profile.scenes.len(), 1);
        assert_eq!(result.excluded_native_scene_ids, ["native-hue-old"]);
        assert_eq!(result.stripped_scene_extension_ids, ["portable"]);
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("secret-fixture"));
    }
}
