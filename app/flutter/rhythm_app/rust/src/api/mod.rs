//! Flutter API for rhythm-core.
//!
//! This module exposes the core lighting calculation functions
//! for use via flutter_rust_bridge.

pub mod curve;
pub mod dto;
pub mod helpers;
pub mod hue;
pub mod hue_registry;
pub mod runner;

#[cfg(test)]
mod tests;

// ============================================================================
// Re-export all public DTOs
// ============================================================================

pub use dto::*;

// ============================================================================
// Re-export all public functions
// ============================================================================

// Curve functions
pub use curve::{
    calculate_lighting, calculate_step_sequences, generate_curve_data,
    generate_curve_data_high_res, generate_curve_data_with_sun_times, get_sun_position,
    get_sun_times, get_twilight_times, is_morning,
};

// Runner functions
pub use runner::{
    calculate_action_result, create_runner_state, runner_add_room, runner_get_enabled_rooms,
    runner_get_room, runner_get_room_ids, runner_get_rooms_by_source, runner_handle_action,
    runner_remove_room, runner_set_room_brightness_offset, runner_set_room_curve_config,
    runner_set_room_devices, runner_set_room_disabled, runner_set_room_lights_on,
    runner_set_room_rhythm_enabled, runner_set_room_time_offset,
};

// Helper functions
pub use helpers::{
    area_ids_match, endpoint_for_manufacturer, get_group_prefix, get_hue_switch_prefixes,
    group_name_for_area, is_hue_ieee, is_light_entity, is_rhythm_group, normalize_area_id,
    normalize_ieee,
};

// Hue button functions
pub use hue::{map_hue_button_event, parse_hue_button_event_type};

// Hue registry functions
pub use hue_registry::{
    behavior_tracker_add, behavior_tracker_behavior_count, behavior_tracker_clear,
    behavior_tracker_configured_devices, behavior_tracker_device_count,
    behavior_tracker_is_configured, behavior_tracker_remove, create_behavior_tracker,
    create_button, create_room, create_switch_device,
};
