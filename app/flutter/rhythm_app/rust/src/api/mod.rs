//! Flutter API for rhythm-core.
//!
//! This module exposes the core lighting calculation functions
//! for use via flutter_rust_bridge.

pub mod dto;
pub mod curve;
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
    generate_curve_data,
    generate_curve_data_high_res,
    calculate_lighting,
    calculate_step_sequences,
    get_sun_position,
    is_morning,
    get_sun_times,
    generate_curve_data_with_sun_times,
    get_twilight_times,
};

// Runner functions
pub use runner::{
    calculate_action_result,
    create_runner_state,
    runner_state_to_json,
    runner_state_from_json,
    runner_add_room,
    runner_remove_room,
    runner_set_room_devices,
    runner_handle_action,
    runner_get_room,
    runner_get_room_ids,
    runner_get_enabled_rooms,
    runner_get_rooms_by_source,
    runner_set_room_disabled,
    runner_set_room_lights_on,
    runner_set_room_rhythm_enabled,
    runner_set_room_time_offset,
    runner_set_room_brightness_offset,
    runner_set_room_curve_config,
};

// Helper functions
pub use helpers::{
    get_hue_switch_prefixes,
    get_hue_oui_prefix,
    get_group_prefix,
    normalize_ieee,
    is_hue_ieee,
    endpoint_for_manufacturer,
    normalize_area_id,
    area_ids_match,
    group_name_for_area,
    is_light_entity,
    is_rhythm_group,
};

// Hue button functions
pub use hue::{
    map_hue_button_event,
    parse_hue_button_event_type,
};

// Hue registry functions
pub use hue_registry::{
    create_behavior_tracker,
    behavior_tracker_add,
    behavior_tracker_remove,
    behavior_tracker_is_configured,
    behavior_tracker_configured_devices,
    behavior_tracker_behavior_count,
    behavior_tracker_device_count,
    behavior_tracker_clear,
    behavior_tracker_to_json,
    behavior_tracker_from_json,
    create_switch_device,
    create_button,
    create_room,
};
