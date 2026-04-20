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

pub mod area_sync;
pub mod desktop_lifecycle;
pub mod post_connect;
pub mod reqwest_transport;
pub mod ws_client;

pub use desktop_lifecycle as reqwest_lifecycle;
