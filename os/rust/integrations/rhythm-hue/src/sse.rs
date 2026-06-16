//! Hue SSE event types and parsing.
//!
//! Platform-agnostic SSE event definitions, JSON serde structs, chunked
//! transfer-encoding decoder, and pure parsing functions. The actual TLS
//! transport and connection management live in the platform crate.

use log::{debug, warn};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::mpsc::SyncSender;
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum age in seconds for button events to be considered actionable.
/// Button presses older than this are stale (user intent is gone).
const BUTTON_STALENESS_SECS: u64 = 5;

/// Maximum age in seconds for motion events. More lenient than buttons
/// because motion state transitions are still useful for a while.
const MOTION_STALENESS_SECS: u64 = 30;

/// Cap on the button-dedup map. The parse state lives for the life of the
/// process (reused across reconnects), so without a bound it grows by one
/// entry per unique button id forever.
const MAX_BUTTON_DEDUP_ENTRIES: usize = 1024;

/// Stateful context for SSE parsing, used to deduplicate button events.
///
/// The Hue bridge batches SSE events: when light state changes, it can
/// re-send button resources with the same `button_report.updated` timestamp.
/// This state tracks the last processed timestamp per button to suppress duplicates.
#[derive(Default)]
pub struct SseParseState {
    last_button_updated: HashMap<String, String>,
}

impl SseParseState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the latest `button_report.updated` timestamp for a button.
    ///
    /// Returns `false` when the timestamp matches the last recorded one for
    /// this button (duplicate event, caller should skip it). To keep the map
    /// bounded, it is cleared once it reaches `MAX_BUTTON_DEDUP_ENTRIES`;
    /// worst case that lets one duplicate event per button slip through
    /// right after the flush, which is acceptable.
    fn record_button_updated(&mut self, id: &str, updated: &str) -> bool {
        if let Some(prev) = self.last_button_updated.get(id) {
            if prev == updated {
                return false;
            }
        }
        if self.last_button_updated.len() >= MAX_BUTTON_DEDUP_ENTRIES {
            self.last_button_updated.clear();
        }
        self.last_button_updated
            .insert(id.to_string(), updated.to_string());
        true
    }
}

/// Configuration for the SSE connection.
pub struct HueSseConfig {
    pub bridge_ip: String,
    pub username: String,
}

/// Events parsed from the Hue SSE stream.
#[derive(Debug, Clone)]
pub enum HueSseEvent {
    /// SSE transport established or re-established.
    Connected,
    /// A button was pressed/released on a Hue device.
    ButtonEvent {
        button_id: String,
        event_type: String,
    },
    /// Motion sensor state changed.
    MotionEvent {
        motion_id: String,
        motion_detected: bool,
    },
    /// SSE heartbeat received (connection is alive).
    Heartbeat,
    /// Connection was lost.
    Disconnected(String),
}

/// SSE event data from the Hue bridge JSON payload.
#[derive(Debug, Deserialize)]
struct SseEventData {
    #[serde(default)]
    creationtime: Option<String>,
    #[serde(default)]
    data: Vec<SseResource>,
}

#[derive(Debug, Deserialize)]
struct SseResource {
    id: Option<String>,
    #[serde(rename = "type")]
    resource_type: Option<String>,
    #[serde(default)]
    button: Option<SseButtonData>,
    #[serde(default)]
    motion: Option<SseMotionData>,
}

#[derive(Debug, Deserialize)]
struct SseButtonReport {
    event: Option<String>,
    updated: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SseButtonData {
    button_report: Option<SseButtonReport>,
}

#[derive(Debug, Deserialize)]
struct SseMotionData {
    motion: Option<bool>,
    motion_valid: Option<bool>,
}

/// Streaming chunked transfer-encoding decoder.
///
/// When `chunked=true`, decodes HTTP chunked encoding:
///   `<hex-size>\r\n<data>\r\n<hex-size>\r\n<data>\r\n...0\r\n\r\n`
///
/// When `chunked=false`, acts as a passthrough (copies input to output).
pub struct ChunkedDecoder {
    chunked: bool,
    state: ChunkState,
    size_buf: Vec<u8>,
}

enum ChunkState {
    /// Parsing hex chunk size, waiting for \n
    Size,
    /// Consuming chunk data, `remaining` bytes left
    Data { remaining: usize },
    /// Skipping \r\n after chunk data
    Trailer { skip: u8 },
}

impl ChunkedDecoder {
    pub fn new(chunked: bool) -> Self {
        Self {
            chunked,
            state: ChunkState::Size,
            size_buf: Vec::with_capacity(16),
        }
    }

