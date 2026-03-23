//! Runner state management functions.

use rhythm_core::{CurveConfig, CurveContext, RhythmCurveModule, SolarTime, StepAction, LightCurveModule};

use super::curve::calculate_lighting;
use super::dto::{
    ActionResultDto, CurveConfigDto, LightCommandDto, LightCommandType, RhythmActionDto,
    RoomDto, RoomSourceDto, RoomStateDto, RunnerActionResultDto, RunnerStateDto,
};

/// Process an action and calculate the resulting lighting state.
///
/// This is the main function for the RhythmRunner. It processes a button
/// action given the current room state and returns the new lighting values
/// and updated state.
///
/// # Arguments
///
/// * `config` - Curve configuration parameters
/// * `solar_noon_hour` - Hour of solar noon (0-24, local time)
/// * `latitude` - Latitude in degrees
/// * `day_of_year` - Day of year (1-365)
/// * `current_hour` - Current time in hours (0-24)
/// * `action` - The action to process
/// * `room_state` - Current state of the room
///
/// # Returns
///
/// ActionResultDto with new lighting values and updated room state.
pub fn calculate_action_result(
    config: CurveConfigDto,
    solar_noon_hour: f64,
    latitude: f64,
    day_of_year: i32,
    current_hour: f64,
    action: RhythmActionDto,
    room_state: RoomStateDto,
) -> ActionResultDto {
    let config_rust: CurveConfig = config.clone().into();
    let solar = SolarTime::new(solar_noon_hour as f32, latitude as f32, day_of_year as u32);
    let module = RhythmCurveModule::new(config_rust);

    // Calculate effective hour with current offset
    let effective_hour = current_hour + (room_state.time_offset_minutes / 60.0);
    let effective_hour = ((effective_hour % 24.0) + 24.0) % 24.0;

    match action {
        RhythmActionDto::OnPress => {
            // Toggle: if lights on, turn off. If lights off, turn on with rhythm.
            if room_state.lights_on {
                ActionResultDto {
                    lighting: None,
                    new_state: RoomStateDto {
                        rhythm_enabled: room_state.rhythm_enabled,
                        lights_on: false,
                        time_offset_minutes: room_state.time_offset_minutes,
                        brightness_offset: room_state.brightness_offset,
                    },
                    should_turn_off: true,
                    should_turn_on: false,
                    state_changed: true,
                }
            } else {
                // Turn on with rhythm at current time (reset offset)
                let values = calculate_lighting(config.clone(), solar_noon_hour, latitude, day_of_year, current_hour);
                ActionResultDto {
                    lighting: Some(values),
                    new_state: RoomStateDto {
                        rhythm_enabled: true,
                        lights_on: true,
                        time_offset_minutes: 0.0,
                        brightness_offset: 0.0,
                    },
                    should_turn_off: false,
                    should_turn_on: true,
                    state_changed: true,
                }
            }
        }

        RhythmActionDto::OffPress => {
            // Turn off lights
            ActionResultDto {
                lighting: None,
                new_state: RoomStateDto {
                    rhythm_enabled: room_state.rhythm_enabled,
                    lights_on: false,
                    time_offset_minutes: room_state.time_offset_minutes,
                    brightness_offset: room_state.brightness_offset,
                },
                should_turn_off: true,
                should_turn_on: false,
                state_changed: room_state.lights_on,
            }
        }

        RhythmActionDto::Reset => {
            // Reset to current time position (clear both offsets)
            let values = calculate_lighting(config, solar_noon_hour, latitude, day_of_year, current_hour);
            ActionResultDto {
                lighting: Some(values),
                new_state: RoomStateDto {
                    rhythm_enabled: true,
                    lights_on: true,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                },
                should_turn_off: false,
                should_turn_on: true,
                state_changed: true,
            }
        }

        RhythmActionDto::RhythmOn => {
            // Enable rhythm timer participation only (no light commands)
            ActionResultDto {
                lighting: None,
                new_state: RoomStateDto {
                    rhythm_enabled: true,
                    lights_on: room_state.lights_on,
                    time_offset_minutes: room_state.time_offset_minutes,
                    brightness_offset: room_state.brightness_offset,
                },
                should_turn_off: false,
                should_turn_on: false,
                state_changed: !room_state.rhythm_enabled,
            }
        }

        RhythmActionDto::RhythmOff => {
            // Disable rhythm mode only (lights unchanged)
            ActionResultDto {
                lighting: None,
                new_state: RoomStateDto {
                    rhythm_enabled: false,
                    lights_on: room_state.lights_on,
                    time_offset_minutes: room_state.time_offset_minutes,
                    brightness_offset: room_state.brightness_offset,
                },
                should_turn_off: false,
                should_turn_on: false,
                state_changed: room_state.rhythm_enabled,
            }
        }

        RhythmActionDto::StepUp | RhythmActionDto::StepDown => {
            // Calculate step along curve
            let step_action = match action {
                RhythmActionDto::StepUp => StepAction::Brighten,
                _ => StepAction::Dim,
            };

            let ctx = CurveContext::new(effective_hour as f32, solar, None);
            let step_result = module.calculate_step(&ctx, step_action);

            // Calculate new time offset
            let new_offset = room_state.time_offset_minutes + (step_result.time_offset_minutes as f64);

            // Get lighting values at new position
            let new_effective_hour = current_hour + (new_offset / 60.0);
            let new_effective_hour = ((new_effective_hour % 24.0) + 24.0) % 24.0;
            let values = calculate_lighting(config, solar_noon_hour, latitude, day_of_year, new_effective_hour);

            ActionResultDto {
                lighting: Some(values),
                new_state: RoomStateDto {
                    rhythm_enabled: room_state.rhythm_enabled,
                    lights_on: true,
                    time_offset_minutes: new_offset,
                    brightness_offset: room_state.brightness_offset,
                },
                should_turn_off: false,
                should_turn_on: true,
                state_changed: true,
            }
        }

        RhythmActionDto::DimUp | RhythmActionDto::DimDown => {
            // Adjust brightness only (no color temp change)
            let delta = if action == RhythmActionDto::DimUp { 10.0 } else { -10.0 };
            let new_brightness_offset = (room_state.brightness_offset + delta).clamp(-100.0, 100.0);

            // Get lighting values at current position, then apply brightness offset
            let mut values = calculate_lighting(config, solar_noon_hour, latitude, day_of_year, effective_hour);
            let adjusted_brightness = (values.brightness as f64 + new_brightness_offset).clamp(1.0, 100.0);
            values.brightness = adjusted_brightness as i32;

            ActionResultDto {
                lighting: Some(values),
                new_state: RoomStateDto {
                    rhythm_enabled: room_state.rhythm_enabled,
                    lights_on: true,
                    time_offset_minutes: room_state.time_offset_minutes,
                    brightness_offset: new_brightness_offset,
                },
                should_turn_off: false,
                should_turn_on: true,
                state_changed: true,
            }
        }
    }
}

