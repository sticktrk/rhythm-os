//! Shared logging helpers for active native platforms.
//!
//! The server, add-on, and Linux appliance builds all use the same native
//! subscriber setup and HTTP request tracing so operators get one consistent
//! log shape across active platforms.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock, RwLock,
};

use serde_json::Value;

static COMMAND_COUNTER: AtomicU64 = AtomicU64::new(1);
static LOG_CLOCK: OnceLock<RwLock<Option<LogClock>>> = OnceLock::new();
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6f%:z";

#[derive(Clone)]
enum LogClock {
    Timezone(rhythm_core::Timezone),
    FixedOffset(chrono::FixedOffset),
}

/// Generate a monotonic command ID for cross-log correlation.
pub fn next_command_id(kind: &str) -> String {
    format!(
        "{}-{}",
        kind,
        COMMAND_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Summarize JSON payloads for logs without printing secrets.
pub fn summarize_json_for_log(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(items) => format!("array(len={})", items.len()),
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().map(String::as_str).collect();
            keys.sort_unstable();
            let preview = keys.iter().take(6).copied().collect::<Vec<_>>().join(",");
            let suffix = if keys.len() > 6 { ",..." } else { "" };
            format!("object(keys=[{}{}],len={})", preview, suffix, keys.len())
        }
    }
}

fn log_clock() -> &'static RwLock<Option<LogClock>> {
    LOG_CLOCK.get_or_init(|| RwLock::new(None))
}

fn fixed_offset_from_hours(utc_offset_hours: f32) -> Option<chrono::FixedOffset> {
    let seconds = (utc_offset_hours as f64 * 3600.0).round() as i32;
    chrono::FixedOffset::east_opt(seconds)
}

fn build_log_clock(
    timezone_name: Option<&str>,
    utc_offset_hours: f32,
    has_location: bool,
) -> Option<LogClock> {
    if let Some(timezone_name) = timezone_name {
        return Some(LogClock::Timezone(rhythm_core::Timezone::new(
            timezone_name,
        )));
    }

    if has_location {
        return fixed_offset_from_hours(utc_offset_hours).map(LogClock::FixedOffset);
    }

    None
}

fn configured_log_clock() -> Option<LogClock> {
    match log_clock().read() {
        Ok(clock) => clock.clone(),
        Err(_) => None,
    }
}

fn format_timestamp_for_log(
    now_utc: chrono::DateTime<chrono::Utc>,
    clock: Option<&LogClock>,
) -> String {
    match clock {
        Some(LogClock::Timezone(timezone)) => timezone
            .local_datetime_with_offset_from_utc(now_utc.naive_utc())
            .format(TIMESTAMP_FORMAT)
            .to_string(),
        Some(LogClock::FixedOffset(offset)) => now_utc
            .with_timezone(offset)
            .format(TIMESTAMP_FORMAT)
            .to_string(),
        None => now_utc
            .with_timezone(&chrono::Local)
            .format(TIMESTAMP_FORMAT)
            .to_string(),
    }
}

/// Keep log timestamps aligned with the configured Rhythm location.
pub fn update_log_clock_from_location(
    timezone_name: Option<&str>,
    utc_offset_hours: f32,
    has_location: bool,
) {
    let next_clock = build_log_clock(timezone_name, utc_offset_hours, has_location);
    match log_clock().write() {
        Ok(mut clock) => *clock = next_clock,
        Err(poisoned) => *poisoned.into_inner() = next_clock,
    }
}

use axum::extract::MatchedPath;
use axum::http::Request;
use axum::response::Response;
use axum::Router;
use tower_http::request_id::{
    MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::trace::TraceLayer;

use crate::axum_router::ApiErrorContext;

struct LocalTimer;

impl tracing_subscriber::fmt::time::FormatTime for LocalTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        let now_utc = chrono::Utc::now();
        let timestamp = format_timestamp_for_log(now_utc, configured_log_clock().as_ref());
        write!(w, "{}", timestamp)
    }
}

fn request_id_from_request<B>(request: &Request<B>) -> String {
    request
        .extensions()
        .get::<RequestId>()
        .and_then(|request_id| request_id.header_value().to_str().ok())
        .unwrap_or("-")
        .to_string()
}

