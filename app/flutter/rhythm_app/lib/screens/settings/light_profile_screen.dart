import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;
import '../../models/config_model.dart';
import '../../providers/room_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../utils/room_visibility.dart';

/// Full-screen modal for configuring the light profile.
///
/// Features a compressed color spectrum slider that emphasizes the dawn/dusk
/// ramps where color changes rapidly, compressing flat night/day regions.
class LightProfileScreen extends StatefulWidget {
  final String? initialProfile;

  const LightProfileScreen({super.key, this.initialProfile});

  static Future<void> show(BuildContext context, {String? initialProfile}) {
    final profile = switch (initialProfile) {
      'idle' || 'day_idle' => 'rhythm',
      'sleep_idle' => 'sleep',
      final value? => value,
      null => 'rhythm',
    };
    AnalyticsService().logScreenView('light_profile');
    AnalyticsService().logLightProfileOpened(profile);
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return LightProfileScreen(initialProfile: initialProfile);
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, 1),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 350),
        reverseTransitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  State<LightProfileScreen> createState() => _LightProfileScreenState();
}

enum _TimeOffsetDispatchAction { preview, reset, absorb }

class _LightProfileScreenState extends State<LightProfileScreen>
    with SingleTickerProviderStateMixin {
  static const Duration _defaultBatchDispatchSpacing =
      Duration(milliseconds: 500);

  static const List<String> _profileOrder = [
    'rhythm',
    'sleep',
    'day_idle',
    'sleep_idle',
    'idle',
  ];

  late String _selectedProfileId;
  final Map<String, sdk.RhythmCurveConfig> _profileConfigs = {};
  List<sdk.RhythmModeConfig> _modeConfigs = const [];
  sdk.RhythmMode? _serverActiveMode;
  Timer? _roomDefaultsDebounce;
  Timer? _curvePreviewRefreshTimer;

  // Light transition duration (from selected profile config).
  double _fadeMs = 500;

  // Background light interval.
  double _intervalSecs = 60;

  // Curve parameters.
  bool _curveConfigDirty = false;
  double _minColorTemp = 1800;
  double _maxColorTemp = 5500;
  double _minBrightness = 2;
  double _maxBrightness = 100;
  double _widthLeftBri = 0.95;
  double _widthRightBri = 0.85;
  double _widthLeftCct = 0.95;
  double _widthRightCct = 1.15;
  double _shapeP = 6.0;
  double _maxDimSteps = 6;
  int _motionTimeoutSecs = 600;
  bool _motionTimeoutAuto = true;

  // Interval auto mode (null = server decides).
  bool _intervalAuto = false;

  // Expand/collapse state for range cards.
  bool _brightnessExpanded = false;
  bool _colorTempExpanded = false;
  bool _idleExpanded = false;
  bool _roomDefaultsExpanded = false;

  /// Notifier bumped on every setState so child screens can rebuild.
  final _rebuildNotifier = ValueNotifier<int>(0);

  // Sleep profile state: fixed direct color plus a single brightness level.
  double _sleepHue = 10;
  double _sleepBrightness = 20;

  // Idle profile state (folded into day/sleep profiles).
  bool _idleCustomBri = false;
  bool _idleCustomColor = false;
  double _idleBrightness = 1;
  double _idleHue = 30; // hue angle for spectrum picker

  bool _loading = true;
  bool _connected = false;

  // Time simulator state.
  CurveData? _curveData;
  double _timeOffsetMinutes = 0;
  double _sliderFraction = 0.5; // raw 0..1 position on the track
  bool _isDraggingTime = false;
  bool _timeOffsetApplied = false;
  bool _timeOffsetPreviewActive = false;
  _TimeOffsetDispatchAction? _timeOffsetDispatchAction;
  bool _curvePreviewRefreshQueued = false;
  bool _curvePreviewRefreshInFlight = false;
  int _curvePreviewRequestId = 0;

  /// Compressed position mapping: 97 entries (0..96) mapping 15-min intervals
  /// to non-linear positions (0.0..1.0). Regions with rapid kelvin/brightness
  /// change get more space; flat night/day regions are compressed.
  List<double>? _compressedPositions;

  // Glow animation for the header icon.
  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  @override
  void initState() {
    super.initState();
    _selectedProfileId = switch (widget.initialProfile) {
      'idle' || 'day_idle' => 'rhythm',
      'sleep_idle' => 'sleep',
      final profile? => profile,
      null => 'rhythm',
    };

    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2500),
      vsync: this,
    )..repeat(reverse: true);
    _glowAnimation = Tween<double>(begin: 0.3, end: 0.7).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );

    _loadConfig();
  }

  @override
  @override
  void setState(VoidCallback fn) {
    super.setState(fn);
    _rebuildNotifier.value++;
  }

  @override
  void dispose() {
    _roomDefaultsDebounce?.cancel();
    _curvePreviewRefreshTimer?.cancel();
    _glowController.dispose();
    _rebuildNotifier.dispose();
    super.dispose();
  }

  bool get _isSleepProfile => _selectedProfileId == 'sleep';

  sdk.RhythmMode _modeForProfileId(String profileId) =>
      profileId == 'sleep' ? sdk.RhythmMode.sleep : sdk.RhythmMode.day;

  sdk.RhythmMode get _selectedMode => _modeForProfileId(_selectedProfileId);

  String get _defaultIdleProfileId =>
      _defaultIdleProfileIdForMode(_selectedMode);

  String _defaultIdleProfileIdForMode(sdk.RhythmMode mode) =>
      mode == sdk.RhythmMode.sleep ? 'sleep_idle' : 'day_idle';

  String _defaultIdleProfileNameForMode(sdk.RhythmMode mode) =>
      mode == sdk.RhythmMode.sleep ? 'Sleep Standby' : 'Day Standby';

  sdk.RhythmModeConfig? _modeConfigForMode(sdk.RhythmMode mode) {
    for (final config in _modeConfigs) {
      if (config.mode == mode) return config;
    }
    return null;
  }

  sdk.RhythmModeConfig? _modeConfigForProfile(String profileId) =>
      _modeConfigForMode(_modeForProfileId(profileId));

  String? _customIdleProfileIdForProfile(String profileId) =>
      _modeConfigForProfile(profileId)?.idleProfileId;

  String? get _selectedCustomIdleProfileId =>
      _customIdleProfileIdForProfile(_selectedProfileId);

  sdk.RhythmModeConfig _defaultModeConfigForMode(sdk.RhythmMode mode) =>
      sdk.RhythmModeConfig(
        mode: mode,
        activeProfileId: mode == sdk.RhythmMode.sleep ? 'sleep' : 'rhythm',
      );

  List<sdk.RhythmModeConfig> _updatedModeConfigsForIdle(String? idleProfileId) {
    final updated = <sdk.RhythmModeConfig>[];
    var found = false;
    for (final config in _modeConfigs) {
      if (config.mode != _selectedMode) {
        updated.add(config);
        continue;
      }
      updated.add(config.copyWith(idleProfileId: idleProfileId));
      found = true;
    }
    if (!found) {
      updated.add(
        _defaultModeConfigForMode(_selectedMode).copyWith(
          idleProfileId: idleProfileId,
        ),
      );
    }
    updated.sort((a, b) => a.mode.index.compareTo(b.mode.index));
    return updated;
  }

  String get _profileTitle => switch (_selectedProfileId) {
        'sleep' => 'Sleep Profile',
        _ => 'Day Profile',
      };

  sdk.RhythmCurveConfig? get _selectedProfileConfig =>
      _profileConfigs[_selectedProfileId];

  sdk.RhythmDirectColor? get _selectedDirectColor {
    final curve = _selectedProfileConfig?.curve;
    return switch (curve) {
      sdk.RhythmSuperGaussianCurve(:final directColor) => directColor,
      sdk.RhythmConstantCurve(:final directColor) => directColor,
      _ => null,
    };
  }

  Color? get _selectedFixedColor {
    final directColor = _selectedDirectColor;
    if (directColor == null) return null;
    return Color.fromARGB(
      255,
      directColor.rgb.r,
      directColor.rgb.g,
      directColor.rgb.b,
    );
  }

  Future<void> _loadConfig({bool checkConnection = true}) async {
    final syncProvider = context.read<ServerSyncProvider>();
    if (checkConnection) {
      _connected = syncProvider.synced;

      if (!_connected) {
        setState(() => _loading = false);
        return;
      }
    }

    final api = syncProvider.api;
    final mode = await api.getMode();
    final cachedModeConfigs = [...syncProvider.modeConfigs];
    final cachedProfiles = [...syncProvider.profiles];
    final settingsProfiles = [...await api.getProfiles()];
    if (!mounted) return;

    _modeConfigs = [...?mode?.configs];
    if (_modeConfigs.isEmpty) {
      _modeConfigs = cachedModeConfigs;
    }
    _serverActiveMode = mode?.active ?? syncProvider.activeMode;
    settingsProfiles.sort((a, b) {
      final ia = _profileOrder.indexOf(a.id);
      final ib = _profileOrder.indexOf(b.id);
      final orderA = ia == -1 ? _profileOrder.length : ia;
      final orderB = ib == -1 ? _profileOrder.length : ib;
      return orderA.compareTo(orderB);
    });
    final profileConfigs = <String, sdk.RhythmCurveConfig>{
      for (final profile in cachedProfiles)
        if (profile.id.isNotEmpty) profile.id: profile,
      for (final profile in settingsProfiles)
        if (profile.id.isNotEmpty) profile.id: profile,
    };

    final requestedProfileId = _selectedProfileId;
    var initialProfileId = requestedProfileId;
    var selectedConfig = requestedProfileId.isEmpty
        ? null
        : profileConfigs[requestedProfileId] ??
            await api.getConfig(id: requestedProfileId);
    if (selectedConfig != null && selectedConfig.id.isNotEmpty) {
      initialProfileId = selectedConfig.id;
      profileConfigs[selectedConfig.id] = selectedConfig;
    }

    if (selectedConfig == null) {
      final fallbackMode =
          _serverActiveMode ?? syncProvider.activeMode ?? sdk.RhythmMode.day;
      final fallbackModeConfig = _modeConfigs.firstWhere(
        (config) => config.mode == fallbackMode,
        orElse: () => _defaultModeConfigForMode(fallbackMode),
      );
      initialProfileId = mode?.activeConfig?.activeProfileId ??
          fallbackModeConfig.activeProfileId.trim();
      if (initialProfileId.isEmpty) {
        initialProfileId = settingsProfiles.firstOrNull?.id ??
            cachedProfiles.firstOrNull?.id ??
            'rhythm';
      }
      selectedConfig = profileConfigs[initialProfileId] ??
          await api.getConfig(id: initialProfileId);
    }
    if (selectedConfig == null) {
      setState(() {
        _selectedProfileId = initialProfileId;
        _loading = false;
      });
      return;
    }

    profileConfigs[selectedConfig.id] = selectedConfig;
    _profileConfigs
      ..clear()
      ..addAll(profileConfigs);
    _applyProfileConfig(selectedConfig);
    final idleProfileId = _customIdleProfileIdForProfile(initialProfileId);
    final idleConfig = idleProfileId != null
        ? profileConfigs[idleProfileId] ??
            await api.getConfig(id: idleProfileId)
        : null;
    if (idleConfig != null) {
      profileConfigs[idleConfig.id] = idleConfig;
      _profileConfigs[idleConfig.id] = idleConfig;
    }
    if (idleConfig != null) {
      _applyIdleConfig(idleConfig);
    } else {
      _applyIdleFallback();
    }
    await _loadCurveData(profileId: initialProfileId);
    if (!mounted) return;

    setState(() {
      _selectedProfileId = initialProfileId;
      _loading = false;
      _sliderFraction = _hourToNowFraction();
    });
  }

  Future<void> _loadCurveData(
      {sdk.RhythmCurveConfig? config, String? profileId}) async {
    try {
      final id = profileId ?? _selectedProfileId;
      final requestId = ++_curvePreviewRequestId;
      final preview = await context.read<ServerSyncProvider>().api.getCurveData(
            id: id,
            overrides: config,
          );
      if (preview == null) return;
      final data = CurveData(
        hours: preview.hours,
        brightness: preview.brightness,
        kelvin: preview.kelvin,
        solar: SolarInfo(
          sunrise: preview.solar.sunrise,
          sunset: preview.solar.sunset,
          solarNoon: preview.solar.solarNoon,
          solarMidnight: preview.solar.solarMidnight,
          dayLength: preview.solar.dayLength,
          dawn: preview.solar.dawn != null
              ? TwilightPhase(
                  civil: preview.solar.dawn!.civil,
                  nautical: preview.solar.dawn!.nautical,
                  astronomical: preview.solar.dawn!.astronomical,
                )
              : null,
          dusk: preview.solar.dusk != null
              ? TwilightPhase(
                  civil: preview.solar.dusk!.civil,
                  nautical: preview.solar.dusk!.nautical,
                  astronomical: preview.solar.dusk!.astronomical,
                )
              : null,
        ),
      );
      if (mounted && requestId == _curvePreviewRequestId) {
        setState(() {
          _curveData = data;
          _compressedPositions = _computeCompressedMapping(data);
        });
      }
    } catch (e) {
      debugPrint('LightProfile: Failed to load curve data: $e');
    }
  }

  void _onCurveChanged(void Function() update) {
    setState(() {
      update();
      _curveConfigDirty = true;
    });
  }

  void _onPreviewRangeChanged(void Function() update) {
    _onCurveChanged(update);
    _queueDraftCurvePreviewRefresh();
  }

  void _queueDraftCurvePreviewRefresh() {
    if (_selectedProfileConfig == null || _isSleepProfile) return;
    _curvePreviewRefreshQueued = true;
    if (_curvePreviewRefreshInFlight ||
        (_curvePreviewRefreshTimer?.isActive ?? false)) {
      return;
    }

    _curvePreviewRefreshTimer =
        Timer(const Duration(milliseconds: 90), () async {
      _curvePreviewRefreshTimer = null;
      if (!_curvePreviewRefreshQueued || !mounted) return;

      _curvePreviewRefreshQueued = false;
      _curvePreviewRefreshInFlight = true;
      try {
        await _loadCurveData(config: _buildDraftConfig());
      } finally {
        _curvePreviewRefreshInFlight = false;
        if (mounted && _curvePreviewRefreshQueued) {
          _queueDraftCurvePreviewRefresh();
        }
      }
    });
  }

  void _onIntervalChanged(double value) {
    _onCurveChanged(() => _intervalSecs = value);
  }

  String _formatInterval(double secs) {
    if (secs >= 60 && secs % 60 == 0) return '${(secs / 60).round()}m';
    return '${secs.round()}s';
  }

  void _applyProfileConfig(sdk.RhythmCurveConfig config) {
    _minColorTemp = config.minColorTemp.toDouble();
    _maxColorTemp = config.maxColorTemp.toDouble();
    _minBrightness = config.minBrightness.toDouble();
    _maxBrightness = config.maxBrightness.toDouble();
    _maxDimSteps = config.maxDimSteps.toDouble();
    _fadeMs = (config.fadeMs ?? 500).toDouble();
    _motionTimeoutAuto = config.motionTimeoutSecs == null;
    _motionTimeoutSecs = config.motionTimeoutSecs ?? 600;
    _intervalAuto = config.rhythmIntervalSecs == null;
    _intervalSecs = (config.rhythmIntervalSecs ?? 60).toDouble();

    final curve = config.curve;
    if (curve is sdk.RhythmSuperGaussianCurve) {
      _widthLeftBri = curve.widthLeftBri;
      _widthRightBri = curve.widthRightBri;
      _widthLeftCct = curve.widthLeftCct;
      _widthRightCct = curve.widthRightCct;
      _shapeP = curve.shapeP;
    }

    final directColor = switch (curve) {
      sdk.RhythmSuperGaussianCurve(:final directColor) => directColor,
      sdk.RhythmConstantCurve(:final directColor) => directColor,
      _ => null,
    };

    if (directColor != null) {
      final rgb = directColor.rgb;
      _sleepHue = HSVColor.fromColor(
        Color.fromARGB(255, rgb.r, rgb.g, rgb.b),
      ).hue;
    }

    _sleepBrightness = switch (curve) {
      sdk.RhythmConstantCurve(:final brightness) => (config.minBrightness +
              (config.maxBrightness - config.minBrightness) * brightness)
          .toDouble()
          .clamp(1, 100),
      _ => config.maxBrightness.toDouble().clamp(1, 100),
    };

    _curveConfigDirty = false;
  }

  void _applyIdleConfig(sdk.RhythmCurveConfig config) {
    final curve = config.curve;
    if (curve is sdk.RhythmInheritActiveCurve) {
      _idleCustomBri = false;
      _idleCustomColor = false;
      _idleBrightness = 1;
    } else if (curve is sdk.RhythmConstantCurve) {
      _idleBrightness = config.maxBrightness.toDouble().clamp(1, 100);
      _idleCustomBri = config.minBrightness != 1 || config.maxBrightness != 1;
      _idleCustomColor = curve.directColor != null;
      if (curve.directColor != null) {
        final rgb = curve.directColor!.rgb;
        final c = Color.fromARGB(255, rgb.r, rgb.g, rgb.b);
        _idleHue = HSVColor.fromColor(c).hue;
      }
    } else {
      _applyIdleFallback();
    }
    _idleExpanded = _idleCustomBri || _idleCustomColor;
  }

  void _applyIdleFallback() {
    _idleCustomBri = false;
    _idleCustomColor = false;
    _idleBrightness = 1;
  }

  sdk.RhythmCurveConfig _buildDraftConfig() {
    final base = _selectedProfileConfig;
    if (base == null) {
      return const sdk.RhythmCurveConfig();
    }

    if (_isSleepProfile) {
      final brightness = _sleepBrightness.round().clamp(1, 100);
      return sdk.RhythmCurveConfig(
        id: base.id,
        name: base.name,
        minColorTemp: 0,
        maxColorTemp: 0,
        minBrightness: brightness,
        maxBrightness: brightness,
        maxDimSteps: base.maxDimSteps,
        fadeMs: base.fadeMs,
        motionTimeoutSecs: _motionTimeoutAuto ? null : _motionTimeoutSecs,
        rhythmIntervalSecs: _intervalAuto ? null : _intervalSecs.round(),
        curve: sdk.RhythmConstantCurve(
          brightness: 1,
          colorTemp: 0,
          directColor: _sleepDirectColor,
        ),
      );
    }

    final curve = base.curve;
    sdk.RhythmCurveShape nextCurve;
    if (curve is sdk.RhythmSuperGaussianCurve) {
      nextCurve = curve.copyWith(
        widthLeftBri: _widthLeftBri,
        widthRightBri: _widthRightBri,
        widthLeftCct: _widthLeftCct,
        widthRightCct: _widthRightCct,
        shapeP: _shapeP,
      );
    } else {
      nextCurve = curve;
    }

    return sdk.RhythmCurveConfig(
      id: base.id,
      name: base.name,
      minColorTemp: _minColorTemp.round(),
      maxColorTemp: _maxColorTemp.round(),
      minBrightness: _minBrightness.round(),
      maxBrightness: _maxBrightness.round(),
      maxDimSteps: _maxDimSteps.round(),
      fadeMs: base.fadeMs,
      motionTimeoutSecs: _motionTimeoutAuto ? null : _motionTimeoutSecs,
      rhythmIntervalSecs: _intervalAuto ? null : _intervalSecs.round(),
      curve: nextCurve,
    );
  }

  sdk.RhythmCurveConfig? _buildIdleDraftConfig() {
    if (!_idleCustomBri && !_idleCustomColor) return null;

    final idleProfileId = _defaultIdleProfileId;
    final base = _profileConfigs[idleProfileId] ?? _defaultIdleProfileConfig();

    final bri = _idleCustomBri ? _idleBrightness.round() : 1;
    sdk.RhythmDirectColor? directColor;

    if (_idleCustomColor) {
      final color = _idleSelectedColor;
      directColor = _rgbToDirectColor(
        (color.r * 255).round(),
        (color.g * 255).round(),
        (color.b * 255).round(),
      );
    }

    return base.copyWith(
      id: idleProfileId,
      minBrightness: bri,
      maxBrightness: bri,
      minColorTemp: 0,
      maxColorTemp: 0,
      curve: sdk.RhythmConstantCurve(
        brightness: 1,
        colorTemp: 0,
        directColor: directColor,
      ),
    );
  }

  sdk.RhythmCurveConfig _defaultIdleProfileConfig() {
    return sdk.RhythmCurveConfig(
      id: _defaultIdleProfileId,
      name: _defaultIdleProfileNameForMode(_selectedMode),
      curve: const sdk.RhythmInheritActiveCurve(),
      minColorTemp: 0,
      maxColorTemp: 0,
      minBrightness: 1,
      maxBrightness: 1,
      maxDimSteps: 1,
      fadeMs: null,
      motionTimeoutSecs: null,
      rhythmIntervalSecs: null,
    );
  }

  // ---------------------------------------------------------------------------
  // Compressed mapping — rate-of-change based position distribution
  // ---------------------------------------------------------------------------

  /// Build a non-linear mapping from 15-minute time slots to slider positions.
  /// Segments where kelvin/brightness change rapidly get more space.
  /// Flat night/day plateaus are compressed.
  List<double> _computeCompressedMapping(CurveData? data) {
    const n = 96; // 15-minute intervals

    if (data == null || data.hours.isEmpty) {
      return List.generate(n + 1, (i) => i / n);
    }

    final weights = <double>[];
    for (int i = 0; i < n; i++) {
      final h0 = (i / n) * 24;
      final h1 = ((i + 1) / n) * 24;

      final k0 = _interpolateCurve(data.hours, data.kelvin, h0);
      final k1 = _interpolateCurve(data.hours, data.kelvin, h1);
      final b0 = _interpolateCurve(data.hours, data.brightness, h0);
      final b1 = _interpolateCurve(data.hours, data.brightness, h1);

      // Normalized rate of change.
      final dK = (k1 - k0).abs() / 5000; // ~5000K range
      final dB = (b1 - b0).abs() / 100; // 100% range
      final rate = dK + dB;

      // Minimum weight so flat regions aren't invisible — just narrow.
      weights.add(math.max(rate, 0.003));
    }

    final totalWeight = weights.reduce((a, b) => a + b);
    final positions = <double>[0.0];
    var cumulative = 0.0;
    for (int i = 0; i < n; i++) {
      cumulative += weights[i] / totalWeight;
      positions.add(cumulative);
    }
    return positions;
  }

  /// Convert a slider position (0..1) back to an hour (0..24).
  double _positionToHour(double position) {
    final p = _compressedPositions;
    if (p == null) return position * 24;

    // Binary-ish search: find the segment containing this position.
    for (int i = 0; i < p.length - 1; i++) {
      if (position <= p[i + 1]) {
        final segSpan = p[i + 1] - p[i];
        final t = segSpan > 0 ? (position - p[i]) / segSpan : 0.0;
        final hourStart = (i / 96) * 24;
        final hourEnd = ((i + 1) / 96) * 24;
        return hourStart + t * (hourEnd - hourStart);
      }
    }
    return 24.0;
  }

  // ---------------------------------------------------------------------------
  // Time simulator helpers
  // ---------------------------------------------------------------------------

  double _currentHour() {
    final now = DateTime.now();
    return now.hour + now.minute / 60.0;
  }

  double _selectedHour() {
    final h = _currentHour() + _timeOffsetMinutes / 60.0;
    return ((h % 24) + 24) % 24;
  }

  bool get _hasTimeOffset => _timeOffsetMinutes.abs() > 0.5;

  bool get _showTimeOffsetActions => _hasTimeOffset || _timeOffsetPreviewActive;

  String _formatHour(double hour) {
    final h = hour.floor() % 24;
    final m = ((hour - hour.floor()) * 60).round();
    final period = h >= 12 ? 'PM' : 'AM';
    final h12 = h == 0 ? 12 : (h > 12 ? h - 12 : h);
    return '$h12:${m.toString().padLeft(2, '0')} $period';
  }

  int _kelvinAtHour(double hour) {
    if (_curveData == null || _curveData!.hours.isEmpty) return 3000;
    return _interpolateCurve(_curveData!.hours, _curveData!.kelvin, hour)
        .toInt();
  }

  int _brightnessAtHour(double hour) {
    if (_curveData == null || _curveData!.hours.isEmpty) return 50;
    return _interpolateCurve(_curveData!.hours, _curveData!.brightness, hour)
        .toInt();
  }

  Color _previewColorAtHour(double hour) {
    final fixedColor = _selectedFixedColor;
    if (fixedColor != null) return fixedColor;

    final kelvin = _kelvinAtHour(hour);
    if (kelvin > 0) {
      return ColorUtils.curveColorForCCT(kelvin);
    }
    return _Palette.amber;
  }

  String _previewValueLabel(double hour) {
    final brightness = _brightnessAtHour(hour);
    final kelvin = _kelvinAtHour(hour);
    if (_selectedFixedColor != null) {
      return '${_formatHour(hour)}  $brightness%  Fixed Color';
    }
    if (kelvin > 0) {
      return '${_formatHour(hour)}  $brightness%  ${kelvin}K';
    }
    return '${_formatHour(hour)}  $brightness%';
  }

  Future<void> _syncActiveConfigModel(sdk.RhythmCurveConfig config) async {
    final activeProfileId = context.read<ServerSyncProvider>().activeProfileId;
    if (config.id != activeProfileId) return;

    final dto = _toCurveConfigDto(config);
    if (dto != null && mounted) {
      context.read<ConfigModel>().updateConfig(dto);
    }

    await context.read<ServerSyncProvider>().fullRefresh();
  }

  CurveConfigDto? _toCurveConfigDto(sdk.RhythmCurveConfig config) {
    final curve = config.curve;
    if (curve is! sdk.RhythmSuperGaussianCurve) return null;

    return CurveConfigDto(
      minColorTemp: config.minColorTemp,
      maxColorTemp: config.maxColorTemp,
      minBrightness: config.minBrightness,
      maxBrightness: config.maxBrightness,
      widthLeftBri: curve.widthLeftBri,
      widthRightBri: curve.widthRightBri,
      widthLeftCct: curve.widthLeftCct,
      widthRightCct: curve.widthRightCct,
      shapeP: curve.shapeP,
      maxDimSteps: config.maxDimSteps,
      fadeMs: config.fadeMs ?? defaultCurveConfig.fadeMs,
      motionTimeoutSecs:
          config.motionTimeoutSecs ?? defaultCurveConfig.motionTimeoutSecs,
    );
  }

  void _onSliderInteraction(double dx, double trackWidth) {
    final fraction = (dx / trackWidth).clamp(0.0, 1.0);
    final tappedHour = _positionToHour(fraction);

    double offset = (tappedHour - _currentHour()) * 60;
    offset = (offset / 5).roundToDouble() * 5;

    setState(() {
      _timeOffsetMinutes = offset;
      _sliderFraction = fraction;
      _timeOffsetApplied = false;
    });
  }

  bool get _timeOffsetDispatching => _timeOffsetDispatchAction != null;

  bool _isTimeOffsetDispatching(_TimeOffsetDispatchAction action) =>
      _timeOffsetDispatchAction == action;

  Future<void> _applyTimeOffset() async {
    if (_timeOffsetDispatching) return;
    final previewOffset = _timeOffsetMinutes;
    setState(
        () => _timeOffsetDispatchAction = _TimeOffsetDispatchAction.preview);

    try {
      final result = await _sendTimeOffset(offsetMinutes: previewOffset);
      await _waitForTimeOffsetDispatch(result);
      if (!mounted) return;
      setState(() {
        _timeOffsetApplied = _hasTimeOffset;
        _timeOffsetPreviewActive = _hasTimeOffset;
      });
      AnalyticsService().logLightProfilePreviewAction(
        profile: _selectedProfileId,
        action: 'apply',
        offsetMinutes: previewOffset,
      );
    } finally {
      if (mounted) {
        setState(() => _timeOffsetDispatchAction = null);
      }
    }
  }

  Future<sdk.RhythmDispatchResult> _sendTimeOffset({
    required double offsetMinutes,
  }) {
    final api = context.read<ServerSyncProvider>().api;
    return api.nodeOffsetPreviewResult(timeOffset: offsetMinutes);
  }

  Future<void> _waitForTimeOffsetDispatch(
      sdk.RhythmDispatchResult result) async {
    final duration = result.estimatedDispatchDuration ??
        (result.queued
            ? _dispatchDurationForCount(result.dispatchCount ?? 1)
            : null);
    if (duration == null || duration <= Duration.zero) return;
    await Future<void>.delayed(duration);
  }

  Duration? _dispatchDurationForCount(int count) {
    if (count <= 1) return null;
    return Duration(
      milliseconds: _defaultBatchDispatchSpacing.inMilliseconds * (count - 1),
    );
  }

  Future<void> _resetTimeOffset() async {
    if (_timeOffsetDispatching) return;
    final previousOffset = _timeOffsetMinutes;
    setState(() {
      _timeOffsetMinutes = 0;
      _sliderFraction = _hourToNowFraction();
      _timeOffsetApplied = false;
      _timeOffsetPreviewActive = false;
      _timeOffsetDispatchAction = _TimeOffsetDispatchAction.reset;
    });
    try {
      final result = await _sendTimeOffset(offsetMinutes: 0);
      await _waitForTimeOffsetDispatch(result);
      AnalyticsService().logLightProfilePreviewAction(
        profile: _selectedProfileId,
        action: 'reset',
        offsetMinutes: previousOffset,
      );
    } finally {
      if (mounted) {
        setState(() => _timeOffsetDispatchAction = null);
      }
    }
  }

  Future<void> _absorbTimeOffset() async {
    if (_timeOffsetDispatching) return;
    final absorbedOffset = _timeOffsetMinutes;
    setState(
        () => _timeOffsetDispatchAction = _TimeOffsetDispatchAction.absorb);

    try {
      final result = await context
          .read<ServerSyncProvider>()
          .api
          .absorbTimeOffsetResult(absorbedOffset, id: _selectedProfileId);
      if (!mounted) return;
      final sdkConfig = result.config;
      if (sdkConfig != null) {
        _profileConfigs[_selectedProfileId] = sdkConfig;
        _applyProfileConfig(sdkConfig);
        await _syncActiveConfigModel(sdkConfig);
        await _loadCurveData(profileId: _selectedProfileId);
      }
      await _waitForTimeOffsetDispatch(result.dispatch);
      if (!mounted) return;
      setState(() {
        _timeOffsetMinutes = 0;
        _sliderFraction = _hourToNowFraction();
        _timeOffsetApplied = false;
        _timeOffsetPreviewActive = false;
        _curveConfigDirty = false;
      });
      AnalyticsService().logLightProfilePreviewAction(
        profile: _selectedProfileId,
        action: 'absorb',
        offsetMinutes: absorbedOffset,
      );
    } finally {
      if (mounted) {
        setState(() => _timeOffsetDispatchAction = null);
      }
    }
  }

  Future<void> _resetToDefaults() async {
    final api = context.read<ServerSyncProvider>().api;
    final sdkConfig = await api.resetConfig(id: _selectedProfileId);
    if (!mounted) return;
    if (sdkConfig != null) {
      _profileConfigs[_selectedProfileId] = sdkConfig;
      _applyProfileConfig(sdkConfig);
      await _syncActiveConfigModel(sdkConfig);
      await _loadCurveData(profileId: _selectedProfileId);
    }
    setState(() {
      _timeOffsetMinutes = 0;
      _sliderFraction = _hourToNowFraction();
      _curveConfigDirty = false;
      _timeOffsetApplied = false;
    });
    AnalyticsService().logLightProfileReset(_selectedProfileId);
  }

  void _resetCurveConfigToDefaults() {
    if (_isSleepProfile) return;

    final defaults = defaultCurveConfig;
    _onCurveChanged(() {
      _minColorTemp = defaults.minColorTemp.toDouble();
      _maxColorTemp = defaults.maxColorTemp.toDouble();
      _minBrightness = defaults.minBrightness.toDouble();
      _maxBrightness = defaults.maxBrightness.toDouble();
      _widthLeftBri = defaults.widthLeftBri;
      _widthRightBri = defaults.widthRightBri;
      _widthLeftCct = defaults.widthLeftCct;
      _widthRightCct = defaults.widthRightCct;
      _shapeP = defaults.shapeP;
      _maxDimSteps = defaults.maxDimSteps.toDouble();
    });
    _queueDraftCurvePreviewRefresh();
    AnalyticsService().logLightProfileReset(_selectedProfileId);
  }

  void _showSaveFeedback(String message, {required bool error}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        backgroundColor: error ? Colors.red.shade700 : Colors.green.shade700,
      ),
    );
  }

  /// Current time as a compressed slider fraction.
  double _hourToNowFraction() {
    final p = _compressedPositions;
    if (p == null) return _currentHour() / 24;
    final idx = (_currentHour() / 24 * 96).clamp(0.0, 96.0);
    final lower = idx.floor().clamp(0, 95);
    final upper = (lower + 1).clamp(0, 96);
    final t = idx - lower;
    return p[lower] + t * (p[upper] - p[lower]);
  }

  // ---------------------------------------------------------------------------
  // Build
  // ---------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: _Palette.bg,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: _loading
                  ? const Center(
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        color: _Palette.amber,
                      ),
                    )
                  : _connected
                      ? _buildContent()
                      : _buildDisconnected(),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _Palette.amber.withValues(alpha: 0.12),
                border: Border.all(
                  color: _Palette.amber.withValues(alpha: 0.25),
                ),
              ),
              child: const Icon(Icons.close, color: _Palette.amber, size: 20),
            ),
          ),
          Expanded(
            child: Text(
              _profileTitle,
              textAlign: TextAlign.center,
              style: const TextStyle(
                color: _Palette.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  Widget _buildDisconnected() {
    return Center(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 40),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 64,
              height: 64,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _Palette.textSecondary.withValues(alpha: 0.08),
              ),
              child: Icon(
                Icons.link_off_rounded,
                color: _Palette.textSecondary.withValues(alpha: 0.5),
                size: 28,
              ),
            ),
            const SizedBox(height: 20),
            Text(
              'Device Not Connected',
              style: TextStyle(
                color: _Palette.textPrimary.withValues(alpha: 0.8),
                fontSize: 17,
                fontWeight: FontWeight.w600,
              ),
            ),
            const SizedBox(height: 8),
            Text(
              'Connect to a Rhythm Server to configure the light profile.',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _Palette.textSecondary.withValues(alpha: 0.7),
                fontSize: 14,
                height: 1.4,
              ),
            ),
          ],
        ),
      ),
    );
  }

  String _formatFade(double ms) {
    if (ms == 0) return '0s';
    return '${(ms / 1000).toStringAsFixed(1)}s';
  }

  String _formatMotionTimeout(int secs) {
    if (secs == 0) return 'Off';
    if (secs >= 60) return '${(secs / 60).round()}m';
    return '${secs}s';
  }

  Widget _buildContent() {
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(20, 8, 20, 40),
      child: Column(
        children: [
          _buildHeroIcon(),
          const SizedBox(height: 24),
          if (_isSleepProfile) ...[
            _buildSleepColorCard(),
            const SizedBox(height: 16),
            _buildSleepBrightnessCard(),
          ] else ...[
            _buildAdvancedColorEditorItem(),
          ],
          const SizedBox(height: 24),
          _buildMotionTimeoutCard(),
          const SizedBox(height: 24),
          _buildIdleSection(),
          const SizedBox(height: 24),
          _buildRoomDefaultsSection(),
          if (_curveConfigDirty) ...[
            const SizedBox(height: 24),
            _buildSaveButton(),
          ],
          const SizedBox(height: 32),
          _buildResetToDefaultsButton(),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Sleep Profile Color Picker
  // ---------------------------------------------------------------------------

  Widget _buildSleepColorCard() {
    final activeColor = _sleepSelectedColor;

    return Container(
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 18),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _Palette.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 36,
                height: 36,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: activeColor.withValues(alpha: 0.2),
                ),
                child:
                    Icon(Icons.palette_rounded, color: activeColor, size: 18),
              ),
              const SizedBox(width: 12),
              const Expanded(
                child: Text(
                  'Fixed Color',
                  style: TextStyle(
                    color: _Palette.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                    letterSpacing: -0.1,
                  ),
                ),
              ),
              Container(
                width: 28,
                height: 28,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: activeColor.withValues(alpha: 0.85),
                  border: Border.all(
                    color: Colors.white.withValues(alpha: 0.18),
                  ),
                ),
              ),
            ],
          ),
          Padding(
            padding: const EdgeInsets.only(top: 18),
            child: _buildFixedColorPicker(
              hue: _sleepHue,
              selectedColor: activeColor,
              isPresetSelected: _isSleepPresetSelected,
              onHueChanged: (hue) {
                setState(() {
                  _sleepHue = hue;
                  _curveConfigDirty = true;
                });
              },
            ),
          ),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Idle Section — folded into day/sleep profiles
  // ---------------------------------------------------------------------------

  Widget _buildIdleSection() {
    final isDefault = !_idleCustomBri && !_idleCustomColor;
    final briColor = _idleCustomBri ? _Palette.amber : _Palette.idle;
    final colorColor = _idleCustomColor ? _idleSelectedColor : _Palette.idle;
    final expanded = _idleExpanded;

    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 18),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: expanded
              ? _Palette.idle.withValues(alpha: 0.25)
              : _Palette.border,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Header — tappable to expand/collapse.
          GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: () => setState(() => _idleExpanded = !_idleExpanded),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Container(
                      width: 36,
                      height: 36,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: _Palette.idle.withValues(alpha: 0.12),
                      ),
                      child: Icon(
                        Icons.brightness_low_rounded,
                        color: _Palette.idle.withValues(alpha: 0.7),
                        size: 18,
                      ),
                    ),
                    const SizedBox(width: 12),
                    const Expanded(
                      child: Text(
                        'Standby Settings',
                        style: TextStyle(
                          color: _Palette.textPrimary,
                          fontSize: 15,
                          fontWeight: FontWeight.w600,
                          letterSpacing: -0.1,
                        ),
                      ),
                    ),
                    if (!expanded && isDefault)
                      Container(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 8, vertical: 4),
                        decoration: BoxDecoration(
                          color: _Palette.idle.withValues(alpha: 0.06),
                          borderRadius: BorderRadius.circular(6),
                        ),
                        child: Text(
                          'Auto',
                          style: TextStyle(
                            color: _Palette.idle.withValues(alpha: 0.5),
                            fontSize: 11,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                    if (!expanded && _idleCustomBri) ...[
                      Container(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 8, vertical: 4),
                        decoration: BoxDecoration(
                          color: _Palette.amber.withValues(alpha: 0.12),
                          borderRadius: BorderRadius.circular(6),
                          border: Border.all(
                              color: _Palette.amber.withValues(alpha: 0.2)),
                        ),
                        child: Text(
                          '${_idleBrightness.round()}%',
                          style: TextStyle(
                            color: _Palette.amber.withValues(alpha: 0.8),
                            fontSize: 11,
                            fontWeight: FontWeight.w600,
                            fontFeatures: const [FontFeature.tabularFigures()],
                          ),
                        ),
                      ),
                    ],
                    if (!expanded && _idleCustomColor) ...[
                      if (_idleCustomBri) const SizedBox(width: 6),
                      Container(
                        width: 26,
                        height: 26,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: _idleSelectedColor,
                          border: Border.all(
                            color: _idleSelectedColor.withValues(alpha: 0.4),
                            width: 2,
                          ),
                        ),
                      ),
                    ],
                    const SizedBox(width: 6),
                    AnimatedRotation(
                      turns: expanded ? 0.5 : 0,
                      duration: const Duration(milliseconds: 250),
                      curve: Curves.easeOutCubic,
                      child: Icon(
                        Icons.keyboard_arrow_down_rounded,
                        color: _Palette.textSecondary.withValues(alpha: 0.3),
                        size: 20,
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: expanded
                ? Column(
                    children: [
                      const SizedBox(height: 18),
                      // Custom Brightness toggle + slider.
                      Row(
                        children: [
                          Icon(
                            Icons.brightness_medium_rounded,
                            color: briColor.withValues(
                                alpha: _idleCustomBri ? 0.8 : 0.35),
                            size: 16,
                          ),
                          const SizedBox(width: 10),
                          Expanded(
                            child: Text(
                              'Custom Brightness',
                              style: TextStyle(
                                color: _idleCustomBri
                                    ? _Palette.textPrimary
                                    : _Palette.textSecondary
                                        .withValues(alpha: 0.5),
                                fontSize: 13,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                          ),
                          if (_idleCustomBri)
                            Padding(
                              padding: const EdgeInsets.only(right: 8),
                              child: Text(
                                '${_idleBrightness.round()}%',
                                style: TextStyle(
                                  color: briColor.withValues(alpha: 0.7),
                                  fontSize: 13,
                                  fontWeight: FontWeight.w700,
                                  fontFeatures: const [
                                    FontFeature.tabularFigures()
                                  ],
                                ),
                              ),
                            ),
                          SizedBox(
                            height: 28,
                            child: Switch.adaptive(
                              value: _idleCustomBri,
                              onChanged: (v) {
                                setState(() {
                                  _idleCustomBri = v;
                                  if (v && _idleBrightness < 1) {
                                    _idleBrightness = 1;
                                  }
                                  _curveConfigDirty = true;
                                });
                              },
                              activeTrackColor: briColor,
                              activeThumbColor: _Palette.textPrimary,
                            ),
                          ),
                        ],
                      ),
                      AnimatedSize(
                        duration: const Duration(milliseconds: 300),
                        curve: Curves.easeOutCubic,
                        alignment: Alignment.topCenter,
                        child: _idleCustomBri
                            ? Padding(
                                padding:
                                    const EdgeInsets.only(top: 8, left: 26),
                                child: SliderTheme(
                                  data: SliderThemeData(
                                    activeTrackColor: briColor,
                                    inactiveTrackColor:
                                        briColor.withValues(alpha: 0.12),
                                    thumbColor: briColor,
                                    overlayColor:
                                        briColor.withValues(alpha: 0.12),
                                    trackHeight: 4,
                                    thumbShape: const RoundSliderThumbShape(
                                        enabledThumbRadius: 7),
                                    overlayShape: const RoundSliderOverlayShape(
                                        overlayRadius: 16),
                                  ),
                                  child: Slider(
                                    value: _idleBrightness.clamp(1, 100),
                                    min: 1,
                                    max: 100,
                                    divisions: 99,
                                    onChanged: (v) {
                                      setState(() {
                                        _idleBrightness = v;
                                        _curveConfigDirty = true;
                                      });
                                    },
                                  ),
                                ),
                              )
                            : const SizedBox.shrink(),
                      ),
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 12),
                        child: Divider(
                          height: 1,
                          color: _Palette.border.withValues(alpha: 0.5),
                        ),
                      ),
                      // Custom Color toggle + picker.
                      Row(
                        children: [
                          Icon(
                            Icons.palette_outlined,
                            color: colorColor.withValues(
                                alpha: _idleCustomColor ? 0.8 : 0.35),
                            size: 16,
                          ),
                          const SizedBox(width: 10),
                          Expanded(
                            child: Text(
                              'Custom Color',
                              style: TextStyle(
                                color: _idleCustomColor
                                    ? _Palette.textPrimary
                                    : _Palette.textSecondary
                                        .withValues(alpha: 0.5),
                                fontSize: 13,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                          ),
                          SizedBox(
                            height: 28,
                            child: Switch.adaptive(
                              value: _idleCustomColor,
                              onChanged: (v) {
                                setState(() {
                                  _idleCustomColor = v;
                                  _curveConfigDirty = true;
                                });
                              },
                              activeTrackColor: colorColor,
                              activeThumbColor: _Palette.textPrimary,
                            ),
                          ),
                        ],
                      ),
                      AnimatedSize(
                        duration: const Duration(milliseconds: 350),
                        curve: Curves.easeOutCubic,
                        alignment: Alignment.topCenter,
                        child: _idleCustomColor
                            ? Padding(
                                padding: const EdgeInsets.only(top: 16),
                                child: _buildIdleColorPicker(),
                              )
                            : const SizedBox.shrink(),
                      ),
                    ],
                  )
                : const SizedBox.shrink(),
          ),
        ],
      ),
    );
  }

  sdk.RhythmDirectColor get _sleepDirectColor {
    final color = HSVColor.fromAHSV(1, _sleepHue, 0.85, 1).toColor();
    return _rgbToDirectColor(
      (color.r * 255).round(),
      (color.g * 255).round(),
      (color.b * 255).round(),
    );
  }

  Color get _sleepSelectedColor =>
      HSVColor.fromAHSV(1, _sleepHue, 0.85, 1).toColor();

  bool _isSleepPresetSelected(Color presetColor) {
    final selected = _sleepSelectedColor;
    return (selected.r - presetColor.r).abs() < 0.04 &&
        (selected.g - presetColor.g).abs() < 0.04 &&
        (selected.b - presetColor.b).abs() < 0.04;
  }

  Widget _buildSleepBrightnessCard() {
    const color = _Palette.amber;
    final brightness = _sleepBrightness.round();

    return Container(
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 10),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _Palette.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 36,
                height: 36,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: color.withValues(alpha: 0.12),
                ),
                child:
                    const Icon(Icons.wb_sunny_rounded, color: color, size: 18),
              ),
              const SizedBox(width: 12),
              const Expanded(
                child: Text(
                  'Brightness',
                  style: TextStyle(
                    color: _Palette.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                    letterSpacing: -0.1,
                  ),
                ),
              ),
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                decoration: BoxDecoration(
                  color: color.withValues(alpha: 0.12),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: color.withValues(alpha: 0.25)),
                ),
                child: Text(
                  '$brightness%',
                  style: const TextStyle(
                    color: color,
                    fontSize: 14,
                    fontWeight: FontWeight.w700,
                    fontFeatures: [FontFeature.tabularFigures()],
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          _buildInlineSlider(
            label: 'Level',
            value: _sleepBrightness,
            min: 1,
            max: 100,
            divisions: 99,
            format: (v) => '${v.round()}%',
            color: color,
            onChanged: (v) => _onCurveChanged(() => _sleepBrightness = v),
          ),
        ],
      ),
    );
  }

  // Shared presets for all fixed/direct-color pickers.
  static const _fixedColorPresets = <({Color color, double hue, String label})>[
    (color: Color(0xFFFF3B30), hue: 4, label: 'Red'),
    (color: Color(0xFFFF6B4A), hue: 14, label: 'Ember'),
    (color: Color(0xFFFF9500), hue: 35, label: 'Amber'),
    (color: Color(0xFFFFB347), hue: 33, label: 'Peach'),
    (color: Color(0xFFFF6E8A), hue: 345, label: 'Rose'),
    (color: Color(0xFFE8A87C), hue: 24, label: 'Sand'),
  ];

  Widget _buildIdleColorPicker() {
    return _buildFixedColorPicker(
      hue: _idleHue,
      selectedColor: _idleSelectedColor,
      isPresetSelected: _isPresetSelected,
      onHueChanged: (hue) {
        setState(() {
          _idleHue = hue;
          _curveConfigDirty = true;
        });
      },
    );
  }

  Widget _buildFixedColorPicker({
    required double hue,
    required Color selectedColor,
    required bool Function(Color presetColor) isPresetSelected,
    required ValueChanged<double> onHueChanged,
  }) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        final thumbX = (hue / 360) * width;

        return Column(
          children: [
            GestureDetector(
              onTapDown: (d) => _onFixedColorSpectrumTap(
                  d.localPosition.dx, width, onHueChanged),
              onHorizontalDragUpdate: (d) => _onFixedColorSpectrumTap(
                  d.localPosition.dx, width, onHueChanged),
              child: SizedBox(
                height: 44,
                child: Stack(
                  clipBehavior: Clip.none,
                  children: [
                    Container(
                      height: 32,
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(16),
                        gradient: const LinearGradient(
                          colors: [
                            Color(0xFFFF0000),
                            Color(0xFFFF8800),
                            Color(0xFFFFFF00),
                            Color(0xFF00FF00),
                            Color(0xFF00FFFF),
                            Color(0xFF0088FF),
                            Color(0xFF0000FF),
                            Color(0xFF8800FF),
                            Color(0xFFFF00FF),
                            Color(0xFFFF0044),
                            Color(0xFFFF0000),
                          ],
                        ),
                        border: Border.all(
                          color: Colors.white.withValues(alpha: 0.06),
                        ),
                      ),
                    ),
                    Positioned(
                      left: thumbX - 10,
                      top: 0,
                      child: Container(
                        width: 20,
                        height: 32,
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(10),
                          border: Border.all(
                            color: Colors.white.withValues(alpha: 0.9),
                            width: 2.5,
                          ),
                          boxShadow: [
                            BoxShadow(
                              color: selectedColor.withValues(alpha: 0.5),
                              blurRadius: 12,
                              spreadRadius: 1,
                            ),
                            BoxShadow(
                              color: Colors.black.withValues(alpha: 0.4),
                              blurRadius: 4,
                            ),
                          ],
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 18),
            Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: _fixedColorPresets.map((preset) {
                final selected = isPresetSelected(preset.color);
                return GestureDetector(
                  onTap: () => onHueChanged(preset.hue),
                  child: Column(
                    children: [
                      Container(
                        width: 40,
                        height: 40,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: preset.color
                              .withValues(alpha: selected ? 0.9 : 0.4),
                          border: Border.all(
                            color: selected
                                ? Colors.white.withValues(alpha: 0.7)
                                : Colors.white.withValues(alpha: 0.06),
                            width: selected ? 2.5 : 1,
                          ),
                          boxShadow: selected
                              ? [
                                  BoxShadow(
                                    color: preset.color.withValues(alpha: 0.4),
                                    blurRadius: 16,
                                    spreadRadius: 2,
                                  ),
                                ]
                              : null,
                        ),
                      ),
                      const SizedBox(height: 6),
                      Text(
                        preset.label,
                        style: TextStyle(
                          color: selected
                              ? _Palette.textPrimary
                              : _Palette.textSecondary.withValues(alpha: 0.5),
                          fontSize: 10,
                          fontWeight:
                              selected ? FontWeight.w600 : FontWeight.w500,
                        ),
                      ),
                    ],
                  ),
                );
              }).toList(),
            ),
          ],
        );
      },
    );
  }

  void _onFixedColorSpectrumTap(
    double dx,
    double width,
    ValueChanged<double> onHueChanged,
  ) {
    final fraction = (dx / width).clamp(0.0, 1.0);
    onHueChanged(fraction * 360);
  }

  /// The currently selected idle color — from the shared fixed-color picker.
  Color get _idleSelectedColor =>
      HSVColor.fromAHSV(1, _idleHue, 0.85, 1).toColor();

  bool _isPresetSelected(Color presetColor) {
    final selected = _idleSelectedColor;
    return (selected.r - presetColor.r).abs() < 0.04 &&
        (selected.g - presetColor.g).abs() < 0.04 &&
        (selected.b - presetColor.b).abs() < 0.04;
  }

  /// Convert sRGB (0-255) to CIE 1931 XY + build [RhythmDirectColor].
  static sdk.RhythmDirectColor _rgbToDirectColor(int r, int g, int b) {
    // 1. Linearise sRGB.
    double linearise(int c) {
      final s = c / 255.0;
      return s <= 0.04045
          ? s / 12.92
          : math.pow((s + 0.055) / 1.055, 2.4).toDouble();
    }

    final rl = linearise(r);
    final gl = linearise(g);
    final bl = linearise(b);

    // 2. Wide-gamut D65 matrix (matches Philips Hue's recommendation).
    final x = rl * 0.4124564 + gl * 0.3575761 + bl * 0.1804375;
    final y = rl * 0.2126729 + gl * 0.7151522 + bl * 0.0721750;
    final z = rl * 0.0193339 + gl * 0.1191920 + bl * 0.9503041;

    final sum = x + y + z;
    final cx = sum > 0 ? x / sum : 0.3127; // D65 white point fallback
    final cy = sum > 0 ? y / sum : 0.3290;

    return sdk.RhythmDirectColor(
      rgb: sdk.RhythmRgbColor(r: r, g: g, b: b),
      xy: sdk.RhythmXyColor(
        x: double.parse(cx.toStringAsFixed(4)),
        y: double.parse(cy.toStringAsFixed(4)),
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Brightness Range Card — primary control
  // ---------------------------------------------------------------------------

  Widget _buildBrightnessRangeCard() {
    const color = _Palette.amber;
    final minPct = _minBrightness.round();
    final maxPct = _maxBrightness.round();
    final expanded = _brightnessExpanded;

    return GestureDetector(
      onTap: () => setState(() => _brightnessExpanded = !_brightnessExpanded),
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 300),
        curve: Curves.easeOutCubic,
        padding: EdgeInsets.fromLTRB(18, 18, 18, expanded ? 10 : 18),
        decoration: BoxDecoration(
          color: _Palette.card,
          borderRadius: BorderRadius.circular(18),
          border: Border.all(
            color: expanded ? color.withValues(alpha: 0.25) : _Palette.border,
          ),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Container(
                  width: 36,
                  height: 36,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: color.withValues(alpha: 0.12),
                  ),
                  child: const Icon(Icons.wb_sunny_rounded,
                      color: color, size: 18),
                ),
                const SizedBox(width: 12),
                const Expanded(
                  child: Text(
                    'Brightness',
                    style: TextStyle(
                      color: _Palette.textPrimary,
                      fontSize: 15,
                      fontWeight: FontWeight.w600,
                      letterSpacing: -0.1,
                    ),
                  ),
                ),
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                  decoration: BoxDecoration(
                    color: color.withValues(alpha: 0.08),
                    borderRadius: BorderRadius.circular(8),
                    border: Border.all(color: color.withValues(alpha: 0.15)),
                  ),
                  child: Text(
                    '$minPct%',
                    style: TextStyle(
                      color: color.withValues(alpha: 0.5),
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ),
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 5),
                  child: Text(
                    '–',
                    style: TextStyle(
                      color: _Palette.textSecondary.withValues(alpha: 0.3),
                      fontSize: 13,
                    ),
                  ),
                ),
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                  decoration: BoxDecoration(
                    color: color.withValues(alpha: 0.12),
                    borderRadius: BorderRadius.circular(8),
                    border: Border.all(color: color.withValues(alpha: 0.25)),
                  ),
                  child: Text(
                    '$maxPct%',
                    style: const TextStyle(
                      color: color,
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                      fontFeatures: [FontFeature.tabularFigures()],
                    ),
                  ),
                ),
                const SizedBox(width: 6),
                AnimatedRotation(
                  turns: expanded ? 0.5 : 0,
                  duration: const Duration(milliseconds: 250),
                  curve: Curves.easeOutCubic,
                  child: Icon(
                    Icons.keyboard_arrow_down_rounded,
                    color: _Palette.textSecondary.withValues(alpha: 0.3),
                    size: 20,
                  ),
                ),
              ],
            ),
            AnimatedSize(
              duration: const Duration(milliseconds: 300),
              curve: Curves.easeOutCubic,
              alignment: Alignment.topCenter,
              child: expanded
                  ? Padding(
                      padding: const EdgeInsets.only(top: 6),
                      child: Column(
                        children: [
                          _buildInlineSlider(
                            label: 'Min',
                            value: _minBrightness,
                            min: 1,
                            max: 50,
                            divisions: 49,
                            format: (v) => '${v.round()}%',
                            color: color.withValues(alpha: 0.5),
                            onChanged: (v) => _onPreviewRangeChanged(
                                () => _minBrightness = v),
                          ),
                          _buildInlineSlider(
                            label: 'Max',
                            value: _maxBrightness,
                            min: 20,
                            max: 100,
                            divisions: 80,
                            format: (v) => '${v.round()}%',
                            color: color,
                            onChanged: (v) => _onPreviewRangeChanged(
                                () => _maxBrightness = v),
                          ),
                        ],
                      ),
                    )
                  : const SizedBox.shrink(),
            ),
          ],
        ),
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Color Temperature Card — kelvin range with gradient preview
  // ---------------------------------------------------------------------------

  Widget _buildColorTempRangeCard() {
    final warmColor = ColorUtils.curveColorForCCT(_minColorTemp.round());
    final coolColor = ColorUtils.curveColorForCCT(_maxColorTemp.round());
    final expanded = _colorTempExpanded;

    return GestureDetector(
      onTap: () => setState(() => _colorTempExpanded = !_colorTempExpanded),
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 300),
        curve: Curves.easeOutCubic,
        padding: EdgeInsets.fromLTRB(18, 18, 18, expanded ? 10 : 18),
        decoration: BoxDecoration(
          color: _Palette.card,
          borderRadius: BorderRadius.circular(18),
          border: Border.all(
            color:
                expanded ? warmColor.withValues(alpha: 0.2) : _Palette.border,
          ),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Container(
                  width: 36,
                  height: 36,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    gradient: LinearGradient(
                      colors: [
                        warmColor.withValues(alpha: 0.18),
                        coolColor.withValues(alpha: 0.18),
                      ],
                    ),
                  ),
                  child: Icon(Icons.thermostat_rounded,
                      color: warmColor, size: 18),
                ),
                const SizedBox(width: 12),
                const Expanded(
                  child: Text(
                    'Color Temperature',
                    style: TextStyle(
                      color: _Palette.textPrimary,
                      fontSize: 15,
                      fontWeight: FontWeight.w600,
                      letterSpacing: -0.1,
                    ),
                  ),
                ),
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                  decoration: BoxDecoration(
                    color: warmColor.withValues(alpha: 0.12),
                    borderRadius: BorderRadius.circular(8),
                    border: Border.all(color: warmColor.withValues(alpha: 0.2)),
                  ),
                  child: Text(
                    '${_minColorTemp.round()}K',
                    style: TextStyle(
                      color: warmColor,
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ),
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 5),
                  child: Text(
                    '–',
                    style: TextStyle(
                      color: _Palette.textSecondary.withValues(alpha: 0.3),
                      fontSize: 13,
                    ),
                  ),
                ),
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                  decoration: BoxDecoration(
                    color: coolColor.withValues(alpha: 0.12),
                    borderRadius: BorderRadius.circular(8),
                    border: Border.all(color: coolColor.withValues(alpha: 0.2)),
                  ),
                  child: Text(
                    '${_maxColorTemp.round()}K',
                    style: TextStyle(
                      color: coolColor,
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ),
                const SizedBox(width: 6),
                AnimatedRotation(
                  turns: expanded ? 0.5 : 0,
                  duration: const Duration(milliseconds: 250),
                  curve: Curves.easeOutCubic,
                  child: Icon(
                    Icons.keyboard_arrow_down_rounded,
                    color: _Palette.textSecondary.withValues(alpha: 0.3),
                    size: 20,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 14),
            // Kelvin gradient strip — always visible as a preview
            Container(
              height: 6,
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(3),
                gradient: LinearGradient(
                  colors: List.generate(8, (i) {
                    final k = _minColorTemp +
                        (i / 7) * (_maxColorTemp - _minColorTemp);
                    return ColorUtils.curveColorForCCT(k.round());
                  }),
                ),
              ),
            ),
            AnimatedSize(
              duration: const Duration(milliseconds: 300),
              curve: Curves.easeOutCubic,
              alignment: Alignment.topCenter,
              child: expanded
                  ? Padding(
                      padding: const EdgeInsets.only(top: 4),
                      child: Column(
                        children: [
                          _buildInlineSlider(
                            label: 'Min',
                            value: _minColorTemp,
                            min: 1500,
                            max: 4000,
                            divisions: 25,
                            format: (v) => '${v.round()}K',
                            color: warmColor,
                            onChanged: (v) =>
                                _onPreviewRangeChanged(() => _minColorTemp = v),
                          ),
                          _buildInlineSlider(
                            label: 'Max',
                            value: _maxColorTemp,
                            min: 2000,
                            max: 6500,
                            divisions: 45,
                            format: (v) => '${v.round()}K',
                            color: coolColor,
                            onChanged: (v) =>
                                _onPreviewRangeChanged(() => _maxColorTemp = v),
                          ),
                        ],
                      ),
                    )
                  : const SizedBox.shrink(),
            ),
          ],
        ),
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Background Light Interval card
  // ---------------------------------------------------------------------------

  Widget _buildIntervalRow() {
    const color = _Palette.blue;
    return Column(
      children: [
        Row(
          children: [
            Container(
              width: 30,
              height: 30,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: color.withValues(alpha: 0.1),
              ),
              child: Icon(Icons.update_rounded, color: color, size: 15),
            ),
            const SizedBox(width: 12),
            const Expanded(
              child: Text(
                'Background Interval',
                style: TextStyle(
                  color: _Palette.textSecondary,
                  fontSize: 14,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
            GestureDetector(
              onTap: () {
                setState(() {
                  _intervalAuto = !_intervalAuto;
                  _curveConfigDirty = true;
                });
              },
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                decoration: BoxDecoration(
                  color: _intervalAuto
                      ? color.withValues(alpha: 0.12)
                      : color.withValues(alpha: 0.06),
                  borderRadius: BorderRadius.circular(6),
                  border: Border.all(
                    color: _intervalAuto
                        ? color.withValues(alpha: 0.25)
                        : color.withValues(alpha: 0.12),
                  ),
                ),
                child: Text(
                  _intervalAuto ? 'Auto' : _formatInterval(_intervalSecs),
                  style: TextStyle(
                    color: _intervalAuto ? color : color.withValues(alpha: 0.7),
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ),
            if (_intervalAuto) ...[
              const SizedBox(width: 8),
              Text(
                _formatInterval(_intervalSecs),
                style: TextStyle(
                  color: _Palette.textSecondary.withValues(alpha: 0.5),
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ],
        ),
        AnimatedSize(
          duration: const Duration(milliseconds: 250),
          curve: Curves.easeOutCubic,
          alignment: Alignment.topCenter,
          child: _intervalAuto
              ? const SizedBox.shrink()
              : Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: Column(
                    children: [
                      SliderTheme(
                        data: SliderThemeData(
                          activeTrackColor: color,
                          inactiveTrackColor: color.withValues(alpha: 0.12),
                          thumbColor: color,
                          overlayColor: color.withValues(alpha: 0.12),
                          trackHeight: 4,
                          thumbShape: const RoundSliderThumbShape(
                              enabledThumbRadius: 8),
                          overlayShape:
                              const RoundSliderOverlayShape(overlayRadius: 18),
                        ),
                        child: Slider(
                          value: _intervalSecs.clamp(30, 300),
                          min: 30,
                          max: 300,
                          divisions: 27,
                          onChanged: _onIntervalChanged,
                        ),
                      ),
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 6),
                        child: Row(
                          mainAxisAlignment: MainAxisAlignment.spaceBetween,
                          children: [
                            Text(
                              _formatInterval(30),
                              style: TextStyle(
                                color: _Palette.textSecondary
                                    .withValues(alpha: 0.4),
                                fontSize: 11,
                              ),
                            ),
                            Text(
                              _formatInterval(300),
                              style: TextStyle(
                                color: _Palette.textSecondary
                                    .withValues(alpha: 0.4),
                                fontSize: 11,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
        ),
      ],
    );
  }

  Widget _buildMotionTimeoutHeader(Color color) {
    return Row(
      children: [
        Container(
          width: 30,
          height: 30,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: color.withValues(alpha: 0.1),
          ),
          child: Icon(Icons.motion_photos_on_rounded, color: color, size: 15),
        ),
        const SizedBox(width: 12),
        const Expanded(
          child: Text(
            'Motion Timeout',
            style: TextStyle(
              color: _Palette.textSecondary,
              fontSize: 14,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
        GestureDetector(
          onTap: () {
            setState(() {
              _motionTimeoutAuto = !_motionTimeoutAuto;
              _curveConfigDirty = true;
            });
          },
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
            decoration: BoxDecoration(
              color: _motionTimeoutAuto
                  ? color.withValues(alpha: 0.12)
                  : color.withValues(alpha: 0.06),
              borderRadius: BorderRadius.circular(6),
              border: Border.all(
                color: _motionTimeoutAuto
                    ? color.withValues(alpha: 0.25)
                    : color.withValues(alpha: 0.12),
              ),
            ),
            child: Text(
              _motionTimeoutAuto
                  ? 'Auto'
                  : _formatMotionTimeout(_motionTimeoutSecs),
              style: TextStyle(
                color:
                    _motionTimeoutAuto ? color : color.withValues(alpha: 0.7),
                fontSize: 11,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ),
        if (_motionTimeoutAuto) ...[
          const SizedBox(width: 8),
          Text(
            _formatMotionTimeout(_motionTimeoutSecs),
            style: TextStyle(
              color: _Palette.textSecondary.withValues(alpha: 0.5),
              fontSize: 13,
              fontWeight: FontWeight.w600,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        ],
      ],
    );
  }

  Widget _buildMotionTimeoutSlider(Color color) {
    return AnimatedSize(
      duration: const Duration(milliseconds: 250),
      curve: Curves.easeOutCubic,
      alignment: Alignment.topCenter,
      child: _motionTimeoutAuto
          ? const SizedBox.shrink()
          : Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Column(
                children: [
                  SliderTheme(
                    data: SliderThemeData(
                      activeTrackColor: color,
                      inactiveTrackColor: color.withValues(alpha: 0.12),
                      thumbColor: color,
                      overlayColor: color.withValues(alpha: 0.12),
                      trackHeight: 4,
                      thumbShape:
                          const RoundSliderThumbShape(enabledThumbRadius: 8),
                      overlayShape:
                          const RoundSliderOverlayShape(overlayRadius: 18),
                    ),
                    child: Slider(
                      value: _motionTimeoutSecs.toDouble().clamp(30, 1800),
                      min: 30,
                      max: 1800,
                      divisions: 59,
                      onChanged: (v) {
                        setState(() {
                          _motionTimeoutSecs = v.round();
                          _curveConfigDirty = true;
                        });
                      },
                    ),
                  ),
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 6),
                    child: Row(
                      mainAxisAlignment: MainAxisAlignment.spaceBetween,
                      children: [
                        Text('30s',
                            style: TextStyle(
                                color: _Palette.textSecondary
                                    .withValues(alpha: 0.4),
                                fontSize: 11)),
                        Text('30m',
                            style: TextStyle(
                                color: _Palette.textSecondary
                                    .withValues(alpha: 0.4),
                                fontSize: 11)),
                      ],
                    ),
                  ),
                ],
              ),
            ),
    );
  }

  // ---------------------------------------------------------------------------
  // Light Tuning — nav item + screen
  // ---------------------------------------------------------------------------

  Widget _buildAdvancedColorEditorItem() {
    const color = _Palette.amber;
    return GestureDetector(
      onTap: _openAdvancedColorEditor,
      child: Container(
        padding: const EdgeInsets.fromLTRB(18, 16, 14, 16),
        decoration: BoxDecoration(
          color: _Palette.card,
          borderRadius: BorderRadius.circular(18),
          border: Border.all(color: _Palette.border),
        ),
        child: Row(
          children: [
            Container(
              width: 36,
              height: 36,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: color.withValues(alpha: 0.12),
              ),
              child: const Icon(Icons.tune_rounded, color: color, size: 18),
            ),
            const SizedBox(width: 12),
            const Expanded(
              child: Text(
                'Light Tuning',
                style: TextStyle(
                  color: _Palette.textPrimary,
                  fontSize: 15,
                  fontWeight: FontWeight.w600,
                  letterSpacing: -0.1,
                ),
              ),
            ),
            Icon(
              Icons.chevron_right_rounded,
              color: _Palette.textSecondary.withValues(alpha: 0.4),
              size: 22,
            ),
          ],
        ),
      ),
    );
  }

  void _openAdvancedColorEditor() {
    AnalyticsService().logLightProfileAdvancedColorEditorOpened(
      _selectedProfileId,
    );
    Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return _AdvancedColorEditorScreen(parent: this);
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(1, 0),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 350),
        reverseTransitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Time Simulator
  // ---------------------------------------------------------------------------

  Widget _buildTimeSimulator() {
    final selectedHour = _selectedHour();
    final previewColor = _previewColorAtHour(selectedHour);

    return Column(
      children: [
        // Kelvin / brightness readout when active.
        AnimatedSize(
          duration: const Duration(milliseconds: 250),
          curve: Curves.easeOutCubic,
          child: _hasTimeOffset
              ? Padding(
                  padding: const EdgeInsets.only(bottom: 16),
                  child: Text(
                    _previewValueLabel(selectedHour),
                    style: TextStyle(
                      color: previewColor.withValues(alpha: 0.7),
                      fontSize: 14,
                      fontWeight: FontWeight.w600,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                )
              : Padding(
                  padding: const EdgeInsets.only(bottom: 16),
                  child: Text(
                    'Drag to simulate',
                    style: TextStyle(
                      color: _Palette.textSecondary.withValues(alpha: 0.3),
                      fontSize: 12,
                    ),
                  ),
                ),
        ),
        // The gradient slider.
        _buildGradientSlider(),
        // Apply / Reset + Absorb buttons.
        AnimatedSize(
          duration: const Duration(milliseconds: 300),
          curve: Curves.easeOutCubic,
          child: _showTimeOffsetActions
              ? Padding(
                  padding: const EdgeInsets.only(top: 16),
                  child: _buildTimeOffsetActions(),
                )
              : const SizedBox.shrink(),
        ),
      ],
    );
  }

  Widget _buildTimeOffsetActions() {
    final children = <Widget>[];

    if (_timeOffsetPreviewActive) {
      children.add(_buildResetTimeButton());
      if (_timeOffsetApplied && !_isSleepProfile) {
        children
          ..add(const SizedBox(width: 12))
          ..add(_buildAbsorbTimeButton());
      } else if (_hasTimeOffset) {
        children
          ..add(const SizedBox(width: 12))
          ..add(_buildApplyTimeButton());
      }
    } else {
      children
        ..add(_buildClearTimeButton())
        ..add(const SizedBox(width: 12))
        ..add(_buildApplyTimeButton());
    }

    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: children,
    );
  }

  Widget _buildGradientSlider() {
    return LayoutBuilder(
      builder: (context, constraints) {
        final trackWidth = constraints.maxWidth;

        return GestureDetector(
          onTapDown: (details) {
            _onSliderInteraction(details.localPosition.dx, trackWidth);
          },
          onHorizontalDragStart: (details) {
            setState(() => _isDraggingTime = true);
            _onSliderInteraction(details.localPosition.dx, trackWidth);
          },
          onHorizontalDragUpdate: (details) {
            _onSliderInteraction(details.localPosition.dx, trackWidth);
          },
          onHorizontalDragEnd: (_) {
            setState(() => _isDraggingTime = false);
          },
          child: AnimatedBuilder(
            animation: _glowAnimation,
            builder: (context, child) {
              return CustomPaint(
                painter: _TimeGradientPainter(
                  curveData: _curveData,
                  compressedPositions: _compressedPositions,
                  currentHour: _currentHour(),
                  thumbFraction: _sliderFraction,
                  selectedHour: _selectedHour(),
                  fixedColor: _selectedFixedColor,
                  isDragging: _isDraggingTime,
                  hasOffset: _hasTimeOffset,
                  glowPhase: _glowAnimation.value,
                  showTimeMarkers: true,
                ),
                size: Size(trackWidth, 74),
              );
            },
          ),
        );
      },
    );
  }

  Widget _buildClearTimeButton() {
    return GestureDetector(
      onTap: _timeOffsetDispatching
          ? null
          : () {
              setState(() {
                _timeOffsetMinutes = 0;
                _sliderFraction = _hourToNowFraction();
                _timeOffsetApplied = false;
                _timeOffsetPreviewActive = false;
              });
            },
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        decoration: BoxDecoration(
          color: _Palette.card,
          borderRadius: BorderRadius.circular(20),
          border: Border.all(
            color: _Palette.border.withValues(alpha: 0.6),
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.close_rounded,
              color: _Palette.textSecondary.withValues(alpha: 0.5),
              size: 14,
            ),
            const SizedBox(width: 6),
            Text(
              'Clear',
              style: TextStyle(
                color: _Palette.textSecondary.withValues(alpha: 0.6),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildApplyTimeButton() {
    final busy = _isTimeOffsetDispatching(_TimeOffsetDispatchAction.preview);
    return GestureDetector(
      onTap: _timeOffsetDispatching ? null : _applyTimeOffset,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 10),
        decoration: BoxDecoration(
          color: const Color(0xFF2A2520),
          borderRadius: BorderRadius.circular(20),
          border: Border.all(
            color: _Palette.amber.withValues(alpha: 0.4),
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            busy
                ? _buildTimeOffsetSpinner(_Palette.amber, size: 16)
                : Icon(
                    Icons.play_arrow_rounded,
                    color: _Palette.amber.withValues(alpha: 0.8),
                    size: 16,
                  ),
            const SizedBox(width: 6),
            Text(
              busy ? 'Updating Lights' : 'Preview on Lights',
              style: TextStyle(
                color: _Palette.amber.withValues(alpha: 0.8),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildResetTimeButton() {
    final busy = _isTimeOffsetDispatching(_TimeOffsetDispatchAction.reset);
    return GestureDetector(
      onTap: _timeOffsetDispatching ? null : _resetTimeOffset,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        decoration: BoxDecoration(
          color: _Palette.card,
          borderRadius: BorderRadius.circular(20),
          border: Border.all(
            color: _Palette.border.withValues(alpha: 0.6),
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            busy
                ? _buildTimeOffsetSpinner(_Palette.textSecondary, size: 14)
                : Icon(
                    Icons.refresh_rounded,
                    color: _Palette.textSecondary.withValues(alpha: 0.5),
                    size: 14,
                  ),
            const SizedBox(width: 6),
            Text(
              busy ? 'Resetting Lights' : 'Reset',
              style: TextStyle(
                color: _Palette.textSecondary.withValues(alpha: 0.6),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildAbsorbTimeButton() {
    final busy = _isTimeOffsetDispatching(_TimeOffsetDispatchAction.absorb);
    return GestureDetector(
      onTap: _timeOffsetDispatching ? null : _absorbTimeOffset,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        decoration: BoxDecoration(
          color: const Color(0xFF2A2520),
          borderRadius: BorderRadius.circular(20),
          border: Border.all(
            color: const Color(0xFFD4A54A).withValues(alpha: 0.4),
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            busy
                ? _buildTimeOffsetSpinner(const Color(0xFFD4A54A), size: 14)
                : Icon(
                    Icons.check_rounded,
                    color: const Color(0xFFD4A54A).withValues(alpha: 0.8),
                    size: 14,
                  ),
            const SizedBox(width: 6),
            Text(
              busy ? 'Updating Lights' : 'Absorb',
              style: TextStyle(
                color: const Color(0xFFD4A54A).withValues(alpha: 0.8),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildTimeOffsetSpinner(Color color, {required double size}) {
    return SizedBox(
      width: size,
      height: size,
      child: CircularProgressIndicator(
        strokeWidth: 2,
        color: color.withValues(alpha: 0.8),
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Timing Card — compact combined motion timeout + light transition
  // ---------------------------------------------------------------------------

  Widget _buildMotionTimeoutCard() {
    if (_isSleepProfile) return _buildMotionTimeoutOnly();
    return _buildTimingCard();
  }

  Widget _buildMotionTimeoutOnly() {
    const mtColor = _Palette.teal;
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 12),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _Palette.border),
      ),
      child: Column(
        children: [
          _buildMotionTimeoutHeader(mtColor),
          _buildMotionTimeoutSlider(mtColor),
        ],
      ),
    );
  }

  Widget _buildTimingCard() {
    const mtColor = _Palette.teal;
    const ltColor = _Palette.amber;
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 12),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _Palette.border),
      ),
      child: Column(
        children: [
          _buildMotionTimeoutHeader(mtColor),
          _buildMotionTimeoutSlider(mtColor),
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 10),
            child: Divider(
              height: 1,
              color: _Palette.border.withValues(alpha: 0.5),
            ),
          ),
          // Light Transition (read-only auto)
          Row(
            children: [
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: ltColor.withValues(alpha: 0.06),
                ),
                child: Icon(Icons.blur_on_rounded,
                    color: ltColor.withValues(alpha: 0.6), size: 15),
              ),
              const SizedBox(width: 12),
              const Expanded(
                child: Text(
                  'Light Transition',
                  style: TextStyle(
                    color: _Palette.textSecondary,
                    fontSize: 14,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                decoration: BoxDecoration(
                  color: ltColor.withValues(alpha: 0.06),
                  borderRadius: BorderRadius.circular(6),
                ),
                child: Text(
                  'Auto',
                  style: TextStyle(
                    color: ltColor.withValues(alpha: 0.5),
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              Text(
                _formatFade(_fadeMs),
                style: TextStyle(
                  color: _Palette.textSecondary.withValues(alpha: 0.5),
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
          // Background Light Interval
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 10),
            child: Divider(
              height: 1,
              color: _Palette.border.withValues(alpha: 0.5),
            ),
          ),
          _buildIntervalRow(),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Hero icon — color shifts to match simulated CCT
  // ---------------------------------------------------------------------------

  Widget _buildHeroIcon() {
    final selectedHour = _selectedHour();
    final previewColor =
        _hasTimeOffset ? _previewColorAtHour(selectedHour) : _Palette.amber;

    return AnimatedBuilder(
      animation: _glowAnimation,
      builder: (context, child) {
        return Container(
          width: 72,
          height: 72,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: previewColor.withValues(alpha: 0.08),
            border: Border.all(
              color: previewColor.withValues(alpha: 0.18),
            ),
            boxShadow: [
              BoxShadow(
                color:
                    previewColor.withValues(alpha: _glowAnimation.value * 0.15),
                blurRadius: 32,
                spreadRadius: 0,
              ),
            ],
          ),
          child: Icon(
            Icons.lightbulb_rounded,
            color: previewColor.withValues(
              alpha: 0.6 + _glowAnimation.value * 0.4,
            ),
            size: 32,
          ),
        );
      },
    );
  }

  // ---------------------------------------------------------------------------
  // Shared widgets
  // ---------------------------------------------------------------------------

  Widget _buildInlineSlider({
    required String label,
    required double value,
    required double min,
    required double max,
    required int divisions,
    required String Function(double) format,
    required Color color,
    required ValueChanged<double> onChanged,
  }) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          SizedBox(
            width: 44,
            child: Text(
              label,
              style: TextStyle(
                color: _Palette.textSecondary.withValues(alpha: 0.5),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
          Expanded(
            child: SliderTheme(
              data: SliderThemeData(
                activeTrackColor: color,
                inactiveTrackColor: color.withValues(alpha: 0.08),
                thumbColor: color,
                overlayColor: color.withValues(alpha: 0.08),
                trackHeight: 4,
                thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 7),
                overlayShape: const RoundSliderOverlayShape(overlayRadius: 16),
              ),
              child: Slider(
                value: value.clamp(min, max),
                min: min,
                max: max,
                divisions: divisions,
                onChanged: onChanged,
              ),
            ),
          ),
          SizedBox(
            width: 52,
            child: Text(
              format(value),
              textAlign: TextAlign.right,
              style: TextStyle(
                color: color,
                fontSize: 13,
                fontWeight: FontWeight.w700,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Room Defaults
  // ---------------------------------------------------------------------------

  Map<String, String> _roomDefaultsForCurrentMode() {
    final config = _modeConfigForMode(_selectedMode);
    if (config == null) return {};
    return {
      for (final rd in config.roomDefaults) rd.roomId: rd.state,
    };
  }

  void _onRoomDefaultChanged(String roomId, String? newState) {
    final defaults = Map<String, String>.from(_roomDefaultsForCurrentMode());
    if (newState == null) {
      defaults.remove(roomId);
    } else {
      defaults[roomId] = newState;
    }

    final updatedRoomDefaults = defaults.entries
        .map((e) => sdk.RoomDefault(roomId: e.key, state: e.value))
        .toList();

    final hasConfig = _modeConfigs.any((c) => c.mode == _selectedMode);
    List<sdk.RhythmModeConfig> updatedConfigs;
    if (hasConfig) {
      updatedConfigs = _modeConfigs.map((config) {
        if (config.mode == _selectedMode) {
          return config.copyWith(roomDefaults: updatedRoomDefaults);
        }
        return config;
      }).toList();
    } else {
      updatedConfigs = [
        ..._modeConfigs,
        sdk.RhythmModeConfig(
          mode: _selectedMode,
          activeProfileId: '',
          roomDefaults: updatedRoomDefaults,
        ),
      ];
    }

    // Update UI immediately, debounce the server push.
    setState(() => _modeConfigs = updatedConfigs);

    _roomDefaultsDebounce?.cancel();
    _roomDefaultsDebounce = Timer(const Duration(milliseconds: 800), () {
      if (!mounted) return;
      context.read<ServerSyncProvider>().api.modeSet(configs: _modeConfigs);
    });
    AnalyticsService().logLightProfileRoomDefaultChanged(
      profile: _selectedProfileId,
      cleared: newState == null,
    );
  }

  Widget _buildRoomDefaultsSection() {
    return Selector<RoomProvider, List<RoomDto>>(
      selector: (_, provider) =>
          provider.rooms.where(showsInAllRooms).toList(growable: false),
      builder: (context, rooms, child) {
        if (rooms.isEmpty) return const SizedBox.shrink();

        final defaults = _roomDefaultsForCurrentMode();
        final roomIds = rooms.map((room) => room.id).toSet();
        final visibleOverrideCount =
            defaults.keys.where((roomId) => roomIds.contains(roomId)).length;
        final expanded = _roomDefaultsExpanded;
        final hasOverrides = visibleOverrideCount > 0;
        const color = _Palette.blue;

        return AnimatedContainer(
          duration: const Duration(milliseconds: 300),
          curve: Curves.easeOutCubic,
          padding: EdgeInsets.fromLTRB(18, 18, 18, expanded ? 14 : 18),
          decoration: BoxDecoration(
            color: _Palette.card,
            borderRadius: BorderRadius.circular(18),
            border: Border.all(
              color: expanded ? color.withValues(alpha: 0.25) : _Palette.border,
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // Header — tappable to expand/collapse.
              GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: () => setState(
                    () => _roomDefaultsExpanded = !_roomDefaultsExpanded),
                child: Row(
                  children: [
                    Container(
                      width: 36,
                      height: 36,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: color.withValues(alpha: 0.12),
                      ),
                      child: Icon(
                        Icons.meeting_room_rounded,
                        color: color.withValues(alpha: 0.7),
                        size: 18,
                      ),
                    ),
                    const SizedBox(width: 12),
                    const Expanded(
                      child: Text(
                        'Room Defaults',
                        style: TextStyle(
                          color: _Palette.textPrimary,
                          fontSize: 15,
                          fontWeight: FontWeight.w600,
                          letterSpacing: -0.1,
                        ),
                      ),
                    ),
                    if (!expanded && hasOverrides)
                      Container(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 8, vertical: 4),
                        decoration: BoxDecoration(
                          color: color.withValues(alpha: 0.12),
                          borderRadius: BorderRadius.circular(6),
                          border:
                              Border.all(color: color.withValues(alpha: 0.2)),
                        ),
                        child: Text(
                          '$visibleOverrideCount set',
                          style: TextStyle(
                            color: color.withValues(alpha: 0.8),
                            fontSize: 11,
                            fontWeight: FontWeight.w600,
                            fontFeatures: const [FontFeature.tabularFigures()],
                          ),
                        ),
                      ),
                    if (!expanded && !hasOverrides)
                      Container(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 8, vertical: 4),
                        decoration: BoxDecoration(
                          color: color.withValues(alpha: 0.06),
                          borderRadius: BorderRadius.circular(6),
                        ),
                        child: Text(
                          'None',
                          style: TextStyle(
                            color: color.withValues(alpha: 0.4),
                            fontSize: 11,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                    const SizedBox(width: 6),
                    AnimatedRotation(
                      turns: expanded ? 0.5 : 0,
                      duration: const Duration(milliseconds: 250),
                      curve: Curves.easeOutCubic,
                      child: Icon(
                        Icons.keyboard_arrow_down_rounded,
                        color: _Palette.textSecondary.withValues(alpha: 0.3),
                        size: 20,
                      ),
                    ),
                  ],
                ),
              ),
              AnimatedSize(
                duration: const Duration(milliseconds: 300),
                curve: Curves.easeOutCubic,
                alignment: Alignment.topCenter,
                child: expanded
                    ? Padding(
                        padding: const EdgeInsets.only(top: 16),
                        child: GridView.count(
                          crossAxisCount: 2,
                          crossAxisSpacing: 10,
                          mainAxisSpacing: 10,
                          childAspectRatio: 1.55,
                          shrinkWrap: true,
                          physics: const NeverScrollableScrollPhysics(),
                          children: [
                            for (final room in rooms)
                              _RoomDefaultCard(
                                key: ValueKey(room.id),
                                roomId: room.id,
                                roomName: room.name,
                                state: defaults[room.id],
                                onStateChanged: (newState) =>
                                    _onRoomDefaultChanged(room.id, newState),
                              ),
                          ],
                        ),
                      )
                    : const SizedBox.shrink(),
              ),
            ],
          ),
        );
      },
    );
  }

  // ---------------------------------------------------------------------------
  // Save
  // ---------------------------------------------------------------------------

  Widget _buildSaveButton() {
    return GestureDetector(
      onTap: _saveCurveConfig,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 14),
        decoration: BoxDecoration(
          color: _Palette.amber.withValues(alpha: 0.1),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(color: _Palette.amber.withValues(alpha: 0.25)),
        ),
        child: const Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(Icons.save_rounded, color: _Palette.amber, size: 18),
            SizedBox(width: 8),
            Text(
              'Save Changes',
              style: TextStyle(
                color: _Palette.amber,
                fontSize: 14,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _saveCurveConfig() async {
    final config = _buildDraftConfig();
    final api = context.read<ServerSyncProvider>().api;
    setState(() => _curveConfigDirty = false);
    final profileSaved = await api.configSet(config, id: _selectedProfileId);
    if (!mounted) return;
    if (!profileSaved) {
      setState(() => _curveConfigDirty = true);
      AnalyticsService().logLightProfileSaveFailed(
        _selectedProfileId,
        stage: 'profile',
      );
      _showSaveFeedback(
        'Failed to save ${_profileTitle.toLowerCase()}.',
        error: true,
      );
      return;
    }

    // Also save idle config.
    final idleConfig = _buildIdleDraftConfig();
    bool idleSaved = true;
    if (idleConfig != null) {
      idleSaved = await api.configSet(idleConfig, id: idleConfig.id);
      if (!mounted) return;
      if (!idleSaved) {
        setState(() => _curveConfigDirty = true);
        AnalyticsService().logLightProfileSaveFailed(
          _selectedProfileId,
          stage: 'idle_profile',
        );
        _showSaveFeedback(
          'Saved ${_profileTitle.toLowerCase()}, but failed to save standby settings.',
          error: true,
        );
        return;
      }
    }

    final targetIdleProfileId = idleConfig?.id;
    final currentIdleProfileId = _selectedCustomIdleProfileId;
    final shouldUpdateIdleModeConfig =
        currentIdleProfileId != targetIdleProfileId;
    List<sdk.RhythmModeConfig>? updatedModeConfigs;
    var idleModeSaved = true;
    if (shouldUpdateIdleModeConfig) {
      updatedModeConfigs = _updatedModeConfigsForIdle(targetIdleProfileId);
      idleModeSaved = await api.modeSet(
        active: _serverActiveMode,
        configs: updatedModeConfigs,
      );
      if (!mounted) return;
      if (!idleModeSaved) {
        setState(() => _curveConfigDirty = true);
        AnalyticsService().logLightProfileSaveFailed(
          _selectedProfileId,
          stage: 'idle_mode',
        );
        _showSaveFeedback(
          targetIdleProfileId == null
              ? 'Saved ${_profileTitle.toLowerCase()}, but failed to disable custom standby.'
              : 'Saved ${_profileTitle.toLowerCase()}, but failed to attach standby settings.',
          error: true,
        );
        return;
      }
    }

    _profileConfigs[_selectedProfileId] = config;
    if (idleConfig != null) {
      _profileConfigs[idleConfig.id] = idleConfig;
    }
    if (updatedModeConfigs != null) {
      _modeConfigs = updatedModeConfigs;
    }
    _applyProfileConfig(config);
    if (idleConfig != null) {
      _applyIdleConfig(idleConfig);
    } else {
      _applyIdleFallback();
    }
    await _syncActiveConfigModel(config);
    await _loadCurveData(profileId: _selectedProfileId);
    if (!mounted) return;
    AnalyticsService().logLightProfileSaved(_selectedProfileId);
  }

  Widget _buildResetToDefaultsButton() {
    return GestureDetector(
      onTap: _resetToDefaults,
      child: Text(
        'Reset to Defaults',
        style: TextStyle(
          color: _Palette.textSecondary.withValues(alpha: 0.3),
          fontSize: 12,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
  }

  Widget _buildResetCurveConfigButton() {
    return GestureDetector(
      onTap: _resetCurveConfigToDefaults,
      child: Text(
        'Reset Curve Defaults',
        style: TextStyle(
          color: _Palette.textSecondary.withValues(alpha: 0.3),
          fontSize: 12,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
  }
}

// -----------------------------------------------------------------------------
// Curve interpolation — shared by painter and state methods
// -----------------------------------------------------------------------------

double _interpolateCurve(
    List<double> hours, List<int> values, double targetHour) {
  if (hours.isEmpty) return 50.0;
  if (hours.length == 1) return values[0].toDouble();

  var lowerIdx = 0;
  var upperIdx = hours.length - 1;

  for (int i = 0; i < hours.length - 1; i++) {
    if (hours[i] <= targetHour && hours[i + 1] >= targetHour) {
      lowerIdx = i;
      upperIdx = i + 1;
      break;
    }
  }

  if (targetHour < hours.first) {
    lowerIdx = hours.length - 1;
    upperIdx = 0;
  } else if (targetHour > hours.last) {
    lowerIdx = hours.length - 1;
    upperIdx = 0;
  }

  final lowerHour = hours[lowerIdx];
  final upperHour = hours[upperIdx];
  final lowerValue = values[lowerIdx];
  final upperValue = values[upperIdx];

  if (lowerHour == upperHour) return lowerValue.toDouble();

  double t;
  if (upperIdx == 0 && lowerIdx == hours.length - 1) {
    final totalSpan = (24 - lowerHour) + upperHour;
    final position = targetHour >= lowerHour
        ? targetHour - lowerHour
        : (24 - lowerHour) + targetHour;
    t = position / totalSpan;
  } else {
    t = (targetHour - lowerHour) / (upperHour - lowerHour);
  }

  return lowerValue + (upperValue - lowerValue) * t;
}

// -----------------------------------------------------------------------------
// Time Gradient Painter — compressed spectrum light bar
// -----------------------------------------------------------------------------

class _TimeGradientPainter extends CustomPainter {
  final CurveData? curveData;
  final List<double>? compressedPositions;
  final double currentHour;
  final double thumbFraction; // raw 0..1 slider position — no hour round-trip
  final double selectedHour; // for CCT color lookup only
  final Color? fixedColor;
  final bool isDragging;
  final bool hasOffset;
  final double glowPhase;
  final bool showTimeMarkers;

  _TimeGradientPainter({
    required this.curveData,
    required this.compressedPositions,
    required this.currentHour,
    required this.thumbFraction,
    required this.selectedHour,
    this.fixedColor,
    required this.isDragging,
    required this.hasOffset,
    required this.glowPhase,
    this.showTimeMarkers = false,
  });

  /// Convert hour to x position using compressed mapping.
  double _hourToX(double hour, double width) {
    if (compressedPositions == null) return (hour / 24) * width;
    final idx = (hour / 24 * 96).clamp(0.0, 96.0);
    final lower = idx.floor().clamp(0, 95);
    final upper = (lower + 1).clamp(0, 96);
    final t = idx - lower;
    final pos = compressedPositions![lower] +
        t * (compressedPositions![upper] - compressedPositions![lower]);
    return pos * width;
  }

  static const double _ribbonHeight = 56;

  @override
  void paint(Canvas canvas, Size size) {
    final ribbonSize = Size(size.width, _ribbonHeight);
    final rect = Offset.zero & ribbonSize;
    final rrect = RRect.fromRectAndRadius(rect, const Radius.circular(14));

    // 1. Dark background.
    canvas.drawRRect(rrect, Paint()..color = const Color(0xFF080A0E));

    // 2. Compressed gradient fill.
    canvas.save();
    canvas.clipRRect(rrect);
    _drawGradientFill(canvas, ribbonSize);
    canvas.restore();

    // 3. Frosted glass overlay for depth.
    canvas.save();
    canvas.clipRRect(rrect);
    canvas.drawRect(
      rect,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [
            Colors.white.withValues(alpha: 0.06),
            Colors.transparent,
            Colors.black.withValues(alpha: 0.1),
          ],
          stops: const [0.0, 0.4, 1.0],
        ).createShader(rect),
    );
    canvas.restore();

    // 4. Border.
    canvas.drawRRect(
      rrect,
      Paint()
        ..style = PaintingStyle.stroke
        ..color = const Color(0xFF1E2530)
        ..strokeWidth = 1,
    );

    // 5. "Now" marker.
    _drawNowMarker(canvas, ribbonSize);

    // 6. Thumb.
    if (hasOffset || isDragging) {
      _drawThumb(canvas, ribbonSize);
    }

    // 7. Time markers below ribbon.
    if (showTimeMarkers) {
      _drawTimeMarkers(canvas, size);
    }
  }

  void _drawGradientFill(Canvas canvas, Size size) {
    final colors = <Color>[];
    final stops = <double>[];
    const n = 96;

    for (int i = 0; i <= n; i++) {
      final hour = (i / n) * 24;
      int kelvin, brightness;

      if (curveData != null && curveData!.hours.isNotEmpty) {
        kelvin = _interpolateCurve(curveData!.hours, curveData!.kelvin, hour)
            .toInt();
        brightness =
            _interpolateCurve(curveData!.hours, curveData!.brightness, hour)
                .toInt();
      } else {
        final t = 1 - ((hour - 12).abs() / 12);
        kelvin = (2000 + t * 3500).toInt();
        brightness = (5 + t * 95).toInt();
      }

      final color = fixedColor ?? ColorUtils.curveColorForCCT(kelvin);
      final opacity = 0.08 + (brightness / 100) * 0.92;
      colors.add(color.withValues(alpha: opacity));

      // Use compressed positions for the gradient stops.
      final stop =
          compressedPositions != null ? compressedPositions![i] : i / n;
      stops.add(stop);
    }

    final gradient = LinearGradient(colors: colors, stops: stops);
    canvas.drawRect(
      Offset.zero & size,
      Paint()..shader = gradient.createShader(Offset.zero & size),
    );
  }

  void _drawNowMarker(Canvas canvas, Size size) {
    final x = _hourToX(currentHour, size.width);

    canvas.drawLine(
      Offset(x, 4),
      Offset(x, size.height - 4),
      Paint()
        ..color = Colors.white.withValues(alpha: hasOffset ? 0.2 : 0.5)
        ..strokeWidth = 1.5
        ..strokeCap = StrokeCap.round,
    );

    final trianglePath = Path()
      ..moveTo(x - 4, size.height + 1)
      ..lineTo(x + 4, size.height + 1)
      ..lineTo(x, size.height - 5)
      ..close();
    canvas.drawPath(
      trianglePath,
      Paint()..color = Colors.white.withValues(alpha: hasOffset ? 0.2 : 0.5),
    );
  }

  void _drawThumb(Canvas canvas, Size size) {
    final x = thumbFraction * size.width;

    final cctColor = fixedColor ??
        (() {
          int kelvin;
          if (curveData != null && curveData!.hours.isNotEmpty) {
            kelvin = _interpolateCurve(
                    curveData!.hours, curveData!.kelvin, selectedHour)
                .toInt();
          } else {
            final t = 1 - ((selectedHour - 12).abs() / 12);
            kelvin = (2000 + t * 3500).toInt();
          }
          return ColorUtils.curveColorForCCT(kelvin);
        })();

    // Outer glow.
    final glowIntensity = isDragging ? 0.25 : 0.15 + glowPhase * 0.06;
    canvas.drawLine(
      Offset(x, 0),
      Offset(x, size.height),
      Paint()
        ..color = cctColor.withValues(alpha: glowIntensity)
        ..strokeWidth = 20
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 12),
    );

    // Inner glow.
    canvas.drawLine(
      Offset(x, 0),
      Offset(x, size.height),
      Paint()
        ..color = Colors.white.withValues(alpha: isDragging ? 0.2 : 0.1)
        ..strokeWidth = 8
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 4),
    );

    // Crisp center line.
    canvas.drawLine(
      Offset(x, 3),
      Offset(x, size.height - 3),
      Paint()
        ..color = Colors.white.withValues(alpha: 0.9)
        ..strokeWidth = 2
        ..strokeCap = StrokeCap.round,
    );

    // Circle handle — shadow, fill, ring.
    canvas.drawCircle(
      Offset(x, size.height / 2),
      7,
      Paint()
        ..color = cctColor.withValues(alpha: 0.3)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 6),
    );
    canvas.drawCircle(
      Offset(x, size.height / 2),
      5.5,
      Paint()..color = Colors.white,
    );
    canvas.drawCircle(
      Offset(x, size.height / 2),
      5.5,
      Paint()
        ..style = PaintingStyle.stroke
        ..color = cctColor.withValues(alpha: 0.5)
        ..strokeWidth = 1.5,
    );
  }

  void _drawTimeMarkers(Canvas canvas, Size size) {
    final markerY = _ribbonHeight + 14;
    const minGap = 32.0;

    // Generate a candidate every hour, compute compressed x positions.
    final all = <({double x, String label, int priority})>[];
    for (int h = 0; h < 24; h++) {
      final x = _hourToX(h.toDouble(), size.width);
      final h12 = h == 0 ? 12 : (h > 12 ? h - 12 : h);
      final suffix = h >= 12 ? 'p' : 'a';
      final label = '$h12$suffix';
      // Priority: 0 = 6h intervals, 1 = 3h, 2 = 2h, 3 = 1h
      final priority = h % 6 == 0
          ? 0
          : h % 3 == 0
              ? 1
              : h % 2 == 0
                  ? 2
                  : 3;
      all.add((x: x, label: label, priority: priority));
    }

    // Place by priority — important markers first, fill in rest.
    final placed = <double>[];
    final visible = <({double x, String label})>[];
    for (int p = 0; p <= 3; p++) {
      for (final e in all) {
        if (e.priority != p) continue;
        if (placed.any((px) => (px - e.x).abs() < minGap)) continue;
        placed.add(e.x);
        visible.add((x: e.x, label: e.label));
      }
    }

    for (final e in visible) {
      // Tick mark.
      canvas.drawLine(
        Offset(e.x, _ribbonHeight + 2),
        Offset(e.x, _ribbonHeight + 6),
        Paint()
          ..color = Colors.white.withValues(alpha: 0.15)
          ..strokeWidth = 1
          ..strokeCap = StrokeCap.round,
      );

      // Label.
      final tp = TextPainter(
        text: TextSpan(
          text: e.label,
          style: TextStyle(
            color: Colors.white.withValues(alpha: 0.25),
            fontSize: 9,
            fontWeight: FontWeight.w500,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      tp.paint(canvas, Offset(e.x - tp.width / 2, markerY - tp.height / 2));
    }
  }

  @override
  bool shouldRepaint(covariant _TimeGradientPainter old) =>
      curveData != old.curveData ||
      compressedPositions != old.compressedPositions ||
      currentHour != old.currentHour ||
      thumbFraction != old.thumbFraction ||
      selectedHour != old.selectedHour ||
      fixedColor != old.fixedColor ||
      isDragging != old.isDragging ||
      hasOffset != old.hasOffset ||
      glowPhase != old.glowPhase ||
      showTimeMarkers != old.showTimeMarkers;
}

// -----------------------------------------------------------------------------
// Room default card
// -----------------------------------------------------------------------------

enum _RoomDefaultMode { none, off, idle, active }

class _RoomDefaultCard extends StatelessWidget {
  final String roomId;
  final String roomName;
  final String? state; // null = no override, "active", "idle", "hard_off"
  final ValueChanged<String?> onStateChanged;

  const _RoomDefaultCard({
    super.key,
    required this.roomId,
    required this.roomName,
    required this.state,
    required this.onStateChanged,
  });

  _RoomDefaultMode get _mode => switch (state) {
        'active' => _RoomDefaultMode.active,
        'idle' => _RoomDefaultMode.idle,
        'hard_off' => _RoomDefaultMode.off,
        _ => _RoomDefaultMode.none,
      };

  String? _stateFromMode(_RoomDefaultMode mode) => switch (mode) {
        _RoomDefaultMode.active => 'active',
        _RoomDefaultMode.idle => 'idle',
        _RoomDefaultMode.off => 'hard_off',
        _RoomDefaultMode.none => null,
      };

  @override
  Widget build(BuildContext context) {
    final mode = _mode;
    final hasOverride = mode != _RoomDefaultMode.none;

    final bgColor = switch (mode) {
      _RoomDefaultMode.active => const Color(0xFF1E1A12),
      _RoomDefaultMode.idle =>
        Color.lerp(_Palette.card, const Color(0xFF2E2518), 0.4)!,
      _RoomDefaultMode.off || _RoomDefaultMode.none => _Palette.card,
    };

    final stateLabel = switch (mode) {
      _RoomDefaultMode.active => 'Active',
      _RoomDefaultMode.idle => 'Standby',
      _RoomDefaultMode.off => 'Off',
      _RoomDefaultMode.none => 'No override',
    };

    final stateLabelColor = switch (mode) {
      _RoomDefaultMode.active => const Color(0xFFD4A020),
      _RoomDefaultMode.idle => _Palette.idle,
      _RoomDefaultMode.off => _Palette.textSecondary,
      _RoomDefaultMode.none => _Palette.textSecondary.withValues(alpha: 0.4),
    };

    final indicatorColor = switch (mode) {
      _RoomDefaultMode.active => const Color(0xFFD4A020),
      _RoomDefaultMode.idle => const Color(0xFFCDBFAA),
      _RoomDefaultMode.off => _Palette.textSecondary.withValues(alpha: 0.8),
      _RoomDefaultMode.none => _Palette.textSecondary.withValues(alpha: 0.75),
    };

    return Selector<RoomProvider,
        ({MotionTimerInfo? motionTimer, bool hasSensor})>(
      selector: (_, provider) => (
        motionTimer: provider.getMotionTimer(roomId),
        hasSensor: provider.hasMotionSensor(roomId),
      ),
      builder: (context, motionState, child) {
        final motionTimer = motionState.motionTimer;
        final hasSensor = motionState.hasSensor;

        return GestureDetector(
          onLongPress: hasOverride
              ? () {
                  HapticFeedback.lightImpact();
                  onStateChanged(null);
                }
              : null,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeInOut,
            decoration: BoxDecoration(
              color: bgColor,
              borderRadius: BorderRadius.circular(14),
              border: Border.all(
                color: hasOverride
                    ? _Palette.border
                    : _Palette.border.withValues(alpha: 0.4),
              ),
            ),
            padding: const EdgeInsets.fromLTRB(12, 12, 12, 10),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      if (motionTimer != null)
                        Padding(
                          padding: const EdgeInsets.only(right: 8, top: 1),
                          child: _RoomDefaultMotionIndicator(
                            info: motionTimer,
                            color: indicatorColor,
                            onExpired: () => context
                                .read<RoomProvider>()
                                .clearMotionTimer(roomId),
                          ),
                        )
                      else if (hasSensor)
                        Padding(
                          padding: const EdgeInsets.only(right: 8, top: 2),
                          child: Icon(
                            Icons.sensors_rounded,
                            size: 18,
                            color: indicatorColor.withValues(alpha: 0.45),
                          ),
                        ),
                      Expanded(
                        child: AnimatedOpacity(
                          opacity: hasOverride ? 1.0 : 0.45,
                          duration: const Duration(milliseconds: 300),
                          child: Text(
                            roomName,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              color: _Palette.textPrimary,
                              fontSize: 14,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
                Text(
                  stateLabel,
                  style: TextStyle(
                    color: stateLabelColor,
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                const SizedBox(height: 6),
                _DefaultStateToggle(
                  mode: mode,
                  onModeChanged: (newMode) {
                    HapticFeedback.lightImpact();
                    onStateChanged(_stateFromMode(newMode));
                  },
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class _RoomDefaultMotionIndicator extends StatefulWidget {
  final MotionTimerInfo info;
  final Color color;
  final VoidCallback? onExpired;

  const _RoomDefaultMotionIndicator({
    required this.info,
    required this.color,
    this.onExpired,
  });

  @override
  State<_RoomDefaultMotionIndicator> createState() =>
      _RoomDefaultMotionIndicatorState();
}

class _RoomDefaultMotionIndicatorState
    extends State<_RoomDefaultMotionIndicator>
    with SingleTickerProviderStateMixin {
  Timer? _countdownTimer;
  int _interpolatedRemaining = 0;
  late final AnimationController _pulseController;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1500),
    );
    _syncFromInfo();
  }

  @override
  void didUpdateWidget(_RoomDefaultMotionIndicator oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.info != widget.info) {
      _syncFromInfo();
    }
  }

  void _syncFromInfo() {
    if (widget.info.motionActive) {
      _countdownTimer?.cancel();
      _countdownTimer = null;
      if (!_pulseController.isAnimating) {
        _pulseController.repeat(reverse: true);
      }
      return;
    }

    _pulseController.stop();
    _pulseController.value = 0;

    if (widget.info.remainingSecs != null) {
      _interpolatedRemaining = widget.info.remainingSecs!;
      _startCountdown();
      return;
    }

    _countdownTimer?.cancel();
    _countdownTimer = null;
  }

  void _startCountdown() {
    _countdownTimer?.cancel();
    _countdownTimer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      final elapsed =
          DateTime.now().difference(widget.info.receivedAt).inSeconds;
      final remaining = (widget.info.remainingSecs ?? 0) - elapsed;
      if (remaining <= 0) {
        _countdownTimer?.cancel();
        _countdownTimer = null;
        widget.onExpired?.call();
        return;
      }
      setState(() {
        _interpolatedRemaining = remaining.clamp(0, widget.info.timeoutSecs);
      });
    });
  }

  @override
  void dispose() {
    _countdownTimer?.cancel();
    _pulseController.dispose();
    super.dispose();
  }

  String _formatTime(int secs) {
    if (secs >= 60) return '${(secs / 60).ceil()}m';
    return '${secs}s';
  }

  @override
  Widget build(BuildContext context) {
    if (widget.info.motionActive) {
      return AnimatedBuilder(
        animation: _pulseController,
        builder: (context, child) {
          final scale = 1.0 + _pulseController.value * 0.1;
          return Transform.scale(scale: scale, child: child);
        },
        child: Icon(
          Icons.directions_walk_rounded,
          size: 20,
          color: widget.color,
        ),
      );
    }

    final progress = widget.info.timeoutSecs > 0
        ? _interpolatedRemaining / widget.info.timeoutSecs
        : 0.0;

    return SizedBox(
      width: 28,
      height: 28,
      child: CustomPaint(
        painter: _RoomDefaultMiniCountdownPainter(
          progress: progress,
          color: widget.color,
        ),
        child: Center(
          child: Text(
            _formatTime(_interpolatedRemaining),
            style: TextStyle(
              color: widget.color,
              fontSize: 9,
              fontWeight: FontWeight.w700,
              height: 1,
            ),
          ),
        ),
      ),
    );
  }
}

class _RoomDefaultMiniCountdownPainter extends CustomPainter {
  final double progress;
  final Color color;

  _RoomDefaultMiniCountdownPainter({
    required this.progress,
    required this.color,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = size.width / 2 - 1.5;
    const strokeWidth = 2.5;

    canvas.drawCircle(
      center,
      radius,
      Paint()
        ..color = color.withValues(alpha: 0.15)
        ..style = PaintingStyle.stroke
        ..strokeWidth = strokeWidth,
    );

    if (progress > 0) {
      canvas.drawArc(
        Rect.fromCircle(center: center, radius: radius),
        -math.pi / 2,
        2 * math.pi * progress,
        false,
        Paint()
          ..color = color.withValues(alpha: 0.7)
          ..style = PaintingStyle.stroke
          ..strokeWidth = strokeWidth
          ..strokeCap = StrokeCap.round,
      );
    }
  }

  @override
  bool shouldRepaint(covariant _RoomDefaultMiniCountdownPainter oldDelegate) =>
      progress != oldDelegate.progress || color != oldDelegate.color;
}

// ---------------------------------------------------------------------------
// 3-state toggle matching the CelestialToggle from room_card.dart
// ---------------------------------------------------------------------------

class _DefaultStateToggle extends StatelessWidget {
  final _RoomDefaultMode mode;
  final ValueChanged<_RoomDefaultMode> onModeChanged;

  const _DefaultStateToggle({
    required this.mode,
    required this.onModeChanged,
  });

  void _onTap() {
    switch (mode) {
      case _RoomDefaultMode.none:
        onModeChanged(_RoomDefaultMode.active);
      case _RoomDefaultMode.active:
        onModeChanged(_RoomDefaultMode.idle);
      case _RoomDefaultMode.idle:
        onModeChanged(_RoomDefaultMode.active);
      case _RoomDefaultMode.off:
        onModeChanged(_RoomDefaultMode.none);
    }
  }

  void _onLongPress() {
    if (mode != _RoomDefaultMode.off) {
      onModeChanged(_RoomDefaultMode.off);
    }
  }

  @override
  Widget build(BuildContext context) {
    final hasOverride = mode != _RoomDefaultMode.none;

    final alignment = switch (mode) {
      _RoomDefaultMode.off => Alignment.centerLeft,
      _RoomDefaultMode.idle => Alignment.center,
      _RoomDefaultMode.active => Alignment.centerRight,
      _RoomDefaultMode.none => Alignment.center,
    };

    final trackGradient = switch (mode) {
      _RoomDefaultMode.off => const LinearGradient(
          colors: [Color(0xFF2A2F38), Color(0xFF30363D)],
        ),
      _RoomDefaultMode.idle => const LinearGradient(
          colors: [Color(0xFF221C14), Color(0xFF2E2518)],
        ),
      _RoomDefaultMode.active => const LinearGradient(
          colors: [Color(0xFF8B6B20), Color(0xFFD4A020)],
        ),
      _RoomDefaultMode.none => const LinearGradient(
          colors: [Color(0xFF1E2228), Color(0xFF1E2228)],
        ),
    };

    final thumbColor = switch (mode) {
      _RoomDefaultMode.off => _Palette.textSecondary,
      _RoomDefaultMode.idle => const Color(0xFFCDBFAA),
      _RoomDefaultMode.active => Colors.white,
      _RoomDefaultMode.none => _Palette.textSecondary.withValues(alpha: 0.3),
    };

    final thumbShadow = switch (mode) {
      _RoomDefaultMode.active => [
          BoxShadow(
            color: _Palette.amber.withValues(alpha: 0.4),
            blurRadius: 8,
            spreadRadius: 1,
          ),
        ],
      _RoomDefaultMode.idle => [
          BoxShadow(
            color: const Color(0xFFD4A574).withValues(alpha: 0.2),
            blurRadius: 6,
          ),
        ],
      _RoomDefaultMode.off || _RoomDefaultMode.none => <BoxShadow>[],
    };

    return GestureDetector(
      onTap: _onTap,
      onLongPress: _onLongPress,
      behavior: HitTestBehavior.opaque,
      child: AnimatedOpacity(
        opacity: hasOverride ? 1.0 : 0.4,
        duration: const Duration(milliseconds: 300),
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 300),
          curve: Curves.easeInOut,
          width: 64,
          height: 28,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(14),
            gradient: trackGradient,
          ),
          padding: const EdgeInsets.all(3),
          child: AnimatedAlign(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeInOut,
            alignment: alignment,
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 300),
              width: 22,
              height: 22,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: thumbColor,
                boxShadow: thumbShadow,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

// Palette
// -----------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Light Tuning — full-screen overlay
// ---------------------------------------------------------------------------

class _AdvancedColorEditorScreen extends StatefulWidget {
  final _LightProfileScreenState parent;

  const _AdvancedColorEditorScreen({required this.parent});

  @override
  State<_AdvancedColorEditorScreen> createState() =>
      _AdvancedColorEditorScreenState();
}

class _AdvancedColorEditorScreenState
    extends State<_AdvancedColorEditorScreen> {
  _LightProfileScreenState get _parent => widget.parent;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: _Palette.bg,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: ValueListenableBuilder<int>(
                valueListenable: _parent._rebuildNotifier,
                builder: (context, _, __) {
                  return SingleChildScrollView(
                    padding: const EdgeInsets.fromLTRB(20, 8, 20, 40),
                    child: Column(
                      children: [
                        _parent._buildHeroIcon(),
                        const SizedBox(height: 24),
                        if (!_parent._isSleepProfile) ...[
                          _parent._buildBrightnessRangeCard(),
                          const SizedBox(height: 14),
                          _parent._buildColorTempRangeCard(),
                          const SizedBox(height: 24),
                        ],
                        _parent._buildTimeSimulator(),
                        if (_parent._curveConfigDirty) ...[
                          const SizedBox(height: 24),
                          _parent._buildSaveButton(),
                        ],
                        const SizedBox(height: 32),
                        _parent._buildResetCurveConfigButton(),
                      ],
                    ),
                  );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _Palette.amber.withValues(alpha: 0.12),
                border: Border.all(
                  color: _Palette.amber.withValues(alpha: 0.25),
                ),
              ),
              child: const Icon(Icons.arrow_back_rounded,
                  color: _Palette.amber, size: 20),
            ),
          ),
          const Expanded(
            child: Text(
              'Light Tuning',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _Palette.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }
}

class _Palette {
  static const bg = Color(0xFF0B0E13);
  static const card = Color(0xFF13171E);
  static const border = Color(0xFF232A35);
  static const textPrimary = Color(0xFFE8EDF4);
  static const textSecondary = Color(0xFF8A919C);
  static const amber = Color(0xFFF9A825);
  static const blue = Color(0xFF58A6FF);
  static const teal = Color(0xFF4ADE80);
  static const idle = Color(0xFFB8A890);
}
