use async_trait::async_trait;
use rhythm_monster::{ble::*, LightError, LightResult, LightSecret};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
fn wifi() -> LightWifiConfig {
    LightWifiConfig {
        ssid: LightSecret::new("lab".into()),
        password: LightSecret::new("test-only-password".into()),
        security: LightWifiSecurity::Wpa2,
    }
}
fn token() -> LightSecret {
    LightSecret::new("0123456789abcdef0123456789abcdef".into())
}
struct Gatt {
    dsn: &'static str,
    writes: Mutex<Vec<(String, Vec<u8>)>>,
    fail_write: bool,
}
#[async_trait]
impl LightGatt for Gatt {
    async fn read(&self, _service: &str, c: &str) -> LightResult<Vec<u8>> {
        if c == DSN {
            return Ok(self.dsn.as_bytes().to_vec());
        }
        let mut status = vec![0; 35];
        status[..3].copy_from_slice(b"lab");
        status[32] = 3;
        status[34] = 5;
        Ok(status)
    }
    async fn write(&self, _service: &str, c: &str, v: &[u8]) -> LightResult<()> {
        self.writes.lock().unwrap().push((c.into(), v.to_vec()));
        if self.fail_write {
            Err(LightError::Unavailable)
        } else {
            Ok(())
        }
    }
}
#[tokio::test]
async fn validates_exact_identity_before_token_and_wifi_write() {
    let gatt = Gatt {
        dsn: "ACFIXTURE123456",
        writes: Mutex::new(vec![]),
        fail_write: false,
    };
    assert_eq!(
        provision_gatt(&gatt, "ACOTHER123456", &token(), &wifi()).await,
        Err(LightError::Identity)
    );
    assert!(gatt.writes.lock().unwrap().is_empty());
    provision_gatt(&gatt, gatt.dsn, &token(), &wifi())
        .await
        .unwrap();
    let writes = gatt.writes.lock().unwrap();
    assert_eq!(writes[0].0, TOKEN);
    assert_eq!(writes[1].0, CONNECT);
    assert_eq!(writes[1].1.len(), 105);
    assert_eq!(writes[1].1[32], 3);
    assert_eq!(writes[1].1[104], 3);
    assert_eq!(&writes[1].1[39..57], b"test-only-password");
}
#[tokio::test]
async fn write_failure_is_uncertain_even_if_transport_says_unavailable() {
    let gatt = Gatt {
        dsn: "ACFIXTURE123456",
        writes: Mutex::new(vec![]),
        fail_write: true,
    };
    assert_eq!(
        provision_gatt(&gatt, gatt.dsn, &token(), &wifi()).await,
        Err(LightError::Uncertain)
    );
    assert_eq!(gatt.writes.lock().unwrap().len(), 1);
}
struct Transport {
    outcome: LightResult<()>,
    calls: AtomicUsize,
}
#[async_trait]
impl LightBleTransport for Transport {
    async fn identify(&self) -> LightResult<String> {
        Ok("ACFIXTURE123456".into())
    }
    async fn provision(&self, _: &str, _: &LightSecret, _: &LightWifiConfig) -> LightResult<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.outcome
    }
}
#[tokio::test]
async fn app_preferred_and_fallback_only_before_writes() {
    for outcome in [
        Ok(()),
        Err(LightError::Unavailable),
        Err(LightError::Uncertain),
        Err(LightError::Identity),
        Err(LightError::Wifi),
    ] {
        let app = Transport {
            outcome,
            calls: AtomicUsize::new(0),
        };
        let server = Transport {
            outcome: Ok(()),
            calls: AtomicUsize::new(0),
        };
        let result =
            provision_prefer_app(Some(&app), &server, "ACFIXTURE123456", &token(), &wifi()).await;
        assert_eq!(app.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            server.calls.load(Ordering::SeqCst),
            usize::from(outcome == Err(LightError::Unavailable))
        );
        assert_eq!(
            result,
            if outcome == Err(LightError::Unavailable) {
                Ok(())
            } else {
                outcome
            }
        );
    }
}
#[test]
fn wifi_lengths_are_bytes_and_never_truncated() {
    let mut w = wifi();
    w.ssid = LightSecret::new("é".repeat(17));
    assert_eq!(w.encode().unwrap_err(), LightError::InvalidInput);
    w.ssid = LightSecret::new("é".repeat(16));
    assert_eq!(w.encode().unwrap()[32], 32);
    w.password = LightSecret::new("x".repeat(64));
    assert_eq!(w.encode().unwrap_err(), LightError::InvalidInput);
}
