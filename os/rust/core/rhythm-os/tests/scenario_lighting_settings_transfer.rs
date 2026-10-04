//! RPiZ settings transfer preserves lighting intent through explicit HA mapping.
mod harness;

use std::collections::BTreeMap;
use std::sync::Arc;

use harness::{light, rooms_with_lights, TestHarness};
use rhythm_core::{
    LightNodeKind, LightProfileNodeOverride, LightScheduleAssignment, RhythmMode,
    RoomProfileSettings,
};
use rhythm_os::bundle::{LightingSettingsBundle, LightingSettingsImportPayload};
use rhythm_os::commands;
use rhythm_os::storage::{FileStorage, Storage};
use serde_json::{json, Value};

fn export(harness: &TestHarness) -> LightingSettingsBundle {
    serde_json::from_str(&commands::build_lighting_settings_bundle(&harness.state).unwrap())
        .unwrap()
}

fn room_harness(id: &str) -> TestHarness {
    let (rooms, devices) = rooms_with_lights(&[(id, "Living room")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();
    harness
}

#[test]
fn rpiz_backup_transfers_room_preferences_and_rebinds_scenes_without_ownership_or_live_state() {
    let source = room_harness("old-room");
    let source_id = source.resolve("old-room");
    let mut backup = commands::build_backup_bundle_dto(&source.state, false).unwrap();
    let mut profile = rhythm_core::default_rhythm_profile();
    profile.id = "cozy".into();
    profile.name = "Cozy".into();
    profile.min_brightness = 17;
    backup.configuration.profiles.push(profile);
    backup.configuration.power_save = true;
    backup.configuration.scenes.push(serde_json::from_value(json!({
        "id":"evening-scene", "name":"Evening", "light": {
            "entries":[{"target":{"kind":"node","node_id":source_id}, "output":{"brightness":36}}]
        }
    })).unwrap());
    backup.configuration.mode_configs[0].active_profile_id = Some("cozy".into());
    let source_room = backup.installation.rooms.get_mut(&source_id).unwrap();
    source_room.profile_settings = RoomProfileSettings {
        profile_id: Some("cozy".into()),
        mood_scene_id: Some("evening-scene".into()),
        motion_activation_enabled: Some(false),
        profile_overrides: BTreeMap::from([(
            "cozy".into(),
            LightProfileNodeOverride {
                min_brightness: Some(25),
                max_color_temp: Some(3900),
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    source_room.standby_enabled = true;
    source_room.soft_off = true;
    source_room.time_offset_minutes = 90.0;
    backup.installation.hub_credentials.push(
        serde_json::from_value(json!({
            "address":"old-controller", "data":{"token":"private-fixture"}
        }))
        .unwrap(),
    );
    let incoming =
        commands::normalize_lighting_settings_payload(&serde_json::to_value(backup).unwrap())
            .unwrap();
    assert!(!serde_json::to_string(&incoming)
        .unwrap()
        .contains("private-fixture"));

    let (target, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("ha-area", "HA area")]);
    let target = target.with_discovery(rooms, devices);
    target.sync();
    let target_id = target.resolve("ha-area");
    let data_dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(FileStorage::new(data_dir.path().to_str().unwrap()).unwrap());
    let (topology_before, canonical_before, mode_before, selection_before) = {
        let mut state = target.state.lock().unwrap();
        state.platform_context = "ha_addon".into();
        state.storage = Some(storage.clone());
        state.managed_ha_lights = Some(Default::default());
        (
            serde_json::to_value(&state.topology).unwrap(),
            serde_json::to_value(&state.canonical_registry).unwrap(),
            state.active_mode,
            state.managed_ha_lights.clone(),
        )
    };
    let before = target.snapshot("ha-area").unwrap();
    let calls_before = spy.calls().len();
    let result: Value = serde_json::from_str(
        &commands::do_lighting_settings_import(
            &target.state,
            LightingSettingsImportPayload {
                settings: incoming,
                node_mappings: BTreeMap::from([(source_id, target_id.clone())]),
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(result["applied_nodes"], 1);
    let after = target.snapshot("ha-area").unwrap();
    assert_eq!(after.profile_settings.profile_id.as_deref(), Some("cozy"));
    assert_eq!(
        after.profile_settings.profile_overrides["cozy"].min_brightness,
        Some(25)
    );
    assert_eq!(
        after.profile_settings.profile_overrides["cozy"].max_color_temp,
        Some(3900)
    );
    assert_eq!(
        after.profile_settings.motion_activation_enabled,
        Some(false)
    );
    assert!(after.standby_enabled);
    assert_eq!(
        (after.soft_off, after.mood_active, after.hard_off),
        (before.soft_off, before.mood_active, before.hard_off)
    );
    assert_eq!(after.time_offset_minutes, before.time_offset_minutes);
    assert_eq!(
        spy.calls().len(),
        calls_before,
        "import must not dispatch lighting commands"
    );
    {
        let state = target.state.lock().unwrap();
        assert!(!state.light_breaker_enabled);
        assert_eq!(
            serde_json::to_value(&state.topology).unwrap(),
            topology_before
        );
        assert_eq!(
            serde_json::to_value(&state.canonical_registry).unwrap(),
            canonical_before
        );
        assert_eq!(state.managed_ha_lights, selection_before);
        assert_eq!(state.active_mode, mode_before);
        assert_eq!(
            state.hub_runtime().unwrap().active_light_profile_id(),
            "cozy"
        );
        assert_eq!(state.active_mode_profile_id(), "cozy");
        assert_eq!(
            state.scenes["evening-scene"]
                .light
                .as_ref()
                .unwrap()
                .entries[0]
                .target
                .node_id(),
            target_id
        );
    }
    let stored = storage.load_rooms().unwrap();
    assert_eq!(
        stored.get(&target_id).unwrap().profile_settings,
        after.profile_settings
    );
    assert!(storage
        .load_light_profiles()
        .unwrap()
        .profiles
        .iter()
        .any(|p| p.id == "cozy"));
    assert!(!storage.load_settings().unwrap().light_breaker_enabled);
    target.sync();
    assert_eq!(
        target.snapshot("ha-area").unwrap().profile_settings,
        after.profile_settings,
        "first HA sync must retain imported preferences"
    );
}

#[test]
fn same_names_never_map_and_invalid_mappings_fail_before_mutation() {
    let source = room_harness("old");
    let target = room_harness("new");
    let source_id = source.resolve("old");
    let target_id = target.resolve("new");
    let mut incoming = export(&source);
    incoming
        .nodes
        .iter_mut()
        .find(|node| node.id == source_id)
        .unwrap()
        .room_profile
        .motion_activation_enabled = Some(false);
    let before = export(&target);
    let invalid = commands::do_lighting_settings_import(
        &target.state,
        LightingSettingsImportPayload {
            settings: incoming.clone(),
            node_mappings: BTreeMap::from([(source_id.clone(), "missing".into())]),
        },
    );
    assert!(invalid
        .unwrap_err()
        .to_string()
        .contains("no longer exists"));
    assert_eq!(
        serde_json::to_value(export(&target)).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    let result: Value = serde_json::from_str(
        &commands::do_lighting_settings_import(
            &target.state,
            LightingSettingsImportPayload {
                settings: incoming.clone(),
                node_mappings: BTreeMap::new(),
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(result["applied_nodes"], 0);
    assert_ne!(
        target
            .snapshot("new")
            .unwrap()
            .profile_settings
            .motion_activation_enabled,
        Some(false)
    );
    incoming
        .nodes
        .iter_mut()
        .find(|node| node.id == source_id)
        .unwrap()
        .kind = LightNodeKind::LightDevice;
    assert!(commands::do_lighting_settings_import(
        &target.state,
        LightingSettingsImportPayload {
            settings: incoming,
            node_mappings: BTreeMap::from([(source_id, target_id)])
        }
    )
    .unwrap_err()
    .to_string()
    .contains("different node kinds"));
}

#[test]
fn standalone_light_preferences_and_named_schedule_survive_reviewed_transfer() {
    let source = TestHarness::new().with_discovery(vec![], vec![light("old-bulb", "")]);
    source.sync();
    source.triage_new(&source.triage_pending_ids()[0]).unwrap();
    let target = TestHarness::new().with_discovery(vec![], vec![light("new-bulb", "")]);
    target.sync();
    target.triage_new(&target.triage_pending_ids()[0]).unwrap();
    let mut incoming = export(&source);
    let source_id = incoming.nodes[0].id.clone();
    let target_id = export(&target).nodes[0].id.clone();
    incoming.profile.light_schedules = Some(vec![serde_json::from_value(
        json!({"id":"weekday","name":"Weekday","active_mode":"sleep","transitions":rhythm_core::default_mode_transition_configs()}),
    )
    .unwrap()]);
    incoming.nodes[0].room_profile = serde_json::from_value(json!({
        "motion_activation_enabled":false,
        "light_schedule":{"kind":"named","schedule_id":"weekday","active_mode":"sleep"},
        "light_schedule_modes":{"weekday":"sleep"},
        "profile_overrides":{"rhythm":{"min_brightness":31}}
    }))
    .unwrap();
    commands::do_lighting_settings_import(
        &target.state,
        LightingSettingsImportPayload {
            settings: incoming,
            node_mappings: BTreeMap::from([(source_id, target_id.clone())]),
        },
    )
    .unwrap();
    let state = target.state.lock().unwrap();
    let snapshot = state
        .hub_runtime()
        .unwrap()
        .engine_node_snapshot(&target_id)
        .unwrap();
    assert_eq!(
        snapshot.profile_settings.motion_activation_enabled,
        Some(false)
    );
    assert_eq!(
        snapshot.profile_settings.profile_overrides["rhythm"].min_brightness,
        Some(31)
    );
    assert_eq!(
        snapshot
            .profile_settings
            .light_schedule
            .as_ref()
            .unwrap()
            .schedule_id(),
        Some("weekday")
    );
    assert_eq!(
        snapshot
            .profile_settings
            .light_schedule
            .as_ref()
            .unwrap()
            .active_mode(),
        Some(state.active_mode)
    );
    assert!(snapshot.profile_settings.light_schedule_modes.is_empty());
    assert_eq!(
        state.light_schedules["weekday"].active_mode,
        state.active_mode
    );
}

#[test]
fn transfer_preserves_destination_sleep_when_imported_mode_uses_a_custom_profile() {
    let target = room_harness("ha-area");
    let target_id = target.resolve("ha-area");
    commands::do_set_active_mode(&target.state, RhythmMode::Sleep).unwrap();
    let mut incoming = export(&target);
    let mut profile = rhythm_core::default_sleep_profile();
    profile.id = "custom-sleep".into();
    incoming.profile.profiles.push(profile);
    incoming
        .mode_configs
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|mode| mode.mode == RhythmMode::Sleep)
        .unwrap()
        .active_profile_id = Some("custom-sleep".into());
    incoming
        .nodes
        .iter_mut()
        .find(|node| node.id == target_id)
        .unwrap()
        .room_profile
        .light_schedule = Some(LightScheduleAssignment::Unscheduled {
        active_mode: RhythmMode::Day,
    });

    commands::do_lighting_settings_import(
        &target.state,
        LightingSettingsImportPayload {
            settings: incoming,
            node_mappings: BTreeMap::from([(target_id.clone(), target_id)]),
        },
    )
    .unwrap();

    assert_eq!(target.state.lock().unwrap().active_mode, RhythmMode::Sleep);
    assert_eq!(
        target
            .snapshot("ha-area")
            .unwrap()
            .profile_settings
            .light_schedule
            .unwrap()
            .active_mode(),
        Some(RhythmMode::Sleep)
    );
}

#[test]
fn transfer_preserves_inherited_and_materialized_destination_modes() {
    for materialized in [false, true] {
        let target = room_harness("parent");
        let parent_id = target.resolve("parent");
        let runtime = target.state.lock().unwrap().hub_runtime().unwrap();
        runtime.add_node(
            "child-room",
            "Child room",
            LightNodeKind::Room,
            Some(parent_id.clone()),
        );
        commands::do_light_schedules_set(
            &target.state,
            vec![serde_json::from_value(json!({
                "id":"weekday", "name":"Weekday", "active_mode":"day",
                "transitions":[rhythm_core::ModeTransitionConfig::new(
                    RhythmMode::Sleep, RhythmMode::Day, 0
                ).with_id("wake")]
            }))
            .unwrap()],
        )
        .unwrap();
        let patch = commands::RoomProfileSettingsPatch {
            light_schedule: Some(Some(LightScheduleAssignment::Named {
                schedule_id: "weekday".into(),
                active_mode: if materialized {
                    RhythmMode::Day
                } else {
                    RhythmMode::Sleep
                },
            })),
            ..Default::default()
        };
        commands::do_node_preferences_set(
            &target.state,
            &parent_id,
            None,
            None,
            None,
            None,
            Some(&patch),
            false,
        )
        .unwrap();
        if materialized {
            let patch = commands::RoomProfileSettingsPatch {
                light_schedule_modes: Some(BTreeMap::from([(
                    "weekday".into(),
                    Some(RhythmMode::Sleep),
                )])),
                ..Default::default()
            };
            commands::do_node_preferences_set(
                &target.state,
                "child-room",
                None,
                None,
                None,
                None,
                Some(&patch),
                false,
            )
            .unwrap();
        }
        let before = runtime
            .engine_effective_node_snapshot("child-room")
            .unwrap();
        assert_eq!(
            before
                .profile_settings
                .light_schedule
                .unwrap()
                .active_mode(),
            Some(RhythmMode::Sleep)
        );
        let mut incoming = export(&target);
        incoming
            .nodes
            .iter_mut()
            .find(|node| node.id == "child-room")
            .unwrap()
            .room_profile
            .light_schedule = Some(LightScheduleAssignment::Unscheduled {
            active_mode: RhythmMode::Day,
        });
        commands::do_lighting_settings_import(
            &target.state,
            LightingSettingsImportPayload {
                settings: incoming,
                node_mappings: BTreeMap::from([("child-room".into(), "child-room".into())]),
            },
        )
        .unwrap();
        let after = runtime
            .engine_effective_node_snapshot("child-room")
            .unwrap();
        assert_eq!(
            after.profile_settings.light_schedule.unwrap().active_mode(),
            Some(RhythmMode::Sleep)
        );
    }
}

#[test]
fn preview_omits_native_scenes_and_secrets_and_accepts_legacy_backup_schemas() {
    for version in 1..=3 {
        let payload = json!({"schema_version":version,"kind":"backup_bundle","created_at":"2026-01-01",
            "configuration":{"rooms":[{"id":"old","name":"Old","rhythm_enabled":true,"disabled":false,"state":"active",
                "room_profile":{"mood_scene_id":"native-hue-a"}}],"scenes":[
                    {"id":"native-hue-a","name":"Native","source":{"kind":"imported","provider":"hue"}},
                    {"id":"custom","name":"Custom","extensions":{"opaque":{"secret":"hidden"}}}
                ]},"installation":{"integration_files":[{"path":"matter/fabric","content":"secret-fabric"}]},"runtime_state":{}});
        let preview = commands::normalize_lighting_settings_payload(&payload).unwrap();
        assert_eq!(preview.nodes.len(), 1);
        assert_eq!(preview.nodes[0].room_profile.mood_scene_id, None);
        assert_eq!(preview.profile.scenes.len(), 1);
        assert_eq!(preview.warnings.len(), 3);
        assert!(!serde_json::to_string(&preview).unwrap().contains("secret"));
        let mut legacy = payload;
        legacy.as_object_mut().unwrap().remove("kind");
        assert_eq!(
            commands::normalize_lighting_settings_payload(&legacy)
                .unwrap()
                .nodes
                .len(),
            1
        );
    }
    assert!(commands::normalize_lighting_settings_payload(
        &json!({"kind":"lighting_settings","schema_version":2,"profile":{}})
    )
    .is_err());
    assert!(commands::normalize_lighting_settings_payload(
        &json!({"kind":"other","schema_version":1,"profile":{}})
    )
    .is_err());
    assert!(commands::normalize_lighting_settings_payload(
        &json!({"kind":"profile_bundle","schema_version":1,"profile":{}})
    )
    .unwrap()
    .nodes
    .is_empty());
}

#[test]
fn transfer_conflicting_with_topology_scene_or_schedule_work_returns_before_mutating() {
    let source = room_harness("old");
    let target = room_harness("new");
    let incoming = export(&source);
    let mappings = BTreeMap::from([(source.resolve("old"), target.resolve("new"))]);
    let (topology_lock, locks) = {
        let state = target.state.lock().unwrap();
        (
            state.external_topology_transaction_lock.clone(),
            vec![
                state.scene_lifecycle_transaction_lock.clone(),
                state.light_schedule_write_lock.clone(),
            ],
        )
    };
    let before = serde_json::to_value(export(&target)).unwrap();
    let assert_busy = || {
        let error = commands::do_lighting_settings_import(
            &target.state,
            LightingSettingsImportPayload {
                settings: incoming.clone(),
                node_mappings: mappings.clone(),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("busy"));
        assert_eq!(serde_json::to_value(export(&target)).unwrap(), before);
    };
    {
        let _guard = topology_lock.lock().unwrap();
        assert_busy();
    }
    for lock in locks {
        let guard = lock.lock().unwrap();
        assert_busy();
        drop(guard);
    }
    commands::do_lighting_settings_import(
        &target.state,
        LightingSettingsImportPayload {
            settings: incoming,
            node_mappings: mappings,
        },
    )
    .unwrap();
}

#[test]
fn transferring_skipped_devices_later_keeps_previously_mapped_scene_entries() {
    let (rooms, lights) = rooms_with_lights(&[("old-a", "A"), ("old-b", "B")]);
    let source = TestHarness::new().with_discovery(rooms, lights);
    source.sync();
    let (rooms, lights) = rooms_with_lights(&[("ha-a", "A"), ("ha-b", "B")]);
    let target = TestHarness::new().with_discovery(rooms, lights);
    target.sync();
    let mut settings = export(&source);
    let source_a = source.resolve("old-a");
    let source_b = source.resolve("old-b");
    let target_a = target.resolve("ha-a");
    let target_b = target.resolve("ha-b");
    settings.profile.scenes.push(
        serde_json::from_value(json!({
            "id":"shared-scene","name":"Shared", "light":{"entries":[
                {"target":{"kind":"node","node_id":source_a},"output":{"brightness":26}},
                {"target":{"kind":"node","node_id":source_b},"output":{"brightness":42}}
            ]}
        }))
        .unwrap(),
    );
    for (source_id, target_id) in [(source_a, target_a.clone()), (source_b, target_b.clone())] {
        commands::do_lighting_settings_import(
            &target.state,
            LightingSettingsImportPayload {
                settings: settings.clone(),
                node_mappings: BTreeMap::from([(source_id, target_id)]),
            },
        )
        .unwrap();
    }
    let state = target.state.lock().unwrap();
    let entries = &state.scenes["shared-scene"].light.as_ref().unwrap().entries;
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.target.node_id() == target_a)
            .unwrap()
            .output
            .brightness,
        26
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.target.node_id() == target_b)
            .unwrap()
            .output
            .brightness,
        42
    );
}
