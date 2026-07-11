//! Platform-agnostic Hue integration layer.
//!
//! Provides device registry, SSE event parsing, button mapping, light controller,
//! and transport abstraction for Philips Hue bridges. No target-specific
//! dependencies — concrete transport implementations live in the platform crate.

pub mod api_types;
pub mod behavior;
pub mod controller;
pub mod device_types;
pub mod discovery;
pub mod events;
pub mod hub_state;
pub mod hue_lifecycle;
pub mod provider;
pub mod registry;
pub mod reqwest_lifecycle;
pub mod reqwest_sse;
pub mod reqwest_transport;
pub mod room_membership;
pub mod sse;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod transport;

// Re-exports for convenient access
pub use behavior::HueBehaviorTracker;
pub use device_types::{HueButton, HueRoom, HueSwitchDevice};
