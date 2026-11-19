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
    let (_, view) = operation(&h, "explain_room_output", json!({"room_id":kitchen})).await;
    assert_eq!(view["active_trial"]["trial_id"], trial["trial_id"]);
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
async fn bundle_reset_lists_omitted_imported_scenes_and_protects_current_room_bindings() {
    let (h, kitchen, _) = installation();
    let id = "imported_daylight";
    {
        let mut app = h.state.lock().unwrap();
        let mut fixture = app.scenes["halloween"].clone();
        fixture.id = id.into();
        fixture.source = rhythm_os::scenes::SceneSource::Imported {
            provider: "hue".into(),
            external_id: Some("scene-1".into()),
        };
        app.scenes.insert(id.into(), fixture);
    }
    let (status, plan) = operation(&h, "plan_profile_bundle_reset", json!({})).await;
    assert_eq!(status, 200, "{plan}");
    assert!(plan["review"]["removed_scene_ids"]
        .as_array()
        .unwrap()
        .contains(&json!(id)));
    let (status, binding) = operation(
        &h,
        "plan_node_preferences_edit",
        json!({"node_id":kitchen,"profile_settings":{"mood_scene_id":id}}),
    )
    .await;
    assert_eq!(status, 200, "{binding}");
    let (status, receipt) = operation(
        &h,
        "apply_node_preferences_edit",
        json!({"plan_id":binding["plan_id"],"confirmed":true}),
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    let (status, _) = operation(
        &h,
        "apply_profile_bundle_reset",
        json!({"plan_id":plan["plan_id"],"confirmed":true}),
    )
    .await;
    assert_eq!(status, 409);
    let (status, receipt) = operation(&h, "plan_profile_bundle_reset", json!({})).await;
    assert_eq!(status, 400, "{receipt}");
    assert!(h.state.lock().unwrap().scenes.contains_key(id));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn curve_draft_and_profile_edit_preserve_other_settings_and_match_the_canonical_resource() {
    let (h, kitchen, _) = installation();
    h.state.lock().unwrap().timezone_name = Some("America/Los_Angeles".into());
    let before = h.config();
    let (_, view) = operation(&h, "explain_room_output", json!({"room_id":kitchen})).await;
    assert_eq!(view["minimum_brightness"], before.min_brightness);
    assert_eq!(view["maximum_brightness"], before.max_brightness);
    assert_eq!(view["has_evening_brightness_ramp"], true);
    assert_eq!(view["scheduled_default_state"], "active");
    let mut draft = serde_json::to_value(&before).unwrap();
    draft["min_brightness"] = json!(32);
    let (status, curve) = operation(
        &h,
        "read_profile_curve",
        json!({"view":"day","profile":draft,"date":"2026-03-21","samples_per_hour":2}),
    )
    .await;
    assert_eq!(status, 200, "{curve}");
    assert_eq!(curve["config"]["min_brightness"], 32);
    assert!(curve["solar"]["sunset"].is_number());
    assert!(curve["curve"]["hours"].as_array().unwrap().len() >= 48);
    let (_, solar) = operation(
        &h,
        "read_profile_curve",
        json!({"view":"solar","date":"2026-03-21"}),
    )
    .await;
    assert_eq!(solar, curve["solar"]);
    let (_, now) = operation(
        &h,
        "read_profile_curve",
        json!({"view":"now","profile_id":before.id,"hour":17}),
    )
    .await;
    assert_eq!(now["hour"], 17.0);
    assert_eq!(
        operation(
            &h,
            "read_profile_curve",
            json!({"view":"solar","date":"2026-02-31"})
        )
        .await
        .0,
        400
    );
    let original = h.state.lock().unwrap().mode_transition_configs();
    let (_, plan) = operation(
        &h,
        "plan_profile_edit",
        json!({"profile_id":before.id,"profile":draft}),
    )
    .await;
    assert_eq!(plan["review"]["after"]["min_brightness"], 32);
    assert_eq!(h.config(), before);
    assert_eq!(
        operation(
            &h,
            "apply_profile_edit",
            json!({"plan_id":plan["plan_id"],"confirmed":false})
        )
        .await
        .0,
        400
    );
    let (status, result) = operation(
        &h,
        "apply_profile_edit",
        json!({"plan_id":plan["plan_id"],"confirmed":true}),
    )
    .await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["canonical_readback"], plan["review"]["after"]);
    assert_eq!(h.state.lock().unwrap().mode_transition_configs(), original);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_warmth_is_explained_before_dispatch_and_supported_warmth_preserves_brightness()
{
    let (h, room, _) = installation();
    let rt = h.state.lock().unwrap().hub_runtime().unwrap();
    let initial = rt.engine_node_snapshot(&room).unwrap();
    let (status, bad) = operation(
        &h,
        "adjust_room_warmth",
        json!({"room_id":room,"kelvin":24_000}),
    )
    .await;
    assert_eq!(status, 400, "{bad}");
    assert_eq!(bad["error"]["mutation_may_have_applied"], false);
    assert_eq!(initial, rt.engine_node_snapshot(&room).unwrap());
    let (status, ok) = operation(
        &h,
        "adjust_room_warmth",
        json!({"room_id":room,"kelvin":3000}),
    )
    .await;
    assert_eq!(status, 200, "{ok}");
    assert_eq!(ok["requested_kelvin"], 3000);
    assert_eq!(ok["brightness_preserved_by_hub"], true);
    let light = h
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .find_by_native_id(&h.hub_key, "light-kitchen")
        .unwrap()
        .id
        .clone();
    h.state
        .lock()
        .unwrap()
        .canonical_registry
        .get_mut(&light)
        .unwrap()
        .endpoints[0]
        .capabilities = Some(json!({"light_capabilities":{}}));
    let (_, view) = operation(&h, "explain_room_output", json!({"room_id":room})).await;
    assert_eq!(
        view["light_readback"][0]["warmth_capability"],
        "unsupported"
    );
    let (status, bad) = operation(
        &h,
        "adjust_room_warmth",
        json!({"room_id":room,"kelvin":3000}),
    )
    .await;
    assert_eq!(status, 400, "{bad}");
    assert_eq!(bad["error"]["mutation_may_have_applied"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_reviews_the_destination_preferences_transitions_and_global_settings() {
    let (h, kitchen, hall) = installation();
    let (_, mut bundle) = operation(&h, "export_lighting_settings", json!({})).await;
    let source = bundle["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["id"] == kitchen)
        .unwrap();
    source["standby_enabled"] = json!(true);
    source["room_profile"]["motion_activation_enabled"] = json!(false);
    bundle["profile"]["mode_transitions"] = json!([]);
    bundle["profile"]["power_save"] = json!(false);
    let (_, plan) = operation(
        &h,
        "plan_lighting_transfer",
        json!({"settings":bundle,"node_mappings":{&kitchen:hall}}),
    )
    .await;
    let changes = &plan["review"]["node_changes"][&hall];
    assert_eq!(changes["source_id"], kitchen, "{plan}");
    assert_eq!(changes["after"]["standby_enabled"], true);
    assert_eq!(
        changes["after"]["room_profile"]["motion_activation_enabled"],
        false
    );
    assert_eq!(plan["review"]["resulting_power_save"], false);
    assert!(plan["review"]["resulting_transitions"].is_array());
    let (status, out) = operation(
        &h,
        "apply_lighting_transfer",
        json!({"plan_id":plan["plan_id"],"confirmed":true}),
    )
    .await;
    assert_eq!(status, 200, "{out}");
    let dest = out["canonical_readback"]["settings"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == hall)
        .unwrap();
    assert_eq!(dest["standby_enabled"], changes["after"]["standby_enabled"]);
    assert_eq!(dest["room_profile"]["motion_activation_enabled"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_preferences_handoff_and_stale_reprovisioning_never_disclose_credentials() {
    use rhythm_os::activity_cloud::StoredActivityCloudConfig;
    use rhythm_os::storage::{FileStorage, Storage};
    let (h, _, _) = installation();
    let data = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(FileStorage::new(data.path().to_str().unwrap()).unwrap());
    h.state.lock().unwrap().storage = Some(storage.clone());
    let (_, handoff) = operation(&h, "plan_activity_upload", json!({"enabled":true})).await;
    assert_eq!(handoff["status"], "handoff");
    assert_eq!(handoff["actor"], "owner");
    let mut config: StoredActivityCloudConfig = serde_json::from_value(json!({"enabled":true,
        "ingest_url":"https://example.invalid/unused","upload_token":"synthetic-upload-value",
        "home_id":"synthetic-home","hub_id":"synthetic-hub","updated_at_epoch_ms":4}))
    .unwrap();
    storage.save_activity_cloud_config(&config).unwrap();
    let plan = |h| operation(h, "plan_activity_upload", json!({"enabled":false}));
    let (status, proposal) = plan(&h).await;
    assert_eq!(status, 200, "{proposal}");
    assert_eq!(proposal["before"]["enabled"], true);
    assert!(!proposal.to_string().contains("synthetic"));
    assert_eq!(
        operation(
            &h,
            "apply_activity_upload",
            json!({"plan_id":proposal["plan_id"],"confirmed":false})
        )
        .await
        .0,
        400
    );
    config.upload_token = "synthetic-rotated-value".into();
    storage.save_activity_cloud_config(&config).unwrap();
    let confirm = |plan: &Value| json!({"plan_id":plan["plan_id"],"confirmed":true});
    assert_eq!(
        operation(&h, "apply_activity_upload", confirm(&proposal))
            .await
            .0,
        409
    );
    assert!(
        storage
            .load_activity_cloud_config()
            .unwrap()
            .unwrap()
            .enabled
    );
    let (_, proposal) = plan(&h).await;
    let (status, receipt) = operation(&h, "apply_activity_upload", confirm(&proposal)).await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["after"]["enabled"], false);
    let (_, readback) = operation(&h, "read_activity_upload", json!({})).await;
    assert_eq!(readback["enabled"], false);
    assert!(!readback.to_string().contains("synthetic"));
    h.state.lock().unwrap().platform_context = "ha_addon";
    let (status, out) = operation(
        &h,
        "plan_update_settings_edit",
        json!({"auto_update":false,"update_channel":"stable"}),
    )
    .await;
    assert_eq!(status, 200, "{out}");
    assert_eq!(out["actor"], "ha_administrator");
    assert_eq!(out["status"], "handoff");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_assignment_cannot_apply_after_its_proposed_schedule_changes() {
    let (h, room, _) = installation();
    let mut definition = json!({"id":"night-shift","name":"Night shift","transitions":[{"id":"night","from_mode":"day","to_mode":"sleep"}]});
    let confirm = |plan: &Value| json!({"plan_id":plan["plan_id"],"confirmed":true});
    let (_, create) = operation(
        &h,
        "plan_light_schedule_edit",
        json!({"schedule_id":"night-shift","definition":definition}),
    )
    .await;
    let (status, res) = operation(&h, "apply_light_schedule_edit", confirm(&create)).await;
    assert_eq!(status, 200, "{res}");
    let (_, assignment) = operation(
        &h,
        "plan_schedule_assignment_edit",
        json!({"node_id":room,"authority":"named","schedule_id":"night-shift"}),
    )
    .await;
    assert_eq!(assignment["before"]["schedule"]["name"], "Night shift");
    definition["name"] = json!("Changed schedule");
    let (_, changed) = operation(
        &h,
        "plan_light_schedule_edit",
        json!({"schedule_id":"night-shift","definition":definition}),
    )
    .await;
    let (status, res) = operation(&h, "apply_light_schedule_edit", confirm(&changed)).await;
    assert_eq!(status, 200, "{res}");
    let (status, res) = operation(&h, "apply_schedule_assignment_edit", confirm(&assignment)).await;
    assert_eq!(status, 409, "{res}");
    assert_eq!(res["error"]["mutation_may_have_applied"], false);
    assert!(h
        .snapshot(&room)
        .unwrap()
        .profile_settings
        .light_schedule
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_offset_audition_can_be_absorbed_and_an_ended_audition_cannot_be_saved() {
    let (h, room, _) = installation();
    let before = h.config();
    let args = json!({"room_id":room,"offset_minutes":-60});
    assert_eq!(
        operation(&h, "plan_absorb_offset", args.clone()).await.0,
        400
    );
    for end_trial in [true, false] {
        let (status, trial) = operation(&h, "try_room_offset", args.clone()).await;
        assert_eq!(status, 200, "{trial}");
        let (status, plan) = operation(&h, "plan_absorb_offset", args.clone()).await;
        assert_eq!(status, 200, "{plan}");
        assert_eq!(plan["trial_id"], trial["trial_id"]);
        if end_trial {
            let (status, res) = operation(
                &h,
                "undo_room_trial",
                json!({"room_id":room,"trial_id":trial["trial_id"]}),
            )
            .await;
            assert_eq!(status, 200, "{res}");
        }
        let (status, res) = operation(
            &h,
            "apply_absorb_offset",
            json!({"plan_id":plan["plan_id"],"confirmed":true}),
        )
        .await;
        if end_trial {
            assert_eq!(status, 409, "{res}");
            assert_eq!(h.config(), before);
        } else {
            assert_eq!(status, 200, "{res}");
            assert_ne!(h.config().curve, before.curve);
            let (_, room) = operation(&h, "explain_room_output", json!({"room_id":room})).await;
            assert!(room["active_trial"].is_null());
        }
    }
}
