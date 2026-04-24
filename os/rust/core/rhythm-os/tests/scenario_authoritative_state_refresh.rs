//! Scenario regression: authoritative state refresh reconciles stale power
//! before the next periodic pass.
//!
//! This covers pull-to-refresh / reconnect flows that should force a live
//! backend sample instead of re-reading stale cached `lights_on`.

mod harness;

use harness::{room, TestHarness};
use rhythm_os::commands;
use rhythm_os::handlers;

fn node<'a>(snapshot: &'a serde_json::Value, node_id: &str) -> &'a serde_json::Value {
    snapshot["nodes"]
        .as_array()
        .expect("nodes should be an array")
        .iter()
        .find(|node| node["id"].as_str() == Some(node_id))
        .expect("node should exist in state snapshot")
}

#[test]
fn authoritative_state_refresh_reconciles_external_off_before_periodic() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    harness
        .action("kitchen", "on")
        .expect("on action should succeed");

    let room_id = harness.resolve("kitchen");
    let before: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    let before_node = node(&before, &room_id);
    assert!(
        before_node["lights_on"]
            .as_bool()
            .expect("lights_on should be a bool"),
        "non-authoritative /api/state should still show the stale cached on-state"
    );
    assert_eq!(
        before_node["observed_power"]["source"].as_str(),
        Some("command")
    );
    assert_eq!(
        before_node["observed_power"]["fresh"].as_bool(),
        Some(false)
    );

    let response = handlers::handle_get_state_with_options(&harness.state, true);
    assert_eq!(response.status, 200);

    let after: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    let after_node = node(&after, &room_id);
    assert!(
        !after_node["lights_on"]
            .as_bool()
            .expect("lights_on should be a bool"),
        "authoritative refresh should replace the stale cached on-state with the live off-state"
    );
    assert_eq!(
        after_node["observed_power"]["lights_on"].as_bool(),
        Some(false)
    );
    assert_eq!(after_node["observed_power"]["fresh"].as_bool(), Some(true));
    assert_eq!(
        after_node["observed_power"]["source"].as_str(),
        Some("authoritative_refresh")
    );
    assert!(
        after_node["observed_power"]["observed_at_epoch_ms"]
            .as_u64()
            .is_some(),
        "authoritative refresh should stamp an observation timestamp"
    );

    assert!(
        !harness.lights_on("kitchen"),
        "authoritative refresh should update the cached lights_on state"
    );
}
