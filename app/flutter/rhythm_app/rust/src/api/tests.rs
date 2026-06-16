//! Tests for the Flutter API.

#[cfg(test)]
mod tests {
    use crate::api::{
        area_ids_match,
        calculate_lighting_with_sun_times,
        calculate_step_sequences_with_sun_times,
        endpoint_for_manufacturer,
        // Curve functions
        generate_curve_data_high_res_with_sun_times,
        generate_curve_data_with_sun_times,
        // Helper functions
        get_group_prefix,
        get_sun_position,
        get_sun_times,
        group_name_for_area,
        is_hue_ieee,
        is_light_entity,
        is_rhythm_group,
        // Hue functions
        map_hue_button_event,
        normalize_area_id,
        normalize_ieee,
        parse_hue_button_event_type,
        // DTOs
        CurveConfigDto,
        HueButtonEventTypeDto,
        RhythmActionDto,
    };

    #[test]
    fn test_generate_curve_data_high_res_with_sun_times() {
        let config = CurveConfigDto::default();
        let result = generate_curve_data_high_res_with_sun_times(
            config,
            35.7796,
            -78.6382,
            2024,
            6,
            21,
            "America/New_York".to_string(),
            4,
        );

        assert_eq!(result.hours.len(), 96);
        assert_eq!(result.brightness.len(), 96);
        assert_eq!(result.kelvin.len(), 96);

        // At noon (index 12), brightness should be high
        let noon_index = result
            .hours
            .iter()
            .position(|hour| (*hour - 12.0).abs() < 0.001)
            .expect("expected noon sample");
        assert!(result.brightness[noon_index] > 90);
        assert!(result.kelvin[noon_index] >= CurveConfigDto::default().max_color_temp - 100);

        // At midnight (index 0), brightness should be low and color warm
        assert!(result.brightness[0] < 10);
        // Min color temp is 1200K from defaults, so kelvin should be close to that
        assert!(result.kelvin[0] <= CurveConfigDto::default().min_color_temp + 100);
    }

    #[test]
    fn test_calculate_lighting_with_sun_times() {
        let config = CurveConfigDto::default();
        let result = calculate_lighting_with_sun_times(
            config,
            35.7796,
            -78.6382,
            2024,
            6,
            21,
            "America/New_York".to_string(),
            12.0,
        );

        assert!(result.brightness > 90);
        assert!(result.kelvin >= CurveConfigDto::default().max_color_temp - 100);
        assert!((result.sun_position - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_step_sequences_with_sun_times() {
        let config = CurveConfigDto::default();
        // Use 9:00 AM - mid-morning between sunrise (~6:00) and noon (12:00)
        // At this time, both step_up (toward noon) and step_down (toward sunrise) are possible
        // Note: At noon, step_up would be empty (already at max brightness)
        //       At sunrise, step_down would be empty (already at min brightness)
        let result = calculate_step_sequences_with_sun_times(
            config,
            35.7796,
            -78.6382,
            2024,
            6,
            21,
            "America/New_York".to_string(),
            9.0,
            10,
        );

        // Should have generated some steps in both directions
        assert!(
            !result.step_up.is_empty(),
            "step_up should not be empty at 9am"
        );
        assert!(
            !result.step_down.is_empty(),
            "step_down should not be empty at 9am"
        );

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
        let times = get_sun_times(
            40.7128,
            -74.006,
            2024,
            6,
            21,
            "America/New_York".to_string(),
        );

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
        assert_eq!(
            endpoint_for_manufacturer("Signify Netherlands B.V.".to_string(), "LCT015".to_string()),
            11
        );
        assert_eq!(
            endpoint_for_manufacturer("IKEA of Sweden".to_string(), "TRADFRI bulb".to_string()),
            1
        );
        assert_eq!(
            endpoint_for_manufacturer("Unknown".to_string(), "Unknown".to_string()),
            11
        );
    }

    #[test]
    fn test_normalize_area_id_ffi() {
        assert_eq!(normalize_area_id("Living Room".to_string()), "living_room");
        assert_eq!(normalize_area_id("living-room".to_string()), "living_room");
    }

    #[test]
    fn test_area_ids_match_ffi() {
        assert!(area_ids_match(
            "Living Room".to_string(),
            "living_room".to_string()
        ));
        assert!(!area_ids_match(
            "living_room".to_string(),
            "bedroom".to_string()
        ));
    }

    #[test]
    fn test_group_name_for_area_ffi() {
        assert_eq!(
            group_name_for_area("Living Room".to_string()),
            "Rhythm_Living_Room"
        );
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