    /// Decode input bytes, appending decoded body data to `out`.
    pub fn decode(&mut self, input: &[u8], out: &mut Vec<u8>) {
        if !self.chunked {
            out.extend_from_slice(input);
            return;
        }

        let mut i = 0;
        while i < input.len() {
            match &mut self.state {
                ChunkState::Size => {
                    let b = input[i];
                    i += 1;
                    if b == b'\n' {
                        // Parse hex size (trim \r if present)
                        let s = if self.size_buf.last() == Some(&b'\r') {
                            &self.size_buf[..self.size_buf.len() - 1]
                        } else {
                            &self.size_buf
                        };
                        let hex = String::from_utf8_lossy(s).to_string();
                        let size = usize::from_str_radix(hex.trim(), 16).unwrap_or(0);
                        self.size_buf.clear();
                        if size == 0 {
                            // Terminal chunk — stop decoding
                            return;
                        }
                        self.state = ChunkState::Data { remaining: size };
                    } else {
                        self.size_buf.push(b);
                    }
                }
                ChunkState::Data { remaining } => {
                    let avail = input.len() - i;
                    let take = avail.min(*remaining);
                    out.extend_from_slice(&input[i..i + take]);
                    *remaining -= take;
                    i += take;
                    if *remaining == 0 {
                        self.state = ChunkState::Trailer { skip: 0 };
                    }
                }
                ChunkState::Trailer { skip } => {
                    // Skip \r\n after chunk data
                    i += 1;
                    *skip += 1;
                    if *skip >= 2 {
                        self.state = ChunkState::Size;
                    }
                }
            }
        }
    }
}

/// Drain complete SSE lines from the buffer, passing each to `process_sse_line`.
pub fn drain_sse_lines(
    buf: &mut Vec<u8>,
    event_tx: &SyncSender<HueSseEvent>,
    state: &mut SseParseState,
) {
    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        let line = String::from_utf8_lossy(&buf[..pos]).to_string();
        buf.drain(..=pos);
        process_sse_line(&line, event_tx, state);
    }
}

/// Process a single SSE line.
pub fn process_sse_line(line: &str, event_tx: &SyncSender<HueSseEvent>, state: &mut SseParseState) {
    let line = line.trim();

    if line.is_empty() {
        return;
    }

    // Heartbeat line (": hi\n")
    if line.starts_with(':') {
        let _ = event_tx.try_send(HueSseEvent::Heartbeat);
        return;
    }

    // Data line ("data: [...]")
    if let Some(data) = line.strip_prefix("data: ") {
        // Fast pre-filter: skip JSON parse for non-button/motion events.
        // Light/grouped_light updates can be 1-5KB — no need to parse them.
        if !data.contains("\"button\"") && !data.contains("\"motion\"") {
            return;
        }
        debug!(
            target: "sse",
            "SSE event: {}...({} bytes)",
            &data[..data.len().min(120)],
            data.len()
        );
        parse_sse_data(data, event_tx, state);
    }
}

