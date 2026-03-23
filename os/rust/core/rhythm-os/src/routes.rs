//! Shared API route definitions.
//!
//! Lists every `(path, methods)` pair that all platform servers must implement.
//! Excludes transport-dependent endpoints (SSE `/api/events`) and
//! platform-specific routes (ESP32 diag/wifi, server discover/ota).
//!
//! Used by:
//! - Axum router test to verify all routes are registered
//! - ESP32 HTTP server to generate CORS OPTIONS handlers

/// A shared API route with its path and accepted HTTP methods.
pub struct SharedRoute {
    pub path: &'static str,
    pub methods: &'static [&'static str],
}

/// All API routes that every platform server must implement.
///
/// Does NOT include:
/// - `GET /api/events` (SSE — transport-dependent)
/// - ESP32-only: `/api/wifi`, `/api/diag/*`, `/api/system/reboot`, `/api/ota/upload`
/// - Server-only: `/api/discover`, `/api/ota/check`, `/api/ota/update`
pub const SHARED_API_ROUTES: &[SharedRoute] = &[
    SharedRoute {
        path: "/health",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/state",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/rooms/state",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/rooms",
        methods: &["PUT", "DELETE"],
    },
    SharedRoute {
        path: "/api/rooms/action",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/rooms/brightness",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/rooms/offset",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/rooms/preferences",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/devices",
        methods: &["PUT", "DELETE"],
    },
    SharedRoute {
        path: "/api/motion-timeout",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/config",
        methods: &["GET", "PUT"],
    },
    SharedRoute {
        path: "/api/config/absorb-offset",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/config/reset",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/location",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/settings",
        methods: &["GET", "PUT"],
    },
    SharedRoute {
        path: "/api/hub/credentials",
        methods: &["PUT", "DELETE"],
    },
    SharedRoute {
        path: "/api/rooms/fix",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/sync",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/ota/version",
        methods: &["GET"],
    },
    // Canonical device management
    SharedRoute {
        path: "/api/devices/canonical",
        methods: &["GET"],
    },
    // Triage queue
    SharedRoute {
        path: "/api/devices/triage",
        methods: &["GET"],
    },
    // Topology room management
    SharedRoute {
        path: "/api/topology/rooms",
        methods: &["GET", "POST"],
    },
    SharedRoute {
        path: "/api/topology/rooms/:id",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/topology/rooms/:id/merge",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/topology/rooms/:id/devices/move",
        methods: &["PUT"],
    },
    // Curve visualization
    SharedRoute {
        path: "/api/curve",
        methods: &["GET", "POST"],
    },
    SharedRoute {
        path: "/api/curve/now",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/curve/solar",
        methods: &["GET"],
    },
];
