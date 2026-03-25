//! Hue SSE event stream for real-time button events (ESP32 transport).
//!
//! Maintains a long-lived HTTPS streaming connection to the Hue bridge's
//! `/eventstream/clip/v2` endpoint. Uses a raw TLS socket (`EspTls`) instead
//! of `esp_http_client` to avoid HTTP-layer buffering that causes 30-60s
//! delays on SSE events.
//!
//! SSE event types, JSON parsing, and chunked decoding are provided by
//! `rhythm_hue::sse`. This module provides the ESP32-specific TLS transport.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use esp_idf_svc::sys;
use esp_idf_svc::tls::{self, EspTls};
use log::{info, warn};

pub use rhythm_hue::sse::{ChunkedDecoder, HueSseConfig, HueSseEvent};
use rhythm_hue::sse::{SseParseState, drain_sse_lines};

/// Run the SSE event stream in a blocking loop with reconnection.
///
/// This function returns when the shutdown flag is set, or runs indefinitely
/// otherwise. It connects to the Hue bridge's SSE endpoint and reads events
/// in a loop. On disconnection, it reconnects with exponential backoff.
pub fn run_hue_sse(config: HueSseConfig, event_tx: SyncSender<HueSseEvent>, shutdown: &Arc<AtomicBool>) {
    let mut backoff_ms: u64 = 1000;
    let mut is_reconnect = false;
    const MAX_BACKOFF_MS: u64 = 60_000;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            info!(target: "conn", "SSE: Shutdown signaled, exiting reader thread");
            return;
        }

        info!(target: "conn", "SSE: Connecting to bridge {}...", config.bridge_ip);
        crate::diag::vitals_hub_conn_state(crate::hub::HubType::new(crate::hub::HubType::HUE), crate::diag::CONN_CONNECTING);

        match connect_and_stream(&config, &event_tx, &mut backoff_ms) {
            Ok(()) => {
                info!(target: "conn", "SSE: Stream ended cleanly, reconnecting...");
            }
            Err(e) => {
                warn!(target: "conn", "SSE: Connection error: {}", e);
                let _ = event_tx.try_send(HueSseEvent::Disconnected(e.to_string()));
            }
        }

        if shutdown.load(Ordering::Relaxed) {
            info!(target: "conn", "SSE: Shutdown signaled after disconnect, exiting reader thread");
            return;
        }

        if is_reconnect {
            crate::diag::vitals_hub_reconnect(crate::hub::HubType::new(crate::hub::HubType::HUE));
        }
        is_reconnect = true;

        info!(target: "conn", "SSE: Reconnecting in {}ms...", backoff_ms);
        thread::sleep(Duration::from_millis(backoff_ms));
        backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
    }
}

/// Max consecutive 5s read timeouts before assuming the connection is dead.
/// 18 timeouts × 5s SO_RCVTIMEO = ~90 seconds of silence.
/// The Hue bridge sends heartbeats every ~30-60s, so 90s is already abnormal.
const MAX_CONSECUTIVE_TIMEOUTS: u32 = 18;

