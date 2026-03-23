import 'rhythm_api.dart';

// Import the generated Rust bindings
import '../src/rust/api/curve.dart' as rust_api;
import '../src/rust/api/dto/curve.dart' show CurveConfigDto;
import '../src/rust/api/dto/solar.dart' show TwilightPhaseDto;
import '../src/rust/frb_generated.dart';

// Conditional import for dart:io (not available on web)
// Use dart.library.html to detect web - it's only available in browser environments
import 'native_brain_io.dart' if (dart.library.html) 'native_brain_web.dart'
    as platform;

/// Local brain implementation using Rust WASM/FFI.
///
/// This provides instant curve calculations without network latency.
/// Used for real-time graph updates during parameter adjustment.
class NativeBrain {
  bool _initialized = false;

  NativeBrain._();

  /// Initialize the Rust library.
  ///
  /// Must be called before any other methods.
  static Future<NativeBrain> init() async {
    // Get platform-specific external library (null for web/default loading)
    final externalLibrary = platform.getExternalLibrary();
    await RustLib.init(
      externalLibrary: externalLibrary,
      // Skip hash check — FRB build-web can produce hash mismatches
      // when codegen and WASM compilation happen in separate steps
      forceSameCodegenVersion: false,
    );
    final brain = NativeBrain._();
    brain._initialized = true;
    return brain;
  }

  /// Check if the brain is initialized.
  bool get isInitialized => _initialized;

  /// Generate curve data for visualization with full solar information.
  ///
  /// This replaces the REST call to /api/curve for preview purposes.
  /// Synchronous - runs on main thread (no web workers needed).
  CurveData getCurveData({
    required CurveConfigDto config,
    required double latitude,
    required double longitude,
    required int year,
    required int month,
    required int day,
    required String timezone,
  }) {
    final result = rust_api.generateCurveDataWithSunTimes(
      config: config,
      latitude: latitude,
      longitude: longitude,
      year: year,
      month: month,
      day: day,
      timezone: timezone,
    );

    return CurveData(
      hours: result.hours.toList(),
      brightness: result.brightness.toList(),
      kelvin: result.kelvin.toList(),
      solar: SolarInfo(
        solarNoon: result.solar.solarNoon,
        solarMidnight: result.solar.solarMidnight,
        sunrise: result.solar.sunrise,
        sunset: result.solar.sunset,
        dayLength: result.solar.dayLength,
        dawn: _mapTwilightPhase(result.solar.dawn),
        dusk: _mapTwilightPhase(result.solar.dusk),
      ),
    );
  }

  /// Generate high-resolution curve data for smooth graph rendering.
  ///
  /// Note: This uses the old API without full sun times calculation.
  /// For accurate sunrise/sunset, use getCurveData() instead.
  CurveData getCurveDataHighRes({
    required CurveConfigDto config,
    required double solarNoonHour,
    required double latitude,
    required int dayOfYear,
    int samplesPerHour = 4,
  }) {
    final result = rust_api.generateCurveDataHighRes(
      config: config,
      solarNoonHour: solarNoonHour,
      latitude: latitude,
      dayOfYear: dayOfYear,
      samplesPerHour: samplesPerHour,
    );

    return CurveData(
      hours: result.hours.toList(),
      brightness: result.brightness.toList(),
      kelvin: result.kelvin.toList(),
      solar: SolarInfo(
        solarNoon: result.solar.solarNoon,
        solarMidnight: result.solar.solarMidnight,
        sunrise: result.solar.sunrise,
        sunset: result.solar.sunset,
        dayLength: result.solar.dayLength,
        dawn: _mapTwilightPhase(result.solar.dawn),
        dusk: _mapTwilightPhase(result.solar.dusk),
      ),
    );
  }

  TwilightPhase? _mapTwilightPhase(TwilightPhaseDto? phase) {
    if (phase == null) return null;
    return TwilightPhase(
      civil: phase.civil,
      nautical: phase.nautical,
      astronomical: phase.astronomical,
    );
  }

  /// Calculate lighting values for a specific time.
  LightingValues calculateLighting({
    required CurveConfigDto config,
    required double solarNoonHour,
    required double latitude,
    required int dayOfYear,
    required double currentHour,
  }) {
    final result = rust_api.calculateLighting(
      config: config,
      solarNoonHour: solarNoonHour,
      latitude: latitude,
      dayOfYear: dayOfYear,
      currentHour: currentHour,
    );

    return LightingValues(
      kelvin: result.kelvin,
      mireds: result.mireds,
      brightness: result.brightness,
      solarTime: result.solarTime,
      sunPosition: result.sunPosition,
      rgb: [result.rgb.r, result.rgb.g, result.rgb.b],
    );
  }

  /// Calculate step sequences for visualization.
  StepSequences getStepSequences({
    required CurveConfigDto config,
    required double solarNoonHour,
    required double latitude,
    required int dayOfYear,
    required double hour,
    required int maxSteps,
  }) {
    final result = rust_api.calculateStepSequences(
      config: config,
      solarNoonHour: solarNoonHour,
      latitude: latitude,
      dayOfYear: dayOfYear,
      startHour: hour,
      maxSteps: maxSteps,
    );

    return StepSequences(
      stepUp: result.stepUp
          .map((s) => StepPoint(
                hour: s.hour,
                brightness: s.brightness,
                kelvin: s.kelvin,
                rgb: s.rgb.toList(),
              ))
          .toList(),
      stepDown: result.stepDown
          .map((s) => StepPoint(
                hour: s.hour,
                brightness: s.brightness,
                kelvin: s.kelvin,
                rgb: s.rgb.toList(),
              ))
          .toList(),
    );
  }

  /// Get sun position at a specific hour.
  double getSunPosition({
    required double solarNoonHour,
    required double currentHour,
  }) {
    return rust_api.getSunPosition(
      solarNoonHour: solarNoonHour,
      currentHour: currentHour,
    );
  }

  /// Check if the current hour is in the morning half.
  bool isMorning({
    required double solarNoonHour,
    required double currentHour,
  }) {
    return rust_api.isMorning(
      solarNoonHour: solarNoonHour,
      currentHour: currentHour,
    );
  }
}

/// Lighting values with RGB.
class LightingValues {
  final int kelvin;
  /// Color temperature in mireds (micro reciprocal degrees).
  /// Calculated as 1,000,000 / kelvin. Used by Hue bulbs (ct parameter).
  final int mireds;
  final int brightness;
  final double solarTime;
  final double sunPosition;
  final List<int> rgb;

  LightingValues({
    required this.kelvin,
    required this.mireds,
    required this.brightness,
    required this.solarTime,
    required this.sunPosition,
    required this.rgb,
  });
}
