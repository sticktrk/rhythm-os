//! Shared local-BLE profile host and adapter runtime.
//!
//! Simple devices are versioned profiles here rather than vendor-shaped hub
//! crates. Behavior-rich drivers such as Hue BLE may remain separate crates,
//! but their Linux transports must consume this crate's single BlueZ owner.

#[cfg(any(
    test,
    feature = "test-support",
    all(target_os = "linux", feature = "bluez")
))]
mod coordination;

pub mod discovery;
pub mod lifecycle;
pub mod profile;
pub mod store;
pub mod transport;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

#[cfg(all(target_os = "linux", feature = "bluez"))]
pub mod bluez;
#[cfg(all(target_os = "linux", feature = "bluez"))]
pub mod bluez_lifecycle;
#[cfg(all(target_os = "linux", feature = "bluez"))]
pub mod bluez_profile;

pub use profile::{OreinOc02001Setup, ValidatedBleSetup, OREIN_OC02001_PROFILE_ID};
pub use store::{LocalBleDevice, LocalBleDeviceStore, ReplayDisposition};
pub use transport::{LocalBlePairingCandidate, LocalBleTransport};

pub const HUB_TYPE: &str = rhythm_os::hub::HubType::LOCAL_BLE;
pub const HUB_ADDRESS: &str = "default";
