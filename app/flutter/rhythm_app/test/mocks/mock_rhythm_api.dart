import 'package:rhythm_core/rhythm_core.dart';

/// Mock implementation of RhythmApi for testing.
///
/// Returns static test data without any network calls.
class MockRhythmApi implements RhythmApi {
  bool healthCheckResult = true;
  ConfigState? configStateOverride;
  CurveData? curveDataOverride;
  StepSequences? stepSequencesOverride;
  TimeInfo? timeInfoOverride;

  // Track method calls for verification
  int healthCheckCallCount = 0;
  int getConfigStateCallCount = 0;
  int saveConfigCallCount = 0;
  int getCurveDataCallCount = 0;
  int getStepSequencesCallCount = 0;
  int getTimeCallCount = 0;

  // Capture saved config
  RawConfig? lastSavedConfig;

  @override
  Future<bool> healthCheck() async {
    healthCheckCallCount++;
    return healthCheckResult;
  }

  @override
  Future<ConfigState> getConfigState() async {
    getConfigStateCallCount++;
    return configStateOverride ?? ConfigState.defaults();
  }

  @override
  Future<void> saveConfig(RawConfig config) async {
    saveConfigCallCount++;
    lastSavedConfig = config;
  }

  @override
  Future<CurveData> getCurveData({int? month, CurveConfigDto? overrides}) async {
    getCurveDataCallCount++;
    return curveDataOverride ?? _createDefaultCurveData();
  }

  @override
  Future<StepSequences> getStepSequences({
    required double hour,
    required int maxSteps,
    CurveConfigDto? overrides,
  }) async {
    getStepSequencesCallCount++;
    return stepSequencesOverride ?? _createDefaultStepSequences();
  }

  @override
  Future<TimeInfo> getTime() async {
    getTimeCallCount++;
    return timeInfoOverride ?? _createDefaultTimeInfo();
  }

  /// Reset all call counts and captured data.
  void reset() {
    healthCheckCallCount = 0;
    getConfigStateCallCount = 0;
    saveConfigCallCount = 0;
    getCurveDataCallCount = 0;
    getStepSequencesCallCount = 0;
    getTimeCallCount = 0;
    lastSavedConfig = null;
  }

  /// Create default curve data for testing.
  CurveData _createDefaultCurveData() {
    final hours = List.generate(24, (i) => i.toDouble());
    final brightness = List.generate(24, (i) {
      // Bell curve peaking at solar noon (12:00)
      final t = (i - 12).abs() / 12.0;
      return (100 * (1 - t * t)).round().clamp(10, 100);
    });
    final kelvin = List.generate(24, (i) {
      // CCT follows similar pattern
      final t = (i - 12).abs() / 12.0;
      return (6500 - (4500 * t * t)).round().clamp(2000, 6500);
    });

    return CurveData(
      hours: hours,
      brightness: brightness,
      kelvin: kelvin,
      solar: SolarInfo(
        solarNoon: 12.0,
        solarMidnight: 0.0,
        sunrise: 6.0,
        sunset: 20.0,
        dayLength: 14.0,
      ),
    );
  }

  /// Create default step sequences for testing.
  StepSequences _createDefaultStepSequences() {
    return StepSequences(
      stepUp: List.generate(10, (i) => StepPoint(
        hour: 12.0 + (i * 0.5),
        brightness: 100 - (i * 10),
        kelvin: 6500 - (i * 450),
        rgb: [255, 200 - (i * 15), 150 - (i * 10)],
      )),
      stepDown: List.generate(10, (i) => StepPoint(
        hour: 12.0 - (i * 0.5),
        brightness: 100 - (i * 10),
        kelvin: 6500 - (i * 450),
        rgb: [255, 200 - (i * 15), 150 - (i * 10)],
      )),
    );
  }

  /// Create default time info for testing.
  TimeInfo _createDefaultTimeInfo() {
    return TimeInfo(
      currentTime: '12:00',
      currentHour: 12.0,
      timezone: 'America/New_York',
      brightness: 100,
      kelvin: 6500,
      solarPosition: 0.0,
    );
  }
}

/// Factory methods for creating test data.
class TestDataFactory {
  /// Create a CurveData with custom values.
  static CurveData createCurveData({
    List<double>? hours,
    List<int>? brightness,
    List<int>? kelvin,
    double solarNoon = 12.0,
    double? sunrise = 6.0,
    double? sunset = 20.0,
  }) {
    return CurveData(
      hours: hours ?? List.generate(24, (i) => i.toDouble()),
      brightness: brightness ?? List.filled(24, 80),
      kelvin: kelvin ?? List.filled(24, 4000),
      solar: SolarInfo(
        solarNoon: solarNoon,
        solarMidnight: (solarNoon + 12) % 24,
        sunrise: sunrise,
        sunset: sunset,
        dayLength: (sunset != null && sunrise != null) ? sunset - sunrise : null,
      ),
    );
  }

  /// Create a RawConfig with custom values.
  static RawConfig createRawConfig({
    int minColorTemp = 2000,
    int maxColorTemp = 6500,
    int minBrightness = 10,
    int maxBrightness = 100,
    double widthLeftBri = 1.0,
    double widthRightBri = 1.0,
    double widthLeftCct = 1.0,
    double widthRightCct = 1.0,
    double shapeP = 4.0,
    int maxDimSteps = 10,
  }) {
    return RawConfig(
      minColorTemp: minColorTemp,
      maxColorTemp: maxColorTemp,
      minBrightness: minBrightness,
      maxBrightness: maxBrightness,
      widthLeftBri: widthLeftBri,
      widthRightBri: widthRightBri,
      widthLeftCct: widthLeftCct,
      widthRightCct: widthRightCct,
      shapeP: shapeP,
      maxDimSteps: maxDimSteps,
    );
  }

  /// Create a ConfigState with custom values.
  static ConfigState createConfigState({
    RawConfig? config,
    SolarContext? solar,
    double? latitude,
    double? longitude,
    String? timezone,
  }) {
    return ConfigState(
      config: config ?? RawConfig.defaults(),
      solar: solar ?? SolarContext.defaults(),
      latitude: latitude,
      longitude: longitude,
      timezone: timezone,
    );
  }
}
