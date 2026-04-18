//! Axum HTTP server — uses shared routes from rhythm-os plus addon-specific config.

use std::time::Duration;

use axum::extract::MatchedPath;
use axum::http::Request;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use rhythm_os::axum_router::ApiErrorContext;
use rhythm_os::state::SharedState;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::Span;

/// Create the Axum router with all API routes.
pub fn create_router(state: SharedState) -> Router {
    let api = rhythm_os::axum_router::api_routes()
        // Legacy HA area sync alias (delegates to generic sync)
        .route(
            "/api/ha/sync-areas",
            get(rhythm_os::axum_router::post_sync).post(rhythm_os::axum_router::post_sync),
        )
        .with_state(state);

    api.layer(CorsLayer::permissive())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<axum::body::Body>| {
                    let matched_path = request
                        .extensions()
                        .get::<MatchedPath>()
                        .map(|matched_path| matched_path.as_str())
                        .unwrap_or("<unmatched>");
                    tracing::info_span!(
                        "http_request",
                        method = %request.method(),
                        uri = %request.uri(),
                        matched_path = matched_path,
                    )
                })
                .on_response(|response: &Response, latency: Duration, span: &Span| {
                    if response.status().is_server_error() {
                        let error = response
                            .extensions()
                            .get::<ApiErrorContext>()
                            .map(|context| context.0.as_str())
                            .unwrap_or("<no error body>");
                        tracing::error!(
                            parent: span,
                            status = %response.status(),
                            latency_ms = latency.as_millis(),
                            error = %error,
                            "request returned server error"
                        );
                    }
                })
                .on_failure(()),
        )
}
