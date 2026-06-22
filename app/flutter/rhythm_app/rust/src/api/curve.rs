//! Curve generation and lighting calculation functions.

use rhythm_core::{
    calculate_sun_times, calculate_twilight_times, kelvin_to_mireds, lookup_timezone,
    solar_time_from_location, CurveContext, LightProfile, LightProfileModule, SolarTime, SunTimes,
    Timezone, TwilightTimes,
};
use rhythm_profile::{
    generate_curve_data as render_curve_data, generate_step_sequences as render_step_sequences,
    CurveData as RenderedCurveData, LightingValues as RenderedLightingValues,
};

use super::dto::{
    CurveConfigDto, CurveDataDto, LightingValuesDto, RgbDto, SolarInfoDto, StepPointDto,
    StepSequencesDto, SunTimesDto, TwilightPhaseDto, TwilightTimesDto, XyDto,
};

#[derive(Clone, Copy)]
struct ResolvedCurveContext {
    solar: SolarTime,
    sun_times: SunTimes,
    twilight: TwilightTimes,
}

fn resolve_curve_context(
    latitude: f64,
    longitude: f64,
    year: i32,
    month: i32,
    day: i32,
    timezone: &str,
) -> ResolvedCurveContext {
    let tz = lookup_timezone(timezone).unwrap_or_else(|| Timezone::new(timezone));

    let sun_times = calculate_sun_times(
        latitude as f32,
        longitude as f32,
        year,
        month as u32,
        day as u32,
        &tz,
    );
    let twilight = calculate_twilight_times(
        latitude as f32,
        longitude as f32,
        year,
        month as u32,
        day as u32,
        &tz,
    );
    let solar = solar_time_from_location(
        latitude as f32,
        longitude as f32,
        year,
        month as u32,
        day as u32,
        &tz,
    );

    ResolvedCurveContext {
        solar,
        sun_times,
        twilight,
    }
}

fn build_curve_data_dto(
    curve_data: RenderedCurveData,
    resolved: &ResolvedCurveContext,
) -> CurveDataDto {
    CurveDataDto {
        hours: curve_data.hours.into_iter().map(f64::from).collect(),
        brightness: curve_data.brightness.into_iter().map(i32::from).collect(),
        kelvin: curve_data.kelvin.into_iter().map(i32::from).collect(),
        solar: SolarInfoDto {
            sunrise: Some(resolved.sun_times.sunrise as f64),
            sunset: Some(resolved.sun_times.sunset as f64),
            solar_noon: resolved.solar.solar_noon_hour as f64,
            solar_midnight: resolved.solar.solar_midnight_hour() as f64,
            day_length: Some(resolved.sun_times.day_length as f64),
            dawn: Some(TwilightPhaseDto {
                civil: resolved.twilight.dawn.civil.map(f64::from),
                nautical: resolved.twilight.dawn.nautical.map(f64::from),
                astronomical: resolved.twilight.dawn.astronomical.map(f64::from),
            }),
            dusk: Some(TwilightPhaseDto {
                civil: resolved.twilight.dusk.civil.map(f64::from),
                nautical: resolved.twilight.dusk.nautical.map(f64::from),
                astronomical: resolved.twilight.dusk.astronomical.map(f64::from),
            }),
        },
    }
}

fn build_lighting_values_dto(values: RenderedLightingValues) -> LightingValuesDto {
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
/// This is the authoritative preview path for hourly sampling: it resolves
/// the same solar context the server preview endpoints use and samples once
/// per hour.
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
    let resolved = resolve_curve_context(latitude, longitude, year, month, day, &timezone);
    let profile = LightProfile::new(config.into());
    let curve_data = render_curve_data(&profile, resolved.solar, Some(resolved.sun_times), 1);

    build_curve_data_dto(curve_data, &resolved)
}

/// Generate high-resolution curve data using full solar context.
///
/// This matches the server preview semantics while allowing the caller to
/// choose the sampling density for live graph updates.
#[allow(clippy::too_many_arguments)]
pub fn generate_curve_data_high_res_with_sun_times(
    config: CurveConfigDto,
    latitude: f64,
    longitude: f64,
    year: i32,
    month: i32,
    day: i32,
    timezone: String,
    samples_per_hour: i32,
) -> CurveDataDto {
    let resolved = resolve_curve_context(latitude, longitude, year, month, day, &timezone);
    let profile = LightProfile::new(config.into());
    let curve_data = render_curve_data(
        &profile,
        resolved.solar,
        Some(resolved.sun_times),
        samples_per_hour.max(1) as u32,
    );

    build_curve_data_dto(curve_data, &resolved)
}

/// Calculate lighting values for a specific hour using full solar context.
#[allow(clippy::too_many_arguments)]
pub fn calculate_lighting_with_sun_times(
    config: CurveConfigDto,
    latitude: f64,
    longitude: f64,
    year: i32,
    month: i32,
    day: i32,
    timezone: String,
    current_hour: f64,
) -> LightingValuesDto {
    let resolved = resolve_curve_context(latitude, longitude, year, month, day, &timezone);
    let profile = LightProfile::new(config.into());
    let ctx = CurveContext::new(
        current_hour as f32,
        resolved.solar,
        Some(resolved.sun_times),
    );
    let values = profile.calculate(&ctx);

    build_lighting_values_dto(values)
}

/// Calculate step sequences using full solar context.
#[allow(clippy::too_many_arguments)]
pub fn calculate_step_sequences_with_sun_times(
    config: CurveConfigDto,
    latitude: f64,
    longitude: f64,
    year: i32,
    month: i32,
    day: i32,
    timezone: String,
    start_hour: f64,
    max_steps: i32,
) -> StepSequencesDto {
    let resolved = resolve_curve_context(latitude, longitude, year, month, day, &timezone);

    let mut profile_config = rhythm_core::LightProfileConfig::from(config);
    let max_steps = max_steps.clamp(1, 500) as u8;
    profile_config.max_dim_steps = max_steps;

    let profile = LightProfile::new(profile_config);
    let sequences = render_step_sequences(
        &profile,
        resolved.solar,
        Some(resolved.sun_times),
        start_hour as f32,
        max_steps,
    );

    StepSequencesDto {
        step_up: sequences
            .step_up
            .into_iter()
            .map(|step| StepPointDto {
                hour: step.hour as f64,
                brightness: step.brightness as i32,
                kelvin: step.kelvin as i32,
                rgb: vec![step.rgb.r as i32, step.rgb.g as i32, step.rgb.b as i32],
            })
            .collect(),
        step_down: sequences
            .step_down
            .into_iter()
            .map(|step| StepPointDto {
                hour: step.hour as f64,
                brightness: step.brightness as i32,
                kelvin: step.kelvin as i32,
                rgb: vec![step.rgb.r as i32, step.rgb.g as i32, step.rgb.b as i32],
            })
            .collect(),
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
