//! Curve generation and lighting calculation functions.

use rhythm_core::{
    calculate_sun_times, calculate_twilight_times, kelvin_to_mireds, lookup_timezone,
    solar_time_from_location, CurveContext, LightProfile, LightProfileModule, SolarTime,
    StepAction, Timezone,
};

use super::dto::{
    CurveConfigDto, CurveDataDto, LightingValuesDto, RgbDto, SolarInfoDto, StepPointDto,
    StepSequencesDto, SunTimesDto, TwilightPhaseDto, TwilightTimesDto, XyDto,
};

/// Generate curve data for visualization.
///
/// This is the main function for generating the lighting curve graph.
/// It samples the curve at each hour from 0-23 and returns brightness
/// and color temperature values.
///
/// # Arguments
///
/// * `config` - Curve configuration parameters
/// * `solar_noon_hour` - Hour of solar noon (0-24, local time)
/// * `latitude` - Latitude in degrees (for elevation calculations)
/// * `day_of_year` - Day of year (1-365) for seasonal adjustments
///
/// # Returns
///
/// CurveDataDto with hourly brightness and kelvin values.
pub fn generate_curve_data(
    config: CurveConfigDto,
    solar_noon_hour: f64,
    latitude: f64,
    day_of_year: i32,
) -> CurveDataDto {
    let profile = LightProfile::new(config.into());
    let solar = SolarTime::new(solar_noon_hour as f32, latitude as f32, day_of_year as u32);

    let mut hours = Vec::with_capacity(24);
    let mut brightness = Vec::with_capacity(24);
    let mut kelvin = Vec::with_capacity(24);

    for h in 0..24 {
        let hour = h as f32;
        hours.push(hour as f64);
        let ctx = CurveContext::new(hour, solar, None);
        let values = profile.calculate(&ctx);
        brightness.push(values.brightness as i32);
        kelvin.push(values.kelvin as i32);
    }

    CurveDataDto {
        hours,
        brightness,
        kelvin,
        solar: SolarInfoDto {
            sunrise: None,
            sunset: None,
            solar_noon: solar_noon_hour,
            solar_midnight: solar.solar_midnight_hour() as f64,
            day_length: None,
            dawn: None,
            dusk: None,
        },
    }
}

/// Generate high-resolution curve data for smooth graph rendering.
///
/// Samples the curve at smaller intervals for a smoother graph.
///
/// # Arguments
///
/// * `config` - Curve configuration parameters
/// * `solar_noon_hour` - Hour of solar noon (0-24, local time)
/// * `latitude` - Latitude in degrees
/// * `day_of_year` - Day of year (1-365)
/// * `samples_per_hour` - Number of samples per hour (default: 4)
///
/// # Returns
///
/// CurveDataDto with high-resolution brightness and kelvin values.
pub fn generate_curve_data_high_res(
    config: CurveConfigDto,
    solar_noon_hour: f64,
    latitude: f64,
    day_of_year: i32,
    samples_per_hour: i32,
) -> CurveDataDto {
    let profile = LightProfile::new(config.into());
    let solar = SolarTime::new(solar_noon_hour as f32, latitude as f32, day_of_year as u32);

    let samples = samples_per_hour.max(1) as usize;
    let total_samples = 24 * samples;
    let step = 1.0 / samples as f32;

    let mut hours = Vec::with_capacity(total_samples);
    let mut brightness = Vec::with_capacity(total_samples);
    let mut kelvin = Vec::with_capacity(total_samples);

    for i in 0..total_samples {
        let hour = i as f32 * step;
        hours.push(hour as f64);
        let ctx = CurveContext::new(hour, solar, None);
        let values = profile.calculate(&ctx);
        brightness.push(values.brightness as i32);
        kelvin.push(values.kelvin as i32);
    }

    CurveDataDto {
        hours,
        brightness,
        kelvin,
        solar: SolarInfoDto {
            sunrise: None,
            sunset: None,
            solar_noon: solar_noon_hour,
            solar_midnight: solar.solar_midnight_hour() as f64,
            day_length: None,
            dawn: None,
            dusk: None,
        },
    }
}

