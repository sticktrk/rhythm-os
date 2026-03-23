//! Platform-agnostic Hue integration layer.
//!
//! Provides device registry, SSE event parsing, button mapping, light controller,
//! and transport abstraction for Philips Hue bridges. No ESP32 or platform-specific
//! dependencies — concrete transport implementations live in the platform crate.

pub mod api_types;
pub mod behavior;
pub mod buttons;
pub mod controller;
pub mod device_types;
pub mod discovery;
#[cfg(feature = "embedded")]
pub mod embedded_lifecycle;
pub mod events;
pub mod hub_state;
pub mod hue_lifecycle;
pub mod provider;
pub mod registry;
#[cfg(feature = "desktop")]
pub mod reqwest_lifecycle;
#[cfg(feature = "desktop")]
pub mod reqwest_sse;
#[cfg(feature = "desktop")]
pub mod reqwest_transport;
pub mod sse;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod transport;

// Re-exports for convenient access
pub use behavior::HueBehaviorTracker;
pub use device_types::{HueButton, HueRoom, HueSwitchDevice};
