//! Scenario regression: assistant room moves use live canonical topology and
//! refuse a reviewed plan after that topology changes.

mod harness;

use harness::{rooms_with_lights, TestHarness};
use rhythm_os::assistant::{
    apply_light_assistant_device_room_move, plan_light_assistant_device_room_move,
    LightAssistantMoveApplyRequest, LightAssistantMovePlanDto, LightAssistantMovePlanRequest,
};

fn apply_request(plan: LightAssistantMovePlanDto) -> LightAssistantMoveApplyRequest {
    LightAssistantMoveApplyRequest {
        plan_id: plan.plan_id,
        correlation_id: plan.correlation_id,
        operation: plan.operation,
        contract_sha256: plan.contract_sha256,
        server_instance_id: plan.server_instance_id,
        topology_resource_sha256: plan.topology_resource_sha256,
        device_id: plan.device.id,
        from_room_id: plan.from_room.map(|room| room.id),
        to_room_id: plan.to_room.id,
    }
}

#[test]
fn reviewed_move_applies_once_and_a_stale_follow_up_is_a_noop() {
    let (rooms, devices) = rooms_with_lights(&[("bathroom", "Bathroom"), ("foyer", "Foyer")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let bathroom_id = harness.resolve("bathroom");
    let foyer_id = harness.resolve("foyer");
    let device_id = harness
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "light-bathroom")
        .unwrap()
        .id
        .clone();

    let move_to_foyer = plan_light_assistant_device_room_move(
        &harness.state,
        LightAssistantMovePlanRequest {
            device_id: device_id.clone(),
            to_room_id: foyer_id.clone(),
            correlation_id: "scenario-move-to-foyer".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        move_to_foyer
            .from_room
            .as_ref()
            .map(|room| room.id.as_str()),
        Some(bathroom_id.as_str())
    );

    let receipt =
        apply_light_assistant_device_room_move(&harness.state, apply_request(move_to_foyer))
            .unwrap();
    assert!(receipt.canonical_readback_verified);
    assert_eq!(receipt.current_parent_id, foyer_id);

    let move_back = plan_light_assistant_device_room_move(
        &harness.state,
        LightAssistantMovePlanRequest {
            device_id: device_id.clone(),
            to_room_id: bathroom_id.clone(),
            correlation_id: "scenario-stale-move".to_string(),
        },
    )
    .unwrap();
    harness
        .state
        .lock()
        .unwrap()
        .topology
        .rename_room(&bathroom_id, "Powder Room");

    let error = apply_light_assistant_device_room_move(&harness.state, apply_request(move_back))
        .unwrap_err();
    assert_eq!(error.code, "topology_changed");
    assert!(!error.mutation_may_have_applied);
    assert_eq!(
        harness
            .state
            .lock()
            .unwrap()
            .topology
            .device_parent_room_id(&device_id),
        Some(receipt.current_parent_id.as_str())
    );
}
