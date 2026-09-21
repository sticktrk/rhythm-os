//! Room activation must survive a switch from grouped to individual routing.
//! Real room sync, engine, motion/input and periodic planning; fake controller I/O.

mod harness;

use std::collections::BTreeMap;

use harness::{rooms_with_lights, TestHarness};
use rhythm_core::{LightNodeKind, RestoredNodeState};
use rhythm_os::light_runtime::run_selected_light_runtime_event;
use rhythm_runtime_api::{InputAction, RuntimeEvent, RuntimeInputEvent, TickContext};
use serde_json::json;

fn activation_survives_fallback(action: Option<InputAction>, hard_off: bool) {
    let (harness, spy) = TestHarness::with_spy_controller_at(14.0, 172);
    let (rooms, devices) = rooms_with_lights(&[("hallway", "Hallway")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let room_id = harness.resolve("hallway");
    let runtime = harness.state.lock().unwrap().hub_runtime().unwrap();
    let child = runtime
        .engine_all_node_snapshots()
        .into_iter()
        .find(|node| {
            node.kind == LightNodeKind::LightDevice
                && node.parent_id.as_deref() == Some(room_id.as_str())
        })
        .unwrap();
    for node_id in [&room_id, &child.id] {
        let mut saved = RestoredNodeState::from(&runtime.engine_node_snapshot(node_id).unwrap());
        saved.rhythm_enabled = true;
        saved.standby_enabled = true;
        saved.soft_off = !hard_off;
        saved.hard_off = hard_off;
        runtime.restore_node_state(node_id, saved);
    }
    {
        let app = harness.state.lock().unwrap();
        assert!(
            app.topology
                .periodic_light_nodes(&app.canonical_registry)
                .iter()
                .any(|node| node.source_node_id == room_id),
            "start with grouped routing"
        );
    }
    spy.set_any_lights_on(true);
    spy.reset();
    if let Some(action) = action {
        run_selected_light_runtime_event(
            &harness.state,
            RuntimeEvent::Input(RuntimeInputEvent {
                source_id: "synthetic-button".into(),
                target_id: room_id.clone(),
                action,
                epoch_ms: None,
                metadata: Default::default(),
            }),
        )
        .unwrap();
    } else {
        assert!(rhythm_os::event_loop::turn_on_node_inline(
            &harness.state,
            &room_id
        ));
    }
    let active = spy.turn_on_calls();
    assert_eq!(active.len(), 1);
    let brightness = active[0].1.brightness;
    assert!(brightness > 1);

    let route = {
        let mut app = harness.state.lock().unwrap();
        app.topology
            .set_external_grouped_dispatch_suspended(&harness.hub_key, true);
        app.topology
            .periodic_light_nodes(&app.canonical_registry)
            .into_iter()
            .find(|node| node.source_node_id == child.id)
            .expect("suspended group must expose an individual route")
    };
    spy.reset();
    let report = run_selected_light_runtime_event(
        &harness.state,
        RuntimeEvent::PeriodicTick(TickContext {
            node_id: route.id.clone(),
            hour: 14.0,
            epoch_ms: None,
            metadata: BTreeMap::from([("source_node_id".into(), json!(route.source_node_id))]),
        }),
    )
    .unwrap();
    assert_eq!(
        report.dispatch_count, 1,
        "active child must remain eligible"
    );
    assert_eq!(
        spy.last_command_for(&route.id).unwrap().brightness,
        brightness,
        "fallback must retain the active brightness"
    );
}

#[test]
fn motion_from_standby_survives_grouped_fallback() {
    activation_survives_fallback(None, false);
}

#[test]
fn brightness_up_from_standby_survives_grouped_fallback() {
    activation_survives_fallback(Some(InputAction::BrightnessUp), false);
}

#[test]
fn reset_from_hard_off_survives_grouped_fallback() {
    activation_survives_fallback(Some(InputAction::Reset), true);
}
