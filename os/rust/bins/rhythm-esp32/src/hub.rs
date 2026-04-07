//! Hub abstraction layer for ESP32.
//!
//! Re-exports types from rhythm-os and provides the platform-specific
//! hub provider lookup.

// Re-export all hub types from rhythm-os
pub use rhythm_os::hub::*;

// Re-export RuntimeHandle and RoomSnapshot from rhythm-core
pub use rhythm_core::{RoomSnapshot, RuntimeHandle};

/// Get the static hub provider for a given hub type.
///
/// **Extension point:** To add a new hub, create a struct implementing
/// `HubProvider`, add a `static` instance here, and add a match arm.
pub fn get_hub_provider(hub_type: HubType) -> &'static dyn HubProvider {
    static HUE: crate::platform::hue::HueHubProvider = crate::platform::hue::HueHubProvider;
    match hub_type.as_str() {
        HubType::HUE => &HUE,
        _ => &HUE, // fallback to Hue (only hub currently supported)
    }
}
