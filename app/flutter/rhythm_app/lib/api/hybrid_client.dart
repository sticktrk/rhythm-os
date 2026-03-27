import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;
import '../services/settings_service.dart';
import '../providers/home_provider.dart';

/// Hybrid client that uses local Rust brain for calculations
/// and REST API for persistence and time sync.
///
/// This provides the best of both worlds:
/// - Instant curve updates via local Rust calculations
/// - Persistence and server sync via REST API
class HybridApiClient implements RhythmApi {
  final RhythmApi _remote;
  final NativeBrain? _brain;

  // Cached solar/location data from server
  double _solarNoonHour = 12.0;
  double _latitude = 35.0;
  double _longitude = -78.9; // Default: Raleigh, NC
  String _timezone = 'America/New_York';
  int _dayOfYear = 172;
  double _currentHour = 12.0;

  HybridApiClient._({
    required RhythmApi remote,
    NativeBrain? brain,
  })  : _remote = remote,
        _brain = brain;

  /// Create a hybrid client with both remote and local capabilities.
  ///
  /// If Rust brain initialization fails, falls back to remote-only mode.
  static Future<HybridApiClient> create({String? baseUrl}) async {
    final effectiveBaseUrl = baseUrl ?? _defaultBaseUrl();
    final remote = _SdkConfigAdapter(sdk.RhythmConfigApi(baseUrl: effectiveBaseUrl));

    NativeBrain? brain;
    try {
      brain = await NativeBrain.init();
    } catch (e) {
      // Rust brain not available (e.g., WASM not loaded)
      // Fall back to remote-only mode
      print('NativeBrain not available, using remote-only mode: $e');
    }

    final client = HybridApiClient._(remote: remote, brain: brain);

    // Try to sync solar data from server
    try {
      await client.syncSolarData();
    } catch (e) {
      // Ignore - will use defaults
    }

    return client;
  }

  /// Create a remote-only client (no local Rust calculations).
  static HybridApiClient remoteOnly({String? baseUrl}) {
    return HybridApiClient._(
      remote: _SdkConfigAdapter(sdk.RhythmConfigApi(baseUrl: baseUrl ?? _defaultBaseUrl())),
      brain: null,
    );
  }

  /// Default base URL: on web, use Uri.base (handles ingress path prefix).
  static String _defaultBaseUrl() {
    if (kIsWeb) {
      final base = Uri.base.toString();
      return base.endsWith('/') ? base : '$base/';
    }
    return '';
  }

  /// Create a local-only client (no remote API, just local brain).
  /// Perfect for UI development without a backend.
  static Future<HybridApiClient> localOnly({NativeBrain? existingBrain}) async {
    NativeBrain? brain = existingBrain;
    if (brain == null) {
      try {
        brain = await NativeBrain.init();
      } catch (e) {
        print('NativeBrain init failed: $e');
      }
    }

    return HybridApiClient._(
      remote: _LocalOnlyApi(),
      brain: brain,
    );
  }

  /// Convert this client to local-only mode, reusing the existing brain.
  HybridApiClient toLocalOnly() {
    return HybridApiClient._(
      remote: _LocalOnlyApi(),
      brain: _brain,
    );
  }

  /// Check if local brain is available.
  bool get hasLocalBrain => _brain != null && _brain.isInitialized;

  /// Get the current cached solar noon hour.
  double get solarNoonHour => _solarNoonHour;

  /// Get the current cached latitude.
  double get latitude => _latitude;

  /// Get the current cached day of year.
  int get dayOfYear => _dayOfYear;

  @override
  Future<ConfigState> getConfigState() => _remote.getConfigState();

  @override
  Future<void> saveConfig(RawConfig config) => _remote.saveConfig(config);

