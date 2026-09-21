use rhythm_os::{
    provisioning::{load_accessory_wifi_credentials, WifiCredentials},
    state::{AppState, SharedState},
    storage::{FileStorage, Storage},
    wifi_profiles,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

fn rig() -> (tempfile::TempDir, SharedState, Arc<FileStorage>) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(FileStorage::new(dir.path().to_str().unwrap()).unwrap());
    let state = Arc::new(Mutex::new(AppState::default()));
    state.lock().unwrap().storage = Some(storage.clone());
    (dir, state, storage)
}
fn list(state: &SharedState) -> Value {
    let response = wifi_profiles::handle_list(state);
    assert_eq!(response.status, 200);
    serde_json::from_str(&response.body).unwrap()
}
fn edit(state: &SharedState, mut body: Value) -> Value {
    body["correlation_id"] = json!(uuid::Uuid::new_v4().to_string());
    let response = wifi_profiles::handle_update(state, &body);
    assert_eq!(response.status, 200, "{}", response.body);
    serde_json::from_str(&response.body).unwrap()
}
#[test]
fn profiles_default_override_restart_backup_and_legacy_writer_contract() {
    let (dir, state, storage) = rig();
    storage
        .save_commissioning_wifi_credentials(&WifiCredentials {
            ssid: "FixtureMain".into(),
            password: "fixture-password".into(),
        })
        .unwrap();
    let first = list(&state);
    let first_id = first["default_id"].as_str().unwrap();
    let second = edit(
        &state,
        json!({"revision":0,"action":"save","ssid":"FixtureExtender","password":"second-password"}),
    );
    let alternate = second["profiles"][1]["id"].as_str().unwrap();
    assert_eq!(second["default_id"], first_id);
    assert_eq!(
        wifi_profiles::selected_credentials(&state, alternate)
            .unwrap()
            .ssid,
        "FixtureExtender"
    );
    assert_eq!(
        load_accessory_wifi_credentials(&state)
            .unwrap()
            .unwrap()
            .ssid,
        "FixtureMain"
    );
    // A Box network change cannot silently replace either saved network.
    storage
        .save_commissioning_wifi_credentials(&WifiCredentials {
            ssid: "DifferentBoxNetwork".into(),
            password: "different-password".into(),
        })
        .unwrap();
    assert_eq!(list(&state), second);
    let third = edit(
        &state,
        json!({"revision":1,"action":"default","id":alternate}),
    );
    let restarted = Arc::new(Mutex::new(AppState::default()));
    restarted.lock().unwrap().storage = Some(Arc::new(
        FileStorage::new(dir.path().to_str().unwrap()).unwrap(),
    ));
    assert_eq!(list(&restarted), third);
    assert_eq!(
        load_accessory_wifi_credentials(&restarted)
            .unwrap()
            .unwrap()
            .ssid,
        "FixtureExtender"
    );
    let raw = std::fs::read_to_string(dir.path().join("commissioning_wifi.json")).unwrap();
    let previous_reader: WifiCredentials = serde_json::from_str(&raw).unwrap();
    assert_eq!(previous_reader.ssid, "FixtureExtender");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("commissioning_wifi.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    // Public catalog and diagnostics never carry passwords; both backup modes
    // leave the local provisioning catalog on the same Box.
    assert!(!third.to_string().contains("password"));
    for secrets in [false, true] {
        let backup = rhythm_os::commands::build_backup_bundle_dto(&state, secrets).unwrap();
        let text = serde_json::to_string(&backup).unwrap();
        for secret in [
            "FixtureMain",
            "FixtureExtender",
            "fixture-password",
            "second-password",
        ] {
            assert!(!text.contains(secret));
        }
        rhythm_os::commands::do_backup_restore(&state, backup).unwrap();
        assert_eq!(list(&state), third);
    }
    // Simulate a previous binary replacing the whole legacy file, then upgrade.
    std::fs::write(
        dir.path().join("commissioning_wifi.json"),
        r#"{"ssid":"LegacyWriter","password":"legacy-password"}"#,
    )
    .unwrap();
    let upgraded = list(&state);
    assert_eq!(upgraded["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(upgraded["profiles"][0]["ssid"], "LegacyWriter");
    storage.clear_factory_reset_state().unwrap();
    assert!(!dir.path().join("commissioning_wifi.json").exists());
}
#[test]
fn stale_edits_empty_catalog_and_corrupt_storage_fail_closed() {
    let (dir, state, _) = rig();
    let first = edit(
        &state,
        json!({"revision":0,"action":"save","ssid":"OnlyNetwork","password":"fixture-secret"}),
    );
    let stale = wifi_profiles::handle_update(
        &state,
        &json!({"revision":0,"action":"remove","id":first["default_id"],"correlation_id":uuid::Uuid::new_v4().to_string()}),
    );
    assert_eq!(stale.status, 409);
    let empty = edit(
        &state,
        json!({"revision":1,"action":"remove","id":first["default_id"]}),
    );
    assert!(empty["default_id"].is_null());
    state
        .lock()
        .unwrap()
        .commissioning_wifi_credentials_provider = Some(Arc::new(|| {
        panic!("empty managed catalog must not restore removed credentials")
    }));
    assert!(load_accessory_wifi_credentials(&state).unwrap().is_none());
    std::fs::write(dir.path().join("commissioning_wifi.json"), "{broken-secret").unwrap();
    let response = wifi_profiles::handle_list(&state);
    assert_eq!(response.status, 500);
    assert!(!response.body.contains("broken-secret"));
    assert!(load_accessory_wifi_credentials(&state).is_err());
}
