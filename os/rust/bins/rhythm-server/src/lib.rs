//! Shared server-side modules reused by multiple native platform binaries.

pub const BUILD_VERSION: &str = match option_env!("RHYTHM_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

pub mod auto_update;
pub mod boot_diagnostics;
pub mod bootstate;
pub mod debug_bundle;
pub mod http_server;
pub mod hub;
pub mod liveness;
pub mod ota_history;
pub mod self_update;
