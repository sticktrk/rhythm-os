//! Transactional Home Assistant entity-area assignment.

use std::future::Future;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::transport::HaConnectionConfig;

const WS_TIMEOUT: Duration = Duration::from_secs(10);

type HaWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HaEntityAreaEntry {
    pub entity_id: String,
    #[serde(default)]
    pub area_id: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HaDeviceAreaEntry {
    pub id: String,
    #[serde(default)]
    pub area_id: Option<String>,
}

/// Exact entity-registry state needed to undo a successful area change.
#[derive(Debug)]
pub struct HaAreaAssignmentRollback {
    entity_id: String,
    original_area_id: Option<String>,
    changed: bool,
}

impl HaAreaAssignmentRollback {
    pub async fn rollback_with_client<C: HaAreaRegistryClient + ?Sized>(
        self,
        client: &mut C,
    ) -> Result<()> {
        if self.changed {
            client
                .update_entity_area(&self.entity_id, self.original_area_id.as_deref())
                .await
                .with_context(|| {
                    format!("Failed to restore HA area for entity {}", self.entity_id)
                })?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
struct AreaEntry {
    area_id: String,
}

struct RegistryWebSocket {
    socket: HaWebSocket,
    next_id: u64,
}

/// Entity/device registry operations needed for an authoritative area move.
///
/// The production implementation uses Home Assistant's authenticated
/// WebSocket API. The trait keeps transaction behavior independently testable.
#[async_trait]
pub trait HaAreaRegistryClient {
    async fn area_exists(&mut self, area_id: &str) -> Result<bool>;
    async fn get_entity(&mut self, entity_id: &str) -> Result<HaEntityAreaEntry>;
    async fn list_devices(&mut self) -> Result<Vec<HaDeviceAreaEntry>>;
    async fn update_entity_area(&mut self, entity_id: &str, area_id: Option<&str>) -> Result<()>;
}

impl RegistryWebSocket {
    async fn connect(config: &HaConnectionConfig) -> Result<Self> {
        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(config.ws_url())
            .header("Authorization", format!("Bearer {}", config.token))
            .header("Host", &config.host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .context("Failed to build HA entity-area WebSocket request")?;
        let (socket, _) =
            tokio::time::timeout(WS_TIMEOUT, tokio_tungstenite::connect_async(request))
                .await
                .context("Timed out connecting to HA for entity-area update")?
                .context("Failed to connect to HA for entity-area update")?;
        let mut client = Self { socket, next_id: 1 };

        loop {
            let message = client.read_json().await?;
            match message.get("type").and_then(Value::as_str) {
                Some("auth_required") => {
                    client
                        .socket
                        .send(Message::Text(
                            serde_json::json!({
                                "type": "auth",
                                "access_token": config.token,
                            })
                            .to_string(),
                        ))
                        .await
                        .context("Failed to send HA authentication")?;
                }
                Some("auth_ok") => return Ok(client),
                Some("auth_invalid") => {
                    anyhow::bail!("HA authentication failed for entity-area update")
                }
                _ => {}
            }
        }
    }

    async fn read_json(&mut self) -> Result<Value> {
        loop {
            let message = tokio::time::timeout(WS_TIMEOUT, self.socket.next())
                .await
                .context("Timed out waiting for HA entity registry response")?
                .ok_or_else(|| anyhow::anyhow!("HA WebSocket closed during entity-area update"))?
                .context("HA WebSocket error during entity-area update")?;
            match message {
                Message::Text(text) => {
                    return serde_json::from_str(&text)
                        .context("HA returned invalid JSON during entity-area update")
                }
                Message::Ping(data) => {
                    self.socket
                        .send(Message::Pong(data))
                        .await
                        .context("Failed to answer HA WebSocket ping")?;
                }
                Message::Close(_) => {
                    anyhow::bail!("HA WebSocket closed during entity-area update")
                }
                _ => {}
            }
        }
    }

    async fn command(&mut self, command_type: &str, fields: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut command = fields
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("HA command fields must be an object"))?;
        command.insert("id".to_string(), Value::from(id));
        command.insert("type".to_string(), Value::from(command_type));
        self.socket
            .send(Message::Text(Value::Object(command).to_string()))
            .await
            .with_context(|| format!("Failed to send HA command {command_type}"))?;

        loop {
            let response = self.read_json().await?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if response.get("success").and_then(Value::as_bool) == Some(true) {
                return Ok(response.get("result").cloned().unwrap_or(Value::Null));
            }
            let message = response
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("unknown Home Assistant error");
            anyhow::bail!("HA command {command_type} failed: {message}");
        }
    }

    async fn close(mut self) {
        let _ = self.socket.send(Message::Close(None)).await;
    }
}

#[async_trait]
impl HaAreaRegistryClient for RegistryWebSocket {
    async fn area_exists(&mut self, area_id: &str) -> Result<bool> {
        let areas: Vec<AreaEntry> = serde_json::from_value(
            self.command("config/area_registry/list", serde_json::json!({}))
                .await?,
        )
        .context("HA returned an invalid area registry")?;
        Ok(areas.iter().any(|area| area.area_id == area_id))
    }

    async fn get_entity(&mut self, entity_id: &str) -> Result<HaEntityAreaEntry> {
        let result = self
            .command(
                "config/entity_registry/get",
                serde_json::json!({"entity_id": entity_id}),
            )
            .await?;
        serde_json::from_value(result)
            .with_context(|| format!("HA returned an invalid registry entry for {entity_id}"))
    }

    async fn list_devices(&mut self) -> Result<Vec<HaDeviceAreaEntry>> {
        let result = self
            .command("config/device_registry/list", serde_json::json!({}))
            .await?;
        serde_json::from_value(result).context("HA returned an invalid device registry")
    }

    async fn update_entity_area(&mut self, entity_id: &str, area_id: Option<&str>) -> Result<()> {
        self.command(
            "config/entity_registry/update",
            serde_json::json!({"entity_id": entity_id, "area_id": area_id}),
        )
        .await?;
        Ok(())
    }
}

fn effective_area(entity: &HaEntityAreaEntry, devices: &[HaDeviceAreaEntry]) -> Option<String> {
    entity.area_id.clone().or_else(|| {
        entity.device_id.as_deref().and_then(|device_id| {
            devices
                .iter()
                .find(|device| device.id == device_id)
                .and_then(|device| device.area_id.clone())
        })
    })
}

async fn rollback_entity_area_best_effort<C: HaAreaRegistryClient + ?Sized>(
    client: &mut C,
    entity_id: &str,
    original_area_id: Option<&str>,
) {
    if let Err(error) = client.update_entity_area(entity_id, original_area_id).await {
        log::error!(
            target: "ha_area_membership",
            "Failed to roll back HA entity {} after area update failure: {}",
            entity_id,
            error
        );
    }
}

/// Apply and verify an HA entity-area move using a registry client.
pub async fn reassign_entity_area_with_client<C: HaAreaRegistryClient + ?Sized>(
    client: &mut C,
    entity_id: &str,
    target_area_id: Option<&str>,
) -> Result<HaAreaAssignmentRollback> {
    if let Some(target) = target_area_id {
        if !client.area_exists(target).await? {
            anyhow::bail!("Home Assistant target area not found: {target}");
        }
    }

    let original = client.get_entity(entity_id).await?;
    if original.entity_id != entity_id {
        anyhow::bail!(
            "HA returned entity {} while looking up {}",
            original.entity_id,
            entity_id
        );
    }
    let original_devices = client.list_devices().await?;
    if effective_area(&original, &original_devices).as_deref() == target_area_id {
        return Ok(HaAreaAssignmentRollback {
            entity_id: entity_id.to_string(),
            original_area_id: original.area_id,
            changed: false,
        });
    }

    let original_direct_area = original.area_id.clone();
    client
        .update_entity_area(entity_id, target_area_id)
        .await
        .with_context(|| format!("Failed to update HA area for entity {entity_id}"))?;

    let verified = async {
        let entity = client.get_entity(entity_id).await?;
        let devices = client.list_devices().await?;
        Ok::<_, anyhow::Error>(effective_area(&entity, &devices))
    }
    .await;

    match verified {
        Ok(area_id) if area_id.as_deref() == target_area_id => Ok(HaAreaAssignmentRollback {
            entity_id: entity_id.to_string(),
            original_area_id: original_direct_area,
            changed: true,
        }),
        Ok(area_id) => {
            rollback_entity_area_best_effort(client, entity_id, original_direct_area.as_deref())
                .await;
            Err(anyhow::anyhow!(
                "Home Assistant did not persist area {:?} for entity {}; effective area is {:?}",
                target_area_id,
                entity_id,
                area_id
            ))
        }
        Err(error) => {
            rollback_entity_area_best_effort(client, entity_id, original_direct_area.as_deref())
                .await;
            Err(error.context(format!(
                "Failed to verify HA area membership for entity {entity_id}"
            )))
        }
    }
}

fn run_registry_task<T, F, Fut>(task: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T>> + Send + 'static,
{
    std::thread::Builder::new()
        .name("ha-area-registry".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("Failed to build runtime for HA entity-area update")?;
            runtime.block_on(task())
        })
        .context("Failed to spawn HA entity-area worker")?
        .join()
        .map_err(|_| anyhow::anyhow!("HA entity-area worker panicked"))?
}

/// Move an HA entity to an area, or remove its entity-level area override.
///
/// The entity registry is updated and then re-read before success is returned.
/// Effective area membership includes the entity's device-level area, so an
/// unassignment fails and rolls back if the entity would still inherit an area.
pub fn reassign_entity_area(
    config: &HaConnectionConfig,
    entity_id: &str,
    target_area_id: Option<&str>,
) -> Result<HaAreaAssignmentRollback> {
    let config = config.clone();
    let entity_id = entity_id.to_string();
    let target_area_id = target_area_id.map(str::to_string);
    run_registry_task(move || async move {
        let mut client = RegistryWebSocket::connect(&config).await?;
        let result =
            reassign_entity_area_with_client(&mut client, &entity_id, target_area_id.as_deref())
                .await;
        client.close().await;
        result
    })
}

/// Undo a successful entity-area change using the exact direct area override
/// captured before the move.
pub fn rollback_entity_area(
    config: &HaConnectionConfig,
    rollback: HaAreaAssignmentRollback,
) -> Result<()> {
    let config = config.clone();
    run_registry_task(move || async move {
        let mut client = RegistryWebSocket::connect(&config).await?;
        let result = rollback.rollback_with_client(&mut client).await;
        client.close().await;
        result
    })
}

#[cfg(test)]
mod tests {
    use super::run_registry_task;

    #[test]
    fn registry_worker_can_be_called_from_an_existing_tokio_runtime() {
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let value = runtime
            .block_on(async { run_registry_task(|| async { Ok::<_, anyhow::Error>(42) }) })
            .unwrap();

        assert_eq!(value, 42);
    }
}
