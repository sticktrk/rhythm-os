//! Scenario coverage for authoritative Hue room membership moves.

use std::sync::Arc;

use rhythm_hue::room_membership::reassign_device_room;
use rhythm_hue::test_support::{HueTransportCall, SpyHueTransport};
use rhythm_hue::transport::HueTransport;

fn room_memberships(response: serde_json::Value, device_id: &str) -> Vec<String> {
    response
        .get("data")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(|room| {
            room.get("children")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .any(|child| {
                    child.get("rtype").and_then(|value| value.as_str()) == Some("device")
                        && child.get("rid").and_then(|value| value.as_str()) == Some(device_id)
                })
        })
        .filter_map(|room| room.get("id").and_then(|value| value.as_str()))
        .map(str::to_string)
        .collect()
}

#[test]
fn moved_bulb_is_removed_from_source_room_and_added_to_target_room() {
    let transport = Arc::new(SpyHueTransport::new());
    transport.set_resource_response(
        "room",
        serde_json::json!({
            "data": [
                {
                    "id": "nook",
                    "children": [
                        {"rid": "keep", "rtype": "device"},
                        {"rid": "moved", "rtype": "device"}
                    ]
                },
                {
                    "id": "office",
                    "children": [{"rid": "desk", "rtype": "device"}]
                }
            ]
        }),
    );

    reassign_device_room(&transport, "user", "moved", Some("office")).unwrap();

    let updates = transport
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            HueTransportCall::UpdateRoomChildren {
                room_id,
                device_ids,
            } => Some((room_id, device_ids)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        updates,
        vec![
            ("nook".to_string(), vec!["keep".to_string()]),
            (
                "office".to_string(),
                vec!["desk".to_string(), "moved".to_string()]
            )
        ]
    );
    let rooms = transport.get_resources("user", "room").unwrap();
    assert_eq!(room_memberships(rooms, "moved"), vec!["office"]);
}

#[test]
fn successful_move_receipt_restores_the_original_hue_room() {
    let transport = Arc::new(SpyHueTransport::new());
    transport.set_resource_response(
        "room",
        serde_json::json!({
            "data": [
                {
                    "id": "nook",
                    "children": [{"rid": "moved", "rtype": "device"}]
                },
                {"id": "office", "children": []}
            ]
        }),
    );

    let rollback = reassign_device_room(&transport, "user", "moved", Some("office")).unwrap();
    rollback.rollback(&transport, "user").unwrap();

    let rooms = transport.get_resources("user", "room").unwrap();
    assert_eq!(room_memberships(rooms, "moved"), vec!["nook"]);
}

#[test]
fn target_failure_rolls_bulb_back_into_source_room() {
    let transport = Arc::new(SpyHueTransport::new());
    transport.set_resource_response(
        "room",
        serde_json::json!({
            "data": [
                {
                    "id": "nook",
                    "children": [{"rid": "moved", "rtype": "device"}]
                },
                {"id": "office", "children": []}
            ]
        }),
    );
    transport.set_fail_room_update(Some("office"));

    let error = reassign_device_room(&transport, "user", "moved", Some("office"))
        .expect_err("target update should fail");

    assert!(error
        .to_string()
        .contains("Failed to update Hue room office"));
    let rooms = transport.get_resources("user", "room").unwrap();
    assert_eq!(room_memberships(rooms, "moved"), vec!["nook"]);
    let updates = transport
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            HueTransportCall::UpdateRoomChildren {
                room_id,
                device_ids,
            } => Some((room_id, device_ids)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        updates,
        vec![
            ("nook".to_string(), Vec::<String>::new()),
            ("office".to_string(), vec!["moved".to_string()]),
            ("nook".to_string(), vec!["moved".to_string()])
        ]
    );
}
