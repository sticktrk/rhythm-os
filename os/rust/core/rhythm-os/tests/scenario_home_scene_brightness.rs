//! Party dimming preserves the scene through the HTTP queue, restart and restore.
mod harness;

use harness::{light, room, TestHarness};
use rhythm_core::RoomModeState;
use rhythm_os::{commands, event_loop, handlers, scenes, storage};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn scene_brightness_batch_preserves_colors_and_survives_backup_restart_and_sync() {
    let (source, spy) = TestHarness::with_spy_controller();
    let source = source.with_discovery(
        vec![room("kitchen", "Kitchen"), room("hall", "Hall")],
        vec![
            light("lamp-1", "kitchen"),
            light("lamp-2", "kitchen"),
            light("lamp-3", "hall"),
        ],
    );
    source.sync();
    let kitchen = source.resolve("kitchen");
    let hall = source.resolve("hall");
    commands::do_home_scene_apply(
        &source.state,
        "halloween",
        scenes::HomeSceneApplyRequest::default(),
        Duration::ZERO,
    )
    .unwrap();
    let initial = spy.turn_on_calls();
    assert_eq!(initial.len(), 3);
    let seed = source
        .snapshot("kitchen")
        .unwrap()
        .profile_settings
        .mood_scene_palette_seed;

    let (tx, rx) = std::sync::mpsc::sync_channel(32);
    source.state.lock().unwrap().work_tx = Some(tx);
    let response = handlers::handle_set_node_curve(
        &source.state,
        &json!({
            "nodes": [{"node_id": kitchen, "brightness": 20}, {"node_id": hall, "brightness": 20}],
            "correlation_id": "party-brightness-test",
            "dispatch_spacing_ms": 0
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["queued"],
        true
    );
    assert_eq!(
        spy.turn_on_calls().len(),
        3,
        "queue acceptance does not mean physical dispatch"
    );
    while let Ok(item) = rx.try_recv() {
        event_loop::process_work_item(&source.state, item);
    }
    let changed = spy.turn_on_calls();
    assert_eq!(changed.len(), 6);
    for (node, command) in &changed[3..] {
        assert_eq!(
            command.rgb,
            initial.iter().find(|(id, _)| id == node).unwrap().1.rgb
        );
        assert_eq!(command.brightness, 20);
    }
    for room_id in [&kitchen, &hall] {
        let snapshot = source.snapshot(room_id).unwrap();
        assert!(snapshot.mood_active);
        assert_eq!(snapshot.profile_settings.mood_scene_device_mode, Some(true));
        assert!(
            snapshot.profile_settings.motion_activation_enabled(),
            "durable motion preference is unchanged"
        );
        assert_eq!(snapshot.brightness_offset, 0.0);
    }
    let records = source.state.lock().unwrap().light_activity.clone();
    assert_eq!(records.len(), 2);
    for record in records {
        assert_eq!(record.action_id, "set_curve");
        assert_eq!(record.brightness, Some(20));
        assert_eq!(
            record.correlation_id.as_deref(),
            Some("party-brightness-test")
        );
        assert_eq!(record.fanout_of.as_deref(), Some("party-brightness-test"));
        assert_eq!(record.payload.unwrap()["scene_brightness"], true);
    }

    let bundle = commands::build_backup_bundle_dto(&source.state, false).unwrap();
    let (mut restored, restored_spy) = TestHarness::with_spy_controller();
    let directory = tempfile::tempdir().unwrap();
    restored.state.lock().unwrap().storage = Some(Arc::new(
        storage::FileStorage::new(directory.path().to_str().unwrap()).unwrap(),
    ));
    commands::do_backup_restore(&restored.state, bundle).unwrap();
    let key = restored.add_hub("mock", "192.168.1.100");
    restored.set_hub_discovery(
        &key,
        vec![room("kitchen", "Kitchen"), room("hall", "Hall")],
        vec![
            light("lamp-1", "kitchen"),
            light("lamp-2", "kitchen"),
            light("lamp-3", "hall"),
        ],
    );
    storage::load_persisted_state(&mut restored.state.lock().unwrap());
    restored.sync_all();
    assert_eq!(
        restored
            .snapshot(&kitchen)
            .unwrap()
            .profile_settings
            .mood_scene_palette_seed,
        seed
    );
    assert_eq!(
        restored
            .snapshot(&kitchen)
            .unwrap()
            .profile_settings
            .mood_scene_device_mode,
        Some(true)
    );
    restored_spy.reset();
    for room_id in [&kitchen, &hall] {
        commands::do_node_preferences_set(
            &restored.state,
            room_id,
            None,
            None,
            None,
            Some(RoomModeState::Mood),
            None,
            false,
        )
        .unwrap();
    }
    let resumed = restored_spy.turn_on_calls();
    assert_eq!(resumed.len(), 3);
    for (node, command) in &resumed {
        assert_eq!(
            command.rgb,
            initial.iter().find(|(id, _)| id == node).unwrap().1.rgb
        );
        assert_eq!(command.brightness, 20);
    }
    let reexport = commands::build_backup_bundle_dto(&restored.state, false).unwrap();
    assert!(reexport
        .configuration
        .rooms
        .iter()
        .filter(|room| room.id == kitchen || room.id == hall)
        .all(|room| room.room_profile.mood_scene_device_mode == Some(true)));
    assert_eq!(
        source.state.lock().unwrap().scenes["halloween"]
            .light
            .as_ref()
            .unwrap()
            .palette[0]
            .brightness,
        80
    );
}