/// Calculate lighting values for a specific hour.
///
/// # Arguments
///
/// * `config` - Curve configuration parameters
/// * `solar_noon_hour` - Hour of solar noon (0-24, local time)
/// * `latitude` - Latitude in degrees
/// * `day_of_year` - Day of year (1-365)
/// * `current_hour` - Current time in hours (0-24)
///
/// # Returns
///
/// LightingValuesDto with brightness, kelvin, RGB, and xy values.
pub fn calculate_lighting(
    config: CurveConfigDto,
    solar_noon_hour: f64,
    latitude: f64,
    day_of_year: i32,
    current_hour: f64,
) -> LightingValuesDto {
    let profile = LightProfile::new(config.into());
    let solar = SolarTime::new(solar_noon_hour as f32, latitude as f32, day_of_year as u32);
    let ctx = CurveContext::new(current_hour as f32, solar, None);

    let values = profile.calculate(&ctx);

    LightingValuesDto {
        kelvin: values.kelvin as i32,
        mireds: kelvin_to_mireds(values.kelvin) as i32,
        brightness: values.brightness as i32,
        rgb: RgbDto {
            r: values.rgb.r as i32,
            g: values.rgb.g as i32,
            b: values.rgb.b as i32,
        },
        xy: XyDto {
            x: values.xy.x as f64,
            y: values.xy.y as f64,
        },
        solar_time: values.solar_time as f64,
        sun_position: values.sun_position as f64,
    }
}

/// Calculate step sequences for visualization.
///
/// This generates the step markers shown on the curve graph,
/// illustrating where each dim/brighten step would land.
/// Each point represents where pressing the step button would take you.
///
/// The step size is calculated as: (max_brightness - min_brightness) / max_steps
/// This matches the HTML/Python reference implementations.
///
/// # Arguments
///
/// * `config` - Curve configuration parameters
/// * `solar_noon_hour` - Hour of solar noon (0-24, local time)
/// * `latitude` - Latitude in degrees
/// * `day_of_year` - Day of year (1-365)
/// * `start_hour` - Starting hour for step sequence
/// * `max_steps` - Number of steps from min to max brightness (determines step size)
///
/// # Returns
///
/// StepSequencesDto with step_up and step_down sequences.
pub fn calculate_step_sequences(
    config: CurveConfigDto,
    solar_noon_hour: f64,
    latitude: f64,
    day_of_year: i32,
    start_hour: f64,
    max_steps: i32,
) -> StepSequencesDto {
    // Override max_dim_steps with the passed value so step size matches UI
    let mut profile_config = rhythm_core::LightProfileConfig::from(config);
    let max_steps = max_steps.clamp(1, 500) as u8;
    profile_config.max_dim_steps = max_steps;

    let solar = SolarTime::new(solar_noon_hour as f32, latitude as f32, day_of_year as u32);
    let profile = LightProfile::new(profile_config);

    let max_iterations = max_steps as usize;

    // Calculate step up sequence (brighten toward boundary)
    let mut step_up = Vec::with_capacity(max_iterations);
    let mut current_hour = start_hour as f32;

    for _ in 0..max_iterations {
        let ctx = CurveContext::new(current_hour, solar, None);
        let result = profile.calculate_step(&ctx, StepAction::Brighten);

        // Stop if we're at the boundary (no more steps possible)
        if result.at_boundary {
            break;
        }

        // Calculate target hour and values
        let target_hour = current_hour + result.time_offset_minutes / 60.0;
        let values = &result.values;

        step_up.push(StepPointDto {
            hour: target_hour as f64,
            brightness: values.brightness as i32,
            kelvin: values.kelvin as i32,
            rgb: vec![
                values.rgb.r as i32,
                values.rgb.g as i32,
                values.rgb.b as i32,
            ],
        });

        current_hour = target_hour;
    }

    // Calculate step down sequence (dim toward boundary)
    let mut step_down = Vec::with_capacity(max_iterations);
    current_hour = start_hour as f32;

    for _ in 0..max_iterations {
        let ctx = CurveContext::new(current_hour, solar, None);
        let result = profile.calculate_step(&ctx, StepAction::Dim);

        // Stop if we're at the boundary (no more steps possible)
        if result.at_boundary {
            break;
        }

        // Calculate target hour and values
        let target_hour = current_hour + result.time_offset_minutes / 60.0;
        let values = &result.values;

        step_down.push(StepPointDto {
            hour: target_hour as f64,
            brightness: values.brightness as i32,
            kelvin: values.kelvin as i32,
            rgb: vec![
                values.rgb.r as i32,
                values.rgb.g as i32,
                values.rgb.b as i32,
            ],
        });

        current_hour = target_hour;
    }

    StepSequencesDto { step_up, step_down }
}

