//! SSE reader for Hue bridge event streams using raw `reqwest` streaming.
//!
//! We stream raw response bytes and feed them through the shared SSE line parser
//! so Hue heartbeat comments
//! (`: hi`) count as real activity. `reqwest-eventsource` only surfaces
//! parsed message events, which made quiet-but-healthy streams look stalled.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use log::{debug, info, warn};

use crate::sse::{drain_sse_lines, HueSseConfig, HueSseEvent, SseParseState};

/// Maximum seconds without any SSE bytes before assuming the connection is
/// stalled and reconnecting. Motion controls are latency-sensitive, and Hue
/// bridges normally send heartbeat comment frames every ~10s, so 15s catches a
/// dead stream after roughly one missed heartbeat instead of waiting through
/// several missed room-entry events.
const SSE_IDLE_TIMEOUT_SECS: u64 = 15;

/// Log an "SSE alive" message at this interval during idle periods.
const ALIVE_LOG_INTERVAL_SECS: u64 = 300;

/// Bound on the connect + TLS handshake + response-header phase of a stream
/// request. `connect_timeout` only covers the TCP connect; without this, a
/// bridge that accepts the connection but never completes the handshake wedges
/// the reconnect loop forever. Deliberately NOT a whole-request `.timeout()`,
/// which would kill the infinite SSE body.
const SSE_CONNECT_TIMEOUT_SECS: u64 = 15;

/// Cap on the buffered partial SSE line. Real Hue payloads are a few KB; a
/// misbehaving bridge streaming bytes with no `\n` would otherwise grow the
/// buffer without bound (slow OOM on appliance hardware).
const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;

fn open_fd_count() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/proc/self/fd")
            .ok()
            .map(|fds| fds.count())
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn fd_log_suffix() -> String {
    open_fd_count()
        .map(|count| format!(" open_fds={}", count))
        .unwrap_or_default()
}

fn build_sse_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .tcp_keepalive(Duration::from_secs(30))
        // The Hue event stream is a single long-lived response. Avoid keeping
        // abandoned reconnect sockets in reqwest's idle pool.
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(5))
        .build()
}

/// Start an SSE event stream reader in a background thread.
///
/// Returns a receiver for parsed SSE events. The thread runs until
/// `shutdown` is set to `true` or the connection is lost.
pub fn start_reqwest_sse(
    config: HueSseConfig,
    shutdown: Arc<AtomicBool>,
) -> std::sync::mpsc::Receiver<HueSseEvent> {
    let (tx, rx) = sync_channel::<HueSseEvent>(64);

    // Spawn/build failures drop `tx`, which the translator observes as a
    // disconnect — never panic here: this runs on hub-configure paths.
    let spawn_result = std::thread::Builder::new()
        .name("hue-sse".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    warn!(target: "sse", "Failed to build SSE tokio runtime: {}", e);
                    let _ = tx.try_send(HueSseEvent::Disconnected(format!(
                        "Failed to build SSE runtime: {}",
                        e
                    )));
                    return;
                }
            };
            rt.block_on(run_sse_loop(&config, &tx, &shutdown));
        });
    if let Err(e) = spawn_result {
        warn!(target: "sse", "Failed to spawn SSE thread: {}", e);
    }

    rx
}

/// SSE event loop using raw reqwest response streaming.
async fn run_sse_loop(config: &HueSseConfig, tx: &SyncSender<HueSseEvent>, shutdown: &AtomicBool) {
    let url = format!("https://{}/eventstream/clip/v2", config.bridge_ip);

    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);
    let mut parse_state = SseParseState::new();
    let mut connect_count: u32 = 0;

    while !shutdown.load(Ordering::Relaxed) {
        connect_count += 1;
        info!(target: "sse", "Connecting SSE to {} (conn #{})...", url, connect_count);

        // Client build fails under fd exhaustion or TLS-init hiccups — both
        // transient. Retry with backoff; returning here would permanently kill
        // the SSE loop (no Hue events until process restart).
        let client = match build_sse_client() {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    target: "sse",
                    "Failed to build SSE client: {}{}",
                    e,
                    fd_log_suffix()
                );
                let _ = tx.try_send(HueSseEvent::Disconnected(format!(
                    "Client build error: {}",
                    e
                )));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };

        let request = client
            .get(&url)
            .header("hue-application-key", &config.username)
            .header("Accept", "text/event-stream");

        let response = match tokio::time::timeout(
            Duration::from_secs(SSE_CONNECT_TIMEOUT_SECS),
            request.send(),
        )
        .await
        {
            Err(_) => {
                warn!(
                    target: "sse",
                    "SSE connect timed out after {}s{}",
                    SSE_CONNECT_TIMEOUT_SECS,
                    fd_log_suffix()
                );
                debug!(target: "sse", "Reconnecting SSE in {:?}...", backoff);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
            Ok(Ok(response)) => match response.error_for_status() {
                Ok(response) => response,
                Err(e) => {
                    warn!(
                        target: "sse",
                        "SSE connect failed: {}{}",
                        e,
                        fd_log_suffix()
                    );
                    debug!(target: "sse", "Reconnecting SSE in {:?}...", backoff);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }
            },
            Ok(Err(e)) => {
                warn!(
                    target: "sse",
                    "SSE request failed: {}{}",
                    e,
                    fd_log_suffix()
                );
                debug!(target: "sse", "Reconnecting SSE in {:?}...", backoff);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };

        let mut chunks_since_alive: u32 = 0;
        let mut line_buf = Vec::with_capacity(4096);
        let mut stream = response.bytes_stream();

        info!(target: "sse", "SSE connected (conn #{})", connect_count);
        let _ = tx.try_send(HueSseEvent::Connected);
        backoff = Duration::from_secs(1);
        let now = Instant::now();
        let mut last_byte_event = now;
        let mut last_alive_log = now;

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
                        debug!(
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
                    if line_buf.len() > MAX_SSE_LINE_BYTES {
                        warn!(
                            target: "sse",
                            "SSE line exceeded {} bytes without newline, reconnecting",
                            MAX_SSE_LINE_BYTES
                        );
                        break;
                    }

                    if last_alive_log.elapsed().as_secs() >= ALIVE_LOG_INTERVAL_SECS {
                        debug!(
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
                    warn!(target: "sse", "SSE error: {}{}", e, fd_log_suffix());
                    break;
                }
                Ok(None) => {
                    warn!(target: "sse", "SSE stream ended");
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
            let _ = tx.try_send(HueSseEvent::Disconnected("Connection lost".to_string()));
            info!(target: "sse", "Reconnecting SSE in {:?}...", backoff);
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fd_log_suffix_is_empty_or_prefixed_with_open_fd_count() {
        let suffix = fd_log_suffix();

        assert!(suffix.is_empty() || suffix.starts_with(" open_fds="));
    }

    #[test]
    fn build_sse_client_succeeds_without_opening_a_connection() {
        let client = build_sse_client().unwrap();

        let request = client
            .get("https://192.0.2.10/eventstream/clip/v2")
            .header("hue-application-key", "user-123")
            .header("Accept", "text/event-stream")
            .build()
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://192.0.2.10/eventstream/clip/v2"
        );
        assert_eq!(
            request
                .headers()
                .get("Accept")
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
    }
}