  @override
  Future<CurveData> getCurveData(
      {int? month, CurveConfigDto? overrides}) async {
    // Use local brain if available
    if (_brain != null) {
      try {
        // Use overrides if provided, otherwise get current config
        CurveConfigDto config;
        if (overrides != null) {
          config = overrides;
        } else {
          final configState = await _remote.getConfigState();
          config = ConfigState.rawConfigToDto(configState.config);
          if (configState.latitude != null) _latitude = configState.latitude!;
          if (configState.longitude != null)
            _longitude = configState.longitude!;
          if (configState.timezone != null) _timezone = configState.timezone!;
        }

        final now = DateTime.now();
        final targetMonth = month ?? now.month;
        final targetDay = month != null
            ? 15
            : now.day; // Use middle of month if month specified
        final targetDate = DateTime(now.year, targetMonth, targetDay);

        // Sync call - no await needed (runs on main thread)
        final curveData = _brain.getCurveData(
          config: config,
          latitude: _latitude,
          longitude: _longitude,
          year: now.year,
          month: targetMonth,
          day: targetDay,
          timezone: _timezone,
        );

        _solarNoonHour = curveData.solar.solarNoon;
        _dayOfYear = _dayOfYearForDate(targetDate);

        CurveData? highRes;
        try {
          highRes = _brain.getCurveDataHighRes(
            config: config,
            solarNoonHour: _solarNoonHour,
            latitude: _latitude,
            dayOfYear: _dayOfYear,
            samplesPerHour: 4,
          );
        } catch (_) {
          highRes = null;
        }

        if (highRes != null) {
          return CurveData(
            hours: highRes.hours,
            brightness: highRes.brightness,
            kelvin: highRes.kelvin,
            solar: curveData.solar,
          );
        }

        return curveData;
      } catch (e) {
        // Fall back to remote on error
        print('Local brain error, falling back to remote: $e');
      }
    }

    // Fetch from server when brain unavailable
    return _remote.getCurveData(month: month, overrides: overrides);
  }

  int _dayOfYearForMonth(int month) {
    // Return middle of the month as day of year
    final date = DateTime(DateTime.now().year, month, 15);
    return date.difference(DateTime(date.year, 1, 1)).inDays + 1;
  }

  int _dayOfYearForDate(DateTime date) {
    return date.difference(DateTime(date.year, 1, 1)).inDays + 1;
  }

  /// Get high-resolution curve data for smooth graph rendering.
  ///
  /// Only available when local brain is active.
  CurveData? getCurveDataHighRes({
    required CurveConfigDto config,
    int samplesPerHour = 4,
  }) {
    if (_brain == null) return null;

    try {
      // Sync call - runs on main thread
      return _brain.getCurveDataHighRes(
        config: config,
        solarNoonHour: _solarNoonHour,
        latitude: _latitude,
        dayOfYear: _dayOfYear,
        samplesPerHour: samplesPerHour,
      );
    } catch (e) {
      return null;
    }
  }

  @override
  Future<StepSequences> getStepSequences({
    required double hour,
    required int maxSteps,
    CurveConfigDto? overrides,
  }) async {
    // Use local brain if available
    if (_brain != null) {
      try {
        final config = overrides ?? ConfigState.rawConfigToDto((await _remote.getConfigState()).config);
        // Sync call - runs on main thread
        return _brain.getStepSequences(
          config: config,
          solarNoonHour: _solarNoonHour,
          latitude: _latitude,
          dayOfYear: _dayOfYear,
          hour: hour,
          maxSteps: maxSteps,
        );
      } catch (e) {
        print('Local brain error, falling back to remote: $e');
      }
    }

    return _remote.getStepSequences(
      hour: hour,
      maxSteps: maxSteps,
      overrides: overrides,
    );
  }

  @override
  Future<TimeInfo> getTime() async {
    final info = await _remote.getTime();

    // Update cached values
    _currentHour = info.currentHour;

    // Extract solar data from response if available
    // (The server would need to include this in the response)

    return info;
  }

  @override
  Future<bool> healthCheck() => _remote.healthCheck();

  /// Sync solar data from the server.
  ///
  /// Call this periodically to keep solar calculations accurate.
  Future<void> syncSolarData() async {
    try {
      final timeInfo = await _remote.getTime();
      _currentHour = timeInfo.currentHour;

      // Calculate day of year from current time
      // The server should ideally provide this
      final now = DateTime.now();
      _dayOfYear = now.difference(DateTime(now.year, 1, 1)).inDays + 1;

      // Solar noon and latitude would need to come from server
      // or be configured in the app settings
    } catch (e) {
      // Ignore - use cached values
    }
  }

  /// Update solar configuration.
  ///
  /// Call this when the user updates their location or timezone.
  void updateSolarConfig({
    double? solarNoonHour,
    double? latitude,
    double? longitude,
    String? timezone,
    int? dayOfYear,
  }) {
    if (solarNoonHour != null) _solarNoonHour = solarNoonHour;
    if (latitude != null) _latitude = latitude;
    if (longitude != null) _longitude = longitude;
    if (timezone != null) _timezone = timezone;
    if (dayOfYear != null) _dayOfYear = dayOfYear;
  }

