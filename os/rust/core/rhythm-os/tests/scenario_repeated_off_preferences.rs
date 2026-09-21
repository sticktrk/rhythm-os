//! Explicit Off requests must reach the controller even when saved state is off.
//! Exercise the HTTP handler, command worker and real engine with fake device I/O.

mod harness;

use std::sync::mpsc::{sync_channel, Receiver};
use std::time::Duration;

use harness::{light, rooms_with_lights, TestHarness};
use rhythm_core::{LightNodeKind, RestoredNodeState};
use rhythm_os::event_loop::process_work_item;
use rhythm_os::handlers::handle_put_node_preferences;
use rhythm_os::state::{ObservedPowerSource, ObservedPowerState, WorkItem};
use serde_json::{json, Value};

fn attach_queue(h: &TestHarness) -> Receiver<WorkItem> {
    let (tx, rx) = sync_channel(16);
    h.state.lock().unwrap().work_tx = Some(tx);
    rx
}

fn apply_preferences(h: &TestHarness, rx: &Receiver<WorkItem>, request: Value) {
    let response = handle_put_node_preferences(&h.state, &request, false);
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).unwrap()["queued"],
        true
    );
    let item = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("queued preference");
    process_work_item(&h.state, item);
}

#[test]
fn repeated_off_reaches_controller_regardless_of_observed_power() {
    for observed_on in [Some(true), Some(false), None] {
        let (h, spy) = TestHarness::with_spy_controller();
        let (rooms, devices) = rooms_with_lights(&[("room", "Room")]);
        let h = h.with_discovery(rooms, devices);
        h.sync();
        let rx = attach_queue(&h);
        let room = h.resolve("room");
        let request = json!({"node_id": room, "state": "hard_off"});

        apply_preferences(&h, &rx, request.clone());
        assert_eq!(spy.turn_off_calls(), vec![room.clone()]);
        assert!(h.snapshot("room").unwrap().hard_off);

        // Saved intent can disagree with a later report after a failed command,
        // an external turn-on or a device restart. Unknown/stale data must not
        // gate an explicit user command either.
        {
            let mut app = h.state.lock().unwrap();
            app.room_observed_power.remove(&room);
            if let Some(on) = observed_on {
                app.room_observed_power.insert(
                    room.clone(),
                    ObservedPowerState::new(on, ObservedPowerSource::AuthoritativeRefresh),
                );
            }
        }
        spy.reset();
        apply_preferences(&h, &rx, request.clone());
        assert_eq!(
            spy.turn_off_calls(),
            vec![room.clone()],
            "a second Off must dispatch when observed power is {observed_on:?}"
        );
        assert_eq!(spy.calls().len(), 1, "do not query power before Off");
        assert!(h.snapshot("room").unwrap().hard_off);
        assert!(h.pending_motion_clears().contains(&room));

        // Recovery must still allow the next normal activation and Off.
        h.action("room", "reset").unwrap();
        assert!(!h.snapshot("room").unwrap().hard_off);
        assert_eq!(spy.turn_on_count(), 1);
        spy.reset();
        apply_preferences(&h, &rx, request);
        assert_eq!(spy.turn_off_calls(), vec![room]);
        assert!(h.snapshot("room").unwrap().hard_off);
    }
}

#[test]
fn settings_save_on_hard_off_room_does_not_dispatch_power() {
    let (h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("room", "Room")]);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    let rx = attach_queue(&h);
    let room = h.resolve("room");
    apply_preferences(&h, &rx, json!({"node_id": room, "state": "hard_off"}));
    h.set_lights_on("room", true);
    spy.reset();

    for patch in [
        json!({"node_id": room, "rhythm_enabled": false}),
        json!({"node_id": room, "disabled": true}),
        json!({"node_id": room, "profile_settings": {"motion_activation_enabled": false}}),
    ] {
        apply_preferences(&h, &rx, patch);
        assert!(spy.calls().is_empty(), "settings alone must not resend Off");
    }
    let saved = h.snapshot("room").unwrap();
    assert!(saved.hard_off);
    assert!(!saved.rhythm_enabled);
    assert!(saved.disabled);
    assert!(!saved.profile_settings.motion_activation_enabled());
}

#[test]
fn restored_off_light_can_be_reasserted_without_changing_sibling_settings() {
    let (h, spy) = TestHarness::with_spy_controller();
    let (rooms, mut devices) = rooms_with_lights(&[("room", "Room"), ("other", "Other")]);
    devices.push(light("sibling", "room"));
    let h = h.with_discovery(rooms, devices);
    h.sync();
    let rx = attach_queue(&h);
    let room = h.resolve("room");
    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    let child = runtime
        .engine_all_node_snapshots()
        .into_iter()
        .find(|node| {
            node.kind == LightNodeKind::LightDevice && node.parent_id.as_deref() == Some(&room)
        })
        .unwrap();
    let before_parent = runtime.engine_node_snapshot(&room).unwrap();
    let before_other = h.snapshot("other").unwrap();
    let sibling = runtime
        .engine_all_node_snapshots()
        .into_iter()
        .find(|node| {
            node.kind == LightNodeKind::LightDevice
                && node.parent_id.as_deref() == Some(&room)
                && node.id != child.id
        })
        .unwrap();
    let mut restored = RestoredNodeState::from(&child);
    restored.hard_off = true;
    restored.soft_off = false;
    restored.mood_active = false;
    restored.rhythm_enabled = false;
    restored.time_offset_minutes = 17.0;
    restored.brightness_offset = -12.0;
    runtime.restore_node_state(&child.id, restored);
    spy.reset();

    apply_preferences(&h, &rx, json!({"node_id": child.id, "state": "hard_off"}));

    assert_eq!(spy.turn_off_calls(), vec![child.id.clone()]);
    assert!(spy.turn_on_calls().is_empty());
    let after = runtime.engine_node_snapshot(&child.id).unwrap();
    assert!(after.hard_off);
    assert!(!after.rhythm_enabled);
    assert_eq!(after.time_offset_minutes, 17.0);
    assert_eq!(after.brightness_offset, -12.0);
    assert_eq!(
        runtime.engine_node_snapshot(&room).unwrap().hard_off,
        before_parent.hard_off
    );
    assert_eq!(h.snapshot("other").unwrap().hard_off, before_other.hard_off);
    let after_sibling = runtime.engine_node_snapshot(&sibling.id).unwrap();
    assert_eq!(after_sibling.hard_off, sibling.hard_off);
    assert_eq!(after_sibling.rhythm_enabled, sibling.rhythm_enabled);
    assert_eq!(after_sibling.profile_settings, sibling.profile_settings);
}
