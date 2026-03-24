//! Tests for the Flutter API.

#[cfg(test)]
mod tests {
    use rhythm_core::config::{DEFAULT_MIN_COLOR_TEMP, DEFAULT_MAX_COLOR_TEMP};

    use crate::api::{
        // Curve functions
        generate_curve_data, calculate_lighting, calculate_step_sequences,
        get_sun_position, get_sun_times, generate_curve_data_with_sun_times,
        // Runner functions
        calculate_action_result,
        create_runner_state,
        runner_add_room, runner_remove_room, runner_set_room_devices,
        runner_handle_action, runner_get_enabled_rooms,
        runner_get_rooms_by_source, runner_set_room_disabled,
        runner_set_room_curve_config,
        // Helper functions
        get_group_prefix, normalize_ieee, is_hue_ieee,
        endpoint_for_manufacturer, normalize_area_id, area_ids_match,
        group_name_for_area, is_light_entity, is_rhythm_group,
        // Hue functions
        map_hue_button_event, parse_hue_button_event_type,
        // DTOs
        CurveConfigDto, RhythmActionDto, RoomDto, RoomSourceDto, RoomStateDto,
        HueButtonEventTypeDto, LightCommandType,
    };

    #[test]
    fn test_generate_curve_data() {
        let config = CurveConfigDto::default();
        let result = generate_curve_data(config, 12.0, 35.0, 172);

        assert_eq!(result.hours.len(), 24);
        assert_eq!(result.brightness.len(), 24);
        assert_eq!(result.kelvin.len(), 24);

        // At noon (index 12), brightness should be high
        assert!(result.brightness[12] > 90);
        assert!(result.kelvin[12] >= DEFAULT_MAX_COLOR_TEMP as i32 - 100);

        // At midnight (index 0), brightness should be low and color warm
        assert!(result.brightness[0] < 10);
        // Min color temp is 1200K from defaults, so kelvin should be close to that
        assert!(result.kelvin[0] <= DEFAULT_MIN_COLOR_TEMP as i32 + 100);
    }

