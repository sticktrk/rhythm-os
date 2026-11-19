//! Exercise assistant operations through the hub's HTTP entry point and real
//! adaptive runtime. No observation is supplied by the spy light controller.
mod harness;

use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use harness::{rooms_with_lights, TestHarness};
use rhythm_os::axum_router::api_routes;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn operation(hub: &TestHarness, id: &str, args: Value) -> (u16, Value) {
    let response = api_routes()
        .with_state(hub.state.clone())
        .oneshot(
            Request::post(format!("/api/assistant/operations/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(args.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

fn installation() -> (TestHarness, String, String) {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen"), ("hall", "Hall")]);
    let h = TestHarness::new_at(17., 170).with_discovery(rooms, devices);
    h.sync();
    let kitchen = h.resolve("kitchen");
    let hall = h.resolve("hall");
    (h, kitchen, hall)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tuning_preserves_warmth_and_other_rooms_and_requires_a_fresh_review() {
    let (h, kitchen, hall) = installation();
    let before = h.config();
    let (status, plan) = operation(
        &h,
        "plan_room_tuning",
        json!({"room_id":kitchen,"setting":"minimum_brightness","value":40}),
    )
    .await;
    assert_eq!(status, 200, "{plan}");
    assert_eq!(plan["affected_rooms"].as_array().unwrap().len(), 1);
    for pair in plan["samples"].as_array().unwrap() {
        assert_eq!(pair["before"]["kelvin"], pair["after"]["kelvin"]);
        assert!(pair["after"]["brightness"].as_u64() >= pair["before"]["brightness"].as_u64());
    }
    let request = json!({"plan_id":plan["plan_id"],"confirmed":true});
    assert_ne!(
        operation(
            &h,
            "apply_room_tuning",
            json!({"plan_id":plan["plan_id"],"confirmed":false})
        )
        .await
        .0,
        200
    );
    let (status, receipt) = operation(&h, "apply_room_tuning", request.clone()).await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["canonical_readback_verified"], true);
    assert_eq!(operation(&h, "apply_room_tuning", request).await.0, 409);
    assert_eq!(h.config(), before);
    let rt = h.state.lock().unwrap().hub_runtime().unwrap();
    assert_eq!(
        rt.engine_node_snapshot(&kitchen)
            .unwrap()
            .profile_settings
            .profile_overrides[&before.id]
            .min_brightness,
        Some(40)
    );
    assert!(rt
        .engine_node_snapshot(&hall)
        .unwrap()
        .profile_settings
        .profile_overrides
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absorb_lists_every_cleared_offset_and_rejects_a_changed_neighbor_atomically() {
    let (h, kitchen, hall) = installation();
    h.set_room_offset(&kitchen, -60.);
    h.set_room_offset(&hall, 20.);
    let before = h.config();
    let args = json!({"room_id":kitchen,"offset_minutes":-60});
    let (status, plan) = operation(&h, "plan_absorb_offset", args.clone()).await;
    assert_eq!(status, 200, "{plan}");
    assert_eq!(plan["cleared_offsets"].as_array().unwrap().len(), 2);
    h.set_room_offset(&hall, 30.);
    let (status, _) = operation(
        &h,
        "apply_absorb_offset",
        json!({"plan_id":plan["plan_id"],"confirmed":true}),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(h.config(), before);
    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    assert_eq!(
        runtime
            .engine_node_snapshot(&kitchen)
            .unwrap()
            .time_offset_minutes,
        -60.
    );
    assert_eq!(
        runtime
            .engine_node_snapshot(&hall)
            .unwrap()
            .time_offset_minutes,
        30.
    );
    let (status, plan) = operation(&h, "plan_absorb_offset", args).await;
    assert_eq!(status, 200, "{plan}");
    let (status, receipt) = operation(
        &h,
        "apply_absorb_offset",
        json!({"plan_id":plan["plan_id"],"confirmed":true}),
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    assert_ne!(h.config().curve, before.curve);
    assert_eq!(
        runtime
            .engine_node_snapshot(&kitchen)
            .unwrap()
            .time_offset_minutes,
        0.
    );
    assert_eq!(
        runtime
            .engine_node_snapshot(&hall)
            .unwrap()
            .time_offset_minutes,
        0.
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_trial_is_undoable_only_on_the_selected_room_and_does_not_edit_the_profile() {
    let (h, kitchen, hall) = installation();
    let config = h.config();
    let (status, trial) = operation(
        &h,
        "try_room_offset",
        json!({"room_id":kitchen,"offset_minutes":-60}),
    )
    .await;
    assert_eq!(status, 200, "{trial}");
    assert_eq!(trial["server_acknowledged"], true);
    assert!(trial["light_readback"]
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["verification"] == "no_fresh_readback"));
    assert_eq!(
        operation(
            &h,
            "undo_room_trial",
            json!({"room_id":hall,"trial_id":trial["trial_id"]})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        operation(
            &h,
            "undo_room_trial",
            json!({"room_id":kitchen,"trial_id":trial["trial_id"]})
        )
        .await
        .0,
        200
    );
    assert_eq!(h.config(), config);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviews_create_edit_and_delete_a_scene_and_reject_a_stale_deletion() {
    let (h, _, _) = installation();
    let id = "blue-reading";
    let scene = json!({"id":id,"name":"Blue reading","light":{"default_output":{"brightness":55,"color":{"kind":"kelvin","kelvin":3000}}}});
    let request = json!({"scene_id":id,"definition":scene});
    let (status, created) = operation(&h, "plan_scene_edit", request).await;
    assert_eq!(status, 200, "{created}");
    let apply = |p: &Value| json!({"plan_id":p["plan_id"],"confirmed":true});
    let (status, receipt) = operation(&h, "apply_scene_edit", apply(&created)).await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["after"]["name"], "Blue reading");
    let del = json!({"scene_id":id,"definition":null});
    let (_, delete_plan) = operation(&h, "plan_scene_edit", del.clone()).await;
    let mut updated = scene.clone();
    updated["name"] = json!("Hall reading");
    let (_, edit) = operation(
        &h,
        "plan_scene_edit",
        json!({"scene_id":id,"definition":updated}),
    )
    .await;
    assert_eq!(operation(&h, "apply_scene_edit", apply(&edit)).await.0, 200);
    assert_eq!(
        operation(&h, "apply_scene_edit", apply(&delete_plan))
            .await
            .0,
        409
    );
    assert_eq!(h.state.lock().unwrap().scenes[id].name, "Hall reading");
    let (_, delete_plan) = operation(&h, "plan_scene_edit", del).await;
    assert_eq!(
        operation(&h, "apply_scene_edit", apply(&delete_plan))
            .await
            .0,
        200
    );
    assert!(!h.state.lock().unwrap().scenes.contains_key(id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_a_node_preference_invalidates_the_review_and_does_not_overwrite_other_settings() {
    let (h, kitchen, _) = installation();
    let args = json!({"node_id":kitchen,"profile_settings":{"motion_activation_enabled":false}});
    let (status, plan) = operation(&h, "plan_node_preferences_edit", args.clone()).await;
    assert_eq!(status, 200, "{plan}");
    h.set_room_mood_enabled(&kitchen, true);
    let apply = |p: &Value| json!({"plan_id":p["plan_id"],"confirmed":true});
    assert_eq!(
        operation(&h, "apply_node_preferences_edit", apply(&plan))
            .await
            .0,
        409
    );
    let (_, plan) = operation(&h, "plan_node_preferences_edit", args).await;
    let (status, result) = operation(&h, "apply_node_preferences_edit", apply(&plan)).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(
        result["after"]["local_profile_settings"]["motion_activation_enabled"],
        false
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_tokens_are_scoped_by_operation_and_hub() {
    let (h, _, _) = installation();
    let (other, _, _) = installation();
    let before = h.state.lock().unwrap().light_breaker_enabled;
    let (_, plan) = operation(&h, "plan_light_breaker_edit", json!({"enabled":!before})).await;
    let apply = json!({"plan_id":plan["plan_id"],"confirmed":true});
    assert_eq!(
        operation(&other, "apply_light_breaker_edit", apply.clone())
            .await
            .0,
        409
    );
    assert_eq!(
        operation(&h, "apply_update_settings_edit", apply.clone())
            .await
            .0,
        409
    );
    assert_eq!(
        operation(&h, "apply_light_breaker_edit", apply).await.0,
        200
    );
    assert_ne!(h.state.lock().unwrap().light_breaker_enabled, before);
}
