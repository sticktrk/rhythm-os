//! Reqwest-based implementation of [`HaTransport`] for server-class targets.
//!
//! Provides a ready-to-use transport so current binaries get HA communication
//! without platform-specific code.

use std::mem::ManuallyDrop;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

use crate::transport::{EntityState, HaConnectionConfig, HaTransport};

const HA_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HA_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const HA_SERVICE_LIVENESS_TIMEOUT: Duration = Duration::from_secs(2);

type ServiceSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A separate reusable command connection gives each service acceptance an
/// exact HA context. The REST changed-state list also includes unrelated
/// concurrent actions, so it cannot establish ownership of those contexts.
struct ServiceSession {
    socket: ServiceSocket,
    next_id: u64,
}

struct ServiceCall {
    command: Value,
    reply: mpsc::Sender<Result<Vec<String>>>,
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
    async fn connect(config: &HaConnectionConfig) -> Result<Self> {
        let (mut socket, _) = tokio_tungstenite::connect_async(config.ws_url()).await?;
        let greeting = receive_service_message(&mut socket).await?;
        anyhow::ensure!(
            greeting["type"] == "auth_required",
            "HA service authentication expected"
        );
        socket
            .send(Message::Text(
                json!({"type":"auth", "access_token":config.token}).to_string(),
            ))
            .await?;
        let authenticated = receive_service_message(&mut socket).await?;
        anyhow::ensure!(
            authenticated["type"] == "auth_ok",
            "HA service authentication rejected"
        );
        Ok(Self { socket, next_id: 1 })
    }

    async fn exchange(&mut self, mut request: Value, response_type: &str) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        request["id"] = id.into();
        self.socket.send(Message::Text(request.to_string())).await?;
        loop {
            let message = receive_service_message(&mut self.socket).await?;
            if message["type"] == response_type && message["id"].as_u64() == Some(id) {
                return Ok(message);
            }
        }
    }

    async fn call(&mut self, command: Value) -> Result<Vec<String>> {
        let message = self.exchange(command, "result").await?;
        anyhow::ensure!(
            message["success"] == true,
            "HA service call rejected: {}",
            message["error"]
        );
        let context = message["result"]["context"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .context("HA service result missing command context")?;
        Ok(vec![context.to_owned()])
    }
}

async fn run_service_session(
    config: HaConnectionConfig,
    mut requests: tokio::sync::mpsc::Receiver<ServiceCall>,
) {
    let mut session: Option<ServiceSession> = None;
    loop {
        let request = if let Some(current) = session.as_mut() {
            tokio::select! {
                message = receive_service_message(&mut current.socket) => {
                    if message.is_err() {
                        log::debug!("HA command WebSocket closed while idle");
                        session = None;
                    }
                    continue;
                }
                request = requests.recv() => request,
            }
        } else {
            requests.recv().await
        };
        let Some(ServiceCall { command, reply }) = request else {
            break;
        };

        let result = tokio::time::timeout(HA_HTTP_REQUEST_TIMEOUT, async {
            // An HA API ping checks the full path through Supervisor to Core.
            // Reconnecting here is safe because no service frame has been sent.
            if let Some(current) = session.as_mut() {
                let ping = current.exchange(json!({"type":"ping"}), "pong");
                if !matches!(
                    tokio::time::timeout(HA_SERVICE_LIVENESS_TIMEOUT, ping).await,
                    Ok(Ok(_))
                ) {
                    log::debug!("HA command WebSocket liveness check failed; reconnecting");
                    session = None;
                }
            }
            if session.is_none() {
                session = Some(ServiceSession::connect(&config).await?);
            }
            session
                .as_mut()
                .expect("authenticated service session")
                .call(command)
                .await
        })
        .await
        .context("HA service deadline exceeded")
        .and_then(|result| result);
        if result.is_err() {
            // A timeout/disconnect may follow a committed side effect. Clear the
            // connection for the next action, but never retry this command.
            session = None;
        }
        let _ = reply.send(result);
    }
}

fn start_service_worker(
    config: HaConnectionConfig,
) -> Result<tokio::sync::mpsc::Sender<ServiceCall>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    std::thread::Builder::new()
        .name("ha-service-websocket".into())
        .spawn(move || runtime.block_on(run_service_session(config, receiver)))?;
    Ok(sender)
}

/// HA transport with blocking REST reads and an authenticated service WebSocket.
///
/// Uses `ManuallyDrop` + custom `Drop` to avoid panicking when the last `Arc`
/// reference is released on a tokio worker thread (`reqwest::blocking::Client`
/// contains an internal tokio runtime that cannot be dropped in an async context).
pub struct ReqwestHaTransport {
    client: ManuallyDrop<reqwest::blocking::Client>,
    service_worker: Mutex<Option<tokio::sync::mpsc::Sender<ServiceCall>>>,
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
        let client = reqwest::blocking::Client::builder()
            // These blocking requests run while runtime operations hold the engine lock.
            // A dead HA call must fail fast instead of hanging buttons and /api/state.
            .connect_timeout(HA_HTTP_CONNECT_TIMEOUT)
            .timeout(HA_HTTP_REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            client: ManuallyDrop::new(client),
            service_worker: Mutex::new(None),
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
        // Keep synchronous callers serialized across request and reply. The
        // worker owns the socket and keeps reading even between calls.
        let mut worker = self
            .service_worker
            .lock()
            .map_err(|_| anyhow::anyhow!("HA service connection lock"))?;
        if worker.as_ref().is_none_or(|sender| sender.is_closed()) {
            *worker = Some(start_service_worker(self.config.clone())?);
        }
        let (reply, result) = mpsc::channel();
        worker
            .as_ref()
            .expect("HA service worker started")
            .try_send(ServiceCall {
                command: json!({
                    "type":"call_service", "domain":domain,
                    "service":service, "service_data":data,
                }),
                reply,
            })
            .context("HA service worker unavailable")?;
        result.recv().context("HA service worker stopped")?
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
