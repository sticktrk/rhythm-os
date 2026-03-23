//! Hub abstraction layer for rhythm-addon.
//!
//! Defines the static integration registry. Adding a new integration
//! is a single line here + a Cargo.toml dependency.

pub use rhythm_os::hub::*;

/// All integrations available to this binary.
pub static INTEGRATIONS: &[&dyn ExternalLightHubIntegration] = &[
    &rhythm_hue::reqwest_lifecycle::INTEGRATION,
    &rhythm_ha::desktop_lifecycle::INTEGRATION,
];
