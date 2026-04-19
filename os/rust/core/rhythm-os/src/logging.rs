//! Shared logging helpers for active native platforms.
//!
//! The server, add-on, and Linux embedded builds all use the same native
//! subscriber setup and HTTP request tracing so operators get one consistent
//! log shape across active platforms.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static COMMAND_COUNTER: AtomicU64 = AtomicU64::new(1);

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

#[cfg(feature = "desktop")]
use axum::extract::MatchedPath;
#[cfg(feature = "desktop")]
use axum::http::{Request, Response};
#[cfg(feature = "desktop")]
use axum::Router;
#[cfg(feature = "desktop")]
use tower_http::request_id::{
    MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
#[cfg(feature = "desktop")]
use tower_http::trace::TraceLayer;

#[cfg(feature = "desktop")]
use crate::axum_router::ApiErrorContext;
#[cfg(feature = "desktop")]
use crate::state::SharedState;

#[cfg(feature = "desktop")]
struct LocalTimer;

#[cfg(feature = "desktop")]
impl tracing_subscriber::fmt::time::FormatTime for LocalTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        write!(
            w,
            "{}",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f")
        )
    }
}

#[cfg(feature = "desktop")]
fn request_id_from_request<B>(request: &Request<B>) -> String {
    request
        .extensions()
        .get::<RequestId>()
        .and_then(|request_id| request_id.header_value().to_str().ok())
        .unwrap_or("-")
        .to_string()
}

#[cfg(feature = "desktop")]
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

#[cfg(feature = "desktop")]
/// Apply request IDs and HTTP latency logging to a shared-state router.
pub fn with_http_observability(router: Router<SharedState>) -> Router<SharedState> {
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
}
