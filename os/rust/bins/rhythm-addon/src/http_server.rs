//! Axum HTTP server — uses shared routes from rhythm-os plus addon-specific config.

use axum::routing::get;
use axum::Router;
use rhythm_os::logging;
use rhythm_os::state::SharedState;
use tower_http::cors::CorsLayer;

/// Create the Axum router with all API routes.
pub fn create_router(state: SharedState) -> Router {
    let api = rhythm_os::axum_router::api_routes()
        // Legacy HA area sync alias (delegates to generic sync)
        .route(
            "/api/ha/sync-areas",
            get(rhythm_os::axum_router::post_sync).post(rhythm_os::axum_router::post_sync),
        )
        .with_state(state);

    logging::with_http_observability(api.layer(CorsLayer::permissive()))
}