  /// Sync location from a ResolvedLocation.
  ///
  /// Call this after HomeProvider resolves location to ensure accurate
  /// solar calculations. This updates the internal latitude, longitude,
  /// and timezone used for all curve and lighting calculations.
  void syncLocation(ResolvedLocation location) {
    _latitude = location.latitude;
    _longitude = location.longitude;
    _timezone = location.timezone;
  }

  /// Get the current cached longitude.
  double get longitude => _longitude;

  /// Get the current cached timezone.
  String get timezone => _timezone;

  /// Calculate lighting values for a specific time using local brain.
  ///
  /// Returns null if local brain is not available.
  LightingValues? calculateLighting({
    required CurveConfigDto config,
    required double currentHour,
  }) {
    if (_brain == null) return null;

    try {
      // Sync call - runs on main thread
      return _brain.calculateLighting(
        config: config,
        solarNoonHour: _solarNoonHour,
        latitude: _latitude,
        dayOfYear: _dayOfYear,
        currentHour: currentHour,
      );
    } catch (e) {
      return null;
    }
  }

  /// Get sun position at a specific hour using local brain.
  double? getSunPosition(double currentHour) {
    if (_brain == null) return null;

    try {
      // Sync call - runs on main thread
      return _brain.getSunPosition(
        solarNoonHour: _solarNoonHour,
        currentHour: currentHour,
      );
    } catch (e) {
      return null;
    }
  }
}

/// Adapter wrapping [sdk.RhythmConfigApi] to implement [RhythmApi].
class _SdkConfigAdapter implements RhythmApi {
  final sdk.RhythmConfigApi _sdk;
  _SdkConfigAdapter(this._sdk);

  @override
  Future<ConfigState> getConfigState() async {
    final s = await _sdk.getConfigState();
    return ConfigState(
      config: _toRawConfig(s.config),
      solar: SolarContext(
        sunrise: s.solar.sunrise,
        sunset: s.solar.sunset,
        solarNoon: s.solar.solarNoon,
        solarMidnight: s.solar.solarMidnight,
        dayLength: s.solar.dayLength,
      ),
      latitude: s.latitude,
      longitude: s.longitude,
      timezone: s.timezone,
    );
  }

  @override
  Future<void> saveConfig(RawConfig config) =>
      _sdk.saveConfig(_toSdkRawConfig(config));

  @override
  Future<CurveData> getCurveData({int? month, CurveConfigDto? overrides}) async {
    final data = await _sdk.getCurveData(
      month: month,
      overrides: overrides != null ? _dtoToSdkCurveConfig(overrides) : null,
    );
    return CurveData(
      hours: data.hours,
      brightness: data.brightness,
      kelvin: data.kelvin,
      solar: _toSolarInfo(data.solar),
    );
  }

  @override
  Future<StepSequences> getStepSequences({
    required double hour,
    required int maxSteps,
    CurveConfigDto? overrides,
  }) async {
    final data = await _sdk.getStepSequences(
      hour: hour,
      maxSteps: maxSteps,
      overrides: overrides != null ? _dtoToSdkCurveConfig(overrides) : null,
    );
    return StepSequences(
      stepUp: data.stepUp.map(_toStepPoint).toList(),
      stepDown: data.stepDown.map(_toStepPoint).toList(),
    );
  }

  @override
  Future<TimeInfo> getTime() async {
    final t = await _sdk.getTime();
    return TimeInfo(
      currentTime: t.currentTime,
      currentHour: t.currentHour,
      timezone: t.timezone,
      brightness: t.brightness,
      kelvin: t.kelvin,
      solarPosition: t.solarPosition,
    );
  }

  @override
  Future<bool> healthCheck() => _sdk.healthCheck();

  static RawConfig _toRawConfig(sdk.RhythmRawConfig c) => RawConfig(
    minColorTemp: c.minColorTemp, maxColorTemp: c.maxColorTemp,
    minBrightness: c.minBrightness, maxBrightness: c.maxBrightness,
    widthLeftBri: c.widthLeftBri, widthRightBri: c.widthRightBri,
    widthLeftCct: c.widthLeftCct, widthRightCct: c.widthRightCct,
    shapeP: c.shapeP, maxDimSteps: c.maxDimSteps,
  );

  static sdk.RhythmRawConfig _toSdkRawConfig(RawConfig c) => sdk.RhythmRawConfig(
    minColorTemp: c.minColorTemp, maxColorTemp: c.maxColorTemp,
    minBrightness: c.minBrightness, maxBrightness: c.maxBrightness,
    widthLeftBri: c.widthLeftBri, widthRightBri: c.widthRightBri,
    widthLeftCct: c.widthLeftCct, widthRightCct: c.widthRightCct,
    shapeP: c.shapeP, maxDimSteps: c.maxDimSteps,
  );

