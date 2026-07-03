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

/// HA WebSocket event types we subscribe to on every fresh connection.
/// Order matches the existing subscription order so existing test fixtures
/// and telemetry remain consistent.
pub(crate) const SUBSCRIBED_EVENT_TYPES: &[&str] = &[
    "rhythm_service_event",
    "zha_event",
    "hue_event",
    "state_changed",
];

/// Bound on the TCP + TLS + WebSocket upgrade handshake. Without this a
/// half-open peer (router reboot, wedged HA) stalls the reconnect loop
/// forever — `connect_async` itself has no deadline.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// After this much silence we send an application-level `ping`. HA does not
/// ping its clients, so on a quiet network an idle-but-healthy connection and
/// a dead one are otherwise indistinguishable.
const WS_IDLE_PING_INTERVAL: Duration = Duration::from_secs(30);
/// How long after our ping we wait for any traffic before declaring the
/// connection dead and reconnecting.
const WS_PONG_TIMEOUT: Duration = Duration::from_secs(10);
/// Fixed delay between reconnect attempts after HA rejects our token.
/// Tokens can become valid again (HA core booting behind the Supervisor
/// proxy, token rotation), so we retry slowly instead of giving up forever.
const WS_AUTH_RETRY_DELAY: Duration = Duration::from_secs(60);

/// What the WS loop should do in response to a parsed HA WebSocket message.
/// Extracted so the HA protocol state machine can be unit-tested without a
/// live WebSocket.
#[derive(Debug)]
pub(crate) enum WsMessageAction {
    /// Reply with `{type: auth, access_token: ...}`.
    SendAuth,
    /// Auth succeeded. Emit `Connected`, then subscribe to the four HA event
    /// topics.
    AuthOk,
    /// Auth failed with the given reason. Emit `Disconnected` and do NOT
    /// reconnect — the token is invalid.
    AuthInvalid(String),
    /// A subscribed event arrived. Emit a `ServiceEvent`.
    Event {
        event_type: String,
        data: Value,
    },
    /// Server pong. Emit a `Heartbeat`.
    Pong,
    /// Result/ack frame. No event, just log on failure.
    ResultSuccess,
    ResultFailure(Value),
    /// Any other message (malformed JSON, unknown type, premature event, etc.).
    Ignore,
}

/// Pure classifier: given a parsed WS JSON frame and the current auth/subscribe
/// state, decide what the loop should do next. Separated out so we can test
/// the HA protocol state machine without a network or mock server.
pub(crate) fn classify_ws_message(
    json: &Value,
    authenticated: bool,
    subscribed: bool,
) -> WsMessageAction {
    let msg_type = match json.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return WsMessageAction::Ignore,
    };

    match msg_type {
        "auth_required" => WsMessageAction::SendAuth,
        "auth_ok" => WsMessageAction::AuthOk,
        "auth_invalid" => {
            let message = json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            WsMessageAction::AuthInvalid(message)
        }
        "event" if authenticated && subscribed => {
            let Some(event) = json.get("event") else {
                return WsMessageAction::Ignore;
            };
            let event_type = event
                .get("event_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let data = event.get("data").cloned().unwrap_or(Value::Null);
            WsMessageAction::Event { event_type, data }
        }
        "pong" => WsMessageAction::Pong,
        "result" => {
            let success = json
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if success {
                WsMessageAction::ResultSuccess
            } else {
                let error = json.get("error").cloned().unwrap_or(Value::Null);
                WsMessageAction::ResultFailure(error)
            }
        }
        _ => WsMessageAction::Ignore,
    }
}

