//! Scenario regression: periodic reconciliation clears stale lights-on cache
//! after an external/manual off.
//!
//! This covers the server-side contract the app depends on:
//! 1. Room is marked on in cached state after a normal server action
//! 2. The underlying controller later reports the room is actually off
//! 3. The next periodic worker pass refreshes cached `lights_on`
//! 4. `/api/state` and the emitted `node_state` event both report off

mod harness;

use harness::{room, TestHarness};
use rhythm_os::commands;
use rhythm_os::event_loop;
use rhythm_os::server_event::ServerEvent;
use rhythm_os::state::WorkItem;

fn node_lights_on(snapshot: &serde_json::Value, node_id: &str) -> bool {
    snapshot["nodes"]
        .as_array()
        .expect("nodes should be an array")
        .iter()
        .find(|node| node["id"].as_str() == Some(node_id))
        .and_then(|node| node["lights_on"].as_bool())
        .expect("node should exist with a lights_on field")
}

fn node<'a>(snapshot: &'a serde_json::Value, node_id: &str) -> &'a serde_json::Value {
    snapshot["nodes"]
        .as_array()
        .expect("nodes should be an array")
        .iter()
        .find(|node| node["id"].as_str() == Some(node_id))
        .expect("node should exist in state snapshot")
}

#[test]
fn periodic_tick_refreshes_state_snapshot_and_node_event_after_external_off() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    harness
        .action("kitchen", "on")
        .expect("on action should succeed");
    assert!(
        harness.lights_on("kitchen"),
        "action should seed cached on-state"
    );

    let room_id = harness.resolve("kitchen");
    let before: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    assert!(
        node_lights_on(&before, &room_id),
        "setup should reproduce the stale cached on-state before periodic reconciliation"
    );

    let (tx, mut rx) = tokio::sync::broadcast::channel(4);
    harness.state.lock().unwrap().event_tx = Some(tx);
    let dispatch_generation = harness.state.lock().unwrap().light_dispatch_generation;

    event_loop::process_work_item(
        &harness.state,
        WorkItem::PeriodicNodeTick {
            command_id: "test-periodic".into(),
            node_id: room_id.clone(),
            settings_node_id: room_id.clone(),
            current_hour: 14.0,
            emit_parent_node_id: None,
            dispatch_spacing: std::time::Duration::ZERO,
            dispatch_generation,
        },
    );

    assert!(
        !harness.lights_on("kitchen"),
        "periodic reconciliation should clear the stale cached on-state"
    );

    let after: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    assert!(
        !node_lights_on(&after, &room_id),
        "/api/state should reflect the refreshed off-state after the periodic pass"
    );
    assert_eq!(
        node(&after, &room_id)["observed_power"]["source"].as_str(),
        Some("periodic")
    );
    assert_eq!(
        node(&after, &room_id)["observed_power"]["fresh"].as_bool(),
        Some(true)
    );

    match rx
        .try_recv()
        .expect("periodic pass should emit a node_state event")
    {
        ServerEvent::NodeState { nodes } => {
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].id, room_id);
            assert!(!nodes[0].lights_on);
            assert_eq!(nodes[0].observed_power.source.as_deref(), Some("periodic"));
            assert!(nodes[0].observed_power.fresh);
            assert!(nodes[0].tick);
        }
        event => panic!("unexpected server event: {:?}", event),
    }
}