    #[test]
    fn test_calculate_lighting() {
        let config = CurveConfigDto::default();
        let result = calculate_lighting(config, 12.0, 35.0, 172, 12.0);

        assert!(result.brightness > 90);
        assert!(result.kelvin >= DEFAULT_MAX_COLOR_TEMP as i32 - 100);
        assert!((result.sun_position - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_step_sequences() {
        let config = CurveConfigDto::default();
        // Use 9:00 AM - mid-morning between sunrise (~6:00) and noon (12:00)
        // At this time, both step_up (toward noon) and step_down (toward sunrise) are possible
        // Note: At noon, step_up would be empty (already at max brightness)
        //       At sunrise, step_down would be empty (already at min brightness)
        let result = calculate_step_sequences(config, 12.0, 35.0, 172, 9.0, 10);

        // Should have generated some steps in both directions
        assert!(!result.step_up.is_empty(), "step_up should not be empty at 9am");
        assert!(!result.step_down.is_empty(), "step_down should not be empty at 9am");

        // Step up should increase brightness (moving toward noon)
        if result.step_up.len() > 1 {
            assert!(result.step_up[1].brightness >= result.step_up[0].brightness);
        }

        // Step down should decrease brightness (moving toward sunrise)
        if result.step_down.len() > 1 {
            assert!(result.step_down[1].brightness <= result.step_down[0].brightness);
        }
    }

    #[test]
    fn test_sun_position() {
        let noon_position = get_sun_position(12.0, 12.0);
        let midnight_position = get_sun_position(12.0, 0.0);

        assert!((noon_position - 1.0).abs() < 0.1);
        assert!((midnight_position - (-1.0)).abs() < 0.1);
    }

    #[test]
    fn test_get_sun_times_nyc() {
        // NYC on summer solstice
        let times = get_sun_times(40.7128, -74.006, 2024, 6, 21, "America/New_York".to_string());

        // Sunrise should be around 5:25 AM
        assert!(
            times.sunrise > 5.2 && times.sunrise < 5.7,
            "NYC sunrise should be ~5:25, got {:.2}",
            times.sunrise
        );

        // Sunset should be around 8:31 PM
        assert!(
            times.sunset > 20.3 && times.sunset < 20.7,
            "NYC sunset should be ~20:31, got {:.2}",
            times.sunset
        );

        // Day length should be ~15 hours
        assert!(
            times.day_length > 14.5 && times.day_length < 15.5,
            "Day length should be ~15h, got {:.2}",
            times.day_length
        );
    }

    #[test]
    fn test_generate_curve_data_with_sun_times() {
        let config = CurveConfigDto::default();
        let result = generate_curve_data_with_sun_times(
            config,
            40.7128,
            -74.006,
            2024,
            6,
            21,
            "America/New_York".to_string(),
        );

        // Should have curve data
        assert_eq!(result.hours.len(), 24);
        assert_eq!(result.brightness.len(), 24);

        // Should have sun times populated
        assert!(result.solar.sunrise.is_some());
        assert!(result.solar.sunset.is_some());
        assert!(result.solar.day_length.is_some());

        let sunrise = result.solar.sunrise.unwrap();
        assert!(sunrise > 5.0 && sunrise < 6.0);
    }

    #[test]
    fn test_action_result_on_press_toggle() {
        let config = CurveConfigDto::default();

        // Starting with lights off
        let room_state = RoomStateDto {
            rhythm_enabled: false,
            lights_on: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
        };

        // OnPress should turn on
        let result = calculate_action_result(
            config.clone(), 12.0, 35.0, 172, 12.0,
            RhythmActionDto::OnPress, room_state
        );

        assert!(result.should_turn_on);
        assert!(!result.should_turn_off);
        assert!(result.new_state.lights_on);
        assert!(result.new_state.rhythm_enabled);
        assert!(result.lighting.is_some());

        // Now with lights on, OnPress should turn off
        let result2 = calculate_action_result(
            config, 12.0, 35.0, 172, 12.0,
            RhythmActionDto::OnPress, result.new_state
        );

        assert!(!result2.should_turn_on);
        assert!(result2.should_turn_off);
        assert!(!result2.new_state.lights_on);
        assert!(result2.lighting.is_none());
    }

    #[test]
    fn test_action_result_step_up() {
        let config = CurveConfigDto::default();

        let room_state = RoomStateDto {
            rhythm_enabled: true,
            lights_on: true,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
        };

        // Step up at 6 AM (morning, before peak)
        let result = calculate_action_result(
            config.clone(), 12.0, 35.0, 172, 6.0,
            RhythmActionDto::StepUp, room_state
        );

        // Should have positive time offset (moving toward brighter)
        assert!(result.new_state.time_offset_minutes > 0.0);
        assert!(result.lighting.is_some());

        // Brightness should be higher than at original position
        let original = calculate_lighting(config, 12.0, 35.0, 172, 6.0);
        assert!(result.lighting.unwrap().brightness > original.brightness);
    }

    #[test]
    fn test_action_result_dim_up() {
        let config = CurveConfigDto::default();

        let room_state = RoomStateDto {
            rhythm_enabled: true,
            lights_on: true,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
        };

        // Dim up at 6 AM
        let result = calculate_action_result(
            config.clone(), 12.0, 35.0, 172, 6.0,
            RhythmActionDto::DimUp, room_state
        );

        // Should have +10 brightness offset
        assert_eq!(result.new_state.brightness_offset, 10.0);
        // Time offset should be unchanged
        assert_eq!(result.new_state.time_offset_minutes, 0.0);
        assert!(result.lighting.is_some());

        // Brightness should be higher than curve value
        let original = calculate_lighting(config, 12.0, 35.0, 172, 6.0);
        assert!(result.lighting.unwrap().brightness > original.brightness);
    }

    #[test]
    fn test_action_result_dim_down() {
        let config = CurveConfigDto::default();

        let room_state = RoomStateDto {
            rhythm_enabled: true,
            lights_on: true,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
        };

        // Dim down at noon (high brightness)
        let result = calculate_action_result(
            config.clone(), 12.0, 35.0, 172, 12.0,
            RhythmActionDto::DimDown, room_state
        );

        // Should have -10 brightness offset
        assert_eq!(result.new_state.brightness_offset, -10.0);
        assert!(result.lighting.is_some());

        // Brightness should be lower than curve value
        let original = calculate_lighting(config, 12.0, 35.0, 172, 12.0);
        assert!(result.lighting.unwrap().brightness < original.brightness);
    }

    #[test]
    fn test_action_result_dim_clamps() {
        let config = CurveConfigDto::default();

        // Start with large positive offset
        let room_state = RoomStateDto {
            rhythm_enabled: true,
            lights_on: true,
            time_offset_minutes: 0.0,
            brightness_offset: 95.0,
        };

        let result = calculate_action_result(
            config, 12.0, 35.0, 172, 12.0,
            RhythmActionDto::DimUp, room_state
        );

        // Offset should clamp at 100
        assert!(result.new_state.brightness_offset <= 100.0);
        // Brightness should clamp at 100
        assert!(result.lighting.unwrap().brightness <= 100);
    }

    #[test]
    fn test_action_result_reset() {
        let config = CurveConfigDto::default();

        // Start with an offset
        let room_state = RoomStateDto {
            rhythm_enabled: true,
            lights_on: true,
            time_offset_minutes: 120.0, // 2 hours offset
            brightness_offset: 0.0,
        };

        let result = calculate_action_result(
            config, 12.0, 35.0, 172, 12.0,
            RhythmActionDto::Reset, room_state
        );

        // Both offsets should be reset to 0
        assert_eq!(result.new_state.time_offset_minutes, 0.0);
        assert_eq!(result.new_state.brightness_offset, 0.0);
        assert!(result.new_state.rhythm_enabled);
        assert!(result.new_state.lights_on);
    }

    #[test]
    fn test_runner_get_enabled_rooms() {
        let mut state = create_runner_state();

        state = runner_add_room(state, RoomDto {
            id: "room1".to_string(),
            name: "Room 1".to_string(),
            source: RoomSourceDto::Hue,
            device_ids: vec![],
            rhythm_enabled: false,
            disabled: false,
            lights_on: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            curve_config: None,
        });

        state = runner_add_room(state, RoomDto {
            id: "room2".to_string(),
            name: "Room 2".to_string(),
            source: RoomSourceDto::Hue,
            device_ids: vec![],
            rhythm_enabled: false,
            disabled: true, // This one is disabled
            lights_on: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            curve_config: None,
        });

        let enabled = runner_get_enabled_rooms(state);
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, "room1");
    }

    #[test]
    fn test_runner_get_rooms_by_source() {
        let mut state = create_runner_state();

        state = runner_add_room(state, RoomDto::with_source("hue1".to_string(), "Hue 1".to_string(), RoomSourceDto::Hue));
        state = runner_add_room(state, RoomDto::with_source("hue2".to_string(), "Hue 2".to_string(), RoomSourceDto::Hue));
        state = runner_add_room(state, RoomDto::with_source("ha1".to_string(), "HA 1".to_string(), RoomSourceDto::HomeAssistant));

        let hue_rooms = runner_get_rooms_by_source(state.clone(), RoomSourceDto::Hue);
        assert_eq!(hue_rooms.len(), 2);

        let ha_rooms = runner_get_rooms_by_source(state, RoomSourceDto::HomeAssistant);
        assert_eq!(ha_rooms.len(), 1);
    }

    #[test]
    fn test_runner_set_room_disabled() {
        let mut state = create_runner_state();
        state = runner_add_room(state, RoomDto::new("test".to_string(), "Test".to_string()));

        assert!(!state.rooms[0].disabled);

        state = runner_set_room_disabled(state, "test".to_string(), true);
        assert!(state.rooms[0].disabled);

        state = runner_set_room_disabled(state, "test".to_string(), false);
        assert!(!state.rooms[0].disabled);
    }

    #[test]
    fn test_runner_set_room_curve_config() {
        let mut state = create_runner_state();
        state = runner_add_room(state, RoomDto::new("test".to_string(), "Test".to_string()));

        assert!(state.rooms[0].curve_config.is_none());

        let config = CurveConfigDto {
            min_brightness: 10,
            max_brightness: 90,
            ..Default::default()
        };
        state = runner_set_room_curve_config(state, "test".to_string(), Some(config));

        assert!(state.rooms[0].curve_config.is_some());
        assert_eq!(state.rooms[0].curve_config.as_ref().unwrap().min_brightness, 10);

        // Set back to None
        state = runner_set_room_curve_config(state, "test".to_string(), None);
        assert!(state.rooms[0].curve_config.is_none());
    }

    #[test]
    fn test_runner_handle_action() {
        let config = CurveConfigDto::default();
        let mut state = create_runner_state();

        // Add a room with devices
        state = runner_add_room(state, RoomDto {
            id: "test_room".to_string(),
            name: "Test Room".to_string(),
            source: RoomSourceDto::Unknown,
            device_ids: vec!["light.test1".to_string(), "light.test2".to_string()],
            rhythm_enabled: false,
            disabled: false,
            lights_on: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            curve_config: None,
        });

        // Turn on via OnPress
        let result = runner_handle_action(
            state,
            config.clone(),
            12.0, 35.0, 172, 12.0,
            "test_room".to_string(),
            RhythmActionDto::OnPress,
        );

        assert!(result.state_changed);
        assert_eq!(result.commands.len(), 1); // One room-level command

        // Verify command is TurnOn with room_id as device_id
        let cmd = &result.commands[0];
        assert!(matches!(cmd.command_type, LightCommandType::TurnOn));
        assert_eq!(cmd.device_id, "test_room");
        assert_eq!(cmd.room_id, "test_room");
        assert!(cmd.brightness.is_some());
        assert!(cmd.kelvin.is_some());

        // Verify state updated
        let room = result.state.rooms.iter().find(|r| r.id == "test_room").unwrap();
        assert!(room.rhythm_enabled);
        assert!(room.lights_on);
    }

    #[test]
    fn test_runner_room_management() {
        let mut state = create_runner_state();

        // Add room
        state = runner_add_room(state, RoomDto::new("room1".to_string(), "Room 1".to_string()));
        assert_eq!(state.rooms.len(), 1);

        // Update devices
        state = runner_set_room_devices(state, "room1".to_string(), vec!["dev1".to_string(), "dev2".to_string()]);
        assert_eq!(state.rooms[0].device_ids.len(), 2);

        // Add another room
        state = runner_add_room(state, RoomDto::new("room2".to_string(), "Room 2".to_string()));
        assert_eq!(state.rooms.len(), 2);

        // Remove first room
        state = runner_remove_room(state, "room1".to_string());
        assert_eq!(state.rooms.len(), 1);
        assert_eq!(state.rooms[0].id, "room2");
    }

    // ========================================================================
    // Device Module Tests
    // ========================================================================

    #[test]
    fn test_get_group_prefix() {
        assert_eq!(get_group_prefix(), "Rhythm_");
    }

    #[test]
    fn test_normalize_ieee_ffi() {
        assert_eq!(
            normalize_ieee("00:17:88:01:09:AB:CD:EF".to_string()),
            "00:17:88:01:09:ab:cd:ef"
        );
        assert_eq!(
            normalize_ieee("00-17-88-01-09-AB-CD-EF".to_string()),
            "00:17:88:01:09:ab:cd:ef"
        );
    }

    #[test]
    fn test_is_hue_ieee_ffi() {
        assert!(is_hue_ieee("00:17:88:01:09:AB:CD:EF".to_string()));
        assert!(!is_hue_ieee("00:11:22:33:44:55:66:77".to_string()));
    }

    #[test]
    fn test_endpoint_for_manufacturer_ffi() {
        assert_eq!(endpoint_for_manufacturer("Signify Netherlands B.V.".to_string(), "LCT015".to_string()), 11);
        assert_eq!(endpoint_for_manufacturer("IKEA of Sweden".to_string(), "TRADFRI bulb".to_string()), 1);
        assert_eq!(endpoint_for_manufacturer("Unknown".to_string(), "Unknown".to_string()), 11);
    }

    #[test]
    fn test_normalize_area_id_ffi() {
        assert_eq!(normalize_area_id("Living Room".to_string()), "living_room");
        assert_eq!(normalize_area_id("living-room".to_string()), "living_room");
    }

    #[test]
    fn test_area_ids_match_ffi() {
        assert!(area_ids_match("Living Room".to_string(), "living_room".to_string()));
        assert!(!area_ids_match("living_room".to_string(), "bedroom".to_string()));
    }

    #[test]
    fn test_group_name_for_area_ffi() {
        assert_eq!(group_name_for_area("Living Room".to_string()), "Rhythm_Living_Room");
    }

    #[test]
    fn test_is_light_entity_ffi() {
        assert!(is_light_entity("light.living_room".to_string()));
        assert!(!is_light_entity("switch.living_room".to_string()));
    }

    #[test]
    fn test_is_rhythm_group_ffi() {
        assert!(is_rhythm_group("Rhythm_Living_Room".to_string()));
        assert!(!is_rhythm_group("User_Group".to_string()));
    }

    // ========================================================================
    // Hue Button Event Tests
    // ========================================================================

    #[test]
    fn test_parse_hue_button_event_type() {
        assert_eq!(
            parse_hue_button_event_type("initial_press".to_string()),
            Some(HueButtonEventTypeDto::InitialPress)
        );
        assert_eq!(
            parse_hue_button_event_type("repeat".to_string()),
            Some(HueButtonEventTypeDto::Repeat)
        );
        assert_eq!(
            parse_hue_button_event_type("short_release".to_string()),
            Some(HueButtonEventTypeDto::ShortRelease)
        );
        assert_eq!(
            parse_hue_button_event_type("long_release".to_string()),
            Some(HueButtonEventTypeDto::LongRelease)
        );
        assert_eq!(
            parse_hue_button_event_type("long_press".to_string()),
            Some(HueButtonEventTypeDto::LongPress)
        );
        assert_eq!(parse_hue_button_event_type("unknown".to_string()), None);
    }

    #[test]
    fn test_map_hue_button_event_on_button() {
        // Button 1 (On) - short press toggles
        assert_eq!(
            map_hue_button_event(1, HueButtonEventTypeDto::ShortRelease),
            Some(RhythmActionDto::OnPress)
        );

        // Initial press is ignored
        assert_eq!(
            map_hue_button_event(1, HueButtonEventTypeDto::InitialPress),
            None
        );
    }

    #[test]
    fn test_map_hue_button_event_dim_up() {
        // Button 2 (Dim Up) - short press dims up (brightness only)
        assert_eq!(
            map_hue_button_event(2, HueButtonEventTypeDto::ShortRelease),
            Some(RhythmActionDto::DimUp)
        );

        // Hold steps up along curve
        assert_eq!(
            map_hue_button_event(2, HueButtonEventTypeDto::Repeat),
            Some(RhythmActionDto::StepUp)
        );
    }

    #[test]
    fn test_map_hue_button_event_dim_down() {
        // Button 3 (Dim Down) - short press dims down (brightness only)
        assert_eq!(
            map_hue_button_event(3, HueButtonEventTypeDto::ShortRelease),
            Some(RhythmActionDto::DimDown)
        );

        // Hold steps down along curve
        assert_eq!(
            map_hue_button_event(3, HueButtonEventTypeDto::Repeat),
            Some(RhythmActionDto::StepDown)
        );
    }

    #[test]
    fn test_map_hue_button_event_off_button() {
        // Button 4 (Off) - short press resets
        assert_eq!(
            map_hue_button_event(4, HueButtonEventTypeDto::ShortRelease),
            Some(RhythmActionDto::Reset)
        );

        // Long press disables rhythm
        assert_eq!(
            map_hue_button_event(4, HueButtonEventTypeDto::LongRelease),
            Some(RhythmActionDto::RhythmOff)
        );
    }

    #[test]
    fn test_map_hue_button_event_invalid() {
        // Invalid button numbers return None
        assert_eq!(
            map_hue_button_event(0, HueButtonEventTypeDto::ShortRelease),
            None
        );
        assert_eq!(
            map_hue_button_event(5, HueButtonEventTypeDto::ShortRelease),
            None
        );
    }
}
