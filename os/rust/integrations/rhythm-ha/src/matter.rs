//! Matter administration through Home Assistant Core, never a direct Matter transport.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

use crate::transport::HaConnectionConfig;

/// HA errors are deliberately bounded. Upstream messages can contain setup codes.
#[derive(Debug)]
pub struct Rejected;
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Home Assistant rejected the operation")
    }
}
impl std::error::Error for Rejected {}

pub struct HaMatterClient {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterDevice {
    pub device_id: String,
    pub identity: String,
    pub node_id: u64,
    pub name: String,
    pub entity_ids: Vec<String>,
    /// A node with bridged child devices. Removal affects every child.
    pub is_bridge: bool,
    pub config_entry_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MatterCatalog {
    pub schema_version: u32,
    pub available: bool,
    pub capabilities: Value,
    pub devices: Vec<MatterDevice>,
    pub reason: Option<&'static str>,
    /// Registry presence for this Matter entry, including disabled and
    /// unrecognized devices. Unknown entry metadata is retained conservatively.
    #[serde(skip)]
    pub registry_device_ids: Vec<String>,
}

impl MatterCatalog {
    pub fn confirms_removal(&self, device: &MatterDevice) -> bool {
        self.available
            && (!self.registry_device_ids.contains(&device.device_id)
                || self
                    .devices
                    .iter()
                    .any(|d| d.device_id == device.device_id && d.identity != device.identity))
    }

