//! SSE reader for Hue bridge event streams using raw `reqwest` streaming.
//!
//! Enabled by the `desktop` feature flag. We stream raw response bytes and
//! feed them through the shared SSE line parser so Hue heartbeat comments
//! (`: hi`) count as real activity. `reqwest-eventsource` only surfaces
//! parsed message events, which made quiet-but-healthy streams look stalled.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use log::{info, warn};

use crate::sse::{drain_sse_lines, HueSseConfig, HueSseEvent, SseParseState};

/// Maximum seconds without any SSE bytes before assuming the connection is
/// stalled and reconnecting. Hue bridges send heartbeat comment frames every
/// ~10s, so 45s ≈ 4 missed heartbeats.
const SSE_IDLE_TIMEOUT_SECS: u64 = 45;

/// Log an "SSE alive" message at this interval during idle periods.
const ALIVE_LOG_INTERVAL_SECS: u64 = 300;

/// Start an SSE event stream reader in a background thread.
///
/// Returns a receiver for parsed SSE events. The thread runs until
/// `shutdown` is set to `true` or the connection is lost.
pub fn start_reqwest_sse(
    config: HueSseConfig,
    shutdown: Arc<AtomicBool>,
) -> std::sync::mpsc::Receiver<HueSseEvent> {
    let (tx, rx) = sync_channel::<HueSseEvent>(64);

    std::thread::Builder::new()
        .name("hue-sse".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build SSE tokio runtime");
            rt.block_on(run_sse_loop(&config, &tx, &shutdown));
        })
        .expect("Failed to spawn SSE thread");

    rx
}

/// SSE event loop using raw reqwest response streaming.
async fn run_sse_loop(config: &HueSseConfig, tx: &SyncSender<HueSseEvent>, shutdown: &AtomicBool) {
    let url = format!("https://{}/eventstream/clip/v2", config.bridge_ip);

    let client = match reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .tcp_keepalive(Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(target: "sse", "Failed to build SSE client: {}", e);
            let _ = tx.try_send(HueSseEvent::Disconnected(format!(
                "Client build error: {}",
                e
            )));
            return;
        }
    };

    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);
    let mut parse_state = SseParseState::new();
    let mut connect_count: u32 = 0;

    while !shutdown.load(Ordering::Relaxed) {
        connect_count += 1;
        info!(target: "sse", "Connecting SSE to {} (conn #{})...", url, connect_count);

        let request = client
            .get(&url)
            .header("hue-application-key", &config.username)
            .header("Accept", "text/event-stream");

        let response = match request.send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => response,
                Err(e) => {
                    warn!(target: "sse", "SSE connect failed: {}", e);
                    info!(target: "sse", "Reconnecting SSE in {:?}...", backoff);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }
            },
            Err(e) => {
                warn!(target: "sse", "SSE request failed: {}", e);
                info!(target: "sse", "Reconnecting SSE in {:?}...", backoff);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };

        let mut connected = false;
        let mut last_byte_event = Instant::now();
        let mut last_alive_log = Instant::now();
        let mut chunks_since_alive: u32 = 0;
        let mut line_buf = Vec::with_capacity(4096);
        let mut stream = response.bytes_stream();

        info!(target: "sse", "SSE connected (conn #{})", connect_count);
        let _ = tx.try_send(HueSseEvent::Connected);
        connected = true;
        backoff = Duration::from_secs(1);
        last_byte_event = Instant::now();
        last_alive_log = Instant::now();

        loop {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }

            match tokio::time::timeout(Duration::from_secs(SSE_IDLE_TIMEOUT_SECS), stream.next())
                .await
            {
                Ok(Some(Ok(chunk))) => {
                    let idle_secs = last_byte_event.elapsed().as_secs();
                    if idle_secs > 60 {
                        info!(
                            target: "sse",
                            "SSE: bytes after {}m{}s idle",
                            idle_secs / 60,
                            idle_secs % 60
                        );
                    }

                    last_byte_event = Instant::now();
                    chunks_since_alive = chunks_since_alive.saturating_add(1);
                    line_buf.extend_from_slice(&chunk);
                    drain_sse_lines(&mut line_buf, tx, &mut parse_state);

                    if last_alive_log.elapsed().as_secs() >= ALIVE_LOG_INTERVAL_SECS {
                        info!(
                            target: "sse",
                            "SSE: alive ({} chunks in last {}m)",
                            chunks_since_alive,
                            ALIVE_LOG_INTERVAL_SECS / 60
                        );
                        last_alive_log = Instant::now();
                        chunks_since_alive = 0;
                    }
                }
                Ok(Some(Err(e))) => {
                    warn!(target: "sse", "SSE error: {}", e);
                    break;
                }
                Ok(None) => {
                    info!(target: "sse", "SSE stream ended");
                    break;
                }
                Err(_) => {
                    warn!(
                        target: "sse",
                        "SSE: No bytes for {}s (stall detected), reconnecting",
                        SSE_IDLE_TIMEOUT_SECS
                    );
                    break;
                }
            }
        }

        if !shutdown.load(Ordering::Relaxed) {
            if connected {
                let _ = tx.try_send(HueSseEvent::Disconnected("Connection lost".to_string()));
            }
            info!(target: "sse", "Reconnecting SSE in {:?}...", backoff);
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff);
        }
    }
}
