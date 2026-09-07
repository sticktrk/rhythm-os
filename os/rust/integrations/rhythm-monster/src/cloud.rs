//! Client for the Rhythm cloud broker, never for Monster authentication APIs.
use crate::{
    ble::{provision_prefer_app, LightBleTransport, LightWifiConfig},
    types::validate_dsn,
    LightCredentials, LightError, LightResult, LightSecret,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub struct LightSetupTicket {
    pub setup_token: LightSecret,
    pub ticket: LightSecret,
}
pub struct LightCloudBroker {
    url: reqwest::Url,
    bearer: LightSecret,
    http: reqwest::Client,
}
impl LightCloudBroker {
    /// `bearer` is the authorized Rhythm user's Supabase access token. Never
    /// pass a Supabase service-role key or Monster password to an appliance.
    pub fn new(url: &str, bearer: LightSecret) -> LightResult<Self> {
        let url = reqwest::Url::parse(url).map_err(|_| LightError::InvalidInput)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
        {
            return Err(LightError::InvalidInput);
        }
        Ok(Self {
            url,
            bearer,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(45))
                .build()
                .map_err(|_| LightError::Unavailable)?,
        })
    }
    async fn call<T: serde::de::DeserializeOwned>(&self, body: Value) -> LightResult<T> {
        let mut response = self
            .http
            .post(self.url.clone())
            .bearer_auth(self.bearer.expose())
            .json(&body)
            .send()
            .await
            .map_err(|_| LightError::Cloud)?;
        match response.status().as_u16() {
            401 | 403 => return Err(LightError::Authentication),
            200 => {}
            _ => return Err(LightError::Cloud),
        }
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| LightError::Cloud)? {
            if bytes.len() + chunk.len() > 8192 {
                return Err(LightError::Cloud);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| LightError::Cloud)
    }
    pub async fn begin(&self, dsn: &str) -> LightResult<LightSetupTicket> {
        validate_dsn(dsn)?;
        self.call(json!({"action":"begin","dsn":dsn})).await
    }
    pub async fn credentials(
        &self,
        dsn: &str,
        ticket: Option<&LightSecret>,
    ) -> LightResult<LightCredentials> {
        validate_dsn(dsn)?;
        let credentials: LightCredentials = self
            .call(json!({
                "action": if ticket.is_some() { "complete" } else { "key" },
                "dsn": dsn,
                "ticket": ticket.map(LightSecret::expose)
            }))
            .await?;
        credentials.validate()?;
        if credentials.dsn != dsn {
            return Err(LightError::Identity);
        }
        Ok(credentials)
    }
    /// Exact DSN must first be read through `LightBleTransport::identify`.
    /// Returning credentials is cloud success; caller must verify a LAN read
    /// before treating the new light as controllable or persisting enrollment.
    pub async fn commission(
        &self,
        dsn: &str,
        app: Option<&dyn LightBleTransport>,
        server: &dyn LightBleTransport,
        wifi: &LightWifiConfig,
    ) -> LightResult<LightCredentials> {
        wifi.encode()?;
        let ticket = self.begin(dsn).await?;
        provision_prefer_app(app, server, dsn, &ticket.setup_token, wifi).await?;
        self.credentials(dsn, Some(&ticket.ticket)).await
    }
}
