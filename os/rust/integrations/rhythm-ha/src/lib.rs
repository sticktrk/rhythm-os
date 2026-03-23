//! Platform-agnostic Home Assistant integration for Rhythm OS.
//!
//! Provides device registry, WebSocket event parsing, light controller,
//! and transport abstraction for Home Assistant. No platform-specific
//! dependencies — concrete transport implementations live in the platform crate.

pub mod controller;
pub mod events;
pub mod ha_lifecycle;
pub mod hub_state;
pub mod provider;
pub mod registry;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod transport;

#[cfg(feature = "desktop")]
pub mod area_sync;
#[cfg(feature = "desktop")]
pub mod desktop_lifecycle;
#[cfg(feature = "desktop")]
pub mod post_connect;
#[cfg(feature = "desktop")]
pub mod reqwest_transport;
#[cfg(feature = "desktop")]
pub mod ws_client;
