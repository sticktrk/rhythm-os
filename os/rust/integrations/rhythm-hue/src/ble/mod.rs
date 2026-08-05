//! Direct Philips Hue Bluetooth support.
//!
//! Hue BLE is a separate local integration (`hue_ble@local`) from a Hue
//! Bridge (`hue@<address>`). Both publish the bulb's Zigbee EUI-64 as a
//! hardware identity, allowing Rhythm's canonical registry to deduplicate the
//! same physical bulb if it later appears through a bridge.

#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
mod connection_pool;
pub mod controller;
pub mod discovery;
#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
mod gatt_runtime;
pub mod lifecycle;
pub mod protocol;
pub mod store;
pub mod transport;
pub mod types;

#[cfg(all(target_os = "linux", feature = "bluez"))]
pub mod bluez;
#[cfg(all(target_os = "linux", feature = "bluez"))]
pub mod bluez_lifecycle;

pub use controller::HueBleLightController;
pub use protocol::HueBleEffect;
pub use store::{HueBleDeviceStore, HueBleFactoryResetPlan};
pub use transport::HueBleTransport;
pub use types::{
    HueBleCapabilities, HueBleColor, HueBleCommand, HueBleDevice, HueBlePairingOutcome,
    HueBlePairingRequest, HueBleState,
};

pub const HUB_TYPE: &str = rhythm_os::hub::HubType::HUE_BLE;
pub const HUB_ADDRESS: &str = "local";

/// The Pi Zero W controller keeps a small warm working set while the durable
/// catalog can contain hundreds of bulbs.
pub(crate) const BLE_CONNECTION_POOL_CAPACITY: usize = 4;
#[cfg(all(target_os = "linux", feature = "bluez"))]
pub(crate) const BLE_PARALLEL_CONNECT_LIMIT: usize = 2;
