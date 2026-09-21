//! Explicit state requests must reach the controller even when saved intent matches.
//! Exercise the HTTP handler, command worker and real engine with fake device I/O.

mod harness;

use std::sync::mpsc::{sync_channel, Receiver};
use std::time::Duration;

use harness::{light, rooms_with_lights, TestHarness};
use rhythm_core::{LightNodeKind, RestoredNodeState};
use rhythm_os::commands::build_node_state;
use rhythm_os::event_loop::process_work_item;
use rhythm_os::handlers::{handle_put_node_preferences, handle_put_room_preferences};
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
    let count = serde_json::from_str::<Value>(&response.body).unwrap()["dispatch_count"]
        .as_u64()
        .expect("dispatch count");
    for _ in 0..count {
        let item = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("queued preference");
        process_work_item(&h.state, item);
    }
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
        h.state.lock().unwrap().pending_motion_clear.clear();
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
        h.state.lock().unwrap().pending_motion_clear.clear();
        spy.reset();
        apply_preferences(&h, &rx, request);
        assert_eq!(spy.turn_off_calls(), vec![room.clone()]);
        assert!(h.snapshot("room").unwrap().hard_off);
        assert!(h.pending_motion_clears().contains(&room));
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
    let children: Vec<_> = runtime
        .engine_all_node_snapshots()
        .into_iter()
        .filter(|node| {
            node.kind == LightNodeKind::LightDevice && node.parent_id.as_deref() == Some(&room)
        })
        .collect();
    let [child, sibling] = children.as_slice() else {
        panic!("expected exactly two light children");
    };
    let before_parent = runtime.engine_node_snapshot(&room).unwrap();
    let before_other = h.snapshot("other").unwrap();
    let mut restored = RestoredNodeState::from(child);
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

fn assert_repeated_on_state(target: &str) {
    let (h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("room", "Room")]);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    let rx = attach_queue(&h);
    let room = h.resolve("room");
    let request = json!({"node_id": room, "state": target, "rhythm_enabled": true});
    apply_preferences(&h, &rx, request.clone());
    // A wall switch or failed delivery may leave the lamp off.
    h.set_lights_on("room", false);
    h.state.lock().unwrap().pending_motion_clear.clear();
    spy.reset();
    apply_preferences(&h, &rx, request);
    let calls = spy.turn_on_calls();
    assert_eq!(calls.len(), 1, "repeated {target} must dispatch once");
    assert_eq!(calls[0].0, room);
    assert!(spy.turn_off_calls().is_empty());
    assert!(h.pending_motion_clears().contains(&room));

    // Non-output settings must not reassert any persistent state.
    spy.reset();
    for patch in [
        json!({"node_id": room, "rhythm_enabled": false}),
        json!({"node_id": room, "disabled": true}),
        json!({"node_id": room, "profile_settings": {"motion_activation_enabled": false}}),
    ] {
        apply_preferences(&h, &rx, patch);
        assert!(spy.calls().is_empty(), "settings-only save while {target}");
    }
}

#[test]
fn repeated_active_reaches_controller_without_settings_only_dispatch() {
    assert_repeated_on_state("active");
}
#[test]
fn repeated_standby_reaches_controller_without_settings_only_dispatch() {
    assert_repeated_on_state("standby");
}
#[test]
fn repeated_mood_reaches_controller_without_settings_only_dispatch() {
    assert_repeated_on_state("mood");
}

#[test]
fn failed_off_can_be_retried_and_updates_reported_power_after_success() {
    let (h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("room", "Room")]);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    let rx = attach_queue(&h);
    let room = h.resolve("room");
    h.set_lights_on("room", true);
    let request = json!({"node_id": room, "state": "hard_off"});
    spy.fail_next_turn_off();
    apply_preferences(&h, &rx, request.clone());
    assert_eq!(spy.turn_off_calls(), vec![room.clone()]);
    assert!(h.snapshot("room").unwrap().hard_off);
    // A failed send must not overwrite the existing power observation.
    assert!(h.lights_on("room"));
    spy.reset();
    apply_preferences(&h, &rx, request);
    assert_eq!(spy.turn_off_calls(), vec![room.clone()]);
    assert!(!build_node_state(&h.state, &room).unwrap().lights_on);
}

