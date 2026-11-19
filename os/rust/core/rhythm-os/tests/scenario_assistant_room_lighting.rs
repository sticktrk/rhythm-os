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
async fn ending_a_trial_preserves_room_opt_out_and_child_power() {
    let (mut h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("den", "Den")]);
    h = h.with_discovery(rooms, devices);
    h.sync();
    let den = h.resolve("den");
    let child = h.state.lock().unwrap().topology.get(&den).unwrap().devices[0]
        .device_id
        .clone();
    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    runtime.lights_off_room(&child, None).unwrap();

    for opt_out in [false, true] {
        let (status, trial) = operation(
            &h,
            "set_room_color",
            json!({"room_id":den,"red":10,"green":20,"blue":200}),
        )
        .await;
        assert_eq!(status, 200, "{trial}");
        if opt_out {
            h.set_room_preferences(&den, Some(false), None, None);
        }
        spy.reset();
        let (status, receipt) = operation(
            &h,
            "undo_room_trial",
            json!({"room_id":den,"trial_id":trial["trial_id"]}),
        )
        .await;
        assert_eq!(status, 200, "{receipt}");
        assert_eq!(
            runtime.engine_node_snapshot(&den).unwrap().rhythm_enabled,
            !opt_out
        );
        assert!(runtime.engine_node_snapshot(&child).unwrap().hard_off);
        assert_eq!(spy.turn_on_calls().is_empty(), opt_out);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn previewing_an_off_scene_does_not_commit_hard_off() {
    let (mut h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("den", "Den")]);
    h = h.with_discovery(rooms, devices);
    h.sync();
    let den = h.resolve("den");
    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    let (status, trial) = operation(&h, "preview_room_scene_draft", json!({
        "room_id": den,
        "scene": {"id":"dark-preview","name":"Dark","light":{"default_output":{"power":"off","brightness":1}}}
    })).await;
    assert_eq!(status, 200, "{trial}");
    assert!(!spy.turn_off_calls().is_empty());
    assert!(runtime
        .engine_all_node_snapshots()
        .iter()
        .all(|node| !node.hard_off));
    spy.reset();
    let (status, receipt) = operation(
        &h,
        "undo_room_trial",
        json!({"room_id":den,"trial_id":trial["trial_id"]}),
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    assert!(!spy.turn_on_calls().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_restore_keeps_the_trial_retryable() {
    let (mut h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("den", "Den")]);
    h = h.with_discovery(rooms, devices);
    h.sync();
    let den = h.resolve("den");
    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    runtime.lights_off_room(&den, None).unwrap();
    let (status, trial) = operation(
        &h,
        "set_room_color",
        json!({"room_id":den,"red":10,"green":20,"blue":200}),
    )
    .await;
    assert_eq!(status, 200, "{trial}");
    let args = json!({"room_id":den,"trial_id":trial["trial_id"]});
    spy.fail_next_turn_off();
    assert_ne!(operation(&h, "undo_room_trial", args.clone()).await.0, 200);
    let (status, receipt) = operation(&h, "undo_room_trial", args.clone()).await;
    assert_eq!(status, 200, "{receipt}");
    assert!(runtime.engine_node_snapshot(&den).unwrap().hard_off);
    assert_eq!(operation(&h, "undo_room_trial", args).await.0, 409);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_room_with_direct_and_ha_lights_reports_each_source_independently() {
    let (mut h, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("den", "Den")]);
    h = h.with_discovery(rooms, devices);
    h.sync();
    let den = h.resolve("den");
    let ha = h.add_hub("homeassistant", "integration.test");
    let (rooms, devices) = rooms_with_lights(&[("study", "Study")]);
    h.set_hub_discovery(&ha, rooms, devices);
    h.sync_hub(&ha);
    let (direct, imported) = {
        let s = h.state.lock().unwrap();
        let registry = &s.canonical_registry;
        (
            registry
                .find_by_native_id(&h.hub_key, "light-den")
                .unwrap()
                .id
                .clone(),
            registry
                .find_by_native_id(&ha, "light-study")
                .unwrap()
                .id
                .clone(),
        )
    };
    rhythm_os::commands::do_canonical_assign_room(&h.state, &imported, Some(&den)).unwrap();
    {
        let mut s = h.state.lock().unwrap();
        s.platform_context = "rpiz";
        s.hub_connection_status.insert(ha.clone(), false);
        s.light_observations.insert(
            direct.clone(),
            rhythm_os::hub::LightObservation {
                availability: "available".into(),
                lights_on: Some(true),
                brightness: Some(63),
                kelvin: Some(3300),
                xy: None,
                rgb: None,
                context_id: None,
                source_at_epoch_ms: None,
                received_at_epoch_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            },
        );
    }
    let (status, view) = operation(&h, "explain_room_output", json!({"room_id":den})).await;
    assert_eq!(status, 200, "{view}");
    let reports = view["light_readback"].as_array().unwrap();
    let lamp = |id: &str| reports.iter().find(|r| r["light_id"] == id).unwrap();
    assert_eq!(lamp(&direct)["verification"], "fresh_device_readback");
    assert_eq!(lamp(&direct)["observed"]["brightness"], 63);
    assert_eq!(lamp(&imported)["availability"], "unavailable");
    assert!(lamp(&imported)["observed"].is_null());
    spy.reset();
    let (status, receipt) = operation(
        &h,
        "control_room_power",
        json!({"room_id":den,"power":"hard_off"}),
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    assert!(
        !spy.turn_off_calls().is_empty(),
        "the standalone controller should receive the reviewed power command"
    );
    assert_eq!(receipt["server_acknowledged"], true);
    assert!(receipt["light_readback"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["verification"] == "no_fresh_readback"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_factory_profile_reset_returns_the_installed_configuration() {
    let (h, _, _) = installation();
    let (status, plan) = operation(&h, "plan_profile_reset", json!({"profile_id":"rhythm"})).await;
    assert_eq!(status, 200, "{plan}");
    assert_eq!(plan["review"]["after"]["id"], "rhythm");
    let (status, receipt) = operation(
        &h,
        "apply_profile_reset",
        json!({"plan_id":plan["plan_id"],"confirmed":true}),
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["canonical_readback"], plan["review"]["after"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whole_home_receipt_distinguishes_queued_output_and_partial_scene_coverage() {
    let (h, _, _) = installation();
    let (tx, _rx) = std::sync::mpsc::sync_channel(32);
    let lamp = {
        let mut s = h.state.lock().unwrap();
        s.light_breaker_enabled = true;
        s.work_tx = Some(tx);
        s.canonical_registry
            .find_by_native_id(&h.hub_key, "light-kitchen")
            .unwrap()
            .id
            .clone()
    };
    let scene = json!({"id":"single-lamp","name":"Single lamp","light":{"entries":[{
        "target":{"kind":"node","node_id":lamp},"output":{"brightness":33,"color":{"kind":"kelvin","kelvin":3000}}
    }]}});
    let (status, plan) = operation(
        &h,
        "plan_scene_edit",
        json!({"scene_id":"single-lamp","definition":scene}),
    )
    .await;
    assert_eq!(status, 200, "{plan}");
    let confirm = |plan: &Value| json!({"plan_id":plan["plan_id"],"confirmed":true});
    assert_eq!(
        operation(&h, "apply_scene_edit", confirm(&plan)).await.0,
        200
    );
    for (scene, expected, failed) in [
        ("halloween", "queued", 0),
        ("single-lamp", "partially_applied", 1),
    ] {
        let (status, plan) = operation(
            &h,
            "plan_home_scene_edit",
            json!({"scene_id":scene,"target_mode":"devices"}),
        )
        .await;
        assert_eq!(status, 200, "{plan}");
        let (status, result) = operation(&h, "apply_home_scene_edit", confirm(&plan)).await;
        assert_eq!(status, 200, "{scene}: {result}");
        assert_eq!(result["status"], expected, "{result}");
        assert_eq!(result["dispatch"]["queued"], true);
        assert_eq!(result["dispatch"]["failed_target_count"], failed);
        assert_eq!(result["physical_verification"], "not_observed");
    }
}