/// Start a WebSocket event stream reader in a background thread.
///
/// Returns a receiver for parsed WS events. The thread runs until
/// `shutdown` is set to `true` or the connection is lost.
pub fn start_ha_ws(
    config: HaConnectionConfig,
    shutdown: Arc<AtomicBool>,
) -> std::sync::mpsc::Receiver<HaWsEvent> {
    let (tx, rx) = sync_channel::<HaWsEvent>(64);

    // Spawn/build failures drop `tx`, which the translator observes as a
    // disconnect — never panic here: this runs on hub-configure paths.
    let spawn_result = std::thread::Builder::new()
        .name("ha-ws".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    warn!(target: "ws", "Failed to build WS tokio runtime: {}", e);
                    let _ = tx.try_send(HaWsEvent::Disconnected(format!(
                        "Failed to build WS runtime: {}",
                        e
                    )));
                    return;
                }
            };
            rt.block_on(run_ws_loop(&config, &tx, &shutdown));
        });
    if let Err(e) = spawn_result {
        warn!(target: "ws", "Failed to spawn WS thread: {}", e);
    }

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

        // Build request with Authorization header (required by HA Supervisor proxy).
        // A token/host with invalid header bytes (trailing newline in a pasted
        // token) makes this fail — retry with backoff so the user can fix the
        // credentials, instead of panicking the event thread.
        let request = match tokio_tungstenite::tungstenite::http::Request::builder()
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
        {
            Ok(request) => request,
            Err(e) => {
                warn!(target: "ws", "Failed to build WS request (bad host/token characters?): {}", e);
                let _ = tx.try_send(HaWsEvent::Disconnected(format!(
                    "Invalid connection config: {}",
                    e
                )));
                tokio::time::sleep(max_backoff).await;
                continue;
            }
        };

        let connect =
            tokio::time::timeout(WS_CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request))
                .await;
        let ws_stream = match connect {
            Ok(Ok((stream, _))) => stream,
            Ok(Err(e)) => {
                warn!(target: "ws", "WS connection failed: {}", e);
                let _ = tx.try_send(HaWsEvent::Disconnected(format!("Connection failed: {}", e)));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
            Err(_) => {
                warn!(target: "ws", "WS connection timed out after {:?}", WS_CONNECT_TIMEOUT);
                let _ = tx.try_send(HaWsEvent::Disconnected("Connection timed out".to_string()));
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
        let mut auth_rejected = false;
        // Liveness: any inbound frame proves the connection is alive. After
        // WS_IDLE_PING_INTERVAL of silence we send an HA `ping`; if nothing at
        // all arrives within WS_PONG_TIMEOUT after that, the TCP connection is
        // half-open (router reboot, AP power cycle) and we reconnect. Without
        // this, `read.next().await` blocks forever on a dead-but-unclosed
        // socket and HA events silently stop.
        let mut awaiting_pong = false;

        loop {
            let idle_limit = if awaiting_pong {
                WS_PONG_TIMEOUT
            } else {
                WS_IDLE_PING_INTERVAL
            };
            let msg = match tokio::time::timeout(idle_limit, read.next()).await {
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    info!(target: "ws", "WS stream ended");
                    break;
                }
                Err(_) => {
                    if shutdown.load(Ordering::Relaxed) {
                        return;
                    }
                    if awaiting_pong || !authenticated {
                        warn!(
                            target: "ws",
                            "WS connection unresponsive (no traffic in {:?}), reconnecting",
                            idle_limit
                        );
                        break;
                    }
                    let ping_msg = serde_json::json!({ "id": msg_id, "type": "ping" });
                    msg_id += 1;
                    if let Err(e) = write.send(Message::Text(ping_msg.to_string())).await {
                        warn!(target: "ws", "Failed to send keepalive ping: {}", e);
                        break;
                    }
                    awaiting_pong = true;
                    continue;
                }
            };
            awaiting_pong = false;

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

            match classify_ws_message(&json, authenticated, subscribed) {
                WsMessageAction::SendAuth => {
                    let auth_msg = serde_json::json!({
                        "type": "auth",
                        "access_token": config.token,
                    });
                    if let Err(e) = write.send(Message::Text(auth_msg.to_string())).await {
                        warn!(target: "ws", "Failed to send auth: {}", e);
                        break;
                    }
                }
                WsMessageAction::AuthOk => {
                    info!(target: "ws", "HA WebSocket authenticated");
                    authenticated = true;
                    let _ = tx.try_send(HaWsEvent::Connected);

                    for event_type in SUBSCRIBED_EVENT_TYPES {
                        let sub_msg = serde_json::json!({
                            "id": msg_id,
                            "type": "subscribe_events",
                            "event_type": event_type,
                        });
                        msg_id += 1;
                        if let Err(e) = write.send(Message::Text(sub_msg.to_string())).await {
                            warn!(target: "ws", "Failed to subscribe to {} events: {}", event_type, e);
                            break;
                        }
                    }

                    subscribed = true;
                    info!(target: "ws", "Subscribed to rhythm_service_event + zha_event + hue_event + state_changed");
                }
                WsMessageAction::AuthInvalid(message) => {
                    // Do NOT give up permanently: HA rejects valid tokens
                    // while core is still booting behind the Supervisor
                    // proxy, and tokens can be rotated/fixed while we run.
                    // Retry slowly instead of leaving events dead until the
                    // daemon restarts.
                    warn!(
                        target: "ws",
                        "HA authentication failed: {} (retrying in {:?})",
                        message,
                        WS_AUTH_RETRY_DELAY
                    );
                    let _ =
                        tx.try_send(HaWsEvent::Disconnected(format!("Auth failed: {}", message)));
                    auth_rejected = true;
                    break;
                }
                WsMessageAction::Event { event_type, data } => {
                    let _ = tx.try_send(HaWsEvent::ServiceEvent { event_type, data });
                }
                WsMessageAction::Pong => {
                    let _ = tx.try_send(HaWsEvent::Heartbeat);
                }
                WsMessageAction::ResultSuccess => {}
                WsMessageAction::ResultFailure(error) => {
                    warn!(target: "ws", "WS command failed: {}", error);
                }
                WsMessageAction::Ignore => {}
            }
        }

        if !shutdown.load(Ordering::Relaxed) {
            if authenticated {
                let _ = tx.try_send(HaWsEvent::Disconnected("Connection lost".to_string()));
            }
            let delay = if auth_rejected {
                WS_AUTH_RETRY_DELAY
            } else {
                backoff
            };
            info!(target: "ws", "Reconnecting WS in {:?}...", delay);
            tokio::time::sleep(delay).await;
            backoff = (backoff * 2).min(max_backoff);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subscribed_event_types_cover_all_rhythm_channels() {
        // These are the HA event types our event translator knows how to
        // handle. If a new one is added, both sides must be updated so the
        // list is a single source of truth.
        assert!(SUBSCRIBED_EVENT_TYPES.contains(&"rhythm_service_event"));
        assert!(SUBSCRIBED_EVENT_TYPES.contains(&"zha_event"));
        assert!(SUBSCRIBED_EVENT_TYPES.contains(&"hue_event"));
        assert!(SUBSCRIBED_EVENT_TYPES.contains(&"state_changed"));
    }

    #[test]
    fn classify_auth_required_requests_auth() {
        let msg = json!({ "type": "auth_required", "ha_version": "2026.4.0" });
        assert!(matches!(
            classify_ws_message(&msg, false, false),
            WsMessageAction::SendAuth
        ));
    }

    #[test]
    fn classify_auth_ok_moves_to_subscription_phase() {
        let msg = json!({ "type": "auth_ok", "ha_version": "2026.4.0" });
        assert!(matches!(
            classify_ws_message(&msg, false, false),
            WsMessageAction::AuthOk
        ));
    }

    #[test]
    fn classify_auth_invalid_surfaces_message_and_refuses_reconnect() {
        let msg = json!({ "type": "auth_invalid", "message": "expired token" });
        let action = classify_ws_message(&msg, false, false);
        match action {
            WsMessageAction::AuthInvalid(reason) => assert_eq!(reason, "expired token"),
            other => panic!("expected AuthInvalid, got {:?}", other),
        }
    }

    #[test]
    fn classify_auth_invalid_without_message_uses_unknown_reason() {
        let msg = json!({ "type": "auth_invalid" });
        match classify_ws_message(&msg, false, false) {
            WsMessageAction::AuthInvalid(reason) => assert_eq!(reason, "unknown"),
            other => panic!("expected AuthInvalid, got {:?}", other),
        }
    }

    #[test]
    fn classify_event_before_auth_is_ignored() {
        let msg = json!({
            "type": "event",
            "event": { "event_type": "zha_event", "data": {} }
        });
        assert!(matches!(
            classify_ws_message(&msg, false, false),
            WsMessageAction::Ignore
        ));
        assert!(matches!(
            classify_ws_message(&msg, true, false),
            WsMessageAction::Ignore
        ));
    }

    #[test]
    fn classify_event_when_subscribed_emits_event_with_data() {
        let msg = json!({
            "type": "event",
            "event": {
                "event_type": "zha_event",
                "data": { "device_id": "abc", "command": "on" }
            }
        });
        match classify_ws_message(&msg, true, true) {
            WsMessageAction::Event { event_type, data } => {
                assert_eq!(event_type, "zha_event");
                assert_eq!(data.get("command").and_then(|v| v.as_str()), Some("on"));
            }
            other => panic!("expected Event, got {:?}", other),
        }
    }

    #[test]
    fn classify_event_with_missing_inner_event_is_ignored() {
        let msg = json!({ "type": "event" });
        assert!(matches!(
            classify_ws_message(&msg, true, true),
            WsMessageAction::Ignore
        ));
    }

    #[test]
    fn classify_pong_emits_heartbeat() {
        let msg = json!({ "type": "pong" });
        assert!(matches!(
            classify_ws_message(&msg, true, true),
            WsMessageAction::Pong
        ));
    }

    #[test]
    fn classify_result_success_is_noop() {
        let msg = json!({ "type": "result", "success": true });
        assert!(matches!(
            classify_ws_message(&msg, true, true),
            WsMessageAction::ResultSuccess
        ));
    }

    #[test]
    fn classify_result_failure_surfaces_error_payload() {
        let msg = json!({
            "type": "result",
            "success": false,
            "error": { "code": "not_found", "message": "event_type unknown" }
        });
        match classify_ws_message(&msg, true, true) {
            WsMessageAction::ResultFailure(error) => {
                assert_eq!(
                    error.get("code").and_then(|v| v.as_str()),
                    Some("not_found")
                );
            }
            other => panic!("expected ResultFailure, got {:?}", other),
        }
    }

    #[test]
    fn classify_unknown_type_is_ignored() {
        let msg = json!({ "type": "ping_response_v999", "foo": "bar" });
        assert!(matches!(
            classify_ws_message(&msg, true, true),
            WsMessageAction::Ignore
        ));
    }

    #[test]
    fn classify_missing_type_field_is_ignored() {
        let msg = json!({ "foo": "bar" });
        assert!(matches!(
            classify_ws_message(&msg, true, true),
            WsMessageAction::Ignore
        ));
    }

    #[test]
    fn reconnect_flow_replays_auth_handshake_from_scratch() {
        // Simulate the message sequence seen on a fresh reconnect:
        // auth_required -> SendAuth -> auth_ok -> AuthOk -> event (after
        // subscribe) -> Event. This mirrors what the real loop does and
        // ensures nothing in the classifier carries state across frames.
        let auth_required = json!({ "type": "auth_required" });
        let auth_ok = json!({ "type": "auth_ok" });
        let event = json!({
            "type": "event",
            "event": { "event_type": "rhythm_service_event", "data": { "service": "step_up" } }
        });

        // Fresh state.
        assert!(matches!(
            classify_ws_message(&auth_required, false, false),
            WsMessageAction::SendAuth
        ));
        assert!(matches!(
            classify_ws_message(&auth_ok, false, false),
            WsMessageAction::AuthOk
        ));
        // After AuthOk the loop sets `authenticated = true` and subscribes.
        assert!(matches!(
            classify_ws_message(&event, true, true),
            WsMessageAction::Event { .. }
        ));

        // Drop + reconnect: same sequence again. No internal state leaked.
        assert!(matches!(
            classify_ws_message(&auth_required, false, false),
            WsMessageAction::SendAuth
        ));
        assert!(matches!(
            classify_ws_message(&auth_ok, false, false),
            WsMessageAction::AuthOk
        ));
        assert!(matches!(
            classify_ws_message(&event, true, true),
            WsMessageAction::Event { .. }
        ));
    }
}
