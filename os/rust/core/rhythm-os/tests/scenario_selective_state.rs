//! State includes preserve authority and backups while omitting unrequested work.
mod harness;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use harness::{light, motion_sensor, room, TestHarness};
use rhythm_os::{
    axum_router, commands, handlers,
    state_selection::{StateNodes, StateSelection},
};
use serde_json::Value;
use tower::ServiceExt;

fn selected(h: &TestHarness, include: &str, authoritative: bool) -> Value {
    let selection = StateSelection::parse(Some(include)).unwrap().unwrap();
    let response =
        handlers::handle_get_state_with_selection(&h.state, authoritative, Some(&selection));
    assert_eq!(response.status, 200, "{}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

#[test]
fn large_installation_only_returns_requested_controls_and_counts() {
    let rooms = (0..15)
        .map(|i| room(&format!("room-{i}"), &format!("Room {i}")))
        .collect();
    let mut devices: Vec<_> = (0..73)
        .map(|i| light(&format!("bulb-{i}"), &format!("room-{}", i % 15)))
        .collect();
    devices.extend(
        (0..16).map(|i| motion_sensor(&format!("sensor-{i}"), &format!("room-{}", i % 15))),
    );
    let h = TestHarness::new().with_discovery(rooms, devices);
    h.sync();
    let full: Value =
        serde_json::from_str(&commands::build_state_snapshot(&h.state).unwrap()).unwrap();
    assert_eq!(full["nodes"].as_array().unwrap().len(), 104);
    assert!(full.get("state_scope").is_none());
    let controls = selected(&h, "controls,configuration", false);
    let nodes = controls["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 15);
    assert!(nodes.iter().all(|node| node["kind"] == "room"));
    assert_eq!(
        nodes
            .iter()
            .map(|n| n["device_counts"]["light"].as_u64().unwrap_or(0))
            .sum::<u64>(),
        73
    );
    assert_eq!(
        nodes
            .iter()
            .map(|n| n["device_counts"]["motion"].as_u64().unwrap_or(0))
            .sum::<u64>(),
        16
    );
    assert_eq!(controls["state_scope"]["nodes"], "controls");
    assert!(controls.to_string().len() < full.to_string().len() / 2);
    assert!(controls.get("profiles").is_some());
    assert_eq!(
        controls["active_profile"]["effective"]["rhythm_interval_secs"],
        full["active_profile"]["effective"]["rhythm_interval_secs"]
    );
    println!(
        "synthetic 104 nodes: full={} bytes, controls={} bytes",
        full.to_string().len(),
        controls.to_string().len()
    );

    let base = selected(&h, "base", false);
    assert!(base.get("nodes").is_none());
    for key in ["profiles", "scenes", "review", "active_profile", "location"] {
        assert!(base.get(key).is_none(), "base unexpectedly includes {key}");
    }
    assert_eq!(base["server_instance_id"], full["server_instance_id"]);
    assert_eq!(base["state_scope"]["nodes"], "none");
    assert!(base.to_string().len() < full.to_string().len() / 10);
    let detailed = selected(&h, "nodes", false);
    assert_eq!(detailed["nodes"].as_array().unwrap().len(), 104);
    assert!(detailed.get("profiles").is_none());
    let poll: Value = serde_json::from_str(
        &commands::build_nodes_state_for_scope(&h.state, StateNodes::Controls).unwrap(),
    )
    .unwrap();
    assert_eq!(poll["nodes"].as_array().unwrap().len(), 15);
}

#[test]
fn control_membership_uses_topology_placement_before_runtime_catches_up() {
    let h = TestHarness::new().with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![light("bulb", "kitchen")],
    );
    h.sync();
    let full = selected(&h, "nodes", false);
    let device_id = full["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "light_device")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    h.state
        .lock()
        .unwrap()
        .topology
        .ensure_standalone_device(&device_id);
    for include in ["controls", "controls,configuration"] {
        let response = selected(&h, include, false);
        let device = response["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == device_id)
            .expect("new standalone control");
        assert!(device.get("parent_id").is_none_or(Value::is_null));
    }
}

#[test]
fn authoritative_external_off_refresh_is_identical_for_every_selection() {
    for include in [
        "base",
        "controls",
        "controls,configuration",
        "nodes",
        "configuration",
    ] {
        let h = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
        h.sync();
        h.action("kitchen", "on").unwrap();
        let before = selected(&h, "controls", false);
        assert_eq!(before["nodes"][0]["observed_power"]["source"], "command");
        assert_eq!(before["nodes"][0]["lights_on"], true);
        selected(&h, include, true);
        let after = selected(&h, "controls", false);
        assert_eq!(after["nodes"][0]["lights_on"], false, "include={include}");
        assert_eq!(after["nodes"][0]["observed_power"]["fresh"], true);
        assert_eq!(
            after["nodes"][0]["observed_power"]["source"],
            "authoritative_refresh"
        );
        assert!(after["nodes"][0]["observed_power"]["observed_at_epoch_ms"]
            .as_u64()
            .is_some());
    }
}

#[test]
fn standalone_controls_and_full_backup_survive_restore() {
    let h = TestHarness::new().with_discovery(vec![], vec![light("standalone", "")]);
    h.sync();
    let triage = h.triage_pending_ids().into_iter().next().unwrap();
    h.triage_new(&triage).unwrap();
    let before = selected(&h, "controls", true);
    assert_eq!(before["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(before["nodes"][0]["kind"], "light_device");
    assert!(before["nodes"][0]
        .get("parent_id")
        .is_none_or(Value::is_null));
    let bundle = commands::build_backup_bundle_dto(&h.state, false).unwrap();
    let expected = serde_json::to_value(&bundle.installation.topology).unwrap();
    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();
    let after = selected(&restored, "controls", false);
    assert_eq!(after["nodes"][0]["id"], before["nodes"][0]["id"]);
    let exported = commands::build_backup_bundle_dto(&restored.state, false).unwrap();
    assert_eq!(
        serde_json::to_value(exported.installation.topology).unwrap(),
        expected
    );
}

#[tokio::test]
async fn route_selection_empty_and_invalid_are_distinct_and_keep_authority() {
    let h = TestHarness::new();
    let app = axum_router::api_routes().with_state(h.state.clone());
    for (uri, expected) in [
        (
            "/api/state?include=controls&authoritative=true",
            StatusCode::OK,
        ),
        ("/api/state?include=base", StatusCode::OK),
        ("/api/state?include=", StatusCode::OK),
        ("/api/state", StatusCode::OK),
        ("/api/state?include=controls,nodes", StatusCode::BAD_REQUEST),
        ("/api/state?include=typo", StatusCode::BAD_REQUEST),
        ("/api/nodes/state?scope=typo", StatusCode::BAD_REQUEST),
        ("/api/nodes/state?scope=controls", StatusCode::OK),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{uri}");
        if expected == StatusCode::OK {
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            if uri.ends_with("include=base") || uri.ends_with("include=") {
                assert!(value.get("nodes").is_none());
            } else {
                assert_eq!(value["nodes"], serde_json::json!([]));
            }
        }
    }
}
