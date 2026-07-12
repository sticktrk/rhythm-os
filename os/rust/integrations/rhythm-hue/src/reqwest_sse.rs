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
use crate::sse_liveness::HueSseLiveness;

/// Maximum time after a successful Hue light write to wait for any raw SSE
/// traffic before treating the event subscription as stalled.
const SSE_EXPECTED_ACTIVITY_TIMEOUT_SECS: u64 = 120;

/// Poll often enough to observe shutdown without shortening the failure
/// timeout above.
const SSE_SHUTDOWN_POLL_SECS: u64 = 15;

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

#[derive(Debug, Eq, PartialEq)]
enum StreamEnd {
    Shutdown,
    Reconnect,
}

/// Consume one established Hue event stream until shutdown or a real stream
/// failure. The intervals are injectable so the quiet-stream contract can be
/// covered without a two-minute test.
async fn consume_sse_stream<S, B, E>(
    stream: &mut S,
    tx: &SyncSender<HueSseEvent>,
    shutdown: &AtomicBool,
    parse_state: &mut SseParseState,
    sse_liveness: &HueSseLiveness,
    idle_poll: Duration,
    expected_activity_timeout: Duration,
) -> StreamEnd
where
    S: futures::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut chunks_since_alive: u32 = 0;
    let mut line_buf = Vec::with_capacity(4096);
    let now = Instant::now();
    let mut last_byte_event = now;
    let mut last_alive_log = now;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            return StreamEnd::Shutdown;
        }

        match tokio::time::timeout(idle_poll, stream.next()).await {
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
                sse_liveness.observe_sse_activity();
                chunks_since_alive = chunks_since_alive.saturating_add(1);
                line_buf.extend_from_slice(chunk.as_ref());
                drain_sse_lines(&mut line_buf, tx, parse_state);
                if line_buf.len() > MAX_SSE_LINE_BYTES {
                    warn!(
                        target: "sse",
                        "SSE line exceeded {} bytes without newline, reconnecting",
                        MAX_SSE_LINE_BYTES
                    );
                    return StreamEnd::Reconnect;
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
                return StreamEnd::Reconnect;
            }
            Ok(None) => {
                warn!(target: "sse", "SSE stream ended");
                return StreamEnd::Reconnect;
            }
            Err(_) => {
                if let Some(failure) =
                    sse_liveness.expected_activity_failure(expected_activity_timeout)
                {
                    warn!(
                        target: "sse",
                        "SSE: no bytes for {}s after successful Hue light writes ({} pending); reconnecting",
                        failure.oldest_age.as_secs(),
                        failure.pending_count
                    );
                    return StreamEnd::Reconnect;
                }

                // Quiet streams are valid until a successful bridge write
                // gives us a concrete reason to expect event traffic.
            }
        }
    }
}

/// Start an SSE event stream reader in a background thread.
///
/// Returns a receiver for parsed SSE events. The thread runs until
/// `shutdown` is set to `true` or the connection is lost.
pub fn start_reqwest_sse(
    config: HueSseConfig,
    shutdown: Arc<AtomicBool>,
    sse_liveness: Arc<HueSseLiveness>,
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
            rt.block_on(run_sse_loop(&config, &tx, &shutdown, &sse_liveness));
        });
    if let Err(e) = spawn_result {
        warn!(target: "sse", "Failed to spawn SSE thread: {}", e);
    }

    rx
}

/// SSE event loop using raw reqwest response streaming.
async fn run_sse_loop(
    config: &HueSseConfig,
    tx: &SyncSender<HueSseEvent>,
    shutdown: &AtomicBool,
    sse_liveness: &HueSseLiveness,
) {
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

        let mut stream = response.bytes_stream();

        info!(target: "sse", "SSE connected (conn #{})", connect_count);
        sse_liveness.reset_for_connected_stream();
        let _ = tx.try_send(HueSseEvent::Connected);
        backoff = Duration::from_secs(1);

        if consume_sse_stream(
            &mut stream,
            tx,
            shutdown,
            &mut parse_state,
            sse_liveness,
            Duration::from_secs(SSE_SHUTDOWN_POLL_SECS),
            Duration::from_secs(SSE_EXPECTED_ACTIVITY_TIMEOUT_SECS),
        )
        .await
            == StreamEnd::Shutdown
        {
            return;
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

    #[tokio::test]
    async fn quiet_stream_without_expected_activity_stays_connected() {
        let mut stream = futures::stream::pending::<Result<&'static [u8], &'static str>>();
        let (tx, _rx) = sync_channel::<HueSseEvent>(4);
        let shutdown = AtomicBool::new(false);
        let mut parse_state = SseParseState::new();
        let sse_liveness = HueSseLiveness::default();

        let outcome = tokio::time::timeout(
            Duration::from_millis(40),
            consume_sse_stream(
                &mut stream,
                &tx,
                &shutdown,
                &mut parse_state,
                &sse_liveness,
                Duration::from_millis(5),
                Duration::from_millis(20),
            ),
        )
        .await;

        assert!(
            outcome.is_err(),
            "an idle but open SSE response must remain connected"
        );
    }

    #[tokio::test]
    async fn ended_stream_still_requests_reconnect() {
        let mut stream = futures::stream::empty::<Result<&'static [u8], &'static str>>();
        let (tx, _rx) = sync_channel::<HueSseEvent>(4);
        let shutdown = AtomicBool::new(false);
        let mut parse_state = SseParseState::new();
        let sse_liveness = HueSseLiveness::default();

        let outcome = consume_sse_stream(
            &mut stream,
            &tx,
            &shutdown,
            &mut parse_state,
            &sse_liveness,
            Duration::from_millis(5),
            Duration::from_millis(100),
        )
        .await;

        assert_eq!(outcome, StreamEnd::Reconnect);
    }

    #[tokio::test]
    async fn missing_expected_activity_requests_reconnect_after_timeout() {
        let mut stream = futures::stream::pending::<Result<&'static [u8], &'static str>>();
        let (tx, _rx) = sync_channel::<HueSseEvent>(4);
        let shutdown = AtomicBool::new(false);
        let mut parse_state = SseParseState::new();
        let sse_liveness = HueSseLiveness::default();
        sse_liveness.begin_expected_activity();

        let outcome = consume_sse_stream(
            &mut stream,
            &tx,
            &shutdown,
            &mut parse_state,
            &sse_liveness,
            Duration::from_millis(5),
            Duration::from_millis(20),
        )
        .await;

        assert_eq!(outcome, StreamEnd::Reconnect);
    }

    #[tokio::test]
    async fn filtered_light_bytes_satisfy_expected_activity() {
        use futures::StreamExt as _;

        let light_update = Ok::<_, &'static str>(
            b"data: [{\"data\":[{\"type\":\"grouped_light\"}]}]\n".as_slice(),
        );
        let mut stream = futures::stream::iter([light_update]).chain(futures::stream::pending::<
            Result<&'static [u8], &'static str>,
        >());
        let (tx, _rx) = sync_channel::<HueSseEvent>(4);
        let shutdown = AtomicBool::new(false);
        let mut parse_state = SseParseState::new();
        let sse_liveness = HueSseLiveness::default();
        sse_liveness.begin_expected_activity();

        let outcome = tokio::time::timeout(
            Duration::from_millis(40),
            consume_sse_stream(
                &mut stream,
                &tx,
                &shutdown,
                &mut parse_state,
                &sse_liveness,
                Duration::from_millis(5),
                Duration::from_millis(20),
            ),
        )
        .await;

        assert!(outcome.is_err());
        assert_eq!(sse_liveness.pending_count(), 0);
    }
}
