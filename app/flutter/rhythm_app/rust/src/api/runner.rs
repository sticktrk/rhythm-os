//! Runner state management functions.

use rhythm_core::{
    kelvin_to_mireds, process_action, ActionResult, LightProfile, RoomAction, RoomActionState,
    SolarTime,
};

use super::dto::{
    ActionResultDto, CurveConfigDto, LightCommandDto, LightCommandType, LightingValuesDto, RgbDto,
    RhythmActionDto, RoomDto, RoomSourceDto, RoomStateDto, RunnerActionResultDto, RunnerStateDto,
    XyDto,
};

/// Process an action and calculate the resulting lighting state.
///
/// Delegates to the shared `process_action` kernel in rhythm-core,
/// then converts the result to FFI-compatible DTOs.
pub fn calculate_action_result(
    config: CurveConfigDto,
    solar_noon_hour: f64,
    latitude: f64,
    day_of_year: i32,
    current_hour: f64,
    action: RhythmActionDto,
    room_state: RoomStateDto,
) -> ActionResultDto {
    let profile = LightProfile::new(config.into());
    let solar = SolarTime::new(solar_noon_hour as f32, latitude as f32, day_of_year as u32);

    let core_action = match action {
        RhythmActionDto::OnPress => RoomAction::OnPress,
        RhythmActionDto::OffPress => RoomAction::OffPress,
        RhythmActionDto::Reset => RoomAction::Reset,
        RhythmActionDto::RhythmOn => RoomAction::RhythmOn,
        RhythmActionDto::RhythmOff => RoomAction::RhythmOff,
        RhythmActionDto::StepUp => RoomAction::StepUp,
        RhythmActionDto::StepDown => RoomAction::StepDown,
        RhythmActionDto::DimUp => RoomAction::DimUp,
        RhythmActionDto::DimDown => RoomAction::DimDown,
    };

    let core_state = RoomActionState {
        rhythm_enabled: room_state.rhythm_enabled,
        lights_on: room_state.lights_on,
        time_offset_minutes: room_state.time_offset_minutes as f32,
        brightness_offset: room_state.brightness_offset as f32,
    };

    let result: ActionResult = process_action(
        &profile,
        solar,
        current_hour as f32,
        core_action,
        &core_state,
    );

    ActionResultDto {
        lighting: result.lighting.map(|v| LightingValuesDto {
            kelvin: v.kelvin as i32,
            mireds: kelvin_to_mireds(v.kelvin) as i32,
            brightness: v.brightness as i32,
            rgb: RgbDto {
                r: v.rgb.r as i32,
                g: v.rgb.g as i32,
                b: v.rgb.b as i32,
            },
            xy: XyDto {
                x: v.xy.x as f64,
                y: v.xy.y as f64,
            },
            solar_time: v.solar_time as f64,
            sun_position: v.sun_position as f64,
        }),
        new_state: RoomStateDto {
            rhythm_enabled: result.new_state.rhythm_enabled,
            lights_on: result.new_state.lights_on,
            time_offset_minutes: result.new_state.time_offset_minutes as f64,
            brightness_offset: result.new_state.brightness_offset as f64,
        },
        should_turn_off: result.should_turn_off,
        should_turn_on: result.should_turn_on,
        state_changed: result.state_changed,
    }
}

// ============================================================================
// Runner State Management Functions
// ============================================================================

/// Create a new empty runner state.
pub fn create_runner_state() -> RunnerStateDto {
    RunnerStateDto::default()
}

/// Add a room to the runner state.
pub fn runner_add_room(state: RunnerStateDto, room: RoomDto) -> RunnerStateDto {
    let mut new_state = state;
    // Remove existing room with same ID if present
    new_state.rooms.retain(|r| r.id != room.id);
    new_state.rooms.push(room);
    new_state
}

/// Remove a room from the runner state.
pub fn runner_remove_room(state: RunnerStateDto, room_id: String) -> RunnerStateDto {
    let mut new_state = state;
    new_state.rooms.retain(|r| r.id != room_id);
    new_state
}

/// Update device IDs for a room.
pub fn runner_set_room_devices(
    state: RunnerStateDto,
    room_id: String,
    device_ids: Vec<String>,
) -> RunnerStateDto {
    let mut new_state = state;
    if let Some(room) = new_state.rooms.iter_mut().find(|r| r.id == room_id) {
        room.device_ids = device_ids;
    }
    new_state
}

