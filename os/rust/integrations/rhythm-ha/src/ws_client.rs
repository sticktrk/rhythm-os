//! WebSocket client for Home Assistant event stream (desktop targets).
//!
//! Connects to HA via WebSocket, authenticates, subscribes to events,
//! and emits `HaWsEvent`s for the event translator.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use log::{info, warn};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use crate::ha_lifecycle::HaWsEvent;
use crate::transport::HaConnectionConfig;

/// Start a WebSocket event stream reader in a background thread.
///
/// Returns a receiver for parsed WS events. The thread runs until
/// `shutdown` is set to `true` or the connection is lost.
pub fn start_ha_ws(
    config: HaConnectionConfig,
    shutdown: Arc<AtomicBool>,
) -> std::sync::mpsc::Receiver<HaWsEvent> {
    let (tx, rx) = sync_channel::<HaWsEvent>(64);

    std::thread::Builder::new()
        .name("ha-ws".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build WS tokio runtime");
            rt.block_on(run_ws_loop(&config, &tx, &shutdown));
        })
        .expect("Failed to spawn WS thread");

    rx
}

/// WebSocket event loop with reconnection.
async fn run_ws_loop(
    config: &HaConnectionConfig,
    tx: &SyncSender<HaWsEvent>,
    shutdown: &AtomicBool,
) {
    let ws_url = config.ws_url();
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    while !shutdown.load(Ordering::Relaxed) {
        info!(target: "ws", "Connecting to HA WebSocket at {}...", ws_url);

        // Build request with Authorization header (required by HA Supervisor proxy)
        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(&ws_url)
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
            .expect("Failed to build WS request");

        let ws_stream = match tokio_tungstenite::connect_async(request).await {
            Ok((stream, _)) => stream,
            Err(e) => {
                warn!(target: "ws", "WS connection failed: {}", e);
                let _ = tx.try_send(HaWsEvent::Disconnected(format!("Connection failed: {}", e)));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };

        info!(target: "ws", "WS connected, authenticating...");
        backoff = Duration::from_secs(1);

        let (mut write, mut read) = ws_stream.split();

        // HA WebSocket protocol:
        // 1. Server sends auth_required
        // 2. Client sends auth with access_token
        // 3. Server sends auth_ok or auth_invalid
        // 4. Client subscribes to events
        // 5. Server sends event messages

        let mut msg_id: u64 = 1;
        let mut authenticated = false;
        let mut subscribed = false;

        while let Some(msg) = read.next().await {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }

            let msg = match msg {
                Ok(Message::Text(text)) => text,
                Ok(Message::Ping(data)) => {
                    let _ = write.send(Message::Pong(data)).await;
                    continue;
                }
                Ok(Message::Close(_)) => {
                    info!(target: "ws", "WS connection closed by server");
                    break;
                }
                Ok(_) => continue,
                Err(e) => {
                    warn!(target: "ws", "WS read error: {}", e);
                    break;
                }
            };

            let json: Value = match serde_json::from_str(&msg) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

            match msg_type {
                "auth_required" => {
                    let auth_msg = serde_json::json!({
                        "type": "auth",
                        "access_token": config.token,
                    });
                    if let Err(e) = write.send(Message::Text(auth_msg.to_string())).await {
                        warn!(target: "ws", "Failed to send auth: {}", e);
                        break;
                    }
                }

                "auth_ok" => {
                    info!(target: "ws", "HA WebSocket authenticated");
                    authenticated = true;
                    let _ = tx.try_send(HaWsEvent::Connected);

                    // Subscribe to rhythm_service_event
                    let sub_msg = serde_json::json!({
                        "id": msg_id,
                        "type": "subscribe_events",
                        "event_type": "rhythm_service_event",
                    });
                    msg_id += 1;
                    if let Err(e) = write.send(Message::Text(sub_msg.to_string())).await {
                        warn!(target: "ws", "Failed to subscribe to rhythm events: {}", e);
                        break;
                    }

                    // Subscribe to zha_event
                    let zha_sub = serde_json::json!({
                        "id": msg_id,
                        "type": "subscribe_events",
                        "event_type": "zha_event",
                    });
                    msg_id += 1;
                    if let Err(e) = write.send(Message::Text(zha_sub.to_string())).await {
                        warn!(target: "ws", "Failed to subscribe to ZHA events: {}", e);
                        break;
                    }

                    // Subscribe to hue_event (native HA Hue integration button events)
                    let hue_sub = serde_json::json!({
                        "id": msg_id,
                        "type": "subscribe_events",
                        "event_type": "hue_event",
                    });
                    msg_id += 1;
                    if let Err(e) = write.send(Message::Text(hue_sub.to_string())).await {
                        warn!(target: "ws", "Failed to subscribe to Hue events: {}", e);
                        break;
                    }

                    // Subscribe to state_changed (for motion sensor binary_sensor.* entities)
                    let state_sub = serde_json::json!({
                        "id": msg_id,
                        "type": "subscribe_events",
                        "event_type": "state_changed",
                    });
                    msg_id += 1;
                    if let Err(e) = write.send(Message::Text(state_sub.to_string())).await {
                        warn!(target: "ws", "Failed to subscribe to state_changed events: {}", e);
                        break;
                    }

                    subscribed = true;
                    info!(target: "ws", "Subscribed to rhythm_service_event + zha_event + hue_event + state_changed");
                }

                "auth_invalid" => {
                    let message = json
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    warn!(target: "ws", "HA authentication failed: {}", message);
                    let _ =
                        tx.try_send(HaWsEvent::Disconnected(format!("Auth failed: {}", message)));
                    return; // Don't retry with invalid token
                }

                "event" if authenticated && subscribed => {
                    if let Some(event) = json.get("event") {
                        let event_type = event
                            .get("event_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let data = event.get("data").cloned().unwrap_or(Value::Null);

                        let _ = tx.try_send(HaWsEvent::ServiceEvent { event_type, data });
                    }
                }

                "pong" => {
                    let _ = tx.try_send(HaWsEvent::Heartbeat);
                }

                "result" => {
                    // Subscription confirmation — check for errors
                    let success = json
                        .get("success")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    if !success {
                        let error = json.get("error").cloned().unwrap_or(Value::Null);
                        warn!(target: "ws", "WS command failed: {}", error);
                    }
                }

                _ => {}
            }
        }

        if !shutdown.load(Ordering::Relaxed) {
            if authenticated {
                let _ = tx.try_send(HaWsEvent::Disconnected("Connection lost".to_string()));
            }
            info!(target: "ws", "Reconnecting WS in {:?}...", backoff);
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff);
        }
    }
}
