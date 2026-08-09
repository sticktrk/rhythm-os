//! Platform-agnostic business logic for Rhythm OS controllers.
//!
//! Shared hub management, command handling, state management, and lifecycle
//! logic reused by the appliance, server, add-on, and future targets.
//!
//! No platform-specific dependencies — concrete storage backends, SSE
//! transports, and HTTP servers are provided by the consuming binary crate.

pub mod activity;
pub mod activity_cloud;
pub mod api_types;
pub mod auth;
pub mod bundle;
pub mod button_resolve;
pub mod canonical;
pub mod commands;
pub mod controller_helpers;
pub mod device_naming;
pub mod discovery;
pub mod event_loop;
pub mod factory_default_config;
pub mod handlers;
pub mod hub;
pub mod hue_buttons;
pub mod lifecycle;
pub mod light_runtime;
pub mod logging;
pub mod mdns;
pub mod pairing;
pub mod periodic;
pub mod provisioning;
pub mod registry;
pub mod remote_access;
pub mod room_sync;
pub mod routes;
pub mod scenes;
pub mod state;
pub mod storage;
pub mod topology;

pub mod axum_router;
pub mod server_event;
