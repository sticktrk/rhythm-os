//! Matter integration for Rhythm OS.
//!
//! Controls Matter-over-WiFi and Matter-over-Thread lights. The server acts
//! as a Matter commissioner — the Matter fabric IS the "hub" in Rhythm's
//! hub abstraction.
//!
//! This is a protocol + integration hybrid: Matter standardizes light control
//! fully (same clusters for every brand), so per-brand crates would be
//! duplication. Brand identity comes from `CanonicalDevice.manufacturer` +
//! the `rhythm-devices` database.

pub mod capabilities;
pub mod capture;
pub mod chip_rpc;
pub mod chip_transport;
pub mod clusters;
pub mod commissioning;
pub mod controller;
pub mod discovery;
pub mod events;
pub mod groups;
pub mod hub_state;
pub mod lifecycle;
pub mod provider;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod transport;

pub mod desktop_lifecycle;
pub mod fabric;
