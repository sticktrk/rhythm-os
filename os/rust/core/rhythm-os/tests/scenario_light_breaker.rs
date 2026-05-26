//! Scenario: global light-breaker pause.
//!
//! Pausing light breaker should stop autonomous Rhythm control paths while
//! leaving the server, hub plumbing, and explicit APIs alive.

mod harness;

use std::time::Duration;

use harness::{motion_sensor, room, rooms_with_lights, TestHarness};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::{ButtonAction, LightingCommand, RhythmMode};
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use rhythm_os::canonical::registry::ResolveResult;
use rhythm_os::commands;
use rhythm_os::event_loop::{self, MotionTimerState};
use rhythm_os::hub::HubEvent;
use rhythm_os::server_event::{InputEventResource, InputEventRoute, ServerEvent};
use rhythm_os::state::WorkItem;
use rhythm_os::topology::{DevicePlacement, InputBinding};

fn add_button_source(harness: &TestHarness, native_id: &str, room_id: &str) -> String {
    let topology_room_id = harness.resolve(room_id);
    let identity = DiscoveredIdentity {
        native_id: native_id.to_string(),
        room_id: Some(room_id.to_string()),
        room_name: Some(room_id.to_string()),
        name: native_id.to_string(),
        device_type: DeviceType::Button,
        hardware_ids: vec![HardwareId::matter(native_id)],
        manufacturer: None,
        model: None,
    };

    let mut state = harness.state.lock().unwrap();
    let canonical_id = match state
        .canonical_registry
        .resolve(&identity, &harness.hub_key, 1)
    {
        ResolveResult::Created { canonical_id } | ResolveResult::AlreadyKnown { canonical_id } => {
            canonical_id
        }
        other => panic!("unexpected resolve result: {:?}", other),
    };
    assert!(state
        .canonical_registry
        .assign_room(&canonical_id, Some(&topology_room_id)));
    state.topology.ensure_standalone_device(&canonical_id);
    assert!(state.topology.assign_device(
        &canonical_id,
        Some(&topology_room_id),
        DevicePlacement::UserOverride,
    ));
    canonical_id
}

fn subscribe_events(harness: &TestHarness) -> tokio::sync::broadcast::Receiver<ServerEvent> {
    let (event_tx, event_rx) = tokio::sync::broadcast::channel(8);
    harness.state.lock().unwrap().event_tx = Some(event_tx);
    event_rx
}

fn expect_button_route(
    event_rx: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
    expected_route: InputEventRoute,
) {
    let event = event_rx.try_recv().expect("expected button input event");
    match event {
        ServerEvent::InputEvent(InputEventResource::Button { route, .. }) => {
            assert_eq!(route, expected_route);
        }
        other => panic!("unexpected event: {:?}", other),
    }
}

fn expect_motion_route(
    event_rx: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
    expected_route: InputEventRoute,
) {
    let event = event_rx.try_recv().expect("expected motion input event");
    match event {
        ServerEvent::InputEvent(InputEventResource::Motion { route, .. }) => {
            assert_eq!(route, expected_route);
        }
        other => panic!("unexpected event: {:?}", other),
    }
}

#[test]
fn disabled_light_breaker_blocks_button_control_but_keeps_input_event() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    add_button_source(&harness, "button_kitchen", "kitchen");

    commands::do_light_breaker_set(&harness.state, false).unwrap();
    let mut event_rx = subscribe_events(&harness);
    let mut motion = MotionTimerState::new();

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Button {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "kitchen".into(),
            action: ButtonAction::OnPress,
            device_id: Some("button_kitchen".into()),
        },
        &mut motion,
    );

    expect_button_route(&mut event_rx, InputEventRoute::NodeControl);
    assert_eq!(
        spy.turn_on_count(),
        0,
        "button stream events must not dispatch while light breaker is disabled"
    );
    assert!(
        !harness.lights_on("kitchen"),
        "button stream events must not mutate cached on-state while disabled"
    );
}

#[test]
fn light_breaker_toggle_invalidates_queued_generated_light_dispatches() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();
    let room_id = harness.resolve("kitchen");
    let stale_generation = harness.state.lock().unwrap().light_dispatch_generation;

    commands::do_light_breaker_set(&harness.state, false).unwrap();
    commands::do_light_breaker_set(&harness.state, true).unwrap();

    event_loop::process_work_item(
        &harness.state,
        WorkItem::ApplyNodeCommand {
            command_id: "test-stale-generated-command".into(),
            node_id: room_id,
            command: LightingCommand::with_transition(80, 4000, 250),
            dispatch_spacing: Duration::ZERO,
            dispatch_generation: stale_generation,
        },
    );

    assert_eq!(
        spy.turn_on_count(),
        0,
        "generated light work queued before the pause must not run after re-enable"
    );
}

