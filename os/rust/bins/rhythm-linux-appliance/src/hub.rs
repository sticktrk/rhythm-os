//! Integrations available on the Linux lighting appliance.
//!
//! Direct Hue Bluetooth is intentionally appliance-only. A generic Linux
//! `rhythm-server` may run in a container without persistent BlueZ state and
//! must not advertise a pairing path that cannot survive a restart.

pub use rhythm_os::hub::*;

pub static INTEGRATIONS: &[&dyn ExternalLightHubIntegration] = &[
    &rhythm_hue::reqwest_lifecycle::INTEGRATION,
    &rhythm_ha::reqwest_lifecycle::INTEGRATION,
    &rhythm_matter::desktop_lifecycle::INTEGRATION,
    #[cfg(target_os = "linux")]
    &rhythm_hue::ble::bluez_lifecycle::INTEGRATION,
    #[cfg(target_os = "linux")]
    &rhythm_aidot::bluez_lifecycle::INTEGRATION,
];