  static SolarInfo _toSolarInfo(sdk.RhythmSolarInfo s) => SolarInfo(
    sunrise: s.sunrise, sunset: s.sunset,
    solarNoon: s.solarNoon, solarMidnight: s.solarMidnight,
    dayLength: s.dayLength,
    dawn: s.dawn != null ? TwilightPhase(
      civil: s.dawn!.civil, nautical: s.dawn!.nautical, astronomical: s.dawn!.astronomical,
    ) : null,
    dusk: s.dusk != null ? TwilightPhase(
      civil: s.dusk!.civil, nautical: s.dusk!.nautical, astronomical: s.dusk!.astronomical,
    ) : null,
  );

  static StepPoint _toStepPoint(sdk.RhythmStepPoint p) => StepPoint(
    hour: p.hour, brightness: p.brightness, kelvin: p.kelvin, rgb: p.rgb,
  );
}

/// Convert [CurveConfigDto] (Rust FFI) to [sdk.RhythmCurveConfig] (SDK).
sdk.RhythmCurveConfig _dtoToSdkCurveConfig(CurveConfigDto c) => sdk.RhythmCurveConfig(
  minColorTemp: c.minColorTemp, maxColorTemp: c.maxColorTemp,
  minBrightness: c.minBrightness, maxBrightness: c.maxBrightness,
  widthLeftBri: c.widthLeftBri, widthRightBri: c.widthRightBri,
  widthLeftCct: c.widthLeftCct, widthRightCct: c.widthRightCct,
  shapeP: c.shapeP, maxDimSteps: c.maxDimSteps,
);

/// Convert [sdk.RhythmCurveConfig] (SDK) to [CurveConfigDto] (Rust FFI).
CurveConfigDto sdkCurveConfigToDto(sdk.RhythmCurveConfig c) => CurveConfigDto(
  minColorTemp: c.minColorTemp, maxColorTemp: c.maxColorTemp,
  minBrightness: c.minBrightness, maxBrightness: c.maxBrightness,
  widthLeftBri: c.widthLeftBri, widthRightBri: c.widthRightBri,
  widthLeftCct: c.widthLeftCct, widthRightCct: c.widthRightCct,
  shapeP: c.shapeP, maxDimSteps: c.maxDimSteps,
);

/// Local-only API implementation with SettingsService (Hive) persistence.
class _LocalOnlyApi implements RhythmApi {
  RawConfig? _cachedConfig;
  bool _loaded = false;

  Future<RawConfig> _loadConfig() async {
    if (_loaded && _cachedConfig != null) {
      return _cachedConfig!;
    }

    try {
      final config = SettingsService.instance.getCurveConfig();
      if (config != null) {
        _cachedConfig = config;
        print('Config loaded from SettingsService');
      }
    } catch (e) {
      print('Failed to load config: $e');
    }

    _cachedConfig ??= RawConfig.defaults();
    _loaded = true;
    return _cachedConfig!;
  }

  @override
  Future<ConfigState> getConfigState() async {
    final config = await _loadConfig();
    final solar = SolarContext.defaults();
    return ConfigState(
      config: config,
      solar: solar,
    );
  }

  @override
  Future<void> saveConfig(RawConfig config) async {
    _cachedConfig = config;
    try {
      await SettingsService.instance.saveCurveConfig(config);
      print('Config saved to SettingsService');
    } catch (e) {
      print('Failed to save config: $e');
      rethrow;
    }
  }

  @override
  Future<CurveData> getCurveData(
      {int? month, CurveConfigDto? overrides}) async {
    // Return empty data - brain will generate it
    throw UnimplementedError('Use local brain for curve data');
  }

  @override
  Future<StepSequences> getStepSequences({
    required double hour,
    required int maxSteps,
    CurveConfigDto? overrides,
  }) async {
    throw UnimplementedError('Use local brain for step sequences');
  }

  @override
  Future<TimeInfo> getTime() async {
    final now = DateTime.now();
    final hour = now.hour + now.minute / 60.0;
    return TimeInfo(
      currentTime: '${now.hour}:${now.minute.toString().padLeft(2, '0')}',
      currentHour: hour,
      timezone: now.timeZoneName,
      brightness: 80,
      kelvin: 4000,
      solarPosition: 0.0,
    );
  }

  @override
  Future<bool> healthCheck() async => true;
}
