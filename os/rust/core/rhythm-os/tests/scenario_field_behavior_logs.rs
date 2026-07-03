//! Field-behavior scenarios from app debug bundles.
//!
//! These are not bug-title regressions. They capture how users and lights behave
//! in the bundles: switch/contact ingress, motion ownership, repeated app
//! preference writes, and topology snapshots with mixed device nodes.

mod harness;

use std::time::Duration;

use harness::{light, motion_sensor, room, TestHarness};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::{LightingCommand, RoomModeState};
use rhythm_os::commands;
use rhythm_os::discovery::DiscoveredDevice;
use rhythm_os::event_loop::{self, MotionTimerState};
use rhythm_os::hub::HubEvent;
use rhythm_os::server_event::{InputEventResource, InputEventRoute, ServerEvent};
use rhythm_os::state::WorkItem;

fn contact_sensor(device_id: &str, room_id: &str) -> DiscoveredDevice {
    DiscoveredDevice {
        device_id: device_id.to_string(),
        room_id: (!room_id.is_empty()).then(|| room_id.to_string()),
        buttons: vec![],
        device_type: DeviceType::Contact,
    }
}

fn subscribe_events(harness: &TestHarness) -> tokio::sync::broadcast::Receiver<ServerEvent> {
    let (event_tx, event_rx) = tokio::sync::broadcast::channel(16);
    harness.state.lock().unwrap().event_tx = Some(event_tx);
    event_rx
}

fn wait_for(mut predicate: impl FnMut() -> bool, message: &str) {
    for _ in 0..100 {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(predicate(), "{}", message);
}

fn expect_contact_event(
    rx: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
    expected_route: InputEventRoute,
    expected_native_id: &str,
    expected_open: bool,
    expected_target_id: Option<&str>,
) -> String {
    for _ in 0..16 {
        let event = rx.try_recv().expect("expected contact input event");
        if let ServerEvent::InputEvent(InputEventResource::Contact {
            route,
            source_node_id,
            target_node_id,
            native_sensor_id,
            open,
            ..
        }) = event
        {
            assert_eq!(route, expected_route);
            assert_eq!(native_sensor_id, expected_native_id);
            assert_eq!(open, expected_open);
            assert_eq!(target_node_id.as_deref(), expected_target_id);
            return source_node_id.expect("contact source should resolve");
        }
    }
    panic!("contact input event was not found in broadcast stream");
}

fn expect_motion_event(
    rx: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
    expected_route: InputEventRoute,
    expected_detected: bool,
    expected_target_id: Option<&str>,
) {
    for _ in 0..16 {
        let event = rx.try_recv().expect("expected motion input event");
        if let ServerEvent::InputEvent(InputEventResource::Motion {
            route,
            target_node_id,
            detected,
            ..
        }) = event
        {
            assert_eq!(route, expected_route);
            assert_eq!(target_node_id.as_deref(), expected_target_id);
            assert_eq!(detected, expected_detected);
            return;
        }
    }
    panic!("motion input event was not found in broadcast stream");
}

#[test]
fn contact_switch_open_and_close_drive_the_assigned_room() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("landing", "Landing")],
        vec![
            light("light-landing", "landing"),
            contact_sensor("ha-switch-landing", "landing"),
        ],
    );
    harness.sync();
    let landing_id = harness.resolve("landing");
    let mut rx = subscribe_events(&harness);
    let mut motion = MotionTimerState::new();

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Contact {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "landing".into(),
            sensor_id: "ha-switch-landing".into(),
            open: true,
        },
        &mut motion,
    );

    let source_id = expect_contact_event(
        &mut rx,
        InputEventRoute::NodeControl,
        "ha-switch-landing",
        true,
        Some(&landing_id),
    );
    wait_for(
        || spy.turn_on_count() >= 1,
        "contact open should turn on the landing",
    );
    assert!(harness.lights_on("landing"));

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Contact {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "landing".into(),
            sensor_id: "ha-switch-landing".into(),
            open: false,
        },
        &mut motion,
    );

    let close_source_id = expect_contact_event(
        &mut rx,
        InputEventRoute::NodeControl,
        "ha-switch-landing",
        false,
        Some(&landing_id),
    );
    assert_eq!(close_source_id, source_id);
    wait_for(
        || spy.turn_off_count() >= 1,
        "contact close should turn off the landing",
    );
    assert!(!harness.lights_on("landing"));
}

