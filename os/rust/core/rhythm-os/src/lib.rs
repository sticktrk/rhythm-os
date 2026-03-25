//! Platform-agnostic business logic for Rhythm OS controllers.
//!
//! Extracted from the ESP32 firmware so the same hub management, command
//! handling, state management, and lifecycle logic can be reused on
//! Raspberry Pi, macOS, Linux server, or any future target.
//!
//! No platform-specific dependencies — concrete storage backends, SSE
//! transports, and HTTP servers are provided by the consuming binary crate.

pub mod api_types;
pub mod button_resolve;
pub mod canonical;
pub mod commands;
pub mod controller_helpers;
pub mod discovery;
pub mod event_loop;
pub mod handlers;
pub mod hub;
pub mod hue_buttons;
pub mod lifecycle;
pub mod mdns;
pub mod pairing;
pub mod periodic;
pub mod registry;
pub mod room_sync;
pub mod routes;
pub mod state;
pub mod storage;
pub mod topology;

#[cfg(feature = "desktop")]
pub mod axum_router;
#[cfg(feature = "desktop")]
pub mod server_event;
