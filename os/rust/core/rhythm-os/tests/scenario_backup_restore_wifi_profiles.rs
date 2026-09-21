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
        json!({"revision":first["revision"],"action":"save","ssid":"FixtureExtender","password":"second-password"}),
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
    assert_eq!(second["box_profile_id"], first_id);
    let third = edit(
        &state,
        json!({"revision":second["revision"],"action":"default","id":alternate}),
    );
    // Choosing where accessories go never retargets Box startup recovery.
    assert_eq!(
        storage
            .load_commissioning_wifi_credentials()
            .unwrap()
            .unwrap()
            .ssid,
        "FixtureMain"
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
    // A previous-version reader restores the Box, so it sees the Box network.
    let previous_reader: WifiCredentials = serde_json::from_str(&raw).unwrap();
    assert_eq!(previous_reader.ssid, "FixtureMain");
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
fn box_connection_and_provisioning_default_are_independent_roles() {
    let (_dir, state, storage) = rig();
    let wifi = |ssid: &str| WifiCredentials {
        ssid: ssid.into(),
        password: "fixture-password".into(),
    };
    let box_ssid = || {
        storage
            .load_commissioning_wifi_credentials()
            .unwrap()
            .map(|wifi| wifi.ssid)
    };
    let accessory_ssid = || {
        load_accessory_wifi_credentials(&state)
            .unwrap()
            .map(|wifi| wifi.ssid)
    };
    storage
        .save_commissioning_wifi_credentials(&wifi("FixtureHome"))
        .unwrap();
    let seeded = list(&state);
    let home = seeded["box_profile_id"].as_str().unwrap().to_string();
    assert_eq!(seeded["default_id"], home.as_str());

    // A default that follows the Box keeps following it when the Box moves,
    // and the previous network stays saved.
    storage
        .save_commissioning_wifi_credentials(&wifi("FixtureMoved"))
        .unwrap();
    let moved = list(&state);
    assert_eq!(moved["profiles"].as_array().unwrap().len(), 2);
    assert_eq!(moved["default_id"], moved["box_profile_id"]);
    assert_eq!(box_ssid().as_deref(), Some("FixtureMoved"));
    assert_eq!(accessory_ssid().as_deref(), Some("FixtureMoved"));
    // Re-recording the same connection is not an owner-visible edit.
    storage
        .save_commissioning_wifi_credentials(&wifi("FixtureMoved"))
        .unwrap();
    assert_eq!(list(&state), moved);

    // An owner-chosen default stays put when the Box moves again.
    let chosen = edit(
        &state,
        json!({"revision":moved["revision"],"action":"default","id":home}),
    );
    storage
        .save_commissioning_wifi_credentials(&wifi("FixtureThird"))
        .unwrap();
    assert_eq!(list(&state)["default_id"], chosen["default_id"]);
    assert_eq!(box_ssid().as_deref(), Some("FixtureThird"));
    assert_eq!(accessory_ssid().as_deref(), Some("FixtureHome"));

    // The owner catalog cannot edit or remove the Box connection, remove the
    // default while a choice remains, or save one network twice.
    let current = list(&state);
    for body in [
        json!({"action":"remove","id":current["box_profile_id"]}),
        json!({"action":"save","id":current["box_profile_id"],"ssid":"FixtureThird","password":"other-password"}),
        json!({"action":"remove","id":home}),
        json!({"action":"save","ssid":"FixtureMoved","password":"other-password"}),
    ] {
        let mut body = body;
        body["revision"] = current["revision"].clone();
        body["correlation_id"] = json!(uuid::Uuid::new_v4().to_string());
        assert_eq!(wifi_profiles::handle_update(&state, &body).status, 400);
    }
    assert_eq!(list(&state), current);

    // Forgetting the Box network keeps saved networks but stops recovery from
    // rejoining anything, including the provisioning default.
    storage.clear_box_wifi_credentials().unwrap();
    assert!(box_ssid().is_none());
    assert!(list(&state)["box_profile_id"].is_null());
    assert_eq!(accessory_ssid().as_deref(), Some("FixtureHome"));
    assert_eq!(list(&state)["profiles"].as_array().unwrap().len(), 3);
}
#[test]
fn a_full_catalog_never_leaves_recovery_pointing_at_the_previous_network() {
    let (_dir, state, storage) = rig();
    storage
        .save_commissioning_wifi_credentials(&WifiCredentials {
            ssid: "FixtureHome".into(),
            password: "fixture-password".into(),
        })
        .unwrap();
    for index in 1..wifi_profiles::MAX_PROFILES {
        let revision = list(&state)["revision"].clone();
        edit(
            &state,
            json!({"revision":revision,"action":"save","ssid":format!("Fixture{index}"),"password":"fixture-password"}),
        );
    }
    assert!(storage
        .save_commissioning_wifi_credentials(&WifiCredentials {
            ssid: "FixtureOverflow".into(),
            password: "fixture-password".into(),
        })
        .is_err());
    assert!(storage
        .load_commissioning_wifi_credentials()
        .unwrap()
        .is_none());
    assert_eq!(
        list(&state)["profiles"].as_array().unwrap().len(),
        wifi_profiles::MAX_PROFILES
    );
}
#[test]
fn stale_edits_empty_catalog_corruption_and_newer_schema() {
    let (dir, state, storage) = rig();
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

    // An unreadable document is quarantined and the platform seeds a working
    // catalog again. Neither the response nor reset leaks or keeps the remains.
    state
        .lock()
        .unwrap()
        .commissioning_wifi_credentials_provider = Some(Arc::new(|| {
        Ok(Some(WifiCredentials {
            ssid: "PlatformNetwork".into(),
            password: "platform-password".into(),
        }))
    }));
    let file = dir.path().join("commissioning_wifi.json");
    std::fs::write(&file, "{broken-secret").unwrap();
    let recovered = wifi_profiles::handle_list(&state);
    assert_eq!(recovered.status, 200);
    assert!(!recovered.body.contains("broken-secret"));
    assert_eq!(list(&state)["profiles"][0]["ssid"], "PlatformNetwork");
    assert_eq!(list(&state)["profiles"], list(&state)["profiles"]);
    let quarantine = dir.path().join("commissioning_wifi.json.corrupt");
    assert!(quarantine.exists());

    // A newer schema is never reinterpreted, replaced or quarantined; the Box
    // and accessories keep working from its legacy mirror.
    let newer = r#"{"ssid":"NewerBox","password":"newer-password","profile_schema":2}"#;
    std::fs::write(&file, newer).unwrap();
    assert_eq!(wifi_profiles::handle_list(&state).status, 500);
    assert!(storage
        .save_commissioning_wifi_credentials(&WifiCredentials {
            ssid: "Replacement".into(),
            password: "fixture-password".into(),
        })
        .is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), newer);
    assert_eq!(
        storage
            .load_commissioning_wifi_credentials()
            .unwrap()
            .unwrap()
            .ssid,
        "NewerBox"
    );
    assert_eq!(
        load_accessory_wifi_credentials(&state)
            .unwrap()
            .unwrap()
            .ssid,
        "NewerBox"
    );
    storage.clear_factory_reset_state().unwrap();
    assert!(!file.exists() && !quarantine.exists());
}
