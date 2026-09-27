use rhythm_os::{
    state::{AppState, SharedState},
    storage::{FileStorage, Storage},
    wifi_change::{self, WifiChangeCode, WifiChangeOutcome},
    wifi_profiles,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

#[test]
fn uncertain_requests_never_replay_and_restart_retains_recovery_fence() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(FileStorage::new(dir.path().to_str().unwrap()).unwrap());
    let state: SharedState = Arc::new(Mutex::new(AppState::default()));
    state.lock().unwrap().storage = Some(storage.clone());
    let saved = wifi_profiles::handle_update(
        &state,
        &json!({"revision":0,"action":"save","ssid":"FixtureTarget","password":"fixture-secret","correlation_id":uuid::Uuid::new_v4().to_string()}),
    );
    let profile: Value = serde_json::from_str(&saved.body).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (release, gate) = std::sync::mpsc::channel();
    let gate = Arc::new(Mutex::new(gate));
    let count = calls.clone();
    state.lock().unwrap().change_wifi_fn = Some(Arc::new(move |_, device, wifi, budget_ms| {
        assert_eq!(device, "matter-100");
        assert!((1..=300_000).contains(&budget_ms));
        assert_eq!(wifi.ssid, "FixtureTarget");
        count.fetch_add(1, Ordering::SeqCst);
        gate.lock().unwrap().recv().unwrap();
        Ok(WifiChangeOutcome {
            code: WifiChangeCode::Succeeded,
            rollback_verified: false,
        })
    }));
    let id = uuid::Uuid::new_v4().to_string();
    let request =
        json!({"operation_id":id,"device_id":"matter-100","profile_id":profile["default_id"]});
    assert_eq!(wifi_change::handle_start(&state, &request).status, 200);
    assert_eq!(wifi_change::handle_start(&state, &request).status, 200);
    assert!(rhythm_os::pairing::clear_persisted_state_for_factory_reset(&state).is_err());
    let mut conflict = request.clone();
    conflict["device_id"] = json!("matter-101");
    assert_eq!(wifi_change::handle_start(&state, &conflict).status, 409);
    conflict["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
    let active = wifi_change::handle_start(&state, &conflict);
    assert_eq!(active.status, 409);
    assert_eq!(
        serde_json::from_str::<Value>(&active.body).unwrap()["reason"],
        "change_in_progress"
    );
    release.send(()).unwrap();
    let mut receipt = Value::Null;
    for _ in 0..200 {
        receipt = serde_json::from_str(&wifi_change::handle_status(&state, &id).body).unwrap();
        if receipt["status"] == "complete" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(receipt["outcome"]["code"], "succeeded");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        wifi_change::handle_start(&state, &request).body,
        wifi_change::handle_status(&state, &id).body
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let journal = storage
        .load_integration_state_file("matter/wifi-changes.json")
        .unwrap()
        .unwrap();
    for forbidden in ["FixtureTarget", "fixture-secret", "ssid", "password"] {
        assert!(!journal.contains(forbidden));
    }
    let backup = storage.load_integration_backup_files(true).unwrap();
    assert!(backup.iter().all(|f| f.path != "matter/wifi-changes.json"));
    // Model a crash after durable admission but before a terminal write.
    let mut document: Value = serde_json::from_str(&journal).unwrap();
    document["entries"][0]["receipt"]["status"] = json!("pending");
    document["entries"][0]["receipt"]["outcome"] = Value::Null;
    document["entries"][0]["receipt"]["retry_after_ms"] =
        json!(chrono::Utc::now().timestamp_millis() as u64 + 300_000);
    storage
        .save_integration_state_file("matter/wifi-changes.json", &document.to_string())
        .unwrap();
    // A failed terminal write in this process also becomes status-only recovery.
    let lost_terminal: Value =
        serde_json::from_str(&wifi_change::handle_status(&state, &id).body).unwrap();
    assert_eq!(lost_terminal["outcome"]["code"], "recovery_required");
    document["entries"][0]["boot"] = json!("previous-process");
    storage
        .save_integration_state_file("matter/wifi-changes.json", &document.to_string())
        .unwrap();
    assert!(rhythm_os::pairing::clear_persisted_state_for_factory_reset(&state).is_err());
    let recovered: Value =
        serde_json::from_str(&wifi_change::handle_status(&state, &id).body).unwrap();
    assert_eq!(recovered["outcome"]["code"], "recovery_required");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut retry = request.clone();
    retry["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(wifi_change::handle_start(&state, &retry).status, 409);
}

fn completed_move(code: WifiChangeCode) -> (tempfile::TempDir, SharedState, Value, Value) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(FileStorage::new(dir.path().to_str().unwrap()).unwrap());
    let state: SharedState = Arc::new(Mutex::new(AppState::default()));
    state.lock().unwrap().storage = Some(storage.clone());
    let saved = wifi_profiles::handle_update(
        &state,
        &json!({
            "revision": 0, "action": "save", "ssid": "FixtureTarget",
            "password": "fixture-secret", "correlation_id": uuid::Uuid::new_v4().to_string()
        }),
    );
    let profile: Value = serde_json::from_str(&saved.body).unwrap();
    state.lock().unwrap().change_wifi_fn = Some(Arc::new(move |_, _, _, _| {
        Ok(WifiChangeOutcome {
            code: code.clone(),
            rollback_verified: false,
        })
    }));
    let request = json!({"operation_id": uuid::Uuid::new_v4().to_string(),
        "device_id": "matter-100", "profile_id": profile["default_id"]});
    assert_eq!(wifi_change::handle_start(&state, &request).status, 200);
    let mut receipt = Value::Null;
    for _ in 0..200 {
        receipt = serde_json::from_str(
            &wifi_change::handle_status(&state, request["operation_id"].as_str().unwrap()).body,
        )
        .unwrap();
        if receipt["status"] == "complete" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(receipt["status"], "complete");
    // Outcome persistence completes just after the terminal receipt write.
    let history = (0..200)
        .find_map(|_| {
            let history = storage.load_pairing_history().unwrap().unwrap();
            if history
                .entries
                .iter()
                .any(|entry| entry.kind == "wifi_change")
            {
                Some(history)
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                None
            }
        })
        .expect("terminal lifecycle outcome was not recorded");
    let event = history
        .entries
        .iter()
        .find(|entry| entry.kind == "wifi_change")
        .unwrap();
    assert_eq!(
        event.correlation_id.as_deref(),
        request["operation_id"].as_str()
    );
    assert_eq!(event.status, receipt["outcome"]["code"].as_str().unwrap());
    let evidence = serde_json::to_string(&event).unwrap();
    for forbidden in ["FixtureTarget", "fixture-secret", "matter-100"] {
        assert!(!evidence.contains(forbidden));
    }
    (dir, state, request, receipt)
}

#[test]
fn read_only_preflight_failures_release_the_cooldown() {
    for code in [WifiChangeCode::Offline, WifiChangeCode::Unsupported] {
        let (_dir, state, mut request, receipt) = completed_move(code);
        assert_eq!(receipt["retry_after_ms"], 0);
        request["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
        assert_eq!(wifi_change::handle_start(&state, &request).status, 200);
        wait_finished(&state, &request);
    }
}

#[test]
fn recovery_blocks_the_same_matter_node_but_not_another_bulb() {
    let (_dir, state, request, receipt) = completed_move(WifiChangeCode::RecoveryRequired);
    assert!(receipt["retry_after_ms"].as_u64().unwrap() > 0);
    let mut next = request.clone();
    next["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
    let conflict = wifi_change::handle_start(&state, &next);
    assert_eq!(conflict.status, 409);
    let conflict: Value = serde_json::from_str(&conflict.body).unwrap();
    assert_eq!(conflict["reason"], "device_recovering");
    assert!((1..=300_000).contains(&conflict["retry_after_ms"].as_u64().unwrap()));
    next["device_id"] = json!("matter-100-2");
    assert_eq!(wifi_change::handle_start(&state, &next).status, 409);
    next["device_id"] = json!("matter-101");
    assert_eq!(wifi_change::handle_start(&state, &next).status, 200);
    wait_finished(&state, &next);
}

#[test]
fn legacy_offline_receipt_is_repaired_after_restart_and_releases_reset_fence() {
    let (_dir, state, request, _) = completed_move(WifiChangeCode::Offline);
    let storage = state.lock().unwrap().storage.clone().unwrap();
    let mut journal: Value = serde_json::from_str(
        &storage
            .load_integration_state_file("matter/wifi-changes.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    journal["entries"][0]["receipt"]["retry_after_ms"] =
        json!(chrono::Utc::now().timestamp_millis() as u64 + 300000);
    journal["entries"][0]["boot"] = json!("previous-process");
    storage
        .save_integration_state_file("matter/wifi-changes.json", &journal.to_string())
        .unwrap();
    let receipt: Value = serde_json::from_str(
        &wifi_change::handle_status(&state, request["operation_id"].as_str().unwrap()).body,
    )
    .unwrap();
    assert_eq!(receipt["retry_after_ms"], 0);
    let snapshot = wifi_change::diagnostic_snapshot(&state);
    assert_eq!(
        snapshot["entries"][0]["receipt"]["outcome"]["code"],
        "offline"
    );
    for forbidden in [
        "FixtureTarget",
        "fixture-secret",
        "profile_id",
        "ssid",
        "password",
    ] {
        assert!(!snapshot.to_string().contains(forbidden));
    }
    assert!(rhythm_os::pairing::clear_persisted_state_for_factory_reset(&state).is_ok());
}

fn wait_finished(state: &SharedState, request: &Value) {
    for _ in 0..200 {
        let receipt: Value = serde_json::from_str(
            &wifi_change::handle_status(state, request["operation_id"].as_str().unwrap()).body,
        )
        .unwrap();
        if receipt["status"] == "complete" {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("worker did not finish");
}