#[test]
fn contact_switch_without_room_is_visible_but_unroutable() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("landing", "Landing")],
        vec![
            light("light-landing", "landing"),
            contact_sensor("roomless-contact", ""),
        ],
    );
    harness.sync();
    let mut rx = subscribe_events(&harness);

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Contact {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "landing".into(),
            sensor_id: "roomless-contact".into(),
            open: true,
        },
        &mut MotionTimerState::new(),
    );

    expect_contact_event(
        &mut rx,
        InputEventRoute::Unroutable,
        "roomless-contact",
        true,
        None,
    );
    assert_eq!(spy.turn_on_count(), 0);
    assert_eq!(spy.turn_off_count(), 0);
}

#[test]
fn contact_switch_respects_light_breaker_but_still_broadcasts_input() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("landing", "Landing")],
        vec![
            light("light-landing", "landing"),
            contact_sensor("ha-switch-landing", "landing"),
        ],
    );
    harness.sync();
    let landing_id = harness.resolve("landing");
    commands::do_light_breaker_set(&harness.state, false).unwrap();
    let mut rx = subscribe_events(&harness);

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Contact {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "landing".into(),
            sensor_id: "ha-switch-landing".into(),
            open: true,
        },
        &mut MotionTimerState::new(),
    );

    expect_contact_event(
        &mut rx,
        InputEventRoute::NodeControl,
        "ha-switch-landing",
        true,
        Some(&landing_id),
    );
    assert_eq!(spy.turn_on_count(), 0);
    assert!(!harness.lights_on("landing"));
}

#[test]
fn contact_switch_unresolved_inputs_are_still_visible() {
    let harness = TestHarness::new().with_discovery(vec![room("landing", "Landing")], vec![]);
    harness.sync();
    let mut rx = subscribe_events(&harness);

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Contact {
            hub_key: None,
            room_id: "landing".into(),
            sensor_id: "orphan-contact".into(),
            open: true,
        },
        &mut MotionTimerState::new(),
    );

    match rx.try_recv().expect("expected unresolved contact event") {
        ServerEvent::InputEvent(InputEventResource::Contact {
            route,
            source_node_id,
            target_node_id,
            native_sensor_id,
            open,
            ..
        }) => {
            assert_eq!(route, InputEventRoute::Unresolved);
            assert_eq!(source_node_id, None);
            assert_eq!(target_node_id, None);
            assert_eq!(native_sensor_id, "orphan-contact");
            assert!(open);
        }
        other => panic!("unexpected event: {:?}", other),
    }

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Contact {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "landing".into(),
            sensor_id: "unknown-contact".into(),
            open: false,
        },
        &mut MotionTimerState::new(),
    );

    match rx
        .try_recv()
        .expect("expected unresolved known-hub contact event")
    {
        ServerEvent::InputEvent(InputEventResource::Contact {
            route,
            source_node_id,
            target_node_id,
            native_sensor_id,
            open,
            ..
        }) => {
            assert_eq!(route, InputEventRoute::Unresolved);
            assert_eq!(source_node_id, None);
            assert_eq!(target_node_id, None);
            assert_eq!(native_sensor_id, "unknown-contact");
            assert!(!open);
        }
        other => panic!("unexpected event: {:?}", other),
    }
}

