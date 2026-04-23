//! Shared API route definitions.
//!
//! Lists every `(path, methods)` pair that all platform servers must implement.
//! Excludes transport-dependent endpoints (SSE `/api/events`) and
//! platform-specific routes (hardware Wi-Fi/diag and OTA).
//!
//! Used by:
//! - Axum router test to verify all routes are registered
//! - Platform HTTP servers to generate CORS OPTIONS handlers

/// A shared API route with its path and accepted HTTP methods.
pub struct SharedRoute {
    pub path: &'static str,
    pub methods: &'static [&'static str],
}

/// All API routes that every platform server must implement.
///
/// Does NOT include:
/// - `GET /api/events` (SSE — transport-dependent)
/// - Hardware-specific: `/api/wifi`, `/api/diag/*`, `/api/system/reboot`
/// - Platform-specific OTA: `/api/ota/version`, `/api/ota/upload`, `/api/ota/capabilities`, `/api/ota/status`, `/api/ota/check`, `/api/ota/update`
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
        path: "/api/profile-bundle",
        methods: &["GET", "PUT"],
    },
    SharedRoute {
        path: "/api/profile-bundle/factory-default",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/profile-bundle/reset",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/factory-reset",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/backup",
        methods: &["GET", "PUT"],
    },
    SharedRoute {
        path: "/api/nodes/state",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/nodes/action",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/nodes/brightness",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/nodes/offset",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/nodes/preferences",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/devices",
        methods: &["DELETE"],
    },
    SharedRoute {
        path: "/api/nodes/motion-timeout",
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
        path: "/api/mode",
        methods: &["GET", "PUT"],
    },
    SharedRoute {
        path: "/api/transitions",
        methods: &["GET", "PUT"],
    },
    SharedRoute {
        path: "/api/transitions/:id/trigger",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/profiles",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/hub/credentials",
        methods: &["PUT", "DELETE"],
    },
    SharedRoute {
        path: "/api/nodes/fix",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/sync",
        methods: &["POST"],
    },
    // Device pairing / unpairing
    SharedRoute {
        path: "/api/devices/pair",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/devices/unpair",
        methods: &["POST"],
    },
    SharedRoute {
        path: "/api/matter/captures",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/matter/captures/:id",
        methods: &["GET"],
    },
    // Canonical device management
    SharedRoute {
        path: "/api/devices/canonical",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/devices/canonical/:id",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/devices/canonical/:id/room",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/devices/canonical/:id/preferred",
        methods: &["PUT"],
    },
    // Triage queue
    SharedRoute {
        path: "/api/triage",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/triage/count",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/triage/:id/merge",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/triage/:id/new",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/triage/:id/dismiss",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/triage/:id/room",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/triage/:id/bind",
        methods: &["PUT"],
    },
    // Topology room management
    SharedRoute {
        path: "/api/topology/rooms",
        methods: &["GET", "POST"],
    },
    SharedRoute {
        path: "/api/topology/nodes",
        methods: &["GET"],
    },
    SharedRoute {
        path: "/api/topology/rooms/:id",
        methods: &["PUT", "DELETE"],
    },
    SharedRoute {
        path: "/api/topology/rooms/:id/merge",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/topology/rooms/:id/devices/move",
        methods: &["PUT"],
    },
    SharedRoute {
        path: "/api/topology/nodes/:id/controls/:kind",
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
    SharedRoute {
        path: "/api/light-profile",
        methods: &["PUT"],
    },
];
