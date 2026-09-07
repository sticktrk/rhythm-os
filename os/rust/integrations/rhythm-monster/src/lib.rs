//! Static Monster/Ayla lighting. Cloud authentication belongs exclusively to
//! the `monster-device` Edge Function, which shares one Rhythm-operated Monster
//! account with authenticated users by default. This crate never accepts
//! Monster account credentials. No effects, music or firmware control APIs.
pub mod ble;
#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
mod ble_cleanup;
#[cfg(all(target_os = "linux", feature = "bluez"))]
pub mod bluez;
pub mod cloud;
pub mod controller;
pub mod crypto;
pub mod lan;
pub mod types;
pub use types::{LightCredentials, LightError, LightProperty, LightResult, LightSecret};