#[test]
fn contact_switch_duplicate_open_is_debounced_after_visible_input() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("landing", "Landing")],
        vec![
            light("light-landing", "landing"),
            contact_sensor("ha-switch-landing", "landing"),
        ],
    );
    harness.sync();
    let landing_id = harness.resolve("landing");
    let mut rx = subscribe_events(&harness);
    let mut motion = MotionTimerState::new();

    for _ in 0..2 {
        event_loop::handle_hub_event(
            &harness.state,
            HubEvent::Contact {
                hub_key: Some(harness.hub_key.clone()),
                room_id: "landing".into(),
                sensor_id: "ha-switch-landing".into(),
                open: true,
            },
            &mut motion,
        );
        expect_contact_event(
            &mut rx,
            InputEventRoute::NodeControl,
            "ha-switch-landing",
            true,
            Some(&landing_id),
        );
    }

    wait_for(
        || spy.turn_on_count() == 1,
        "duplicate switch open should not dispatch twice",
    );
    assert_eq!(spy.turn_on_count(), 1);
}

#[test]
fn motion_activation_then_clear_starts_owned_countdown() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("hallway", "Hallway")],
        vec![
            light("light-hallway", "hallway"),
            motion_sensor("motion-hallway", "hallway"),
        ],
    );
    harness.sync();
    let hallway_id = harness.resolve("hallway");
    let mut rx = subscribe_events(&harness);
    let mut motion = MotionTimerState::new();

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Motion {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "hallway".into(),
            sensor_id: "motion-hallway".into(),
            detected: true,
        },
        &mut motion,
    );

    expect_motion_event(
        &mut rx,
        InputEventRoute::NodeControl,
        true,
        Some(hallway_id.as_str()),
    );
    wait_for(
        || spy.turn_on_count() >= 1,
        "motion activation should turn on the hallway",
    );
    assert!(motion.motion_owned.contains(&hallway_id));

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Motion {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "hallway".into(),
            sensor_id: "motion-hallway".into(),
            detected: false,
        },
        &mut motion,
    );

    expect_motion_event(
        &mut rx,
        InputEventRoute::NodeControl,
        false,
        Some(hallway_id.as_str()),
    );
    assert!(
        motion
            .sensors
            .values()
            .any(|source| source.target_node_id == hallway_id && source.stopped_at.is_some()),
        "cleared motion should keep ownership and start a countdown"
    );
}

#[test]
fn motion_reactivation_restores_warning_brightness_before_turn_on() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("hallway", "Hallway")],
        vec![
            light("light-hallway", "hallway"),
            motion_sensor("motion-hallway", "hallway"),
        ],
    );
    harness.sync();
    let hallway_id = harness.resolve("hallway");
    let mut motion = MotionTimerState::new();
    motion.warning_active.insert(hallway_id.clone());

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::Motion {
            hub_key: Some(harness.hub_key.clone()),
            room_id: "hallway".into(),
            sensor_id: "motion-hallway".into(),
            detected: true,
        },
        &mut motion,
    );

    wait_for(
        || spy.turn_on_count() >= 2,
        "warning restore should dim back up and then request motion turn-on",
    );
    assert!(!motion.warning_active.contains(&hallway_id));
    assert!(motion.motion_owned.contains(&hallway_id));
}

#[test]
fn queued_single_preference_write_updates_when_worker_processes_it() {
    let harness = TestHarness::new().with_discovery(vec![room("drop", "Drop Zone")], vec![]);
    harness.sync();
    let drop_id = harness.resolve("drop");
    let (work_tx, work_rx) = std::sync::mpsc::sync_channel::<WorkItem>(8);
    harness.state.lock().unwrap().work_tx = Some(work_tx);

    commands::queue_node_preferences_set(
        &harness.state,
        &drop_id,
        Some(true),
        None,
        Some(true),
        Some(RoomModeState::Standby),
        None,
        false,
        Duration::ZERO,
    )
    .unwrap();

    let item = work_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("preference write should queue a worker item");
    match &item {
        WorkItem::SetNodePreferences {
            node_id,
            target_state,
            standby_enabled,
            ..
        } => {
            assert_eq!(node_id, &drop_id);
            assert_eq!(*target_state, Some(RoomModeState::Standby));
            assert_eq!(*standby_enabled, Some(true));
        }
        _ => panic!("expected SetNodePreferences work item"),
    }

    event_loop::process_work_item(&harness.state, item);
    let snapshot = harness.snapshot("drop").unwrap();
    assert!(snapshot.soft_off);
    assert!(snapshot.standby_enabled);
}