/// Parse a UTC timestamp like `"2026-03-21T10:57:58Z"` into seconds since UNIX epoch.
/// Returns `None` if the format is unexpected.
fn parse_utc_timestamp(s: &str) -> Option<u64> {
    if s.len() < 20 || !s.ends_with('Z') {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year: u32 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u32 = s[11..13].parse().ok()?;
    let min: u32 = s[14..16].parse().ok()?;
    let sec: u32 = s[17..19].parse().ok()?;

    let is_leap = |y: u32| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let days_in_month: [u32; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    if !(1..=12).contains(&month) || day < 1 || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    let max_day = if month == 2 && is_leap(year) {
        29
    } else {
        days_in_month[month as usize]
    };
    if day > max_day {
        return None;
    }

    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += days_in_month[m as usize] as u64;
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    days += (day - 1) as u64;

    Some(days * 86400 + hour as u64 * 3600 + min as u64 * 60 + sec as u64)
}

/// Current time as seconds since UNIX epoch.
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Parse the JSON data from an SSE data line.
fn parse_sse_data(data: &str, event_tx: &SyncSender<HueSseEvent>, state: &mut SseParseState) {
    let events: Vec<SseEventData> = match serde_json::from_str(data) {
        Ok(events) => events,
        Err(e) => {
            warn!(target: "conn",
                "SSE: Failed to parse JSON: {} (data: {}...)",
                e,
                &data[..data.len().min(100)]
            );
            return;
        }
    };

    let now = now_epoch_secs();

    for event in events {
        // Compute event age from the envelope's creationtime (fail-open if missing)
        let event_age_secs = event
            .creationtime
            .as_deref()
            .and_then(parse_utc_timestamp)
            .map(|ts| now.saturating_sub(ts));

        for resource in event.data {
            let resource_type = resource.resource_type.as_deref().unwrap_or("");
            let id = resource.id.unwrap_or_default();

            match resource_type {
                "button" => {
                    let button_data = match resource.button {
                        Some(bd) => bd,
                        None => continue,
                    };

                    // Require button_report for real button events.
                    // Without it, this is just a stale echo of the button
                    // resource re-sent in a batch with light state changes.
                    let report = match button_data.button_report {
                        Some(r) => r,
                        None => continue,
                    };
                    let event_type = match report.event {
                        Some(e) => e,
                        None => continue,
                    };

                    // Use button_report.updated for staleness (per-button
                    // timestamp, more accurate than envelope creationtime).
                    // Fall back to envelope creationtime if updated is missing.
                    let age = report
                        .updated
                        .as_deref()
                        .and_then(parse_utc_timestamp)
                        .map(|ts| now.saturating_sub(ts))
                        .or(event_age_secs);
                    if let Some(age) = age {
                        if age > BUTTON_STALENESS_SECS {
                            warn!(target: "sse",
                                "SSE: Dropping stale button event (button={}, age={}s, max={}s)",
                                id, age, BUTTON_STALENESS_SECS);
                            continue;
                        }
                    }

                    // Dedup: skip if we already processed this exact button event.
                    // The bridge can re-send button_report with the same updated
                    // timestamp in a subsequent batched SSE message.
                    if let Some(updated) = &report.updated {
                        if !state.record_button_updated(&id, updated) {
                            continue;
                        }
                    }

                    debug!(target: "sse",
                        "SSE: Button event passed (button={}, type={}, age={}s)",
                        id, event_type, age.unwrap_or(0));

                    if let Err(e) = event_tx.try_send(HueSseEvent::ButtonEvent {
                        button_id: id.clone(),
                        event_type: event_type.clone(),
                    }) {
                        warn!(target: "conn", "SSE: Dropped button event (button={}, type={}): {}", id, event_type, e);
                    }
                }
                "motion" => {
                    if let Some(age) = event_age_secs {
                        if age > MOTION_STALENESS_SECS {
                            warn!(target: "sse",
                                "SSE: Dropping stale motion event (sensor={}, age={}s, max={}s)",
                                id, age, MOTION_STALENESS_SECS);
                            continue;
                        }
                    }
                    if let Some(motion_data) = resource.motion {
                        let valid = motion_data.motion_valid.unwrap_or(false);
                        if valid {
                            if let Some(detected) = motion_data.motion {
                                debug!(target: "sse", "SSE: Motion sensor {} -> detected={}", id, detected);
                                if let Err(e) = event_tx.try_send(HueSseEvent::MotionEvent {
                                    motion_id: id.clone(),
                                    motion_detected: detected,
                                }) {
                                    warn!(target: "conn", "SSE: Dropped motion event (id={}, detected={}): {}", id, detected, e);
                                }
                            } else {
                                debug!(target: "sse", "SSE: Motion sensor {} valid but no motion field", id);
                            }
                        } else {
                            debug!(target: "sse", "SSE: Motion sensor {} motion_valid=false (dropped)", id);
                        }
                    } else {
                        debug!(target: "sse", "SSE: Motion resource {} has no motion data (dropped)", id);
                    }
                }
                // grouped_light events are intentionally not parsed.
                // External-off detection is handled by the pre-tick
                // any_lights_on() check in periodic_update instead.
                "grouped_light" => {}
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn chunked_decoder_passthrough() {
        let mut decoder = ChunkedDecoder::new(false);
        let mut out = Vec::new();
        decoder.decode(b"hello world", &mut out);
        assert_eq!(out, b"hello world");
    }

    #[test]
    fn chunked_decoder_single_chunk() {
        let mut decoder = ChunkedDecoder::new(true);
        let mut out = Vec::new();
        decoder.decode(b"5\r\nhello\r\n0\r\n", &mut out);
        assert_eq!(out, b"hello");
    }

    #[test]
    fn chunked_decoder_multiple_chunks() {
        let mut decoder = ChunkedDecoder::new(true);
        let mut out = Vec::new();
        decoder.decode(b"5\r\nhello\r\n6\r\n world\r\n0\r\n", &mut out);
        assert_eq!(out, b"hello world");
    }

    #[test]
    fn chunked_decoder_terminal_zero_chunk() {
        let mut decoder = ChunkedDecoder::new(true);
        let mut out = Vec::new();
        decoder.decode(b"3\r\nabc\r\n0\r\ntrailing garbage", &mut out);
        // Should stop at zero-size chunk, ignoring trailing data
        assert_eq!(out, b"abc");
    }

    #[test]
    fn sse_empty_line_ignored() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        process_sse_line("", &tx, &mut SseParseState::new());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sse_heartbeat() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        process_sse_line(": hi", &tx, &mut SseParseState::new());
        match rx.try_recv() {
            Ok(HueSseEvent::Heartbeat) => {}
            other => panic!("Expected Heartbeat, got {:?}", other),
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sse_button_event() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let fresh = format_utc(now_epoch_secs().saturating_sub(1));
        let line = format!(
            r#"data: [{{"data":[{{"id":"btn1","type":"button","button":{{"button_report":{{"event":"initial_press","updated":"{}"}},"last_event":"initial_press"}}}}]}}]"#,
            fresh
        );
        process_sse_line(&line, &tx, &mut SseParseState::new());
        match rx.try_recv() {
            Ok(HueSseEvent::ButtonEvent {
                button_id,
                event_type,
            }) => {
                assert_eq!(button_id, "btn1");
                assert_eq!(event_type, "initial_press");
            }
            other => panic!("Expected ButtonEvent, got {:?}", other),
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sse_button_last_event_only_dropped() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        // Only last_event, no button_report — stale echo from batched update
        let line = r#"data: [{"data":[{"id":"btn1","type":"button","button":{"last_event":"initial_press"}}]}]"#;
        process_sse_line(line, &tx, &mut SseParseState::new());
        assert!(
            rx.try_recv().is_err(),
            "Button with only last_event (no button_report) should be dropped"
        );
    }

    #[test]
    fn sse_button_dedup_same_updated() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let mut state = SseParseState::new();
        let fresh = format_utc(now_epoch_secs().saturating_sub(1));
        let line = format!(
            r#"data: [{{"data":[{{"id":"btn1","type":"button","button":{{"button_report":{{"event":"initial_press","updated":"{}"}},"last_event":"initial_press"}}}}]}}]"#,
            fresh
        );
        // First event passes
        process_sse_line(&line, &tx, &mut state);
        assert!(rx.try_recv().is_ok(), "First button event should pass");
        // Same event again (same updated timestamp) — deduped
        process_sse_line(&line, &tx, &mut state);
        assert!(
            rx.try_recv().is_err(),
            "Duplicate button_report.updated should be deduped"
        );
    }

    #[test]
    fn sse_button_different_updated_passes() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let mut state = SseParseState::new();
        let now = now_epoch_secs();
        let time1 = format_utc(now.saturating_sub(2));
        let time2 = format_utc(now.saturating_sub(1));
        let line1 = format!(
            r#"data: [{{"data":[{{"id":"btn1","type":"button","button":{{"button_report":{{"event":"initial_press","updated":"{}"}},"last_event":"initial_press"}}}}]}}]"#,
            time1
        );
        let line2 = format!(
            r#"data: [{{"data":[{{"id":"btn1","type":"button","button":{{"button_report":{{"event":"initial_press","updated":"{}"}},"last_event":"initial_press"}}}}]}}]"#,
            time2
        );
        process_sse_line(&line1, &tx, &mut state);
        assert!(rx.try_recv().is_ok(), "First button event should pass");
        process_sse_line(&line2, &tx, &mut state);
        assert!(
            rx.try_recv().is_ok(),
            "Different updated timestamp should pass"
        );
    }

    #[test]
    fn sse_motion_event() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let line = r#"data: [{"data":[{"id":"mot1","type":"motion","motion":{"motion":true,"motion_valid":true}}]}]"#;
        process_sse_line(line, &tx, &mut SseParseState::new());
        match rx.try_recv() {
            Ok(HueSseEvent::MotionEvent {
                motion_id,
                motion_detected,
            }) => {
                assert_eq!(motion_id, "mot1");
                assert!(motion_detected);
            }
            other => panic!("Expected MotionEvent, got {:?}", other),
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sse_motion_invalid_not_valid() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let line = r#"data: [{"data":[{"id":"mot1","type":"motion","motion":{"motion":true,"motion_valid":false}}]}]"#;
        process_sse_line(line, &tx, &mut SseParseState::new());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sse_non_button_motion_filtered() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let line = r#"data: [{"data":[{"id":"x","type":"light"}]}]"#;
        process_sse_line(line, &tx, &mut SseParseState::new());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sse_invalid_json_resilient() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let line = r#"data: {broken json with "button" in it"#;
        process_sse_line(line, &tx, &mut SseParseState::new());
        // Should not panic and should not send any event
        assert!(rx.try_recv().is_err());
    }

    // ========================================================================
    // parse_utc_timestamp tests
    // ========================================================================

    #[test]
    fn parse_utc_timestamp_epoch() {
        assert_eq!(parse_utc_timestamp("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn parse_utc_timestamp_known_value() {
        // 2024-01-01T00:00:00Z = 19723 days * 86400 = 1704067200
        // (2024 is a leap year, 54 leap years from 1970..2024)
        let ts = parse_utc_timestamp("2024-01-01T00:00:00Z").unwrap();
        assert_eq!(ts, 1704067200);
    }

    #[test]
    fn parse_utc_timestamp_with_time() {
        let base = parse_utc_timestamp("2024-01-01T00:00:00Z").unwrap();
        let with_time = parse_utc_timestamp("2024-01-01T12:30:45Z").unwrap();
        assert_eq!(with_time - base, 12 * 3600 + 30 * 60 + 45);
    }

    #[test]
    fn parse_utc_timestamp_leap_year() {
        let feb28 = parse_utc_timestamp("2024-02-28T00:00:00Z").unwrap();
        let feb29 = parse_utc_timestamp("2024-02-29T00:00:00Z").unwrap();
        let mar01 = parse_utc_timestamp("2024-03-01T00:00:00Z").unwrap();
        assert_eq!(feb29 - feb28, 86400);
        assert_eq!(mar01 - feb29, 86400);
    }

    #[test]
    fn parse_utc_timestamp_non_leap_year() {
        let feb28 = parse_utc_timestamp("2023-02-28T00:00:00Z").unwrap();
        let mar01 = parse_utc_timestamp("2023-03-01T00:00:00Z").unwrap();
        assert_eq!(mar01 - feb28, 86400); // no feb 29
        assert_eq!(parse_utc_timestamp("2023-02-29T00:00:00Z"), None);
    }

    #[test]
    fn parse_utc_timestamp_invalid() {
        assert_eq!(parse_utc_timestamp("not a timestamp"), None);
        assert_eq!(parse_utc_timestamp(""), None);
        assert_eq!(parse_utc_timestamp("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_utc_timestamp("2026-00-01T00:00:00Z"), None);
        assert_eq!(parse_utc_timestamp("2026-01-32T00:00:00Z"), None);
        assert_eq!(parse_utc_timestamp("2026-01-01T25:00:00Z"), None);
    }

    // ========================================================================
    // Staleness filter tests
    // ========================================================================

    /// Format epoch seconds as `YYYY-MM-DDTHH:MM:SSZ`.
    fn format_utc(epoch_secs: u64) -> String {
        let is_leap =
            |y: u32| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
        let days_in_month: [u32; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

        let mut remaining = epoch_secs;
        let secs = (remaining % 60) as u32;
        remaining /= 60;
        let mins = (remaining % 60) as u32;
        remaining /= 60;
        let hours = (remaining % 24) as u32;
        let mut days = (remaining / 24) as u32;

        let mut year = 1970u32;
        loop {
            let days_in_year = if is_leap(year) { 366 } else { 365 };
            if days < days_in_year {
                break;
            }
            days -= days_in_year;
            year += 1;
        }
        let mut month = 1u32;
        loop {
            let dim = if month == 2 && is_leap(year) {
                29
            } else {
                days_in_month[month as usize]
            };
            if days < dim {
                break;
            }
            days -= dim;
            month += 1;
        }
        let day = days + 1;

        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            year, month, day, hours, mins, secs
        )
    }

    #[test]
    fn format_utc_roundtrip() {
        let epoch = 1704067200u64; // 2024-01-01T00:00:00Z
        let formatted = format_utc(epoch);
        assert_eq!(formatted, "2024-01-01T00:00:00Z");
        assert_eq!(parse_utc_timestamp(&formatted), Some(epoch));
    }

    #[test]
    fn stale_button_event_dropped() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let old_time = format_utc(now_epoch_secs().saturating_sub(60));
        let line = format!(
            r#"data: [{{"creationtime":"{}","data":[{{"id":"btn1","type":"button","button":{{"button_report":{{"event":"initial_press","updated":"{}"}},"last_event":"initial_press"}}}}]}}]"#,
            old_time, old_time
        );
        process_sse_line(&line, &tx, &mut SseParseState::new());
        assert!(
            rx.try_recv().is_err(),
            "Stale button event should be dropped"
        );
    }

    #[test]
    fn fresh_button_event_passes() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let fresh_time = format_utc(now_epoch_secs().saturating_sub(1));
        let line = format!(
            r#"data: [{{"creationtime":"{}","data":[{{"id":"btn1","type":"button","button":{{"button_report":{{"event":"initial_press","updated":"{}"}},"last_event":"initial_press"}}}}]}}]"#,
            fresh_time, fresh_time
        );
        process_sse_line(&line, &tx, &mut SseParseState::new());
        match rx.try_recv() {
            Ok(HueSseEvent::ButtonEvent { button_id, .. }) => assert_eq!(button_id, "btn1"),
            other => panic!("Expected fresh ButtonEvent, got {:?}", other),
        }
    }

    #[test]
    fn stale_button_report_in_fresh_envelope_dropped() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let fresh_envelope = format_utc(now_epoch_secs().saturating_sub(1));
        let old_button = format_utc(now_epoch_secs().saturating_sub(60));
        // Envelope is fresh but button_report.updated is old — should be dropped
        let line = format!(
            r#"data: [{{"creationtime":"{}","data":[{{"id":"btn1","type":"button","button":{{"button_report":{{"event":"initial_press","updated":"{}"}},"last_event":"initial_press"}}}}]}}]"#,
            fresh_envelope, old_button
        );
        process_sse_line(&line, &tx, &mut SseParseState::new());
        assert!(
            rx.try_recv().is_err(),
            "Stale button_report.updated should be dropped even with fresh envelope"
        );
    }

    #[test]
    fn stale_motion_event_dropped() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let old_time = format_utc(now_epoch_secs().saturating_sub(120));
        let line = format!(
            r#"data: [{{"creationtime":"{}","data":[{{"id":"mot1","type":"motion","motion":{{"motion":true,"motion_valid":true}}}}]}}]"#,
            old_time
        );
        process_sse_line(&line, &tx, &mut SseParseState::new());
        assert!(
            rx.try_recv().is_err(),
            "Stale motion event should be dropped"
        );
    }

    #[test]
    fn recent_motion_event_passes() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let recent_time = format_utc(now_epoch_secs().saturating_sub(10));
        let line = format!(
            r#"data: [{{"creationtime":"{}","data":[{{"id":"mot1","type":"motion","motion":{{"motion":true,"motion_valid":true}}}}]}}]"#,
            recent_time
        );
        process_sse_line(&line, &tx, &mut SseParseState::new());
        match rx.try_recv() {
            Ok(HueSseEvent::MotionEvent { motion_id, .. }) => assert_eq!(motion_id, "mot1"),
            other => panic!("Expected fresh MotionEvent, got {:?}", other),
        }
    }

    #[test]
    fn missing_creationtime_with_button_report_passes() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        // No creationtime, no button_report.updated — should pass through (fail-open)
        let line = r#"data: [{"data":[{"id":"btn1","type":"button","button":{"button_report":{"event":"initial_press"},"last_event":"initial_press"}}]}]"#;
        process_sse_line(line, &tx, &mut SseParseState::new());
        match rx.try_recv() {
            Ok(HueSseEvent::ButtonEvent { button_id, .. }) => assert_eq!(button_id, "btn1"),
            other => panic!("Expected ButtonEvent (no timestamps), got {:?}", other),
        }
    }

    // ========================================================================
    // Reconnect / stream-interruption tests
    // ========================================================================

    #[test]
    fn drain_preserves_partial_line_at_end_of_buffer() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let mut state = SseParseState::new();
        let mut buf = Vec::new();

        // A heartbeat line followed by a partial `data:` prefix.
        buf.extend_from_slice(b": hi\ndata: [{\"da");
        drain_sse_lines(&mut buf, &tx, &mut state);

        assert!(matches!(rx.try_recv(), Ok(HueSseEvent::Heartbeat)));
        assert!(
            rx.try_recv().is_err(),
            "partial line must not be dispatched"
        );
        assert_eq!(
            std::str::from_utf8(&buf).unwrap(),
            "data: [{\"da",
            "partial line must be retained in buf for the next decode()"
        );
    }

    #[test]
    fn drain_assembles_line_across_two_chunks() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let mut state = SseParseState::new();
        let mut buf = Vec::new();

        // First chunk: partial heartbeat line.
        buf.extend_from_slice(b": hi");
        drain_sse_lines(&mut buf, &tx, &mut state);
        assert!(rx.try_recv().is_err());

        // Second chunk completes the line.
        buf.extend_from_slice(b"\n");
        drain_sse_lines(&mut buf, &tx, &mut state);
        assert!(matches!(rx.try_recv(), Ok(HueSseEvent::Heartbeat)));
    }

    #[test]
    fn reset_parse_state_simulating_reconnect_allows_same_button_through() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let fresh = format_utc(now_epoch_secs().saturating_sub(1));
        let line = format!(
            r#"data: [{{"data":[{{"id":"btn1","type":"button","button":{{"button_report":{{"event":"initial_press","updated":"{}"}},"last_event":"initial_press"}}}}]}}]"#,
            fresh
        );

        // First connection: event passes, then deduped.
        let mut state = SseParseState::new();
        process_sse_line(&line, &tx, &mut state);
        assert!(rx.try_recv().is_ok());
        process_sse_line(&line, &tx, &mut state);
        assert!(
            rx.try_recv().is_err(),
            "duplicate within a session is deduped"
        );

        // Simulate SSE reconnect: the production loop creates a fresh
        // `SseParseState` per run_sse_loop invocation (reset on reconnect is
        // baked in). A replay of the same button after reconnect must NOT be
        // silently swallowed, because it's a new user interaction.
        let mut fresh_state = SseParseState::new();
        process_sse_line(&line, &tx, &mut fresh_state);
        assert!(
            rx.try_recv().is_ok(),
            "after reconnect (new SseParseState), the same event must dispatch"
        );
    }

    // ========================================================================
    // Button-dedup map bound tests
    // ========================================================================

    #[test]
    fn record_button_updated_under_cap_retains_entries_and_dedups() {
        let mut state = SseParseState::new();

        for i in 0..100 {
            assert!(
                state.record_button_updated(&format!("btn{}", i), "t1"),
                "first sighting of a button must pass"
            );
        }
        assert_eq!(state.last_button_updated.len(), 100);

        // Same (id, updated) pair is a duplicate; entries are retained.
        assert!(!state.record_button_updated("btn0", "t1"));
        assert!(!state.record_button_updated("btn99", "t1"));
        assert_eq!(state.last_button_updated.len(), 100);

        // A new timestamp for a known button passes and updates in place.
        assert!(state.record_button_updated("btn0", "t2"));
        assert!(!state.record_button_updated("btn0", "t2"));
        assert_eq!(state.last_button_updated.len(), 100);
    }

    #[test]
    fn record_button_updated_exceeding_cap_resets_map_and_keeps_deduping() {
        let mut state = SseParseState::new();

        for i in 0..MAX_BUTTON_DEDUP_ENTRIES {
            state.record_button_updated(&format!("btn{}", i), "t1");
        }
        assert_eq!(state.last_button_updated.len(), MAX_BUTTON_DEDUP_ENTRIES);

        // The insert that would exceed the cap flushes the map first.
        assert!(state.record_button_updated("overflow", "t1"));
        assert_eq!(state.last_button_updated.len(), 1);

        // Dedup still functions after the flush.
        assert!(!state.record_button_updated("overflow", "t1"));
        assert!(state.record_button_updated("btn0", "t1"));
        assert!(!state.record_button_updated("btn0", "t1"));
        assert!(state.last_button_updated.len() <= MAX_BUTTON_DEDUP_ENTRIES);
    }

    #[test]
    fn mid_stream_drop_does_not_leave_partial_event_partially_dispatched() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let mut state = SseParseState::new();
        let mut buf = Vec::new();

        // Simulate a drop in the middle of a data line (no trailing '\n').
        buf.extend_from_slice(
            br#"data: [{"data":[{"id":"btn1","type":"button","button":{"button_report":{"event":"initial_press","updated":"2099-01-01T00:00:00Z"}}}]}]"#,
        );
        drain_sse_lines(&mut buf, &tx, &mut state);

        // No terminator, so the line must not have been dispatched.
        assert!(
            rx.try_recv().is_err(),
            "incomplete data line must not produce an event before its terminator arrives"
        );
        assert!(
            !buf.is_empty(),
            "buffer must retain the partial line for retry after reconnect"
        );
    }

    #[test]
    fn multiple_complete_lines_in_one_chunk_dispatch_in_order() {
        let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
        let mut state = SseParseState::new();
        let mut buf = Vec::new();

        buf.extend_from_slice(b": hi\n: hi\n: hi\n");
        drain_sse_lines(&mut buf, &tx, &mut state);

        let mut count = 0;
        while let Ok(HueSseEvent::Heartbeat) = rx.try_recv() {
            count += 1;
        }
        assert_eq!(count, 3, "all three heartbeat lines must dispatch");
        assert!(buf.is_empty(), "fully-consumed buffer must be empty");
    }
}
