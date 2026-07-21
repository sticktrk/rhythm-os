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
static NATIVE_LOG_ERROR_COUNTERS: OnceLock<Vec<tracing_appender::non_blocking::ErrorCounter>> =
    OnceLock::new();
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6f%:z";
const NATIVE_LOG_BUFFERED_LINES: usize = 1024;

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

/// Param keys whose values are safe to print in pairing logs. Everything
/// else is shown as `<redacted>` — notably `setup_payload`, which contains
/// the Matter pairing code. Key-only summaries hid the one fact that
/// mattered in issue #123 triage (`force=true` vs a graceful attempt).
const SAFE_PAIRING_PARAM_KEYS: &[&str] = &["device_id", "force", "network", "rendezvous"];

/// Summarize pairing/unpairing params showing values for known-safe keys.
pub fn summarize_pairing_params_for_log(value: &Value) -> String {
    let Value::Object(map) = value else {
        return summarize_json_for_log(value);
    };
    let mut keys: Vec<_> = map.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let rendered = keys
        .iter()
        .map(|key| {
            if SAFE_PAIRING_PARAM_KEYS.contains(key) {
                format!("{}={}", key, map[*key])
            } else {
                format!("{key}=<redacted>")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{rendered}}}")
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

    let (stdout, stdout_guard, stdout_errors) = non_blocking_log_writer(std::io::stdout());
    let mut guards = vec![stdout_guard];
    let mut error_counters = vec![stdout_errors];

    let result = match format.as_str() {
        "json" => tracing_subscriber::fmt()
            .with_timer(LocalTimer)
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_target(true)
            .with_thread_names(true)
            .with_writer(stdout)
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .try_init()
            .map_err(|e| anyhow::anyhow!("failed to initialize logging: {}", e)),
        "compact" => tracing_subscriber::fmt()
            .with_timer(LocalTimer)
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_target(true)
            .with_thread_names(true)
            .with_writer(stdout)
            .compact()
            .try_init()
            .map_err(|e| anyhow::anyhow!("failed to initialize logging: {}", e)),
        _ => init_full_format(filter, stdout, &mut guards, &mut error_counters),
    };

    if result.is_ok() {
        let _ = NATIVE_LOG_ERROR_COUNTERS.set(error_counters);
        // A WorkerGuard flushes and joins its writer thread when dropped. Native
        // logging lives for the process lifetime, so intentionally retain the
        // guards instead of blocking initialization while the sinks are active.
        for guard in guards {
            Box::leak(Box::new(guard));
        }
    }

    result
}

fn non_blocking_log_writer<W>(
    writer: W,
) -> (
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
    tracing_appender::non_blocking::ErrorCounter,
)
where
    W: std::io::Write + Send + 'static,
{
    let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(NATIVE_LOG_BUFFERED_LINES)
        .lossy(true)
        .finish(writer);
    let errors = writer.error_counter();
    (writer, guard, errors)
}

/// Lines discarded because a native log sink could not keep up.
///
/// Native logging is deliberately lossy under backpressure so storage I/O can
/// never stop periodic scheduling, integration work, or the liveness watchdog.
pub fn native_log_dropped_lines() -> usize {
    NATIVE_LOG_ERROR_COUNTERS
        .get()
        .map(|counters| counters.iter().map(|counter| counter.dropped_lines()).sum())
        .unwrap_or(0)
}

/// Chatty targets that get their own log files when splitting is active.
/// Each pair is (tracing target, file name under `RHYTHM_LOG_DIR`).
const SPLIT_LOG_TARGETS: &[(&str, &str)] = &[
    ("http", "rhythm-http.log"),
    ("periodic", "rhythm-periodic.log"),
    ("sse", "rhythm-sse.log"),
];

fn is_split_log_target(target: &str) -> bool {
    SPLIT_LOG_TARGETS
        .iter()
        .any(|(split_target, _)| *split_target == target)
}

/// Chatter splitting is active when `RHYTHM_LOG_DIR` names a directory (the
/// appliance launcher exports it) and `RHYTHM_LOG_SPLIT` isn't disabled.
/// HTTP request, periodic-cycle, and SSE chatter historically consumed the
/// whole main-log rotation budget within hours, leaving multi-hour holes in
/// debug bundles.
fn split_log_dir() -> Option<std::path::PathBuf> {
    if matches!(
        std::env::var("RHYTHM_LOG_SPLIT").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    ) {
        return None;
    }
    let dir = std::env::var("RHYTHM_LOG_DIR").ok()?;
    if dir.trim().is_empty() {
        return None;
    }
    Some(std::path::PathBuf::from(dir))
}

fn init_full_format(
    filter: String,
    stdout: tracing_appender::non_blocking::NonBlocking,
    guards: &mut Vec<tracing_appender::non_blocking::WorkerGuard>,
    error_counters: &mut Vec<tracing_appender::non_blocking::ErrorCounter>,
) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;

    // Append mode keeps the files compatible with rhythm-log-prune's
    // truncate-in-place rotation. If any sink fails to open, keep the
    // proven single-stream path — losing chatter routing beats losing logs.
    let split_files = split_log_dir().and_then(|dir| {
        let mut files = Vec::new();
        for (target, name) in SPLIT_LOG_TARGETS {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(name))
            {
                Ok(file) => files.push((*target, file)),
                Err(error) => {
                    eprintln!(
                        "rhythm: chatter log {name} unavailable ({error}); using single-stream logging"
                    );
                    return None;
                }
            }
        }
        Some(files)
    });

    let ansi = std::io::stdout().is_terminal();

    let Some(split_files) = split_files else {
        return tracing_subscriber::fmt()
            .with_timer(LocalTimer)
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_target(true)
            .with_thread_names(true)
            .with_ansi(ansi)
            .with_writer(stdout)
            .try_init()
            .map_err(|e| anyhow::anyhow!("failed to initialize logging: {}", e));
    };

    let mut layers: Vec<Box<dyn Layer<tracing_subscriber::registry::Registry> + Send + Sync>> =
        Vec::new();
    layers.push(
        tracing_subscriber::fmt::layer()
            .with_timer(LocalTimer)
            .with_target(true)
            .with_thread_names(true)
            .with_ansi(ansi)
            .with_writer(stdout)
            .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                !is_split_log_target(metadata.target())
            }))
            .boxed(),
    );
    for (target, file) in split_files {
        let (file, guard, errors) = non_blocking_log_writer(file);
        guards.push(guard);
        error_counters.push(errors);
        layers.push(
            tracing_subscriber::fmt::layer()
                .with_timer(LocalTimer)
                .with_target(true)
                .with_thread_names(true)
                .with_ansi(false)
                .with_writer(file)
                .with_filter(tracing_subscriber::filter::filter_fn(move |metadata| {
                    metadata.target() == target
                }))
                .boxed(),
        );
    }

    tracing_subscriber::registry()
        .with(layers)
        .with(tracing_subscriber::EnvFilter::new(filter))
        .try_init()
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

    struct BlockingLogSink {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        blocked_once: bool,
    }

    impl std::io::Write for BlockingLogSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if !self.blocked_once {
                self.blocked_once = true;
                let _ = self.entered.send(());
                let _ = self.release.recv();
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn native_log_writer_never_blocks_runtime_threads_on_a_stalled_sink() {
        use std::io::Write as _;

        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (mut writer, guard, errors) = non_blocking_log_writer(BlockingLogSink {
            entered: entered_tx,
            release: release_rx,
            blocked_once: false,
        });

        writer.write_all(b"first line\n").unwrap();
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("background writer should reach the blocked sink");

        let started = std::time::Instant::now();
        for _ in 0..(NATIVE_LOG_BUFFERED_LINES + 32) {
            writer.write_all(b"queued line\n").unwrap();
        }
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "runtime writes must remain bounded while the sink is stalled"
        );
        assert!(
            errors.dropped_lines() > 0,
            "a full bounded queue should expose dropped-line evidence"
        );

        release_tx.send(()).unwrap();
        drop(writer);
        drop(guard);
    }

    #[test]
    fn split_targets_cover_the_chatter_families() {
        assert!(is_split_log_target("http"));
        assert!(is_split_log_target("periodic"));
        assert!(is_split_log_target("sse"));
        assert!(!is_split_log_target("sys"));
        assert!(!is_split_log_target("pair"));
        assert!(!is_split_log_target("evt"));
    }

    #[test]
    fn pairing_param_summary_shows_safe_values_and_redacts_the_rest() {
        let summary = summarize_pairing_params_for_log(&serde_json::json!({
            "setup_payload": "MT:SECRET",
            "device_id": "matter-102",
            "force": true,
        }));
        assert_eq!(
            summary,
            "{device_id=\"matter-102\", force=true, setup_payload=<redacted>}"
        );
        assert!(!summary.contains("SECRET"));
    }

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
    fn summarize_json_scalars_do_not_log_secret_values() {
        assert_eq!(summarize_json_for_log(&serde_json::Value::Null), "null");
        assert_eq!(summarize_json_for_log(&serde_json::json!(true)), "bool");
        assert_eq!(summarize_json_for_log(&serde_json::json!(123)), "number");
        assert_eq!(
            summarize_json_for_log(&serde_json::json!("secret-token")),
            "string"
        );
    }

    #[test]
    fn summarize_json_object_truncates_long_key_lists() {
        let summary = summarize_json_for_log(&serde_json::json!({
            "a": 1,
            "b": 2,
            "c": 3,
            "d": 4,
            "e": 5,
            "f": 6,
            "g": 7,
        }));

        assert_eq!(summary, "object(keys=[a,b,c,d,e,f,...],len=7)");
    }

    #[test]
    fn command_ids_are_monotonic_and_prefixed() {
        let first = next_command_id("test");
        let second = next_command_id("test");

        let first_num = first.strip_prefix("test-").unwrap().parse::<u64>().unwrap();
        let second_num = second
            .strip_prefix("test-")
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert_eq!(second_num, first_num + 1);
    }

    #[test]
    fn build_log_clock_requires_location_for_fixed_offset() {
        assert!(build_log_clock(None, -4.0, false).is_none());
        assert!(matches!(
            build_log_clock(None, -4.0, true),
            Some(LogClock::FixedOffset(_))
        ));
        assert!(matches!(
            build_log_clock(Some("America/New_York"), -4.0, false),
            Some(LogClock::Timezone(_))
        ));
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

    #[test]
    fn update_log_clock_sets_configured_clock() {
        update_log_clock_from_location(None, 2.0, true);
        assert!(matches!(
            configured_log_clock(),
            Some(LogClock::FixedOffset(_))
        ));

        update_log_clock_from_location(None, 0.0, false);
        assert!(configured_log_clock().is_none());
    }
}