#[test]
fn queued_app_light_controls_apply_from_worker() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("drop", "Drop Zone")], vec![]);
    harness.sync();
    let drop_id = harness.resolve("drop");
    let (work_tx, work_rx) = std::sync::mpsc::sync_channel::<WorkItem>(8);
    harness.state.lock().unwrap().work_tx = Some(work_tx);

    commands::queue_node_action(&harness.state, &drop_id, "on", false, Duration::ZERO).unwrap();
    commands::queue_set_node_brightness(&harness.state, &drop_id, 41, false, Duration::ZERO)
        .unwrap();
    commands::queue_set_node_curve_modifier(
        &harness.state,
        &drop_id,
        commands::NodeCurveModifier::ColorTemperature {
            kelvin: 3100,
            preserve_brightness: true,
        },
        false,
        Duration::ZERO,
    )
    .unwrap();

    for _ in 0..3 {
        let item = work_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("queued app control should reach worker");
        event_loop::process_work_item(&harness.state, item);
    }

    assert!(spy.turn_on_count() >= 3);
    assert!(harness.lights_on("drop"));
}

#[test]
fn authoritative_refresh_emits_power_state_after_external_change() {
    let harness = TestHarness::new().with_discovery(vec![room("drop", "Drop Zone")], vec![]);
    harness.sync();
    harness.action("drop", "on").unwrap();
    let drop_id = harness.resolve("drop");
    let mut rx = subscribe_events(&harness);

    let event_count = commands::refresh_observed_power_authoritatively_and_emit(&harness.state)
        .expect("authoritative refresh should succeed");
    assert_eq!(event_count, 1);

    let mut saw_drop = false;
    for _ in 0..16 {
        let event = rx
            .try_recv()
            .expect("authoritative refresh should emit node state");
        if let ServerEvent::NodeState { nodes } = event {
            saw_drop = nodes.iter().any(|node| {
                node.id == drop_id
                    && !node.lights_on
                    && node.observed_power.source.as_deref() == Some("authoritative_refresh")
            });
            if saw_drop {
                break;
            }
        }
    }
    assert!(saw_drop);
    assert!(!harness.lights_on("drop"));
}

#[test]
fn worker_apply_node_command_updates_cached_power() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("drop", "Drop Zone")], vec![]);
    harness.sync();
    let drop_id = harness.resolve("drop");
    let generation = harness.state.lock().unwrap().light_dispatch_generation;

    event_loop::process_work_item(
        &harness.state,
        WorkItem::ApplyNodeCommand {
            command_id: "field-apply-node-command".into(),
            node_id: drop_id,
            command: LightingCommand::with_transition(77, 3600, 250),
            dispatch_spacing: Duration::ZERO,
            dispatch_generation: generation,
        },
    );

    assert_eq!(spy.turn_on_count(), 1);
    assert!(harness.lights_on("drop"));
}

#[test]
fn state_snapshot_falls_back_to_topology_devices_when_runtime_is_absent() {
    let harness = TestHarness::new().with_discovery(
        vec![room("drop", "Drop Zone")],
        vec![
            light("matter-100", "drop"),
            light("matter-102", "drop"),
            motion_sensor("motion-drop", "drop"),
            contact_sensor("switch-drop", "drop"),
        ],
    );
    harness.sync();
    {
        let mut state = harness.state.lock().unwrap();
        for hub in state.hubs.values_mut() {
            hub.runtime = None;
        }
    }

    let snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    let nodes = snapshot["nodes"]
        .as_array()
        .expect("nodes should be present");

    for expected_name in ["matter-100", "matter-102", "motion-drop", "switch-drop"] {
        assert!(
            nodes
                .iter()
                .any(|node| node["name"].as_str() == Some(expected_name)),
            "topology fallback should expose device node {expected_name}"
        );
    }

    let room_id = harness.resolve("drop");
    let child_count = nodes
        .iter()
        .filter(|node| node["parent_id"].as_str() == Some(room_id.as_str()))
        .count();
    assert_eq!(child_count, 4);
}
