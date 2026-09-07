use rhythm_monster::{crypto::LightLanCrypto, LightCredentials, LightError, LightSecret};
use serde_json::json;
const KEY: &str = "fixture-key-not-a-real-secret";
fn app() -> LightLanCrypto {
    LightLanCrypto::new(KEY, "AAAAAAAAAAAAAAAA", "BBBBBBBBBBBBBBBB", 123, 456).unwrap()
}
fn device() -> LightLanCrypto {
    LightLanCrypto::new(KEY, "BBBBBBBBBBBBBBBB", "AAAAAAAAAAAAAAAA", 456, 123).unwrap()
}
#[test]
fn matches_independent_python_aes_hmac_fixture() {
    let encrypted = app().encrypt(json!({"name":"power","value":1})).unwrap();
    assert_eq!(
        encrypted,
        json!({"enc":"dBfpOBtGHyixodSchthZrdVmJ3qgFItAx34ZaLM9aZeJGplo9Bmp+NEtecYWXfuo","sign":"apjc/vk5UIy6nBoFJkT5Qvh4LAwDnpHaCIjFhfVFeJg="})
    );
}
#[test]
fn rejects_tampering_replay_wrong_key_and_preserves_chain_after_bad_signature() {
    let mut app = app();
    let mut device = device();
    let first = device.encrypt(json!({"name":"power","value":1})).unwrap();
    let mut bad = first.clone();
    bad["sign"] = json!("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
    assert_eq!(app.decrypt(&bad), Err(LightError::Integrity));
    assert_eq!(app.decrypt(&first).unwrap()["value"], 1);
    assert_eq!(app.decrypt(&first), Err(LightError::Integrity));
    let second = device.encrypt(json!({"name":"power","value":0})).unwrap();
    assert_eq!(app.decrypt(&second).unwrap()["value"], 0);
    let mut wrong = LightLanCrypto::new(
        "different-fixture-key",
        "AAAAAAAAAAAAAAAA",
        "BBBBBBBBBBBBBBBB",
        123,
        456,
    )
    .unwrap();
    assert_eq!(wrong.decrypt(&first), Err(LightError::Integrity));
}
#[test]
fn secrets_and_invalid_address_never_escape_diagnostics() {
    let creds = LightCredentials {
        dsn: "ACFIXTURE123456".into(),
        ip: "8.8.8.8".parse().unwrap(),
        local_key: LightSecret::new(KEY.into()),
        local_key_id: 1,
    };
    assert_eq!(creds.validate(), Err(LightError::InvalidInput));
    assert!(!format!("{creds:?}").contains(KEY));
    assert!(!format!("{:?}", creds.local_key).contains(KEY));
}
