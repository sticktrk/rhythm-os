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
