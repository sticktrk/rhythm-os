//! Integration-level SSE reconnect scenarios against the public `sse` API.
//!
//! These exercise the line-level parser + `SseParseState` the way the real
//! `reqwest_sse` loop does, but without touching reqwest/TLS. Each scenario
//! simulates a specific reconnect-adjacent failure mode we want the rpiz
//! appliance to survive.

use std::sync::mpsc;

use rhythm_hue::sse::{drain_sse_lines, HueSseEvent, SseParseState};

fn format_utc(epoch_secs: u64) -> String {
    let is_leap = |y: u32| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
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
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
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

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn button_line(id: &str, updated_secs_ago: u64) -> String {
    let t = format_utc(now().saturating_sub(updated_secs_ago));
    format!(
        r#"data: [{{"data":[{{"id":"{id}","type":"button","button":{{"button_report":{{"event":"initial_press","updated":"{t}"}},"last_event":"initial_press"}}}}]}}]
"#
    )
}

#[test]
fn reconnect_passes_fresh_state_allowing_repeat_event_after_drop() {
    let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);

    // -- Connection 1 --
    let mut state = SseParseState::new();
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(button_line("btn-kitchen", 1).as_bytes());
    drain_sse_lines(&mut buf, &tx, &mut state);
    assert!(
        matches!(rx.try_recv(), Ok(HueSseEvent::ButtonEvent { button_id, .. }) if button_id == "btn-kitchen")
    );

    // Same event re-sent in the same connection is deduplicated by updated timestamp.
    buf.extend_from_slice(button_line("btn-kitchen", 1).as_bytes());
    drain_sse_lines(&mut buf, &tx, &mut state);
    assert!(
        rx.try_recv().is_err(),
        "duplicate event within one connection must be deduped"
    );

    // -- Connection drops, reconnect -- production wipes SseParseState.
    let mut state2 = SseParseState::new();

    // User presses the same button again; it's a new user intent.
    buf.extend_from_slice(button_line("btn-kitchen", 0).as_bytes());
    drain_sse_lines(&mut buf, &tx, &mut state2);
    assert!(
        matches!(rx.try_recv(), Ok(HueSseEvent::ButtonEvent { button_id, .. }) if button_id == "btn-kitchen"),
        "post-reconnect event with fresh state must dispatch (user intent preserved)"
    );
}

#[test]
fn partial_line_at_end_of_chunk_waits_for_next_chunk_before_dispatching() {
    let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
    let mut state = SseParseState::new();
    let mut buf: Vec<u8> = Vec::new();

    let line = button_line("btn1", 1);
    let (left, right) = line.split_at(line.len() / 2);

    buf.extend_from_slice(left.as_bytes());
    drain_sse_lines(&mut buf, &tx, &mut state);
    assert!(
        rx.try_recv().is_err(),
        "half-arrived line must not dispatch"
    );

    buf.extend_from_slice(right.as_bytes());
    drain_sse_lines(&mut buf, &tx, &mut state);
    assert!(
        matches!(rx.try_recv(), Ok(HueSseEvent::ButtonEvent { .. })),
        "once the line completes, event must dispatch"
    );
}

#[test]
fn mid_drop_retains_buffer_and_recovers_on_retry() {
    let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
    let mut state = SseParseState::new();
    let mut buf: Vec<u8> = Vec::new();

    // Simulate a dropped write that ended without the line's '\n'.
    let line = button_line("btn1", 1);
    let no_terminator = &line[..line.len() - 1];
    buf.extend_from_slice(no_terminator.as_bytes());

    drain_sse_lines(&mut buf, &tx, &mut state);
    assert!(rx.try_recv().is_err(), "no terminator -> no event");
    assert!(
        !buf.is_empty(),
        "the partial payload must be preserved so a later retry (after reconnect) can complete it"
    );

    // Reconnect: the production loop would create a fresh buffer. Nothing
    // here relies on the old partial data — the next connection's stream is
    // independent. Simulate that by clearing the buffer and feeding a fresh
    // complete line.
    buf.clear();
    buf.extend_from_slice(line.as_bytes());
    drain_sse_lines(&mut buf, &tx, &mut state);
    assert!(
        matches!(rx.try_recv(), Ok(HueSseEvent::ButtonEvent { .. })),
        "post-reconnect full line dispatches normally"
    );
}

#[test]
fn many_heartbeats_dispatch_in_order_without_data_interleaving() {
    let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(64);
    let mut state = SseParseState::new();
    let mut buf: Vec<u8> = Vec::new();

    for _ in 0..10 {
        buf.extend_from_slice(b": hi\n");
    }

    drain_sse_lines(&mut buf, &tx, &mut state);

    let mut count = 0;
    while let Ok(HueSseEvent::Heartbeat) = rx.try_recv() {
        count += 1;
    }
    assert_eq!(count, 10);
}

#[test]
fn invalid_json_on_data_line_does_not_poison_subsequent_events() {
    let (tx, rx) = mpsc::sync_channel::<HueSseEvent>(16);
    let mut state = SseParseState::new();
    let mut buf: Vec<u8> = Vec::new();

    // Malformed data line (valid "button" keyword so it isn't pre-filtered,
    // but JSON parse fails).
    buf.extend_from_slice(b"data: [{\"button\" : broken}\n");
    drain_sse_lines(&mut buf, &tx, &mut state);
    assert!(
        rx.try_recv().is_err(),
        "broken JSON must not dispatch (and must not panic)"
    );

    // A valid event after the broken one must still dispatch.
    buf.extend_from_slice(button_line("btn1", 1).as_bytes());
    drain_sse_lines(&mut buf, &tx, &mut state);
    assert!(matches!(rx.try_recv(), Ok(HueSseEvent::ButtonEvent { .. })));
}
