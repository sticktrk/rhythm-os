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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_snapshot_does_not_wait_for_room_io_and_emits_coalesced_corrections() {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use rhythm_core::spy_controller::SpyCall;
    use rhythm_os::{axum_router, server_event::ServerEvent};
    use std::time::Duration;
    use tower::ServiceExt;

    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        (0..8).map(|i| room(&format!("room-{i}"), "Room")).collect(),
        vec![],
    );
    harness.sync();
    for i in 0..8 {
        harness.action(&format!("room-{i}"), "on").unwrap();
    }
    tokio::time::sleep(Duration::from_millis(2)).await;
    spy.reset();
    spy.set_any_lights_on(false);
    spy.set_any_lights_on_delay(Duration::from_millis(200));
    let (tx, mut events) = tokio::sync::broadcast::channel(32);
    harness.state.lock().unwrap().event_tx = Some(tx);
    let app = axum_router::api_routes().with_state(harness.state.clone());
    for _ in 0..3 {
        let request = Request::builder()
            .uri("/api/state?include=controls,configuration&refresh_observed_power=true")
            .body(Body::empty())
            .unwrap();
        // Eight sequential slow rooms take at least 1.6 seconds. Neither the
        // first resume nor a concurrent client is allowed to wait for them.
        let response =
            tokio::time::timeout(Duration::from_millis(500), app.clone().oneshot(request))
                .await
                .expect("resume waited for integration I/O")
                .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["nodes"].as_array().unwrap().len(), 8);
    }
    let mut corrected = std::collections::HashSet::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while corrected.len() < 8 {
            if let ServerEvent::NodeState { nodes } = events.recv().await.unwrap() {
                for node in nodes {
                    let value = serde_json::to_value(node).unwrap();
                    assert_eq!(value["lights_on"], false);
                    assert_eq!(value["observed_power"]["source"], "authoritative_refresh");
                    corrected.insert(value["id"].as_str().unwrap().to_owned());
                }
            }
        }
    })
    .await
    .expect("power corrections must arrive without another request");
    assert_eq!(
        spy.calls()
            .iter()
            .filter(|call| matches!(call, SpyCall::AnyLightsOn { .. }))
            .count(),
        8,
        "concurrent resumes should share one reconciliation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_refresh_cannot_repopulate_state_after_hubs_are_removed() {
    use std::time::Duration;
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("room", "Room")], vec![]);
    harness.sync();
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    spy.hold_any_lights_on_until(gate.clone());
    let before = spy.any_lights_on_started_count();
    commands::request_observed_power_refresh_on_resume(&harness.state);
    tokio::time::timeout(Duration::from_secs(2), async {
        while spy.any_lights_on_started_count() == before {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("background query must be in flight before removal");
    {
        // This is the atomic runtime/cache boundary used by hub removal/reset.
        let mut state = harness.state.lock().unwrap();
        state.hubs.clear();
        state.room_observed_power.clear();
        state.invalidate_queued_light_dispatches();
    }
    gate.wait();
    tokio::time::timeout(Duration::from_secs(2), async {
        while harness
            .state
            .lock()
            .unwrap()
            .observed_power_resume_refresh_running
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(harness.state.lock().unwrap().room_observed_power.is_empty());
}
