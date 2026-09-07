//! Ayla provisioning GATT. The app supplies a GATT bridge; Linux supplies the
//! shared BlueZ adapter. Bluetooth is provisioning only, not lighting control.
use crate::{types::validate_dsn, LightError, LightResult, LightSecret};
use async_trait::async_trait;
use std::time::Duration;
use zeroize::Zeroizing;

pub const ID_SERVICE: &str = "0000fe28-0000-1000-8000-00805f9b34fb";
pub const DSN: &str = "00000001-fe28-435b-991a-f1b21bb9bcd0";
pub const TOKEN_SERVICE: &str = "fce3ec41-59b6-4873-ae36-fab25bd59adc";
pub const TOKEN: &str = "7e9869ed-4db3-4520-88ea-1c21ef1ba834";
pub const WIFI_SERVICE: &str = "1cf0fe66-3ecf-4d6e-a9fc-e287ab124b96";
pub const CONNECT: &str = "1f80af6a-2b71-4e35-94e5-00f854d8f16f";
pub const STATUS: &str = "1f80af6c-2b71-4e35-94e5-00f854d8f16f";

#[derive(Clone, Copy, Debug)]
pub enum LightWifiSecurity {
    Open,
    Wpa2,
    Wpa3,
}
#[derive(Clone)]
pub struct LightWifiConfig {
    pub ssid: LightSecret,
    pub password: LightSecret,
    pub security: LightWifiSecurity,
}
impl LightWifiConfig {
    /// Exact 105-byte Ayla Wi-Fi payload; lengths are UTF-8 bytes, not chars.
    pub fn encode(&self) -> LightResult<Zeroizing<Vec<u8>>> {
        let ssid = self.ssid.expose().as_bytes();
        let password = self.password.expose().as_bytes();
        let secured = !matches!(self.security, LightWifiSecurity::Open);
        if ssid.is_empty()
            || ssid.len() > 32
            || ssid.contains(&0)
            || password.contains(&0)
            || (secured && !(8..=63).contains(&password.len()))
            || (!secured && !password.is_empty())
        {
            return Err(LightError::InvalidInput);
        }
        let mut bytes = Zeroizing::new(vec![0; 105]);
        bytes[..ssid.len()].copy_from_slice(ssid);
        bytes[32] = ssid.len() as u8;
        bytes[39..39 + password.len()].copy_from_slice(password);
        bytes[103] = password.len() as u8;
        bytes[104] = match self.security {
            LightWifiSecurity::Open => 0,
            LightWifiSecurity::Wpa2 => 3,
            LightWifiSecurity::Wpa3 => 4,
        };
        Ok(bytes)
    }
}
#[async_trait]
pub trait LightGatt: Send + Sync {
    /// Must verify the characteristic belongs to the specified service.
    async fn read(&self, service: &str, characteristic: &str) -> LightResult<Vec<u8>>;
    /// One complete GATT write-with-response, including long-write support.
    async fn write(&self, service: &str, characteristic: &str, value: &[u8]) -> LightResult<()>;
}
pub async fn identify_gatt(gatt: &dyn LightGatt) -> LightResult<String> {
    let bytes = gatt.read(ID_SERVICE, DSN).await?;
    let dsn = std::str::from_utf8(&bytes)
        .map_err(|_| LightError::Identity)?
        .trim_end_matches('\0')
        .to_owned();
    validate_dsn(&dsn).map_err(|_| LightError::Identity)?;
    Ok(dsn)
}
pub async fn provision_gatt(
    gatt: &dyn LightGatt,
    expected: &str,
    token: &LightSecret,
    wifi: &LightWifiConfig,
) -> LightResult<()> {
    let payload = wifi.encode()?;
    if token.expose().len() != 32 || !token.expose().bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(LightError::InvalidInput);
    }
    let actual = tokio::time::timeout(Duration::from_secs(10), identify_gatt(gatt))
        .await
        .map_err(|_| LightError::Unavailable)??;
    if actual != expected {
        return Err(LightError::Identity);
    }
    // From the first write onwards failures cannot safely trigger fallback.
    tokio::time::timeout(Duration::from_secs(90), async {
        gatt.write(TOKEN_SERVICE, TOKEN, token.expose().as_bytes())
            .await
            .map_err(|_| LightError::Uncertain)?;
        gatt.write(WIFI_SERVICE, CONNECT, &payload)
            .await
            .map_err(|_| LightError::Uncertain)?;
        loop {
            let status = gatt
                .read(WIFI_SERVICE, STATUS)
                .await
                .map_err(|_| LightError::Uncertain)?;
            if status.len() != 35 || status[32] > 32 || status[34] > 5 {
                return Err(LightError::Uncertain);
            }
            if status[33] != 0 && status[33] != 20 {
                return Err(LightError::Wifi);
            }
            if status[34] == 5 && status[33] == 0 {
                if &status[..status[32] as usize] != wifi.ssid.expose().as_bytes() {
                    return Err(LightError::Identity);
                }
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .map_err(|_| LightError::Uncertain)?
}
#[async_trait]
pub trait LightBleTransport: Send + Sync {
    async fn identify(&self) -> LightResult<String>;
    async fn provision(
        &self,
        dsn: &str,
        token: &LightSecret,
        wifi: &LightWifiConfig,
    ) -> LightResult<()>;
}
/// Adapter for an app-owned GATT bridge. The app retains radio/connection
/// ownership and disconnects after the operation, including cancellation.
pub struct LightAppBleTransport<G: LightGatt>(pub G);
#[async_trait]
impl<G: LightGatt> LightBleTransport for LightAppBleTransport<G> {
    async fn identify(&self) -> LightResult<String> {
        identify_gatt(&self.0).await
    }
    async fn provision(
        &self,
        dsn: &str,
        token: &LightSecret,
        wifi: &LightWifiConfig,
    ) -> LightResult<()> {
        provision_gatt(&self.0, dsn, token, wifi).await
    }
}
/// Never fall back after a write, an identity mismatch, invalid Wi-Fi input,
/// cancellation or an uncertain outcome. Reconcile the device first.
pub async fn provision_prefer_app(
    app: Option<&dyn LightBleTransport>,
    server: &dyn LightBleTransport,
    dsn: &str,
    token: &LightSecret,
    wifi: &LightWifiConfig,
) -> LightResult<()> {
    if let Some(app) = app {
        match app.provision(dsn, token, wifi).await {
            Err(LightError::Unavailable) => {}
            other => return other,
        }
    }
    server.provision(dsn, token, wifi).await
}