/// Get sun position at a specific hour.
///
/// Returns a value from -1 (solar midnight) to +1 (solar noon).
///
/// # Arguments
///
/// * `solar_noon_hour` - Hour of solar noon (0-24, local time)
/// * `current_hour` - Current time in hours (0-24)
///
/// # Returns
///
/// Sun position value (-1 to +1).
pub fn get_sun_position(solar_noon_hour: f64, current_hour: f64) -> f64 {
    let solar = SolarTime::new(solar_noon_hour as f32, 35.0, 172);
    solar.get_sun_position(current_hour as f32) as f64
}

/// Check if the current hour is in the morning half.
///
/// # Arguments
///
/// * `solar_noon_hour` - Hour of solar noon (0-24, local time)
/// * `current_hour` - Current time in hours (0-24)
///
/// # Returns
///
/// True if before solar noon, false otherwise.
pub fn is_morning(solar_noon_hour: f64, current_hour: f64) -> bool {
    let solar = SolarTime::new(solar_noon_hour as f32, 35.0, 172);
    solar.is_morning(current_hour as f32)
}

/// Calculate sunrise, sunset, and solar noon times for a location and date.
///
/// # Arguments
///
/// * `latitude` - Latitude in degrees (positive = north)
/// * `longitude` - Longitude in degrees (negative = west)
/// * `year` - Full year (e.g., 2024)
/// * `month` - Month (1-12)
/// * `day` - Day of month (1-31)
/// * `timezone` - IANA timezone name (e.g., "America/New_York") or alias (e.g., "EST")
///
/// # Returns
///
/// SunTimesDto with sunrise, sunset, solar noon, and day length.
///
/// # Example
///
/// ```ignore
/// let times = get_sun_times(40.7128, -74.006, 2024, 6, 21, "America/New_York".to_string());
/// // times.sunrise ≈ 5.42 (5:25 AM)
/// // times.sunset ≈ 20.52 (8:31 PM)
/// ```
pub fn get_sun_times(
    latitude: f64,
    longitude: f64,
    year: i32,
    month: i32,
    day: i32,
    timezone: String,
) -> SunTimesDto {
    let tz = lookup_timezone(&timezone).unwrap_or_else(|| Timezone::new(&timezone));
    let sun_times = calculate_sun_times(
        latitude as f32,
        longitude as f32,
        year,
        month as u32,
        day as u32,
        &tz,
    );

    // Calculate solar noon
    let solar = solar_time_from_location(
        latitude as f32,
        longitude as f32,
        year,
        month as u32,
        day as u32,
        &tz,
    );

    SunTimesDto {
        sunrise: sun_times.sunrise as f64,
        sunset: sun_times.sunset as f64,
        solar_noon: solar.solar_noon_hour as f64,
        solar_midnight: solar.solar_midnight_hour() as f64,
        day_length: sun_times.day_length as f64,
    }
}