#[test]
fn disabled_light_breaker_keeps_explicit_room_actions_available() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();

    commands::do_light_breaker_set(&harness.state, false).unwrap();
    harness.action("kitchen", "on").unwrap();

    assert_eq!(spy.turn_on_count(), 1);
    assert!(harness.lights_on("kitchen"));
}

#[test]
fn disabled_light_breaker_keeps_light_power_status_updates_available() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();

    commands::do_light_breaker_set(&harness.state, false).unwrap();
    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::LightPower {
            hub_key: Some(harness.hub_key.clone()),
            device_id: "light-kitchen".into(),
            lights_on: true,
        },
        &mut MotionTimerState::new(),
    );

    assert!(harness.lights_on("kitchen"));
    assert_eq!(spy.turn_on_count(), 0);
    assert_eq!(spy.turn_off_count(), 0);
}

#[test]
fn disabled_light_breaker_blocks_motion_control_but_keeps_input_event() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("hallway", "Hallway")],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();

    commands::do_light_breaker_set(&harness.state, false).unwrap();
    let mut event_rx = subscribe_events(&harness);
    let mut motion = MotionTimerState::new();

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Motion {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "hallway".into(),
            sensor_id: "motion_01".into(),
            detected: true,
        },
        &mut motion,
    );

    expect_motion_route(&mut event_rx, InputEventRoute::NodeControl);
    assert_eq!(
        spy.turn_on_count(),
        0,
        "motion stream events must not dispatch while light breaker is disabled"
    );
    assert!(motion.sensors.is_empty(), "motion timers must not arm");
}

#[test]
fn disabled_light_breaker_blocks_input_binding_actions() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let source_id = add_button_source(&harness, "button_kitchen", "kitchen");
    harness
        .state
        .lock()
        .unwrap()
        .topology
        .set_input_binding(InputBinding::day_sleep_toggle(
            source_id,
            Some(ButtonAction::OnPress),
        ));

    commands::do_light_breaker_set(&harness.state, false).unwrap();
    let mut event_rx = subscribe_events(&harness);
    let mut motion = MotionTimerState::new();

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Button {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "kitchen".into(),
            action: ButtonAction::OnPress,
            device_id: Some("button_kitchen".into()),
        },
        &mut motion,
    );

    expect_button_route(&mut event_rx, InputEventRoute::InputBinding);
    assert_eq!(harness.state.lock().unwrap().active_mode, RhythmMode::Day);
    assert_eq!(spy.turn_on_count(), 0);
}

#[test]
fn disabled_light_breaker_keeps_unroutable_button_triage_alive() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    add_button_source(&harness, "button_kitchen", "kitchen");

    commands::do_light_breaker_set(&harness.state, false).unwrap();
    let mut event_rx = subscribe_events(&harness);
    let mut motion = MotionTimerState::new();

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::UnroutableButton {
            hub_key: Some(harness.hub_key.clone()),
            device_id: Some("button_kitchen".into()),
            button_id: "button_resource".into(),
        },
        &mut motion,
    );

    expect_button_route(&mut event_rx, InputEventRoute::Unroutable);
    assert_eq!(harness.triage_pending_count(), 1);
    assert_eq!(spy.turn_on_count(), 0);
    assert_eq!(spy.turn_off_count(), 0);
}

#[test]
fn disabled_light_breaker_blocks_periodic_light_ticks() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    commands::do_light_breaker_set(&harness.state, false).unwrap();
    let room_id = harness.resolve("kitchen");
    let dispatch_generation = harness.state.lock().unwrap().light_dispatch_generation;

    event_loop::process_work_item(
        &harness.state,
        WorkItem::PeriodicNodeTick {
            command_id: "test-periodic-disabled".into(),
            node_id: room_id.clone(),
            settings_node_id: room_id,
            current_hour: 14.0,
            emit_parent_node_id: None,
            dispatch_spacing: Duration::ZERO,
            dispatch_generation,
        },
    );

    assert_eq!(
        spy.turn_on_count(),
        0,
        "periodic ticks must not turn lights on while light breaker is disabled"
    );
    assert_eq!(
        spy.turn_off_count(),
        0,
        "periodic ticks must not turn lights off while light breaker is disabled"
    );
}