#[test]
fn legacy_room_preferences_reassert_off_for_object_and_array_requests() {
    for array in [false, true] {
        let (h, spy) = TestHarness::with_spy_controller();
        let (rooms, devices) = rooms_with_lights(&[("room", "Room")]);
        let h = h.with_discovery(rooms, devices);
        h.sync();
        let room = h.resolve("room");
        let item = json!({"room_id": room, "state": "hard_off"});
        let request = if array { json!([item]) } else { item };
        for _ in 0..2 {
            spy.reset();
            let response = handle_put_room_preferences(&h.state, &request, false);
            assert_eq!(response.status, 200, "{}", response.body);
            assert_eq!(spy.turn_off_calls(), vec![room.clone()]);
        }
    }
}

#[test]
fn repeated_standby_toggle_does_not_reassert_unchanged_state() {
    let (h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("room", "Room")]);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    let rx = attach_queue(&h);
    let room = h.resolve("room");
    apply_preferences(&h, &rx, json!({"node_id": room, "state": "standby"}));
    h.set_lights_on("room", false);
    spy.reset();
    apply_preferences(&h, &rx, json!({"node_id": room, "standby_enabled": true}));
    assert!(spy.calls().is_empty());
    assert!(h.snapshot("room").unwrap().soft_off);
}

#[test]
fn all_off_batch_stably_prioritizes_cached_on_rooms() {
    for observed in [
        [false, false, false],
        [false, true, false],
        [true, false, true],
    ] {
        let (h, spy) = TestHarness::with_spy_controller();
        let (rooms, devices) =
            rooms_with_lights(&[("one", "One"), ("two", "Two"), ("three", "Three")]);
        let h = h.with_discovery(rooms, devices);
        h.sync();
        let rx = attach_queue(&h);
        let rooms = [h.resolve("one"), h.resolve("two"), h.resolve("three")];
        let request = json!({"items": rooms.iter().map(|room| json!({"node_id": room, "state": "hard_off"})).collect::<Vec<_>>(), "dispatch_spacing_ms": 0});
        apply_preferences(&h, &rx, request.clone());
        for (room, on) in rooms.iter().zip(observed) {
            h.set_lights_on(room, on);
        }
        spy.reset();
        apply_preferences(&h, &rx, request);
        let mut expected: Vec<_> = rooms.iter().zip(observed).collect();
        expected.sort_by_key(|(_, on)| !on);
        assert_eq!(
            spy.turn_off_calls(),
            expected
                .iter()
                .map(|(id, _)| (*id).clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(spy.calls().len(), 3, "ordering must not add physical reads");
    }
}

#[test]
fn two_already_off_rooms_reassert_in_request_order() {
    let (h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("one", "One"), ("two", "Two")]);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    let rx = attach_queue(&h);
    // Deliberately reverse discovery order to assert caller order wins.
    let rooms = [h.resolve("two"), h.resolve("one")];
    let request = json!({"items": rooms.iter().map(|room| json!({"node_id": room, "state": "hard_off"})).collect::<Vec<_>>(), "dispatch_spacing_ms": 0});
    apply_preferences(&h, &rx, request.clone());
    spy.reset();
    apply_preferences(&h, &rx, request);
    assert_eq!(spy.turn_off_calls(), rooms);
    assert_eq!(spy.calls().len(), 2);
}

#[test]
fn mixed_state_batch_keeps_request_order() {
    let (h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("one", "One"), ("two", "Two")]);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    let rx = attach_queue(&h);
    let one = h.resolve("one");
    let two = h.resolve("two");
    h.set_lights_on(&one, false);
    h.set_lights_on(&two, true);
    spy.reset();
    apply_preferences(
        &h,
        &rx,
        json!({"items": [
        {"node_id": one, "state": "hard_off"},
        {"node_id": two, "state": "active"}
    ], "dispatch_spacing_ms": 0}),
    );
    let calls = spy.calls();
    assert_eq!(calls.len(), 2);
    assert!(
        matches!(&calls[0], rhythm_core::spy_controller::SpyCall::TurnOff {room_id, ..} if room_id == &one)
    );
    assert!(
        matches!(&calls[1], rhythm_core::spy_controller::SpyCall::TurnOn {room_id, ..} if room_id == &two)
    );
}
