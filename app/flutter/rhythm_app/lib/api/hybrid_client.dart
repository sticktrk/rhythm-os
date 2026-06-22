import 'dart:async';

import 'package:flutter/foundation.dart'
    show debugPrint, kIsWeb, visibleForTesting;
import 'package:dio/dio.dart';
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
  static const Duration _defaultRemoteReadTimeout = Duration(seconds: 2);
  static const Duration _defaultRemoteOverrideReadTimeout =
      Duration(seconds: 10);

  final RhythmApi _remote;
  final RhythmApi? Function()? _remoteOverride;
  final NativeBrain? _brain;
  final Duration _remoteReadTimeout;
  final Duration _remoteOverrideReadTimeout;
  final _LocalOnlyApi _localFallback = _LocalOnlyApi();

  // Cached preview/location data for local curve math.
  double _solarNoonHour = 12.0;
  double _latitude = 35.0;
  double _longitude = -78.9; // Default: Raleigh, NC
  String _timezone = 'America/New_York';
  int _dayOfYear = 172;
  DateTime _previewDate = DateTime.now();

  HybridApiClient._({
    required RhythmApi remote,
    RhythmApi? Function()? remoteOverride,
    NativeBrain? brain,
    Duration remoteReadTimeout = _defaultRemoteReadTimeout,
    Duration remoteOverrideReadTimeout = _defaultRemoteOverrideReadTimeout,
  })  : _remote = remote,
        _remoteOverride = remoteOverride,
        _brain = brain,
        _remoteReadTimeout = remoteReadTimeout,
        _remoteOverrideReadTimeout = remoteOverrideReadTimeout;

  /// Create a hybrid client with both remote and local capabilities.
  ///
  /// If Rust brain initialization fails, falls back to remote-only mode.
  static Future<HybridApiClient> create({
    String? baseUrl,
    Iterable<Hub> storedHubs = const [],
    bool syncSolarDataOnCreate = true,
    Dio? remoteDio,
    Dio? Function()? remoteDioOverride,
    RhythmApi? Function()? remoteOverride,
    Duration remoteReadTimeout = _defaultRemoteReadTimeout,
    Duration remoteOverrideReadTimeout = _defaultRemoteOverrideReadTimeout,
  }) async {
    final startupServerHub = _selectStartupServerHub(storedHubs);
    final effectiveBaseUrl = resolveHybridApiBaseUrl(
      baseUrl: baseUrl,
      isWeb: kIsWeb,
      storedHubs: startupServerHub == null ? const [] : [startupServerHub],
    );
    NativeBrain? brain;
    try {
      brain = await NativeBrain.init();
    } catch (e) {
      // Rust brain not available (e.g., WASM not loaded)
      // Fall back to remote-only mode
      debugPrint('NativeBrain not available, using remote-only mode: $e');
    }
    final authToken = remoteDio == null &&
            effectiveBaseUrl ==
                _normalizeBaseUrl(startupServerHub?.endpoint.baseUrl)
        ? await _resolveStartupAuthToken(
            effectiveBaseUrl,
            startupServerHub?.token,
          )
        : null;

    final dynamicRemoteOverride = remoteOverride ??
        (remoteDioOverride == null
            ? null
            : () {
                final dio = remoteDioOverride();
                if (dio == null) return null;
                return _SdkConfigAdapter(sdk.RhythmConfigApi(
                  baseUrl: 'https://remote-override.invalid/',
                  dio: dio,
                ));
              });

    final client = HybridApiClient._(
      remote: remoteDio == null && effectiveBaseUrl == null
          ? _LocalOnlyApi()
          : _SdkConfigAdapter(sdk.RhythmConfigApi(
              baseUrl: effectiveBaseUrl ?? 'https://remote-override.invalid/',
              dio: remoteDio,
              authToken: authToken,
            )),
      remoteOverride: dynamicRemoteOverride,
      brain: brain,
      remoteReadTimeout: remoteReadTimeout,
      remoteOverrideReadTimeout: remoteOverrideReadTimeout,
    );

    if (remoteDio == null && effectiveBaseUrl == null) {
      debugPrint(
          'HybridApiClient: No remote base URL configured, starting in local-only mode');
    }

    if (syncSolarDataOnCreate) {
      // Try to sync solar data from server. This is optional; the app can start
      // with cached/default solar data and reconcile once the server responds.
      try {
        await client.syncSolarData();
      } catch (e) {
        // Ignore - will use defaults
      }
    }

    return client;
  }

  static Future<String?> _resolveStartupAuthToken(
    String? baseUrl,
    String? storedToken,
  ) async {
    if (baseUrl == null) return null;
    final token = storedToken?.trim();
    try {
      final status = await sdk.RhythmAuthApi(baseUrl: baseUrl).getStatus();
      if (!status.requiresAuth) return null;
      return token == null || token.isEmpty ? null : token;
    } catch (error) {
      debugPrint('HybridApiClient: auth status unavailable: $error');
      return token == null || token.isEmpty ? null : token;
    }
  }

  /// Create a remote-only client (no local Rust calculations).
  static HybridApiClient remoteOnly({String? baseUrl}) {
    final effectiveBaseUrl =
        resolveHybridApiBaseUrl(baseUrl: baseUrl, isWeb: kIsWeb);
    if (effectiveBaseUrl == null) {
      throw ArgumentError(
        'remoteOnly requires a non-empty baseUrl on native platforms',
      );
    }
    return HybridApiClient._(
      remote: _SdkConfigAdapter(sdk.RhythmConfigApi(baseUrl: effectiveBaseUrl)),
      brain: null,
    );
  }

  /// Create a local-only client (no remote API, just local brain).
  /// Perfect for UI development without a backend.
  static Future<HybridApiClient> localOnly({NativeBrain? existingBrain}) async {
    NativeBrain? brain = existingBrain;
    if (brain == null) {
      try {
        brain = await NativeBrain.init();
      } catch (e) {
        debugPrint('NativeBrain init failed: $e');
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
      remoteReadTimeout: _remoteReadTimeout,
      remoteOverrideReadTimeout: _remoteOverrideReadTimeout,
    );
  }

  /// Check if local brain is available.
  bool get hasLocalBrain => _brain != null && _brain.isInitialized;

  ({RhythmApi remote, bool fromOverride}) get _selectedRemote {
    final override = _remoteOverride?.call();
    return (
      remote: override ?? _remote,
      fromOverride: override != null,
    );
  }

  RhythmApi get _effectiveRemote => _selectedRemote.remote;

  bool get _remoteIsLocalOnly => _effectiveRemote is _LocalOnlyApi;

  RhythmApi get _localApi =>
      _remote is _LocalOnlyApi ? _remote : _localFallback;

  Future<T> _readRemote<T>(Future<T> Function(RhythmApi remote) request) {
    final selected = _selectedRemote;
    return _readSelectedRemote(selected, request);
  }

  Future<T> _readSelectedRemote<T>(
    ({RhythmApi remote, bool fromOverride}) selected,
    Future<T> Function(RhythmApi remote) request,
  ) {
    final future = request(selected.remote);
    if (selected.remote is _LocalOnlyApi) return future;
    return future.timeout(
      selected.fromOverride ? _remoteOverrideReadTimeout : _remoteReadTimeout,
    );
  }

  Future<ConfigState> _fallbackConfigState(Object error) async {
    if (!_remoteIsLocalOnly) {
      debugPrint(
          'HybridApiClient: Remote config unavailable, using local config: $error');
    }
    return _localApi.getConfigState();
  }

  /// Get the current cached solar noon hour.
  double get solarNoonHour => _solarNoonHour;

  /// Get the current cached latitude.
  double get latitude => _latitude;

  /// Get the current cached day of year.
  int get dayOfYear => _dayOfYear;

  /// Get the current cached preview date.
  DateTime get previewDate => _previewDate;

  @override
  Future<ConfigState> getConfigState() async {
    final selected = _selectedRemote;
    try {
      return await _readSelectedRemote(
        selected,
        (remote) => remote.getConfigState(),
      );
    } catch (e) {
      if (selected.fromOverride) rethrow;
      return _fallbackConfigState(e);
    }
  }

  @override
  Future<void> saveConfig(RawConfig config) async {
    final selected = _selectedRemote;
    try {
      await selected.remote.saveConfig(config);
    } catch (e) {
      if (selected.fromOverride) rethrow;
      if (selected.remote is _LocalOnlyApi) rethrow;
      debugPrint(
          'HybridApiClient: Remote config save failed, saving local copy: $e');
      await _localApi.saveConfig(config);
    }
  }

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
          final configState = await getConfigState();
          config = ConfigState.rawConfigToDto(configState.config);
          if (configState.latitude != null) _latitude = configState.latitude!;
          if (configState.longitude != null) {
            _longitude = configState.longitude!;
          }
          if (configState.timezone != null) _timezone = configState.timezone!;
        }

        final now = DateTime.now();
        final targetMonth = month ?? now.month;
        final targetDay = month != null
            ? 15
            : now.day; // Use middle of month if month specified
        final targetDate =
            _dateOnly(DateTime(now.year, targetMonth, targetDay));

        // Sync call - no await needed (runs on main thread)
        final curveData = _brain.getCurveData(
          config: config,
          latitude: _latitude,
          longitude: _longitude,
          year: targetDate.year,
          month: targetMonth,
          day: targetDay,
          timezone: _timezone,
        );

        _cachePreviewCurve(curveData, targetDate);

        return curveData;
      } catch (e) {
        // Fall back to remote on error
        debugPrint('Local brain error, falling back to remote: $e');
      }
    }

    // Fetch from server when brain unavailable
    return _readRemote(
      (remote) => remote.getCurveData(month: month, overrides: overrides),
    );
  }

  int _dayOfYearForDate(DateTime date) {
    return date.difference(DateTime(date.year, 1, 1)).inDays + 1;
  }

  DateTime _dateOnly(DateTime date) =>
      DateTime(date.year, date.month, date.day);

  DateTime _curveDateFor(DateTime? date) => _dateOnly(date ?? _previewDate);

  void _cachePreviewCurve(CurveData curveData, DateTime date) {
    _previewDate = _dateOnly(date);
    _solarNoonHour = curveData.solar.solarNoon;
    _dayOfYear = _dayOfYearForDate(_previewDate);
  }

  /// Get high-resolution curve data for smooth graph rendering.
  ///
  /// Only available when local brain is active.
  CurveData? getCurveDataHighRes({
    required CurveConfigDto config,
    int samplesPerHour = 4,
    DateTime? date,
  }) {
    if (_brain == null) return null;

    try {
      final targetDate = _curveDateFor(date);

      // Sync call - runs on main thread
      final curveData = _brain.getCurveDataHighRes(
        config: config,
        latitude: _latitude,
        longitude: _longitude,
        year: targetDate.year,
        month: targetDate.month,
        day: targetDate.day,
        timezone: _timezone,
        samplesPerHour: samplesPerHour,
      );

      if (date == null) {
        _solarNoonHour = curveData.solar.solarNoon;
        _dayOfYear = _dayOfYearForDate(targetDate);
      }

      return curveData;
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
        final config = overrides ??
            ConfigState.rawConfigToDto((await getConfigState()).config);
        final targetDate = _curveDateFor(null);
        // Sync call - runs on main thread
        return _brain.getStepSequences(
          config: config,
          latitude: _latitude,
          longitude: _longitude,
          year: targetDate.year,
          month: targetDate.month,
          day: targetDate.day,
          timezone: _timezone,
          hour: hour,
          maxSteps: maxSteps,
        );
      } catch (e) {
        debugPrint('Local brain error, falling back to remote: $e');
      }
    }

    return _readRemote(
      (remote) => remote.getStepSequences(
        hour: hour,
        maxSteps: maxSteps,
        overrides: overrides,
      ),
    );
  }

  @override
  Future<TimeInfo> getTime() async {
    TimeInfo info;
    try {
      info = await _readRemote((remote) => remote.getTime());
    } catch (e) {
      if (!_remoteIsLocalOnly) {
        debugPrint(
            'HybridApiClient: Remote time unavailable, using local time: $e');
      }
      info = await _localApi.getTime();
    }

    // Update cached values
    // Extract solar data from response if available
    // (The server would need to include this in the response)

    return info;
  }

  @override
  Future<bool> healthCheck() async {
    try {
      return await _readRemote((remote) => remote.healthCheck());
    } catch (_) {
      return false;
    }
  }

  /// Sync solar data from the server.
  ///
  /// Call this periodically to keep solar calculations accurate.
  Future<void> syncSolarData() async {
    try {
      await getTime();
      // Calculate day of year from current time
      // The server should ideally provide this
      _dayOfYear = _dayOfYearForDate(_previewDate);

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
    DateTime? date,
  }) {
    if (_brain == null) return null;

    try {
      final targetDate = _curveDateFor(date);

      // Sync call - runs on main thread
      return _brain.calculateLighting(
        config: config,
        latitude: _latitude,
        longitude: _longitude,
        year: targetDate.year,
        month: targetDate.month,
        day: targetDate.day,
        timezone: _timezone,
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

@visibleForTesting
String? resolveHybridApiBaseUrl({
  String? baseUrl,
  required bool isWeb,
  Uri? webBaseUri,
  Iterable<Hub> storedHubs = const [],
}) {
  final explicitBaseUrl = _normalizeBaseUrl(baseUrl);
  if (explicitBaseUrl != null) return explicitBaseUrl;

  if (isWeb) {
    return _normalizeBaseUrl((webBaseUri ?? Uri.base).toString());
  }

  final serverHub = _selectStartupServerHub(storedHubs);
  if (serverHub == null) return null;
  return _normalizeBaseUrl(serverHub.endpoint.baseUrl);
}

Hub? _selectStartupServerHub(Iterable<Hub> storedHubs) {
  final candidates = storedHubs
      .where((hub) => hub.type == HubType.server && hub.enabled)
      .toList()
    ..sort((a, b) {
      final aRecency = a.lastConnected ?? a.updatedAt;
      final bRecency = b.lastConnected ?? b.updatedAt;
      return bRecency.compareTo(aRecency);
    });
  return candidates.isEmpty ? null : candidates.first;
}

String? _normalizeBaseUrl(String? baseUrl) {
  final trimmed = baseUrl?.trim();
  if (trimmed == null || trimmed.isEmpty) return null;
  return trimmed.endsWith('/') ? trimmed : '$trimmed/';
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
  Future<CurveData> getCurveData(
      {int? month, CurveConfigDto? overrides}) async {
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
        minColorTemp: c.minColorTemp,
        maxColorTemp: c.maxColorTemp,
        minBrightness: c.minBrightness,
        maxBrightness: c.maxBrightness,
        widthLeftBri: c.widthLeftBri,
        widthRightBri: c.widthRightBri,
        widthLeftCct: c.widthLeftCct,
        widthRightCct: c.widthRightCct,
        shapeP: c.shapeP,
        maxDimSteps: c.maxDimSteps,
        fadeMs: c.fadeMs,
        motionTimeoutSecs: c.motionTimeoutSecs,
      );

  static sdk.RhythmRawConfig _toSdkRawConfig(RawConfig c) =>
      sdk.RhythmRawConfig(
        minColorTemp: c.minColorTemp,
        maxColorTemp: c.maxColorTemp,
        minBrightness: c.minBrightness,
        maxBrightness: c.maxBrightness,
        widthLeftBri: c.widthLeftBri,
        widthRightBri: c.widthRightBri,
        widthLeftCct: c.widthLeftCct,
        widthRightCct: c.widthRightCct,
        shapeP: c.shapeP,
        maxDimSteps: c.maxDimSteps,
        fadeMs: c.fadeMs,
        motionTimeoutSecs: c.motionTimeoutSecs,
      );

  static SolarInfo _toSolarInfo(sdk.RhythmSolarInfo s) => SolarInfo(
        sunrise: s.sunrise,
        sunset: s.sunset,
        solarNoon: s.solarNoon,
        solarMidnight: s.solarMidnight,
        dayLength: s.dayLength,
        dawn: s.dawn != null
            ? TwilightPhase(
                civil: s.dawn!.civil,
                nautical: s.dawn!.nautical,
                astronomical: s.dawn!.astronomical,
              )
            : null,
        dusk: s.dusk != null
            ? TwilightPhase(
                civil: s.dusk!.civil,
                nautical: s.dusk!.nautical,
                astronomical: s.dusk!.astronomical,
              )
            : null,
      );

  static StepPoint _toStepPoint(sdk.RhythmStepPoint p) => StepPoint(
        hour: p.hour,
        brightness: p.brightness,
        kelvin: p.kelvin,
        rgb: p.rgb,
      );
}

/// Convert [CurveConfigDto] (Rust FFI) to [sdk.RhythmCurveConfig] (SDK).
sdk.RhythmCurveConfig _dtoToSdkCurveConfig(CurveConfigDto c) =>
    sdk.RhythmCurveConfig(
      minColorTemp: c.minColorTemp,
      maxColorTemp: c.maxColorTemp,
      minBrightness: c.minBrightness,
      maxBrightness: c.maxBrightness,
      widthLeftBri: c.widthLeftBri,
      widthRightBri: c.widthRightBri,
      widthLeftCct: c.widthLeftCct,
      widthRightCct: c.widthRightCct,
      shapeP: c.shapeP,
      maxDimSteps: c.maxDimSteps,
      fadeMs: c.fadeMs,
      motionTimeoutSecs: c.motionTimeoutSecs,
    );

/// Convert [sdk.RhythmCurveConfig] (SDK) to [CurveConfigDto] (Rust FFI).
CurveConfigDto sdkCurveConfigToDto(sdk.RhythmCurveConfig c) => CurveConfigDto(
      minColorTemp: c.minColorTemp,
      maxColorTemp: c.maxColorTemp,
      minBrightness: c.minBrightness,
      maxBrightness: c.maxBrightness,
      widthLeftBri: c.widthLeftBri,
      widthRightBri: c.widthRightBri,
      widthLeftCct: c.widthLeftCct,
      widthRightCct: c.widthRightCct,
      shapeP: c.shapeP,
      maxDimSteps: c.maxDimSteps,
      fadeMs: c.fadeMs ?? defaultCurveConfig.fadeMs,
      motionTimeoutSecs:
          c.motionTimeoutSecs ?? defaultCurveConfig.motionTimeoutSecs,
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
        debugPrint('Config loaded from SettingsService');
      }
    } catch (e) {
      debugPrint('Failed to load config: $e');
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
      debugPrint('Config saved to SettingsService');
    } catch (e) {
      debugPrint('Failed to save config: $e');
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