/// Generate curve data with full solar information including sunrise/sunset.
///
/// This is an enhanced version of `generate_curve_data` that also calculates
/// sunrise and sunset times.
///
/// # Arguments
///
/// * `config` - Curve configuration parameters
/// * `latitude` - Latitude in degrees (positive = north)
/// * `longitude` - Longitude in degrees (negative = west)
/// * `year` - Full year (e.g., 2024)
/// * `month` - Month (1-12)
/// * `day` - Day of month (1-31)
/// * `timezone` - IANA timezone name (e.g., "America/New_York")
///
/// # Returns
///
/// CurveDataDto with hourly values and full solar information.
pub fn generate_curve_data_with_sun_times(
    config: CurveConfigDto,
    latitude: f64,
    longitude: f64,
    year: i32,
    month: i32,
    day: i32,
    timezone: String,
) -> CurveDataDto {
    let tz = lookup_timezone(&timezone).unwrap_or_else(|| Timezone::new(&timezone));

    // Calculate sun times
    let sun_times = calculate_sun_times(
        latitude as f32,
        longitude as f32,
        year,
        month as u32,
        day as u32,
        &tz,
    );

    // Calculate twilight times
    let twilight = calculate_twilight_times(
        latitude as f32,
        longitude as f32,
        year,
        month as u32,
        day as u32,
        &tz,
    );

    // Create solar time from location
    let solar = solar_time_from_location(
        latitude as f32,
        longitude as f32,
        year,
        month as u32,
        day as u32,
        &tz,
    );

    let profile = LightProfile::new(config.into());

    let mut hours = Vec::with_capacity(24);
    let mut brightness = Vec::with_capacity(24);
    let mut kelvin = Vec::with_capacity(24);

    for h in 0..24 {
        let hour = h as f32;
        hours.push(hour as f64);
        let ctx = CurveContext::new(hour, solar, Some(sun_times));
        let values = profile.calculate(&ctx);
        brightness.push(values.brightness as i32);
        kelvin.push(values.kelvin as i32);
    }

    CurveDataDto {
        hours,
        brightness,
        kelvin,
        solar: SolarInfoDto {
            sunrise: Some(sun_times.sunrise as f64),
            sunset: Some(sun_times.sunset as f64),
            solar_noon: solar.solar_noon_hour as f64,
            solar_midnight: solar.solar_midnight_hour() as f64,
            day_length: Some(sun_times.day_length as f64),
            dawn: Some(TwilightPhaseDto {
                civil: twilight.dawn.civil.map(|v| v as f64),
                nautical: twilight.dawn.nautical.map(|v| v as f64),
                astronomical: twilight.dawn.astronomical.map(|v| v as f64),
            }),
            dusk: Some(TwilightPhaseDto {
                civil: twilight.dusk.civil.map(|v| v as f64),
                nautical: twilight.dusk.nautical.map(|v| v as f64),
                astronomical: twilight.dusk.astronomical.map(|v| v as f64),
            }),
        },
    }
}

/// Calculate twilight times for a location and date.
///
/// Returns dawn (morning) and dusk (evening) times for:
/// - Civil twilight (6° below horizon)
/// - Nautical twilight (12° below horizon)
/// - Astronomical twilight (18° below horizon)
///
/// # Arguments
///
/// * `latitude` - Latitude in degrees (positive = north)
/// * `longitude` - Longitude in degrees (negative = west)
/// * `year` - Full year (e.g., 2024)
/// * `month` - Month (1-12)
/// * `day` - Day of month (1-31)
/// * `timezone` - IANA timezone name (e.g., "America/New_York")
///
/// # Returns
///
/// TwilightTimesDto with dawn and dusk phases.
///
/// # Example
///
/// ```ignore
/// let twilight = get_twilight_times(40.7128, -74.006, 2024, 6, 21, "America/New_York".to_string());
/// // twilight.dawn.civil ≈ 4.87 (4:52 AM)
/// // twilight.dusk.civil ≈ 21.02 (9:01 PM)
/// ```
pub fn get_twilight_times(
    latitude: f64,
    longitude: f64,
    year: i32,
    month: i32,
    day: i32,
    timezone: String,
) -> TwilightTimesDto {
    let tz = lookup_timezone(&timezone).unwrap_or_else(|| Timezone::new(&timezone));
    let twilight = calculate_twilight_times(
        latitude as f32,
        longitude as f32,
        year,
        month as u32,
        day as u32,
        &tz,
    );

    TwilightTimesDto {
        dawn: TwilightPhaseDto {
            civil: twilight.dawn.civil.map(|v| v as f64),
            nautical: twilight.dawn.nautical.map(|v| v as f64),
            astronomical: twilight.dawn.astronomical.map(|v| v as f64),
        },
        dusk: TwilightPhaseDto {
            civil: twilight.dusk.civil.map(|v| v as f64),
            nautical: twilight.dusk.nautical.map(|v| v as f64),
            astronomical: twilight.dusk.astronomical.map(|v| v as f64),
        },
    }
}