    pub fn unavailable() -> Self {
        Self::new(false, Vec::new())
    }
    fn new(available: bool, devices: Vec<MatterDevice>) -> Self {
        Self {
            schema_version: 1,
            available,
            capabilities: json!({"pair_on_network":available,"phone_commissioning":available,
                "share":available,"remove":available,"original_setup_code":available,
                "acknowledge_pairing":true}),
            devices,
            reason: (!available).then_some("Home Assistant Matter integration is unavailable"),
            registry_device_ids: Vec::new(),
        }
    }
}

impl HaMatterClient {
    pub async fn connect(config: &HaConnectionConfig) -> Result<Self> {
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
            .body(())?;
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .context("HA connection timed out")??;
        let mut client = Self { socket, id: 0 };
        tokio::time::timeout(Duration::from_secs(10), async {
            anyhow::ensure!(
                client.read().await?["type"] == "auth_required",
                "HA authentication unavailable"
            );
            client
                .socket
                .send(Message::Text(
                    json!({"type":"auth","access_token":config.token}).to_string(),
                ))
                .await?;
            anyhow::ensure!(
                client.read().await?["type"] == "auth_ok",
                "HA authentication rejected"
            );
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("HA authentication timed out")??;
        Ok(client)
    }

    async fn read(&mut self) -> Result<Value> {
        loop {
            match self.socket.next().await.context("HA disconnected")?? {
                Message::Text(text) => return Ok(serde_json::from_str(&text)?),
                Message::Ping(bytes) => self.socket.send(Message::Pong(bytes)).await?,
                Message::Close(_) => anyhow::bail!("HA disconnected"),
                _ => {}
            }
        }
    }

    pub async fn command(
        &mut self,
        kind: &str,
        mut fields: Value,
        deadline: Duration,
    ) -> Result<Value> {
        self.id += 1;
        let id = self.id;
        fields["id"] = json!(id);
        fields["type"] = json!(kind);
        tokio::time::timeout(deadline, async {
            self.socket.send(Message::Text(fields.to_string())).await?;
            loop {
                let result = self.read().await?;
                if result["id"].as_u64() != Some(id) {
                    continue;
                }
                if result["success"] == true {
                    return Ok(result["result"].clone());
                }
                return Err(Rejected.into());
            }
        })
        .await
        .context("HA operation outcome is unknown")?
    }

    pub async fn catalog(&mut self, scope: &str) -> Result<MatterCatalog> {
        let entries = self
            .command(
                "config_entries/get",
                json!({"domain":"matter"}),
                Duration::from_secs(10),
            )
            .await?;
        let entries: Vec<Value> = serde_json::from_value(entries)?;
        let active: Vec<_> = entries
            .iter()
            .filter(|e| e["domain"] == "matter" && e["state"] == "loaded")
            .filter_map(|e| e["entry_id"].as_str())
            .collect();
        // Core's Matter facade addresses one fabric. Ambiguous installations fail closed.
        if active.len() != 1 {
            return Ok(MatterCatalog::unavailable());
        }
        let devices = self
            .command(
                "config/device_registry/list",
                json!({}),
                Duration::from_secs(10),
            )
            .await?;
        let entities = self
            .command(
                "config/entity_registry/list",
                json!({}),
                Duration::from_secs(10),
            )
            .await?;
        let registry_device_ids = registry_presence(active[0], &devices)?;
        let devices = parse_catalog(scope, active[0], &devices, &entities)?;
        let mut catalog = MatterCatalog::new(true, devices);
        catalog.registry_device_ids = registry_device_ids;
        Ok(catalog)
    }
}

fn registry_presence(entry: &str, devices: &Value) -> Result<Vec<String>> {
    let mut present = Vec::new();
    for device in devices.as_array().context("Invalid HA device registry")? {
        let id = device["id"]
            .as_str()
            .context("Invalid HA device identity")?;
        let entries = device["config_entries"].as_array();
        let legacy = device["config_entry_id"].as_str();
        let belongs =
            legacy == Some(entry) || entries.is_some_and(|es| es.iter().any(|e| e == entry));
        let known = entries.is_some_and(|es| es.iter().all(Value::is_string)) || legacy.is_some();
        // An explicit disassociation is authoritative even when HA retains the
        // same registry device for another integration. Missing metadata is not.
        if belongs || !known {
            present.push(id.to_owned());
        }
    }
    Ok(present)
}

/// Root node identifiers contain the fabric and node, unlike a mutable entity ID.
pub fn parse_catalog(
    scope: &str,
    entry: &str,
    devices: &Value,
    entities: &Value,
) -> Result<Vec<MatterDevice>> {
    let devices = devices.as_array().context("Invalid HA device registry")?;
    let entities = entities.as_array().context("Invalid HA entity registry")?;
    let mut result = Vec::new();
    for device in devices {
        let Some(id) = device["id"].as_str() else {
            continue;
        };
        let belongs = device["config_entry_id"].as_str() == Some(entry)
            || device["config_entries"]
                .as_array()
                .is_some_and(|es| es.iter().any(|e| e == entry));
        if !belongs {
            continue;
        }
        let identifiers: Vec<_> = device["identifiers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i[0] == "matter")
            .filter_map(|i| i[1].as_str())
            .filter(|i| i.ends_with("-MatterNodeDevice"))
            .collect();
        if identifiers.len() != 1 {
            continue;
        }
        let identifier = identifiers[0];
        let Some(base) = identifier
            .strip_prefix("deviceid_")
            .and_then(|i| i.strip_suffix("-MatterNodeDevice"))
        else {
            continue;
        };
        let Some((fabric, node)) = base.split_once('-') else {
            continue;
        };
        if fabric.len() != 16 || node.len() != 16 || !fabric.bytes().all(|b| b.is_ascii_hexdigit())
        {
            continue;
        }
        let Ok(node_id) = u64::from_str_radix(node, 16) else {
            continue;
        };
        let identity = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(scope, entry, id, identifier))?)
        );
        let entity_ids = entities
            .iter()
            .filter(|e| e["device_id"] == id && e["platform"] == "matter")
            .filter_map(|e| e["entity_id"].as_str())
            .map(str::to_owned)
            .collect();
        let is_bridge = devices.iter().any(|d| d["via_device_id"] == id);
        result.push(MatterDevice {
            device_id: id.to_owned(),
            identity,
            node_id,
            name: device["name_by_user"]
                .as_str()
                .or_else(|| device["name"].as_str())
                .unwrap_or("Matter device")
                .to_owned(),
            entity_ids,
            is_bridge,
            config_entry_id: entry.to_owned(),
        });
    }
    Ok(result)
}

/// Syntax only; HA owns protocol validation. Never accept control characters or arbitrary URLs.
pub fn validate_setup_code(code: &str) -> Result<()> {
    let qr = code.strip_prefix("MT:").is_some_and(|s| {
        (1..=1021).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase() || b".-".contains(&b))
    });
    let manual = matches!(code.len(), 11 | 21) && code.bytes().all(|b| b.is_ascii_digit());
    anyhow::ensure!(
        qr || manual,
        "Enter a Matter QR payload or manual setup code"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn core_protocol_catalog_null_success_sharing_removal_and_redacted_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket
                .send(Message::Text(json!({"type":"auth_required"}).to_string()))
                .await
                .unwrap();
            let auth: Value =
                serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(auth, json!({"type":"auth","access_token":"synthetic"}));
            socket
                .send(Message::Text(json!({"type":"auth_ok"}).to_string()))
                .await
                .unwrap();
            let exchanges = [
                (
                    "config_entries/get",
                    json!({"domain":"matter"}),
                    json!([{"entry_id":"entry","domain":"matter","state":"loaded"}]),
                ),
                (
                    "config/device_registry/list",
                    json!({}),
                    json!([{"id":"device","config_entry_id":"entry","identifiers":[["matter","deviceid_0123456789ABCDEF-0000000000000001-MatterNodeDevice"]]}]),
                ),
                (
                    "config/entity_registry/list",
                    json!({}),
                    json!([{"entity_id":"light.fixture","device_id":"device","platform":"matter"}]),
                ),
                (
                    "matter/commission",
                    json!({"code":"MT:ABC123","network_only":true}),
                    Value::Null,
                ),
                (
                    "matter/open_commissioning_window",
                    json!({"device_id":"device"}),
                    json!({"setup_qr_code":"MT:SHARING","setup_manual_code":"12345678901","setup_pin_code":20202021}),
                ),
                (
                    "config/device_registry/remove_config_entry",
                    json!({"device_id":"device","config_entry_id":"entry"}),
                    Value::Null,
                ),
            ];
            for (kind, mut expected, result) in exchanges {
                let request: Value =
                    serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                expected["id"] = request["id"].clone();
                expected["type"] = json!(kind);
                assert_eq!(request, expected);
                socket
                    .send(Message::Text(
                        json!({"id":request["id"],"type":"result","success":true,"result":result})
                            .to_string(),
                    ))
                    .await
                    .unwrap();
            }
            let request: Value =
                serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            socket.send(Message::Text(json!({"id":request["id"],"success":false,"error":{"message":"secret=MT:PRIVATE"}}).to_string())).await.unwrap();
        });
        let mut client = HaMatterClient::connect(&HaConnectionConfig {
            host: "127.0.0.1".into(),
            port,
            token: "synthetic".into(),
            use_ssl: false,
        })
        .await
        .unwrap();
        let catalog = client.catalog("scope").await.unwrap();
        assert!(catalog.available);
        assert_eq!(catalog.devices[0].entity_ids, vec!["light.fixture"]);
        assert!(client
            .command(
                "matter/commission",
                json!({"code":"MT:ABC123","network_only":true}),
                Duration::from_secs(1)
            )
            .await
            .unwrap()
            .is_null());
        client
            .command(
                "matter/open_commissioning_window",
                json!({"device_id":"device"}),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        client
            .command(
                "config/device_registry/remove_config_entry",
                json!({"device_id":"device","config_entry_id":"entry"}),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        let error = client
            .command("matter/commission", json!({}), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<Rejected>().is_some());
        assert!(!format!("{error:?}").contains("PRIVATE"));
        server.await.unwrap();
    }
    #[test]
    fn roots_only_and_identity_changes_with_fabric_or_registry_owner() {
        let devices = json!([
            {"id":"root","config_entry_id":"entry","identifiers":[["matter","deviceid_0123456789ABCDEF-0000000000000001-MatterNodeDevice"]]},
            {"id":"child","config_entry_id":"entry","via_device_id":"root","identifiers":[["matter","deviceid_0123456789ABCDEF-0000000000000001-2"]]},
            {"id":"hue","config_entry_id":"entry","identifiers":[["hue","bridge"]]}
        ]);
        let parsed = parse_catalog("scope", "entry", &devices, &json!([])).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].node_id, 1);
        assert!(parsed[0].is_bridge);
        assert_ne!(
            parsed[0].identity,
            parse_catalog("other", "entry", &devices, &json!([])).unwrap()[0].identity
        );
        assert!(parse_catalog("scope", "other", &devices, &json!([]))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn removal_requires_registry_proof_and_preserves_unknown_metadata() {
        let registry = json!([
            {"id":"root","config_entries":["entry"],"identifiers":[["matter","deviceid_0123456789ABCDEF-0000000000000001-MatterNodeDevice"]]},
            {"id":"unknown"},
            {"id":"other","config_entries":["different"]}
        ]);
        let device = parse_catalog("scope", "entry", &registry, &json!([]))
            .unwrap()
            .remove(0);
        let mut catalog = MatterCatalog::new(true, vec![]);
        catalog.registry_device_ids = registry_presence("entry", &registry).unwrap();
        assert_eq!(catalog.registry_device_ids, vec!["root", "unknown"]);
        assert!(
            !catalog.confirms_removal(&device),
            "omitted identifiers do not prove removal"
        );
        catalog.registry_device_ids = registry_presence(
            "entry",
            &json!([
                {"id":"root","config_entries":["different"]}
            ]),
        )
        .unwrap();
        assert!(
            catalog.confirms_removal(&device),
            "explicit Matter disassociation proves removal"
        );
        assert!(!MatterCatalog::unavailable().confirms_removal(&device));
    }
}
