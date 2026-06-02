//! Axum HTTP server — uses shared routes from rhythm-os plus addon-specific config.

use axum::middleware;
use axum::routing::get;
use axum::Router;
use rhythm_os::logging;
use rhythm_os::state::SharedState;
use tower_http::cors::CorsLayer;

/// Create the Axum router with all API routes.
pub fn create_router(state: SharedState) -> Router {
    let auth_state = state.clone();
    let api = rhythm_os::axum_router::api_routes()
        // Legacy HA area sync alias (delegates to generic sync)
        .route(
            "/api/ha/sync-areas",
            get(rhythm_os::axum_router::post_sync).post(rhythm_os::axum_router::post_sync),
        )
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            auth_state,
            rhythm_os::auth::require_api_auth_middleware,
        ));

    logging::with_http_observability(api.layer(CorsLayer::permissive()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_os::state::AppState;
    use std::sync::{Arc, Mutex};

    #[test]
    fn create_router_builds_addon_api_routes() {
        let state = Arc::new(Mutex::new(AppState::default()));
        let _router = create_router(state);
    }
}