/// Initialize the native tracing subscriber used by active platforms.
pub fn init_native_logging(default_level: &str) -> anyhow::Result<()> {
    let base_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| default_level.to_string());
    let filter = format!("{},mdns_sd=off", base_filter);
    let format = std::env::var("RHYTHM_LOG_FORMAT")
        .unwrap_or_else(|_| "full".to_string())
        .to_ascii_lowercase();

    match format.as_str() {
        "json" => tracing_subscriber::fmt()
            .with_timer(LocalTimer)
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_target(true)
            .with_thread_names(true)
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .try_init(),
        "compact" => tracing_subscriber::fmt()
            .with_timer(LocalTimer)
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_target(true)
            .with_thread_names(true)
            .compact()
            .try_init(),
        _ => tracing_subscriber::fmt()
            .with_timer(LocalTimer)
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_target(true)
            .with_thread_names(true)
            .try_init(),
    }
    .map_err(|e| anyhow::anyhow!("failed to initialize logging: {}", e))
}

/// Apply request IDs and HTTP latency logging to a shared-state router.
pub fn with_http_observability<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<axum::body::Body>| {
                    let matched_path = request
                        .extensions()
                        .get::<MatchedPath>()
                        .map(|matched_path| matched_path.as_str())
                        .unwrap_or("<unmatched>");
                    let request_id = request_id_from_request(request);
                    tracing::info_span!(
                        target: "http",
                        "http_request",
                        request_id = %request_id,
                        method = %request.method(),
                        uri = %request.uri(),
                        matched_path = matched_path,
                    )
                })
                .on_request(
                    |_request: &Request<axum::body::Body>, span: &tracing::Span| {
                        tracing::info!(
                            target: "http",
                            parent: span,
                            event = "http_request_start",
                            "request started"
                        );
                    },
                )
                .on_response(
                    |response: &Response, latency: std::time::Duration, span: &tracing::Span| {
                        let status = response.status();
                        let latency_ms = latency.as_millis();

                        if status.is_server_error() {
                            let error = response
                                .extensions()
                                .get::<ApiErrorContext>()
                                .map(|context| context.0.as_str())
                                .unwrap_or("<no error body>");
                            tracing::error!(
                                target: "http",
                                parent: span,
                                event = "http_response",
                                status = %status,
                                latency_ms,
                                error = %error,
                                "request returned server error"
                            );
                        } else if status.is_client_error() {
                            tracing::warn!(
                                target: "http",
                                parent: span,
                                event = "http_response",
                                status = %status,
                                latency_ms,
                                "request returned client error"
                            );
                        } else {
                            tracing::info!(
                                target: "http",
                                parent: span,
                                event = "http_response",
                                status = %status,
                                latency_ms,
                                "request completed"
                            );
                        }
                    },
                )
                .on_failure(()),
        )
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_json_object_uses_sorted_keys() {
        let summary = summarize_json_for_log(&serde_json::json!({
            "token": "secret",
            "bridge_ip": "1.2.3.4",
            "username": "app-key",
        }));

        assert_eq!(summary, "object(keys=[bridge_ip,token,username],len=3)");
    }

    #[test]
    fn summarize_json_array_only_reports_len() {
        let summary = summarize_json_for_log(&serde_json::json!(["a", "b", "c"]));
        assert_eq!(summary, "array(len=3)");
    }

    #[test]
    fn format_timestamp_uses_rhythm_timezone_override() {
        let now_utc = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 4, 23, 19, 32, 47)
            .single()
            .unwrap();
        let clock = LogClock::Timezone(rhythm_core::Timezone::new("America/New_York"));

        let timestamp = format_timestamp_for_log(now_utc, Some(&clock));

        assert_eq!(timestamp, "2026-04-23T15:32:47.000000-04:00");
    }

    #[test]
    fn format_timestamp_uses_fixed_offset_override_when_timezone_is_unavailable() {
        let now_utc = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 4, 23, 19, 32, 47)
            .single()
            .unwrap();
        let clock = LogClock::FixedOffset(chrono::FixedOffset::west_opt(5 * 3600).unwrap());

        let timestamp = format_timestamp_for_log(now_utc, Some(&clock));

        assert_eq!(timestamp, "2026-04-23T14:32:47.000000-05:00");
    }
}
