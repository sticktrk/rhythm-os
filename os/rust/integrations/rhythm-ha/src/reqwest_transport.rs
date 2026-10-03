//! Reqwest-based implementation of [`HaTransport`] for server-class targets.
//!
//! Provides a ready-to-use transport so current binaries get HA communication
//! without platform-specific code.

use std::mem::ManuallyDrop;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

use crate::transport::{EntityState, HaConnectionConfig, HaTransport};

const HA_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HA_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

type ServiceSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A separate reusable command connection gives each service acceptance an
/// exact HA context. The REST changed-state list also includes unrelated
/// concurrent actions, so it cannot establish ownership of those contexts.
struct ServiceSession {
    runtime: tokio::runtime::Runtime,
    socket: Option<ServiceSocket>,
    next_id: u64,
}

async fn receive_service_message(socket: &mut ServiceSocket) -> Result<Value> {
    loop {
        match socket
            .next()
            .await
            .context("HA service connection closed")??
        {
            Message::Text(text) => return Ok(serde_json::from_str(&text)?),
            Message::Ping(data) => socket.send(Message::Pong(data)).await?,
            Message::Close(_) => anyhow::bail!("HA service connection closed"),
            _ => {}
        }
    }
}

impl ServiceSession {
    fn call(
        &mut self,
        config: &HaConnectionConfig,
        domain: &str,
        service: &str,
        data: &Value,
    ) -> Result<Vec<String>> {
        let Self {
            runtime,
            socket,
            next_id,
        } = self;
        let result = runtime.block_on(async {
            tokio::time::timeout(HA_HTTP_REQUEST_TIMEOUT, async {
                if socket.is_none() {
                    let (mut connected, _) =
                        tokio_tungstenite::connect_async(config.ws_url()).await?;
                    let greeting = receive_service_message(&mut connected).await?;
                    anyhow::ensure!(
                        greeting["type"] == "auth_required",
                        "HA service authentication expected"
                    );
                    connected
                        .send(Message::Text(
                            json!({"type":"auth", "access_token":config.token}).to_string(),
                        ))
                        .await?;
                    let authenticated = receive_service_message(&mut connected).await?;
                    anyhow::ensure!(
                        authenticated["type"] == "auth_ok",
                        "HA service authentication rejected"
                    );
                    *socket = Some(connected);
                    *next_id = 1;
                }
                let id = *next_id;
                *next_id += 1;
                let connected = socket.as_mut().expect("authenticated service socket");
                connected
                    .send(Message::Text(
                        json!({
                            "id":id, "type":"call_service", "domain":domain,
                            "service":service, "service_data":data,
                        })
                        .to_string(),
                    ))
                    .await?;
                loop {
                    let message = receive_service_message(connected).await?;
                    if message["type"] != "result" || message["id"].as_u64() != Some(id) {
                        continue;
                    }
                    anyhow::ensure!(
                        message["success"] == true,
                        "HA service call rejected: {}",
                        message["error"]
                    );
                    let context = message["result"]["context"]["id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .context("HA service result missing command context")?;
                    return Ok(vec![context.to_owned()]);
                }
            })
            .await
            .context("HA service deadline exceeded")?
        });
        if result.is_err() {
            // A timeout/disconnect may follow a committed side effect. Clear the
            // connection for the next action, but never retry this command.
            *socket = None;
        }
        result
    }
}

/// HA transport with blocking REST reads and an authenticated service WebSocket.
///
/// Uses `ManuallyDrop` + custom `Drop` to avoid panicking when the last `Arc`
/// reference is released on a tokio worker thread (`reqwest::blocking::Client`
/// contains an internal tokio runtime that cannot be dropped in an async context).
pub struct ReqwestHaTransport {
    client: ManuallyDrop<reqwest::blocking::Client>,
    service_session: ManuallyDrop<Mutex<ServiceSession>>,
    config: HaConnectionConfig,
}

impl Drop for ReqwestHaTransport {
    fn drop(&mut self) {
        let client = unsafe { ManuallyDrop::take(&mut self.client) };
        let service_session = unsafe { ManuallyDrop::take(&mut self.service_session) };
        std::thread::Builder::new()
            .name("reqwest-drop".to_string())
            .spawn(move || {
                drop(service_session);
                drop(client);
            })
            .ok();
    }
}

impl ReqwestHaTransport {
    /// Create a new transport with the given HA connection config.
    pub fn new(config: HaConnectionConfig) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            // These blocking requests run while runtime operations hold the engine lock.
            // A dead HA call must fail fast instead of hanging buttons and /api/state.
            .connect_timeout(HA_HTTP_CONNECT_TIMEOUT)
            .timeout(HA_HTTP_REQUEST_TIMEOUT)
            .build()?;
        let service_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        Ok(Self {
            client: ManuallyDrop::new(client),
            service_session: ManuallyDrop::new(Mutex::new(ServiceSession {
                runtime: service_runtime,
                socket: None,
                next_id: 1,
            })),
            config,
        })
    }
}

impl HaTransport for ReqwestHaTransport {
    fn call_service(&self, domain: &str, service: &str, data: &serde_json::Value) -> Result<()> {
        self.call_service_contexts(domain, service, data)
            .map(|_| ())
    }

    fn call_service_contexts(
        &self,
        domain: &str,
        service: &str,
        data: &serde_json::Value,
    ) -> Result<Vec<String>> {
        self.service_session
            .lock()
            .map_err(|_| anyhow::anyhow!("HA service connection lock"))?
            .call(&self.config, domain, service, data)
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