/// Connect to the SSE endpoint and stream events using a raw TLS socket.
///
/// Bypasses `esp_http_client` entirely — that library explicitly doesn't
/// support SSE and buffers data in its chunked-encoding parser. `EspTls`
/// calls `mbedtls_ssl_read()` directly, returning bytes as soon as a TLS
/// record is decrypted.
fn connect_and_stream(
    config: &HueSseConfig,
    event_tx: &SyncSender<HueSseEvent>,
    backoff_ms: &mut u64,
) -> Result<(), anyhow::Error> {
    // Create TLS connection with 5s socket timeout (SO_RCVTIMEO).
    // This bounds worst-case read() blocking — events normally return
    // in milliseconds since there's no HTTP-layer buffering.
    let tls_config = tls::Config {
        timeout_ms: 5_000,
        use_global_ca_store: false,
        #[cfg(esp_idf_mbedtls_certificate_bundle)]
        use_crt_bundle_attach: false,
        ..Default::default()
    };
    let mut tls = EspTls::new()?;
    tls.connect(&config.bridge_ip, 443, &tls_config)?;

    // Send HTTP GET request manually
    let request = format!(
        "GET /eventstream/clip/v2 HTTP/1.1\r\n\
         Host: {}\r\n\
         hue-application-key: {}\r\n\
         Accept: text/event-stream\r\n\
         \r\n",
        config.bridge_ip, config.username
    );
    tls.write_all(request.as_bytes())?;

    // Read response headers, detect chunked encoding
    let (chunked, leftover) = read_response_headers(&mut tls)?;

    info!(target: "conn", "SSE: Connected to event stream (chunked={})", chunked);
    crate::diag::vitals_hub_conn_state(crate::hub::HubType::new(crate::hub::HubType::HUE), crate::diag::CONN_CONNECTED);
    *backoff_ms = 1000;

    let mut decoder = ChunkedDecoder::new(chunked);
    let mut sse_buf = Vec::with_capacity(4096);
    let mut read_buf = [0u8; 1024];
    let mut consecutive_timeouts: u32 = 0;
    let mut parse_state = SseParseState::new();

    // Feed any leftover bytes from header read
    if !leftover.is_empty() {
        decoder.decode(&leftover, &mut sse_buf);
        drain_sse_lines(&mut sse_buf, event_tx, &mut parse_state);
    }

    loop {
        if consecutive_timeouts >= MAX_CONSECUTIVE_TIMEOUTS {
            warn!(target: "conn", "SSE: {} consecutive timeouts (~{}s with no data), assuming dead",
                consecutive_timeouts, consecutive_timeouts * 5);
            return Err(anyhow::anyhow!(
                "SSE: {} consecutive timeouts, assuming connection dead",
                consecutive_timeouts
            ));
        }

        match tls.read(&mut read_buf) {
            Ok(0) => return Ok(()), // EOF
            Ok(n) => {
                consecutive_timeouts = 0;
                decoder.decode(&read_buf[..n], &mut sse_buf);

                if sse_buf.len() > 32_768 {
                    warn!(target: "conn", "SSE: Buffer exceeded 32KB ({} bytes), clearing", sse_buf.len());
                    sse_buf.clear();
                    continue;
                }

                drain_sse_lines(&mut sse_buf, event_tx, &mut parse_state);
            }
            Err(e) => {
                let code = e.code();
                if code == sys::ESP_TLS_ERR_SSL_WANT_READ
                    || code == sys::ESP_TLS_ERR_SSL_WANT_WRITE
                {
                    // Socket timeout (5s SO_RCVTIMEO) — normal idle, not an error
                    consecutive_timeouts += 1;
                    continue;
                }
                // Real connection error (reset, broken pipe, etc.)
                return Err(anyhow::anyhow!("SSE read error (code {}): {}", code, e));
            }
        }
    }
}

/// Read HTTP response headers from the TLS socket.
///
/// Returns `(is_chunked, leftover_body_bytes)`. The leftover contains any
/// body bytes that were read past the `\r\n\r\n` header terminator.
fn read_response_headers(tls: &mut EspTls<esp_idf_svc::tls::InternalSocket>) -> Result<(bool, Vec<u8>), anyhow::Error> {
    let mut header_buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 256];

    loop {
        let n = tls.read(&mut tmp).map_err(|e| anyhow::anyhow!("Header read error: {}", e))?;
        if n == 0 {
            return Err(anyhow::anyhow!("Connection closed during header read"));
        }
        header_buf.extend_from_slice(&tmp[..n]);

        if let Some(end) = find_header_end(&header_buf) {
            let header_str = String::from_utf8_lossy(&header_buf[..end]).to_string();

            // Verify HTTP 200
            if let Some(status_line) = header_str.lines().next() {
                if !status_line.contains("200") {
                    return Err(anyhow::anyhow!("SSE: {}", status_line));
                }
            }

            let chunked = header_str
                .to_ascii_lowercase()
                .contains("transfer-encoding: chunked");

            // Everything after \r\n\r\n is body data
            let leftover = header_buf[end + 4..].to_vec();
            return Ok((chunked, leftover));
        }

        if header_buf.len() > 8192 {
            return Err(anyhow::anyhow!("Response headers exceeded 8KB"));
        }
    }
}

/// Find the `\r\n\r\n` boundary in a byte buffer. Returns the index of the
/// first `\r` in the sequence.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
