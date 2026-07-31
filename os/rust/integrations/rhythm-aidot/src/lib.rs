//! Orein/AiDot single-button Bluetooth input support.
//!
//! The accessory is associated from its printed QR identity. Presses remain
//! advertisement-only and enter Rhythm through the existing generic button
//! event path; this crate does not create a parallel input runtime.

pub mod discovery;
pub mod lifecycle;
pub mod protocol;
pub mod store;
pub mod transport;

#[cfg(all(target_os = "linux", feature = "bluez"))]
pub mod bluez;
#[cfg(all(target_os = "linux", feature = "bluez"))]
pub mod bluez_lifecycle;

pub use protocol::AidotSetupCode;
pub use store::{AidotButtonDevice, AidotButtonStore, CounterDisposition};
pub use transport::{AidotBleTransport, AidotPairingCandidate};

pub const HUB_TYPE: &str = rhythm_os::hub::HubType::AIDOT_BLE;
pub const HUB_ADDRESS: &str = "local";
