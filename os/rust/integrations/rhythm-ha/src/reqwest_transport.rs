//! Reqwest-based implementation of [`HaTransport`] for desktop targets.
//!
//! Enabled by the `desktop` feature flag. Provides a ready-to-use transport
//! so desktop targets get HA communication without platform-specific code.

use std::mem::ManuallyDrop;

use anyhow::Result;

use crate::transport::{EntityState, HaConnectionConfig, HaTransport};

/// HA transport using `reqwest` with blocking HTTP client.
///
/// Uses `ManuallyDrop` + custom `Drop` to avoid panicking when the last `Arc`
/// reference is released on a tokio worker thread (`reqwest::blocking::Client`
/// contains an internal tokio runtime that cannot be dropped in an async context).
pub struct ReqwestHaTransport {
    client: ManuallyDrop<reqwest::blocking::Client>,
    config: HaConnectionConfig,
}

impl Drop for ReqwestHaTransport {
    fn drop(&mut self) {
        let client = unsafe { ManuallyDrop::take(&mut self.client) };
        std::thread::Builder::new()
            .name("reqwest-drop".to_string())
            .spawn(move || drop(client))
            .ok();
    }
}

impl ReqwestHaTransport {
    /// Create a new transport with the given HA connection config.
    pub fn new(config: HaConnectionConfig) -> Result<Self> {
        let client = reqwest::blocking::Client::builder().build()?;

        Ok(Self {
            client: ManuallyDrop::new(client),
            config,
        })
    }
}

impl HaTransport for ReqwestHaTransport {
    fn call_service(&self, domain: &str, service: &str, data: &serde_json::Value) -> Result<()> {
        let url = self
            .config
            .rest_url(&format!("/api/services/{}/{}", domain, service));

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.token))
            .header("Content-Type", "application/json")
            .json(data)
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(anyhow::anyhow!(
                "POST /api/services/{}/{} failed with status {}: {}",
                domain,
                service,
                status,
                body
            ));
        }

        Ok(())
    }

    fn get_states(&self) -> Result<Vec<EntityState>> {
        let url = self.config.rest_url("/api/states");

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.token))
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "GET /api/states failed with status {}",
                status
            ));
        }

        let states: Vec<EntityState> = resp.json()?;
        Ok(states)
    }

    fn get_state(&self, entity_id: &str) -> Result<EntityState> {
        let url = self.config.rest_url(&format!("/api/states/{}", entity_id));

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.token))
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "GET /api/states/{} failed with status {}",
                entity_id,
                status
            ));
        }

        let state: EntityState = resp.json()?;
        Ok(state)
    }

    fn test_connection(&self) -> Result<bool> {
        let url = self.config.rest_url("/api/");

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.token))
            .send()?;

        Ok(resp.status().is_success())
    }

    fn get_config(&self) -> Result<serde_json::Value> {
        let url = self.config.rest_url("/api/config");

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.token))
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(anyhow::anyhow!(
                "GET /api/config failed with status {}",
                status
            ));
        }

        Ok(resp.json()?)
    }
}