// ============================================================================
// Runner State Management Functions
// ============================================================================

/// Create a new empty runner state.
pub fn create_runner_state() -> RunnerStateDto {
    RunnerStateDto::default()
}

/// Serialize runner state to JSON string for persistence.
pub fn runner_state_to_json(state: RunnerStateDto) -> String {
    // Manual JSON serialization to avoid serde dependency issues with FRB
    let rooms_json: Vec<String> = state.rooms.iter().map(|room| {
        let device_ids_json: Vec<String> = room.device_ids.iter()
            .map(|id| format!("\"{}\"", id.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect();
        let source_str = room_source_to_string(room.source);
        let curve_config_json = room.curve_config.as_ref()
            .map(curve_config_to_json)
            .unwrap_or_else(|| "null".to_string());
        format!(
            r#"{{"id":"{}","name":"{}","source":"{}","device_ids":[{}],"rhythm_enabled":{},"disabled":{},"lights_on":{},"time_offset_minutes":{},"brightness_offset":{},"curve_config":{}}}"#,
            room.id.replace('\\', "\\\\").replace('"', "\\\""),
            room.name.replace('\\', "\\\\").replace('"', "\\\""),
            source_str,
            device_ids_json.join(","),
            room.rhythm_enabled,
            room.disabled,
            room.lights_on,
            room.time_offset_minutes,
            room.brightness_offset,
            curve_config_json
        )
    }).collect();

    format!(r#"{{"rooms":[{}]}}"#, rooms_json.join(","))
}

fn room_source_to_string(source: RoomSourceDto) -> &'static str {
    match source {
        RoomSourceDto::Unknown => "unknown",
        RoomSourceDto::Hue => "hue",
        RoomSourceDto::HomeAssistant => "home_assistant",
        RoomSourceDto::Esp32 => "esp32",
    }
}

fn room_source_from_string(s: &str) -> RoomSourceDto {
    match s.to_lowercase().as_str() {
        "hue" => RoomSourceDto::Hue,
        "home_assistant" | "homeassistant" => RoomSourceDto::HomeAssistant,
        "esp32" => RoomSourceDto::Esp32,
        _ => RoomSourceDto::Unknown,
    }
}

fn curve_config_to_json(config: &CurveConfigDto) -> String {
    format!(
        r#"{{"min_color_temp":{},"max_color_temp":{},"min_brightness":{},"max_brightness":{},"width_left_bri":{},"width_right_bri":{},"width_left_cct":{},"width_right_cct":{},"shape_p":{},"max_dim_steps":{}}}"#,
        config.min_color_temp,
        config.max_color_temp,
        config.min_brightness,
        config.max_brightness,
        config.width_left_bri,
        config.width_right_bri,
        config.width_left_cct,
        config.width_right_cct,
        config.shape_p,
        config.max_dim_steps
    )
}

/// Deserialize runner state from JSON string.
/// Returns None if the JSON is invalid.
pub fn runner_state_from_json(json: String) -> Option<RunnerStateDto> {
    // Simple JSON parsing - this is intentionally basic
    // In production, you might want to use a proper JSON parser
    parse_runner_state_json(&json)
}

// Simple JSON parser for RunnerStateDto
fn parse_runner_state_json(json: &str) -> Option<RunnerStateDto> {
    let json = json.trim();
    if !json.starts_with('{') || !json.ends_with('}') {
        return None;
    }

    // Find "rooms" array
    let rooms_start = json.find("\"rooms\"")?;
    let array_start = json[rooms_start..].find('[')? + rooms_start;
    let array_end = find_matching_bracket(json, array_start)?;

    let rooms_str = &json[array_start + 1..array_end];
    let rooms = parse_rooms_array(rooms_str);

    Some(RunnerStateDto { rooms })
}

fn find_matching_bracket(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let open = bytes[start];
    let close = match open {
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };

    let mut depth = 1;
    let mut i = start + 1;
    let mut in_string = false;
    let mut escape = false;

    while i < bytes.len() && depth > 0 {
        if escape {
            escape = false;
        } else if bytes[i] == b'\\' {
            escape = true;
        } else if bytes[i] == b'"' {
            in_string = !in_string;
        } else if !in_string {
            if bytes[i] == open {
                depth += 1;
            } else if bytes[i] == close {
                depth -= 1;
            }
        }
        i += 1;
    }

    if depth == 0 { Some(i - 1) } else { None }
}

fn parse_rooms_array(s: &str) -> Vec<RoomDto> {
    let mut rooms = Vec::new();
    let s = s.trim();
    if s.is_empty() {
        return rooms;
    }

    let mut i = 0;
    while i < s.len() {
        // Find start of next object
        if let Some(obj_start) = s[i..].find('{') {
            let obj_start = i + obj_start;
            if let Some(obj_end) = find_matching_bracket(s, obj_start) {
                let obj_str = &s[obj_start..=obj_end];
                if let Some(room) = parse_room_object(obj_str) {
                    rooms.push(room);
                }
                i = obj_end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    rooms
}

fn parse_room_object(s: &str) -> Option<RoomDto> {
    let id = extract_string_field(s, "id")?;
    let name = extract_string_field(s, "name").unwrap_or_else(|| id.clone());
    let source_str = extract_string_field(s, "source").unwrap_or_else(|| "unknown".to_string());
    let source = room_source_from_string(&source_str);
    let device_ids = extract_string_array_field(s, "device_ids").unwrap_or_default();
    let rhythm_enabled = extract_bool_field(s, "rhythm_enabled").unwrap_or(false);
    let disabled = extract_bool_field(s, "disabled").unwrap_or(false);
    let lights_on = extract_bool_field(s, "lights_on").unwrap_or(false);
    let time_offset_minutes = extract_number_field(s, "time_offset_minutes").unwrap_or(0.0);
    let brightness_offset = extract_number_field(s, "brightness_offset").unwrap_or(0.0);
    let curve_config = extract_curve_config_field(s, "curve_config");

    Some(RoomDto {
        id,
        name,
        source,
        device_ids,
        rhythm_enabled,
        disabled,
        lights_on,
        time_offset_minutes,
        brightness_offset,
        curve_config,
    })
}

fn extract_curve_config_field(s: &str, field: &str) -> Option<CurveConfigDto> {
    let pattern = format!("\"{}\"", field);
    let field_start = s.find(&pattern)?;
    let after_field = &s[field_start + pattern.len()..];
    let colon_pos = after_field.find(':')?;
    let after_colon = after_field[colon_pos + 1..].trim_start();

    // Check if it's null
    if after_colon.starts_with("null") {
        return None;
    }

    // Check if it's an object
    if !after_colon.starts_with('{') {
        return None;
    }

    let obj_start = field_start + pattern.len() + colon_pos + 1 + (after_field[colon_pos + 1..].len() - after_colon.len());
    let obj_end = find_matching_bracket(s, obj_start)?;
    let config_str = &s[obj_start..=obj_end];

    parse_curve_config_object(config_str)
}

fn parse_curve_config_object(s: &str) -> Option<CurveConfigDto> {
    // Use rhythm-core defaults for missing fields
    let defaults = CurveConfigDto::default();
    Some(CurveConfigDto {
        min_color_temp: extract_number_field(s, "min_color_temp")
            .map(|v| v as i32)
            .unwrap_or(defaults.min_color_temp),
        max_color_temp: extract_number_field(s, "max_color_temp")
            .map(|v| v as i32)
            .unwrap_or(defaults.max_color_temp),
        min_brightness: extract_number_field(s, "min_brightness")
            .map(|v| v as i32)
            .unwrap_or(defaults.min_brightness),
        max_brightness: extract_number_field(s, "max_brightness")
            .map(|v| v as i32)
            .unwrap_or(defaults.max_brightness),
        width_left_bri: extract_number_field(s, "width_left_bri")
            .unwrap_or(defaults.width_left_bri),
        width_right_bri: extract_number_field(s, "width_right_bri")
            .unwrap_or(defaults.width_right_bri),
        width_left_cct: extract_number_field(s, "width_left_cct")
            .unwrap_or(defaults.width_left_cct),
        width_right_cct: extract_number_field(s, "width_right_cct")
            .unwrap_or(defaults.width_right_cct),
        shape_p: extract_number_field(s, "shape_p")
            .unwrap_or(defaults.shape_p),
        max_dim_steps: extract_number_field(s, "max_dim_steps")
            .map(|v| v as i32)
            .unwrap_or(defaults.max_dim_steps),
    })
}

fn extract_string_field(s: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{}\"", field);
    let field_start = s.find(&pattern)?;
    let colon_pos = s[field_start + pattern.len()..].find(':')? + field_start + pattern.len();
    let value_start = s[colon_pos + 1..].find('"')? + colon_pos + 2;
    let value_end = find_string_end(s, value_start)?;
    Some(unescape_json_string(&s[value_start..value_end]))
}

fn find_string_end(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'"' {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

fn unescape_json_string(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                chars.next();
                match next {
                    '"' => result.push('"'),
                    '\\' => result.push('\\'),
                    'n' => result.push('\n'),
                    't' => result.push('\t'),
                    'r' => result.push('\r'),
                    _ => {
                        result.push('\\');
                        result.push(next);
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn extract_string_array_field(s: &str, field: &str) -> Option<Vec<String>> {
    let pattern = format!("\"{}\"", field);
    let field_start = s.find(&pattern)?;
    let array_start = s[field_start..].find('[')? + field_start;
    let array_end = find_matching_bracket(s, array_start)?;

    let array_content = &s[array_start + 1..array_end];
    let mut result = Vec::new();

    let mut i = 0;
    while i < array_content.len() {
        if let Some(str_start) = array_content[i..].find('"') {
            let str_start = i + str_start + 1;
            if let Some(str_end) = find_string_end(array_content, str_start) {
                result.push(unescape_json_string(&array_content[str_start..str_end]));
                i = str_end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    Some(result)
}

fn extract_bool_field(s: &str, field: &str) -> Option<bool> {
    let pattern = format!("\"{}\"", field);
    let field_start = s.find(&pattern)?;
    let after_colon = &s[field_start + pattern.len()..];
    let colon_pos = after_colon.find(':')?;
    let value_part = after_colon[colon_pos + 1..].trim_start();

    if value_part.starts_with("true") {
        Some(true)
    } else if value_part.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn extract_number_field(s: &str, field: &str) -> Option<f64> {
    let pattern = format!("\"{}\"", field);
    let field_start = s.find(&pattern)?;
    let after_colon = &s[field_start + pattern.len()..];
    let colon_pos = after_colon.find(':')?;
    let value_part = after_colon[colon_pos + 1..].trim_start();

    // Find end of number
    let end = value_part.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-' && c != '+' && c != 'e' && c != 'E')
        .unwrap_or(value_part.len());

    value_part[..end].parse().ok()
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
    state.rooms.into_iter().filter(|r| r.source == source).collect()
}

/// Set room disabled state.
///
/// Returns a new state with the room's disabled flag updated.
pub fn runner_set_room_disabled(state: RunnerStateDto, room_id: String, disabled: bool) -> RunnerStateDto {
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
pub fn runner_set_room_lights_on(state: RunnerStateDto, room_id: String, lights_on: bool) -> RunnerStateDto {
    let mut new_state = state;
    if let Some(room) = new_state.rooms.iter_mut().find(|r| r.id == room_id) {
        room.lights_on = lights_on;
    }
    new_state
}

/// Set room rhythm_enabled state.
///
/// Returns a new state with the room's rhythm_enabled flag updated.
pub fn runner_set_room_rhythm_enabled(state: RunnerStateDto, room_id: String, rhythm_enabled: bool) -> RunnerStateDto {
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
pub fn runner_set_room_time_offset(state: RunnerStateDto, room_id: String, time_offset_minutes: f64) -> RunnerStateDto {
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
pub fn runner_set_room_brightness_offset(state: RunnerStateDto, room_id: String, brightness_offset: f64) -> RunnerStateDto {
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
pub fn runner_set_room_curve_config(state: RunnerStateDto, room_id: String, config: Option<CurveConfigDto>) -> RunnerStateDto {
    let mut new_state = state;
    if let Some(room) = new_state.rooms.iter_mut().find(|r| r.id == room_id) {
        room.curve_config = config;
    }
    new_state
}
