//! Device identification and handling utilities.
//!
//! This module provides shared device logic for:
//! - IEEE address normalization and Hue detection
//! - ZigBee endpoint determination by manufacturer
//! - Area ID normalization
//!
//! These functions are used across multiple platforms:
//! - Home Assistant add-on
//! - ESP32 firmware
//! - macOS/Linux server

pub mod area;
pub mod endpoint;
pub mod ieee;

// Re-export commonly used items
pub use area::{ids_match, normalize_id};
pub use endpoint::{for_ieee, for_manufacturer, ENDPOINT_DEFAULT, ENDPOINT_HUE, ENDPOINT_IKEA};
pub use ieee::{
    extract_from_identifiers, has_zha_identifier, is_hue, normalize, HUE_SWITCH_PREFIXES,
};