/// Handle an action for a specific room.
///
/// This is the main function for processing user actions. It takes the current
/// runner state, processes the action for the specified room, and returns
/// the new state along with commands to execute.
pub fn runner_handle_action(
    state: RunnerStateDto,
    config: CurveConfigDto,
    solar_noon_hour: f64,
    latitude: f64,
    day_of_year: i32,
    current_hour: f64,
    room_id: String,
    action: RhythmActionDto,
) -> RunnerActionResultDto {
    let mut new_state = state;

    // Find the room
    let room_idx = new_state.rooms.iter().position(|r| r.id == room_id);

    let Some(room_idx) = room_idx else {
        // Room not found, return unchanged
        return RunnerActionResultDto {
            state: new_state,
            commands: Vec::new(),
            state_changed: false,
        };
    };

    let room = &new_state.rooms[room_idx];

    // Build legacy room state for calculation
    let room_state = RoomStateDto {
        rhythm_enabled: room.rhythm_enabled,
        lights_on: room.lights_on,
        time_offset_minutes: room.time_offset_minutes,
        brightness_offset: room.brightness_offset,
    };

    // Calculate action result
    let result = calculate_action_result(
        config,
        solar_noon_hour,
        latitude,
        day_of_year,
        current_hour,
        action,
        room_state,
    );

    // Update room state
    let room = &mut new_state.rooms[room_idx];
    room.rhythm_enabled = result.new_state.rhythm_enabled;
    room.lights_on = result.new_state.lights_on;
    room.time_offset_minutes = result.new_state.time_offset_minutes;
    room.brightness_offset = result.new_state.brightness_offset;

    // Generate a single room-level command (not per-device)
    let mut commands = Vec::new();
    let room_id_clone = room.id.clone();

    if result.should_turn_off {
        commands.push(LightCommandDto {
            device_id: room_id_clone.clone(),
            room_id: room_id_clone,
            command_type: LightCommandType::TurnOff,
            brightness: None,
            kelvin: None,
        });
    } else if result.should_turn_on {
        if let Some(ref lighting) = result.lighting {
            commands.push(LightCommandDto {
                device_id: room_id_clone.clone(),
                room_id: room_id_clone,
                command_type: LightCommandType::TurnOn,
                brightness: Some(lighting.brightness),
                kelvin: Some(lighting.kelvin),
            });
        }
    }

    RunnerActionResultDto {
        state: new_state,
        commands,
        state_changed: result.state_changed,
    }
}

/// Get a room by ID from the runner state.
pub fn runner_get_room(state: RunnerStateDto, room_id: String) -> Option<RoomDto> {
    state.rooms.into_iter().find(|r| r.id == room_id)
}

/// Get all room IDs from the runner state.
pub fn runner_get_room_ids(state: RunnerStateDto) -> Vec<String> {
    state.rooms.iter().map(|r| r.id.clone()).collect()
}

/// Get rooms that are not disabled (for kiosk mode).
///
/// Returns a copy of all rooms that have `disabled = false`.
pub fn runner_get_enabled_rooms(state: RunnerStateDto) -> Vec<RoomDto> {
    state.rooms.into_iter().filter(|r| !r.disabled).collect()
}

/// Get rooms by source.
///
/// Returns a copy of all rooms with the specified source.
pub fn runner_get_rooms_by_source(state: RunnerStateDto, source: RoomSourceDto) -> Vec<RoomDto> {
    state
        .rooms
        .into_iter()
        .filter(|r| r.source == source)
        .collect()
}

/// Set room disabled state.
///
/// Returns a new state with the room's disabled flag updated.
pub fn runner_set_room_disabled(
    state: RunnerStateDto,
    room_id: String,
    disabled: bool,
) -> RunnerStateDto {
    let mut new_state = state;
    if let Some(room) = new_state.rooms.iter_mut().find(|r| r.id == room_id) {
        room.disabled = disabled;
    }
    new_state
}

/// Set room lights_on state for syncing actual device state.
///
/// This is used to sync the Rust state with the actual light state
/// reported by external systems (e.g., Hue bridge).
///
/// Returns a new state with the room's lights_on flag updated.
pub fn runner_set_room_lights_on(
    state: RunnerStateDto,
    room_id: String,
    lights_on: bool,
) -> RunnerStateDto {
    let mut new_state = state;
    if let Some(room) = new_state.rooms.iter_mut().find(|r| r.id == room_id) {
        room.lights_on = lights_on;
    }
    new_state
}

/// Set room rhythm_enabled state.
///
/// Returns a new state with the room's rhythm_enabled flag updated.
pub fn runner_set_room_rhythm_enabled(
    state: RunnerStateDto,
    room_id: String,
    rhythm_enabled: bool,
) -> RunnerStateDto {
    let mut new_state = state;
    if let Some(room) = new_state.rooms.iter_mut().find(|r| r.id == room_id) {
        room.rhythm_enabled = rhythm_enabled;
    }
    new_state
}

/// Set room time offset in minutes.
///
/// This is used to pin a room to a specific time position on the curve.
/// A positive offset moves forward in time (brighter in the morning),
/// a negative offset moves backward.
///
/// Returns a new state with the room's time_offset_minutes updated.
pub fn runner_set_room_time_offset(
    state: RunnerStateDto,
    room_id: String,
    time_offset_minutes: f64,
) -> RunnerStateDto {
    let mut new_state = state;
    if let Some(room) = new_state.rooms.iter_mut().find(|r| r.id == room_id) {
        room.time_offset_minutes = time_offset_minutes;
    }
    new_state
}

/// Set room brightness offset.
///
/// This is used to adjust a room's brightness relative to the curve.
/// A positive offset brightens, a negative offset dims.
///
/// Returns a new state with the room's brightness_offset updated.
pub fn runner_set_room_brightness_offset(
    state: RunnerStateDto,
    room_id: String,
    brightness_offset: f64,
) -> RunnerStateDto {
    let mut new_state = state;
    if let Some(room) = new_state.rooms.iter_mut().find(|r| r.id == room_id) {
        room.brightness_offset = brightness_offset;
    }
    new_state
}

/// Set per-room curve config.
///
/// Pass `None` to use the global configuration.
/// Returns a new state with the room's curve_config updated.
pub fn runner_set_room_curve_config(
    state: RunnerStateDto,
    room_id: String,
    config: Option<CurveConfigDto>,
) -> RunnerStateDto {
    let mut new_state = state;
    if let Some(room) = new_state.rooms.iter_mut().find(|r| r.id == room_id) {
        room.curve_config = config;
    }
    new_state
}
