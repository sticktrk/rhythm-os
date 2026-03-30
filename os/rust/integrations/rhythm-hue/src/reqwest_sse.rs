//! SSE reader for Hue bridge event streams using `reqwest-eventsource`.
//!
//! Enabled by the `desktop` feature flag. Uses a proper SSE client that
//! handles reconnection, chunked transfer encoding, and keep-alive natively.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use log::{info, warn};
use reqwest_eventsource::retry;
use reqwest_eventsource::{Event, EventSource};

use crate::sse::{process_sse_line, HueSseConfig, HueSseEvent, SseParseState};

/// Maximum seconds without any SSE data (including heartbeats) before
/// assuming the connection is stalled and reconnecting. Hue bridges send
/// heartbeat comments every ~10s, so 45s ≈ 4 missed heartbeats.
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

/// SSE event loop using reqwest-eventsource.
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

        let mut es = EventSource::new(request).unwrap();
        // Disable auto-reconnection. The library would send Last-Event-ID
        // on retry, causing the bridge to replay missed events — which is
        // the root cause of button-press bundling after idle. Our outer
        // loop handles reconnection with fresh EventSource instances.
        es.set_retry_policy(Box::new(retry::Never));

        let mut connected = false;
        let mut last_data_event = Instant::now();
        let mut last_alive_log = Instant::now();
        let mut msg_since_data: u32 = 0;

        loop {
            let event =
                match tokio::time::timeout(Duration::from_secs(SSE_IDLE_TIMEOUT_SECS), es.next())
                    .await
                {
                    Ok(Some(event)) => event,
                    Ok(None) => break, // stream ended naturally
                    Err(_) => {
                        // No data (not even heartbeats) for SSE_IDLE_TIMEOUT_SECS
                        warn!(
                            target: "sse",
                            "SSE: No data for {}s (stall detected), reconnecting",
                            SSE_IDLE_TIMEOUT_SECS
                        );
                        es.close();
                        break;
                    }
                };

            if shutdown.load(Ordering::Relaxed) {
                es.close();
                return;
            }

            match event {
                Ok(Event::Open) => {
                    info!(target: "sse", "SSE connected (conn #{})", connect_count);
                    connected = true;
                    backoff = Duration::from_secs(1);
                    last_data_event = Instant::now();
                    last_alive_log = Instant::now();
                }
                Ok(Event::Message(msg)) => {
                    let data = msg.data;
                    if data.is_empty() {
                        // Empty SSE message (keepalive). Log periodically
                        // so we can verify the connection was alive during
                        // idle periods.
                        msg_since_data += 1;
                        if last_alive_log.elapsed().as_secs() >= ALIVE_LOG_INTERVAL_SECS {
                            info!(
                                target: "sse",
                                "SSE: alive (idle {}m, {} msgs since last data)",
                                last_data_event.elapsed().as_secs() / 60,
                                msg_since_data
                            );
                            last_alive_log = Instant::now();
                        }
                        continue;
                    }

                    // Log gap if we've been idle for a while
                    let idle_secs = last_data_event.elapsed().as_secs();
                    if idle_secs > 60 {
                        info!(
                            target: "sse",
                            "SSE: data event after {}m{}s idle ({} msgs during gap)",
                            idle_secs / 60, idle_secs % 60, msg_since_data
                        );
                    }
                    last_data_event = Instant::now();
                    msg_since_data = 0;

                    process_sse_line(&format!("data: {}", data), tx, &mut parse_state);
                }
                Err(reqwest_eventsource::Error::StreamEnded) => {
                    info!(target: "sse", "SSE stream ended");
                    break;
                }
                Err(e) => {
                    warn!(target: "sse", "SSE error: {}", e);
                    es.close();
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
