import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;
import '../../api/hybrid_client.dart';
import '../../config/feature_flags.dart';
import '../../models/config_model.dart';
import '../../models/plan_tier.dart';
import '../../providers/room_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../providers/subscription_provider.dart';
import '../../services/analytics_service.dart';
import '../../utils/room_visibility.dart';
import '../../widgets/info_tooltip.dart';
import '../../widgets/plan_tier_modal.dart';

/// Full-screen modal for configuring the light profile.
///
/// Features a compressed color spectrum slider that emphasizes the dawn/dusk
/// ramps where color changes rapidly, compressing flat night/day regions.
class LightProfileScreen extends StatefulWidget {
  final String? initialProfile;

  const LightProfileScreen({super.key, this.initialProfile});

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
  bool _fadeAuto = true;

  // Background light interval.
  double _intervalSecs = 60;

  // Curve parameters.
  bool _curveConfigDirty = false;
  bool _isSaving = false;
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
  bool _advancedExpanded = false;

  // Sleep profile state: optional fixed color + optional fixed brightness.
  // When neither toggle is on, sleep inherits CCT + brightness from the wake
  // (rhythm) profile's min_color_temp and min_brightness.
  double _sleepHue = 10;
  double _sleepBrightness = 20;
  bool _sleepCustomBri = false;
  bool _sleepCustomColor = false;
  bool _sleepExpanded = false;

  // Idle profile state (folded into day/sleep profiles).
  bool _idleCustomBri = false;
  bool _idleCustomColor = false;
  double _idleBrightness = 1;
  double _idleHue = 30; // hue angle for spectrum picker

  bool _loading = true;
  bool _connected = false;
  bool _configLoadInFlight = false;
  String? _serverConfigSignature;
  late final ServerSyncProvider _serverSync;

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

    _serverSync = context.read<ServerSyncProvider>();
    _serverSync.addListener(_handleServerSyncChanged);
    unawaited(_loadConfig());
  }

  @override
  void dispose() {
    _serverSync.removeListener(_handleServerSyncChanged);
    _roomDefaultsDebounce?.cancel();
    _curvePreviewRefreshTimer?.cancel();
    _glowController.dispose();
    super.dispose();
  }

  void _handleServerSyncChanged() {
    if (!mounted) return;

    // Use `hasBeenSynced` (sticky across reconnects), not `synced`. Reading
    // `synced` would dip false during the transient reconnect that fires
    // after every HTTP action (e.g. save), causing this screen to flash its
    // "Device Not Connected" state for one frame before snapping back.
    final synced = _serverSync.hasBeenSynced;
    if (synced) {
      final serverConfigChanged = _connected &&
          _serverConfigSignature != _currentServerConfigSignature();
      final canAdoptServerConfig =
          !_hasLocalDraft && !_configLoadInFlight && !_loading;
      final shouldLoadConfig =
          !_connected || (serverConfigChanged && canAdoptServerConfig);
      if (shouldLoadConfig && !_configLoadInFlight) {
        if (!_connected) {
          setState(() => _loading = true);
        }
        unawaited(_loadConfig());
      }
      return;
    }

    if (_connected && !_loading) {
      setState(() {
        _connected = false;
        _serverConfigSignature = null;
      });
    }
  }

  bool get _hasLocalDraft =>
      _curveConfigDirty ||
      _isSaving ||
      (_roomDefaultsDebounce?.isActive ?? false);

  String _currentServerConfigSignature() {
    final buffer = StringBuffer()
      ..write(_serverSync.activeMode?.name ?? '')
      ..write('|')
      ..write(_serverSync.activeProfileId ?? '')
      ..write('|');

    for (final config in _serverSync.modeConfigs) {
      buffer
        ..write(config.mode.name)
        ..write(':')
        ..write(config.activeProfileId)
        ..write(':')
        ..write(config.idleProfileId ?? '')
        ..write(':')
        ..write(config.wakeProfileId ?? '')
        ..write(':')
        ..write(config.warningProfileId ?? '');
      for (final roomDefault in config.roomDefaults) {
        buffer
          ..write(',')
          ..write(roomDefault.roomId)
          ..write('=')
          ..write(roomDefault.state);
      }
      buffer.write('|');
    }

    for (final profile in _serverSync.profiles) {
      buffer
        ..write(profile.id)
        ..write(':')
        ..write(profile.hashCode)
        ..write('|');
    }

    return buffer.toString();
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
    if (_configLoadInFlight) return;
    _configLoadInFlight = true;

    try {
      final syncProvider = _serverSync;
      if (checkConnection) {
        _connected = syncProvider.hasBeenSynced;

        if (!_connected) {
          if (mounted) {
            setState(() {
              _loading = false;
              _serverConfigSignature = null;
            });
          }
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
          _connected = syncProvider.hasBeenSynced;
          _loading = false;
          _serverConfigSignature = _currentServerConfigSignature();
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
        _connected = syncProvider.hasBeenSynced;
        _loading = false;
        _sliderFraction = _hourToNowFraction();
        _serverConfigSignature = _currentServerConfigSignature();
      });
    } catch (e) {
      debugPrint('LightProfile: Failed to load config: $e');
      if (mounted) {
        setState(() {
          _connected = _serverSync.hasBeenSynced;
          _loading = false;
          _serverConfigSignature =
              _connected ? _currentServerConfigSignature() : null;
        });
      }
    } finally {
      _configLoadInFlight = false;
      if (mounted && _serverSync.hasBeenSynced && !_connected) {
        _handleServerSyncChanged();
      }
    }
  }

  Future<void> _loadCurveData(
      {sdk.RhythmCurveConfig? config, String? profileId}) async {
    try {
      final id = profileId ?? _selectedProfileId;
      final requestId = ++_curvePreviewRequestId;

      final localData =
          _tryBuildLocalGaussianCurveData(config ?? _profileConfigs[id]);
      if (localData != null) {
        _applyCurveData(localData, requestId);
        return;
      }

      final preview = await context.read<ServerSyncProvider>().api.getCurveData(
            id: id,
            overrides: config,
          );
      if (preview == null) return;
      _applyCurveData(_curveDataFromSdk(preview), requestId);
    } catch (e) {
      debugPrint('LightProfile: Failed to load curve data: $e');
    }
  }

  CurveData? _tryBuildLocalGaussianCurveData(sdk.RhythmCurveConfig? config) {
    if (config == null) return null;

    final curve = config.curve;
    if (curve is! sdk.RhythmSuperGaussianCurve || curve.directColor != null) {
      return null;
    }

    try {
      final api = context.read<RhythmApi>();
      if (api is! HybridApiClient) return null;

      final dto = _toCurveConfigDto(config);
      if (dto == null) return null;

      return api.getCurveDataHighRes(
        config: dto,
        samplesPerHour: 4,
      );
    } catch (e) {
      debugPrint('LightProfile: Local Gaussian preview unavailable: $e');
      return null;
    }
  }

  CurveData _curveDataFromSdk(sdk.RhythmCurveData preview) => CurveData(
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

  void _applyCurveData(CurveData data, int requestId) {
    if (!mounted || requestId != _curvePreviewRequestId) return;

    setState(() {
      _curveData = data;
      _compressedPositions = _computeCompressedMapping(data);
    });
  }

  void _onCurveChanged(void Function() update) {
    setState(() {
      update();
      _markDirty();
    });
  }

  /// Recomputes [_curveConfigDirty] by comparing the current draft against the
  /// last loaded/saved baseline. Call after any state mutation that the user
  /// might want to revert; toggling a value back to its baseline clears dirty.
  void _markDirty() {
    _curveConfigDirty = _computeDirty();
  }

  bool _computeDirty() {
    final baseline = _profileConfigs[_selectedProfileId];
    if (baseline == null) return false;
    if (_buildDraftConfig() != baseline) return true;

    final baselineIdleId = _customIdleProfileIdForProfile(_selectedProfileId);
    final draftIdle = _buildIdleDraftConfig();
    if ((draftIdle?.id) != baselineIdleId) return true;
    if (draftIdle != null) {
      final baselineIdle = _profileConfigs[draftIdle.id];
      if (baselineIdle != draftIdle) return true;
    }
    return false;
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
    _fadeAuto = config.fadeMs == null;
    _fadeMs = (config.fadeMs ?? _serverSync.effectiveFadeMs ?? 500).toDouble();
    _motionTimeoutAuto = config.motionTimeoutSecs == null;
    _motionTimeoutSecs = config.motionTimeoutSecs ??
        _serverSync.effectiveMotionTimeoutSecs ??
        600;
    _intervalAuto = config.rhythmIntervalSecs == null;
    _intervalSecs =
        (config.rhythmIntervalSecs ?? _serverSync.rhythmIntervalSecs)
            .toDouble();

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

    if (config.id == 'sleep') {
      final wakeMinBri = _wakeMinBrightness;
      _sleepCustomColor = directColor != null;
      _sleepCustomBri = config.maxBrightness != wakeMinBri ||
          config.minBrightness != wakeMinBri;
      _sleepBrightness = _sleepCustomBri
          ? config.maxBrightness.toDouble().clamp(1, 100)
          : wakeMinBri.toDouble().clamp(1, 100);
      _sleepExpanded = _sleepCustomBri || _sleepCustomColor;
    } else {
      _sleepBrightness = switch (curve) {
        sdk.RhythmConstantCurve(:final brightness) => (config.minBrightness +
                (config.maxBrightness - config.minBrightness) * brightness)
            .toDouble()
            .clamp(1, 100),
        _ => config.maxBrightness.toDouble().clamp(1, 100),
      };
    }

    _curveConfigDirty = false;
  }

  /// Wake (rhythm) profile's min brightness — used as the sleep default when
  /// the user hasn't enabled Custom Brightness. Falls back to the SDK default
  /// (20) until the wake config has loaded.
  int get _wakeMinBrightness =>
      _profileConfigs['rhythm']?.minBrightness ??
      sdk.RhythmCurveConfig.defaultMinBrightness;

  /// Wake (rhythm) profile's min color temp — used as the sleep CCT default
  /// when the user hasn't enabled Custom Color (direct color overrides CCT).
  int get _wakeMinColorTemp =>
      _profileConfigs['rhythm']?.minColorTemp ??
      sdk.RhythmCurveConfig.defaultMinColorTemp;

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
      final canUseSleepPrimary = context
          .read<SubscriptionProvider>()
          .has(Entitlement.sleepPrimarySettings);
      final wakeMinBri = _wakeMinBrightness;
      final wakeMinCct = _wakeMinColorTemp;
      final brightness = canUseSleepPrimary && _sleepCustomBri
          ? _sleepBrightness.round().clamp(1, 100)
          : wakeMinBri;
      final directColor =
          canUseSleepPrimary && _sleepCustomColor ? _sleepDirectColor : null;
      return sdk.RhythmCurveConfig(
        id: base.id,
        name: base.name,
        minColorTemp: wakeMinCct,
        maxColorTemp: wakeMinCct,
        minBrightness: brightness,
        maxBrightness: brightness,
        maxDimSteps: base.maxDimSteps,
        fadeMs: canUseSleepPrimary
            ? (_fadeAuto ? null : _fadeMs.round())
            : base.fadeMs,
        motionTimeoutSecs: canUseSleepPrimary
            ? (_motionTimeoutAuto ? null : _motionTimeoutSecs)
            : base.motionTimeoutSecs,
        rhythmIntervalSecs: canUseSleepPrimary
            ? (_intervalAuto ? null : _intervalSecs.round())
            : base.rhythmIntervalSecs,
        curve: sdk.RhythmConstantCurve(
          brightness: 0,
          colorTemp: 0,
          directColor: directColor,
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

    final canUseAdvancedDay = context
        .read<SubscriptionProvider>()
        .has(Entitlement.advancedDayControls);
    return sdk.RhythmCurveConfig(
      id: base.id,
      name: base.name,
      minColorTemp: _minColorTemp.round(),
      maxColorTemp: _maxColorTemp.round(),
      minBrightness: _minBrightness.round(),
      maxBrightness: _maxBrightness.round(),
      maxDimSteps: _maxDimSteps.round(),
      fadeMs: canUseAdvancedDay
          ? (_fadeAuto ? null : _fadeMs.round())
          : base.fadeMs,
      motionTimeoutSecs: canUseAdvancedDay
          ? (_motionTimeoutAuto ? null : _motionTimeoutSecs)
          : base.motionTimeoutSecs,
      rhythmIntervalSecs: canUseAdvancedDay
          ? (_intervalAuto ? null : _intervalSecs.round())
          : base.rhythmIntervalSecs,
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

  bool _isSignificantTimeOffset(double offsetMinutes) =>
      offsetMinutes.abs() > 0.5;

  bool get _hasTimeOffset => _isSignificantTimeOffset(_timeOffsetMinutes);

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
    final serverSync = context.read<ServerSyncProvider>();
    final activeProfileId = serverSync.activeProfileId;

    if (config.id == activeProfileId) {
      final dto = _toCurveConfigDto(config);
      if (dto != null && mounted) {
        context.read<ConfigModel>().updateConfig(dto);
      }
    }

    await serverSync.fullRefresh();
    _serverConfigSignature = _currentServerConfigSignature();
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
        _timeOffsetApplied =
            (_timeOffsetMinutes - previewOffset).abs() <= 0.5 &&
                _isSignificantTimeOffset(previewOffset);
        _timeOffsetPreviewActive = _isSignificantTimeOffset(previewOffset);
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
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
      child: Center(
        child: Text(
          _profileTitle,
          style: const TextStyle(
            color: _Palette.textPrimary,
            fontSize: 18,
            fontWeight: FontWeight.w600,
            letterSpacing: 0.3,
          ),
        ),
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

  String _formatFade(double ms) => '${ms.round()}ms';

  /// Two-chip segmented toggle: `[Auto] [valueLabel]`.
  ///
  /// Both chips are always visible so the current mode is unambiguous and the
  /// "reset to Auto" affordance is one tap away in any state.
  Widget _buildAutoToggle({
    required Color color,
    required bool isAuto,
    required String valueLabel,
    required VoidCallback onAuto,
    required VoidCallback onManual,
  }) {
    Widget chip(String label, bool active, VoidCallback onTap) {
      return GestureDetector(
        onTap: onTap,
        behavior: HitTestBehavior.opaque,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          decoration: BoxDecoration(
            color: active
                ? color.withValues(alpha: 0.14)
                : Colors.transparent,
            borderRadius: BorderRadius.circular(6),
            border: Border.all(
              color: active
                  ? color.withValues(alpha: 0.30)
                  : color.withValues(alpha: 0.14),
            ),
          ),
          child: Text(
            label,
            style: TextStyle(
              color: active ? color : color.withValues(alpha: 0.45),
              fontSize: 11,
              fontWeight: FontWeight.w600,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        ),
      );
    }

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        chip('Auto', isAuto, isAuto ? () {} : onAuto),
        const SizedBox(width: 6),
        chip(valueLabel, !isAuto, !isAuto ? () {} : onManual),
      ],
    );
  }

  String _formatMotionTimeout(int secs) {
    if (secs == 0) return 'Off';
    if (secs >= 60) return '${(secs / 60).round()}m';
    return '${secs}s';
  }

  Widget _buildContent() {
    final subscription = context.watch<SubscriptionProvider>();
    final canUseSleepPrimary =
        subscription.has(Entitlement.sleepPrimarySettings);
    final canUseAdvancedDay = subscription.has(Entitlement.advancedDayControls);
    final canUseStandby = subscription.has(Entitlement.standby);
    final canUseTimeSimulator = subscription.has(Entitlement.timeSimulator);
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(20, 8, 20, 40),
      child: Column(
        children: [
          _buildHeroIcon(),
          const SizedBox(height: 24),
          if (!_isSleepProfile) ...[
            _buildBrightnessRangeCard(),
            const SizedBox(height: 14),
            _buildColorTempRangeCard(),
            const SizedBox(height: 24),
            _ProLockWrap(
              unlocked: canUseTimeSimulator,
              entitlement: Entitlement.timeSimulator,
              child: _buildTimeSimulator(),
            ),
            const SizedBox(height: 24),
          ],
          _buildRoomDefaultsSection(canUseStandby: canUseStandby),
          const SizedBox(height: 14),
          _buildAdvancedSection(
            canUseAdvancedDay: canUseAdvancedDay,
            canUseStandby: canUseStandby,
            canUseSleepPrimary: canUseSleepPrimary,
          ),
          if (_curveConfigDirty || _isSaving) ...[
            const SizedBox(height: 24),
            _buildPendingChangesActions(),
          ],
          const SizedBox(height: 32),
          _buildResetToDefaultsButton(),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Sleep Profile — Primary Settings
  //
  // Mirrors the Standby Settings widget. Both Custom Color and Custom
  // Brightness are optional toggles; when neither is on the sleep profile
  // inherits CCT + brightness from the wake (rhythm) profile's
  // min_color_temp and min_brightness.
  // ---------------------------------------------------------------------------

  Widget _buildSleepPrimarySettingsCard() {
    final isDefault = !_sleepCustomBri && !_sleepCustomColor;
    final briColor = _sleepCustomBri ? _Palette.amber : _Palette.idle;
    final colorColor = _sleepCustomColor ? _sleepSelectedColor : _Palette.idle;
    final expanded = _sleepExpanded;

    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 18),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: expanded
              ? _Palette.amber.withValues(alpha: 0.25)
              : _Palette.border,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: () => setState(() => _sleepExpanded = !_sleepExpanded),
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
                        color: _Palette.amber.withValues(alpha: 0.12),
                      ),
                      child: Icon(
                        Icons.nights_stay_rounded,
                        color: _Palette.amber.withValues(alpha: 0.7),
                        size: 18,
                      ),
                    ),
                    const SizedBox(width: 12),
                    const Expanded(
                      child: Text(
                        'Primary Settings',
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
                    if (!expanded && _sleepCustomBri) ...[
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
                          '${_sleepBrightness.round()}%',
                          style: TextStyle(
                            color: _Palette.amber.withValues(alpha: 0.8),
                            fontSize: 11,
                            fontWeight: FontWeight.w600,
                            fontFeatures: const [FontFeature.tabularFigures()],
                          ),
                        ),
                      ),
                    ],
                    if (!expanded && _sleepCustomColor) ...[
                      if (_sleepCustomBri) const SizedBox(width: 6),
                      Container(
                        width: 26,
                        height: 26,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: _sleepSelectedColor,
                          border: Border.all(
                            color: _sleepSelectedColor.withValues(alpha: 0.4),
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
                      Row(
                        children: [
                          Icon(
                            Icons.brightness_medium_rounded,
                            color: briColor.withValues(
                                alpha: _sleepCustomBri ? 0.8 : 0.35),
                            size: 16,
                          ),
                          const SizedBox(width: 10),
                          Expanded(
                            child: Text(
                              'Custom Brightness',
                              style: TextStyle(
                                color: _sleepCustomBri
                                    ? _Palette.textPrimary
                                    : _Palette.textSecondary
                                        .withValues(alpha: 0.5),
                                fontSize: 13,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                          ),
                          if (_sleepCustomBri)
                            Padding(
                              padding: const EdgeInsets.only(right: 8),
                              child: Text(
                                '${_sleepBrightness.round()}%',
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
                              value: _sleepCustomBri,
                              onChanged: (v) {
                                setState(() {
                                  _sleepCustomBri = v;
                                  if (v && _sleepBrightness < 1) {
                                    _sleepBrightness = 1;
                                  }
                                  _markDirty();
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
                        child: _sleepCustomBri
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
                                    value: _sleepBrightness.clamp(1, 100),
                                    min: 1,
                                    max: 100,
                                    divisions: 99,
                                    onChanged: (v) {
                                      setState(() {
                                        _sleepBrightness = v;
                                        _markDirty();
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
                      Row(
                        children: [
                          Icon(
                            Icons.palette_outlined,
                            color: colorColor.withValues(
                                alpha: _sleepCustomColor ? 0.8 : 0.35),
                            size: 16,
                          ),
                          const SizedBox(width: 10),
                          Expanded(
                            child: Text(
                              'Custom Color',
                              style: TextStyle(
                                color: _sleepCustomColor
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
                              value: _sleepCustomColor,
                              onChanged: (v) {
                                setState(() {
                                  _sleepCustomColor = v;
                                  _markDirty();
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
                        child: _sleepCustomColor
                            ? Padding(
                                padding: const EdgeInsets.only(top: 16),
                                child: _buildFixedColorPicker(
                                  hue: _sleepHue,
                                  selectedColor: _sleepSelectedColor,
                                  isPresetSelected: _isSleepPresetSelected,
                                  onHueChanged: (hue) {
                                    setState(() {
                                      _sleepHue = hue;
                                      _markDirty();
                                    });
                                  },
                                ),
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
                                  _markDirty();
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
                                        _markDirty();
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
                                  _markDirty();
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
          _markDirty();
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
  // Light Transition (fade) row
  // ---------------------------------------------------------------------------

  /// Unified header + Auto-toggle + slider used by Light Transition,
  /// Background Interval, and Motion Timeout.
  ///
  /// Tap behaviors:
  /// - Tap anywhere on the header row (while in Auto) -> open the slider.
  /// - Tap the Auto chip -> collapse the slider.
  /// - Tap the value chip -> open the slider (same as row tap).
  Widget _buildTimingRow({
    required IconData icon,
    required String title,
    required Color color,
    required bool isAuto,
    required double sliderValue,
    required double effectiveValue,
    required double sliderMin,
    required double sliderMax,
    required int divisions,
    required String Function(double) format,
    required ValueChanged<double> onSliderChanged,
    required VoidCallback onAuto,
    required VoidCallback onManual,
    String? tooltip,
  }) {
    final displayValue = isAuto ? effectiveValue : sliderValue;
    return Column(
      children: [
        GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: isAuto ? onManual : null,
          child: Row(
            children: [
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: color.withValues(alpha: 0.1),
                ),
                child: Icon(icon, color: color, size: 15),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Row(
                  children: [
                    Flexible(
                      child: Text(
                        title,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: _Palette.textSecondary,
                          fontSize: 14,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ),
                    if (tooltip != null) ...[
                      const SizedBox(width: 2),
                      InfoTooltip(message: tooltip, iconSize: 13),
                    ],
                  ],
                ),
              ),
              _buildAutoToggle(
                color: color,
                isAuto: isAuto,
                valueLabel: format(displayValue),
                onAuto: onAuto,
                onManual: onManual,
              ),
            ],
          ),
        ),
        AnimatedSize(
          duration: const Duration(milliseconds: 250),
          curve: Curves.easeOutCubic,
          alignment: Alignment.topCenter,
          child: isAuto
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
                          value: sliderValue.clamp(sliderMin, sliderMax),
                          min: sliderMin,
                          max: sliderMax,
                          divisions: divisions,
                          onChanged: onSliderChanged,
                        ),
                      ),
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 6),
                        child: Row(
                          mainAxisAlignment: MainAxisAlignment.spaceBetween,
                          children: [
                            Text(
                              format(sliderMin),
                              style: TextStyle(
                                color: _Palette.textSecondary
                                    .withValues(alpha: 0.4),
                                fontSize: 11,
                              ),
                            ),
                            Text(
                              format(sliderMax),
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

  // ---------------------------------------------------------------------------
  // Time Simulator
  // ---------------------------------------------------------------------------

  Widget _buildTimeSimulator() {
    final selectedHour = _selectedHour();
    final previewColor = _previewColorAtHour(selectedHour);
    final active = _hasTimeOffset;

    return Container(
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(20),
        gradient: const LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [
            Color(0xFF06080C),
            Color(0xFF0A0D13),
          ],
        ),
        border: Border.all(
          color: active
              ? _Palette.amber.withValues(alpha: 0.30)
              : Colors.white.withValues(alpha: 0.04),
        ),
        boxShadow: [
          // Outer cast — sits on the surrounding card.
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.35),
            blurRadius: 8,
            spreadRadius: -2,
            offset: const Offset(0, 2),
          ),
          // Warm amber bloom when the simulator is engaged.
          if (active)
            BoxShadow(
              color: _Palette.amber.withValues(alpha: 0.12),
              blurRadius: 24,
              spreadRadius: -6,
            ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Eyebrow: small "horizon" marker + label.
          Row(
            children: [
              Container(
                width: 6,
                height: 6,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: active
                      ? _Palette.amber
                      : _Palette.amber.withValues(alpha: 0.35),
                  boxShadow: active
                      ? [
                          BoxShadow(
                            color: _Palette.amber.withValues(alpha: 0.6),
                            blurRadius: 6,
                          ),
                        ]
                      : null,
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  'TIME SIMULATOR',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: _Palette.amber.withValues(alpha: 0.75),
                    fontSize: 10,
                    fontWeight: FontWeight.w800,
                    letterSpacing: 2.0,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              // Tiny right-side readout: live kelvin value when engaged,
              // otherwise the drag-to-simulate prompt.
              Flexible(
                child: Align(
                  alignment: Alignment.centerRight,
                  child: AnimatedSwitcher(
                    duration: const Duration(milliseconds: 200),
                    child: active
                        ? Text(
                            _previewValueLabel(selectedHour),
                            key: const ValueKey('readout'),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              color: previewColor.withValues(alpha: 0.85),
                              fontSize: 11.5,
                              fontWeight: FontWeight.w700,
                              letterSpacing: 0.2,
                              fontFeatures: const [
                                FontFeature.tabularFigures(),
                              ],
                            ),
                          )
                        : Text(
                            'Drag to simulate',
                            key: const ValueKey('hint'),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              color: _Palette.textSecondary
                                  .withValues(alpha: 0.45),
                              fontSize: 11,
                              fontWeight: FontWeight.w500,
                              letterSpacing: 0.1,
                            ),
                          ),
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          // The gradient slider — the magical part.
          _buildGradientSlider(),
          // Apply / Reset + Absorb buttons.
          AnimatedSize(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeOutCubic,
            child: _showTimeOffsetActions
                ? Padding(
                    padding: const EdgeInsets.only(top: 14),
                    child: _buildTimeOffsetActions(),
                  )
                : const SizedBox.shrink(),
          ),
        ],
      ),
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

  Widget _buildTimingCard() {
    const mtColor = _Palette.teal;
    const ltColor = _Palette.amber;
    const intColor = _Palette.blue;

    final effectiveMotionSecs =
        (_serverSync.effectiveMotionTimeoutSecs ?? _motionTimeoutSecs)
            .toDouble();
    final effectiveFadeMs =
        _serverSync.effectiveFadeMs?.toDouble() ?? _fadeMs;
    final effectiveIntervalSecs = _serverSync.rhythmIntervalSecs.toDouble();

    final divider = Padding(
      padding: const EdgeInsets.symmetric(vertical: 10),
      child: Divider(
        height: 1,
        color: _Palette.border.withValues(alpha: 0.5),
      ),
    );

    return Container(
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 12),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _Palette.border),
      ),
      child: Column(
        children: [
          _buildTimingRow(
            icon: Icons.motion_photos_on_rounded,
            title: 'Motion Timeout',
            color: mtColor,
            isAuto: _motionTimeoutAuto,
            sliderValue: _motionTimeoutSecs.toDouble(),
            effectiveValue: effectiveMotionSecs,
            sliderMin: 30,
            sliderMax: 1800,
            divisions: 59,
            format: (v) => _formatMotionTimeout(v.round()),
            tooltip:
                'How long the lights stay on after motion is last detected '
                'before timing out to standby.',
            onSliderChanged: (v) => setState(() {
              _motionTimeoutSecs = v.round();
              _markDirty();
            }),
            onAuto: () => setState(() {
              _motionTimeoutAuto = true;
              _markDirty();
            }),
            onManual: () => setState(() {
              _motionTimeoutSecs = effectiveMotionSecs.round();
              _motionTimeoutAuto = false;
              _markDirty();
            }),
          ),
          divider,
          _buildTimingRow(
            icon: Icons.blur_on_rounded,
            title: 'Light Transition',
            color: ltColor,
            isAuto: _fadeAuto,
            sliderValue: _fadeMs,
            effectiveValue: effectiveFadeMs,
            sliderMin: 0,
            sliderMax: 1000,
            divisions: 20,
            format: _formatFade,
            tooltip:
                'How smoothly the lights fade between brightness and color '
                'changes. Lower values feel snappier, higher values feel '
                'gentler.',
            onSliderChanged: (v) => setState(() {
              _fadeMs = v;
              _markDirty();
            }),
            onAuto: () => setState(() {
              _fadeAuto = true;
              _markDirty();
            }),
            onManual: () => setState(() {
              _fadeMs = effectiveFadeMs;
              _fadeAuto = false;
              _markDirty();
            }),
          ),
          divider,
          _buildTimingRow(
            icon: Icons.update_rounded,
            title: 'Background Interval',
            color: intColor,
            isAuto: _intervalAuto,
            sliderValue: _intervalSecs,
            effectiveValue: effectiveIntervalSecs,
            sliderMin: 30,
            sliderMax: 300,
            divisions: 27,
            format: _formatInterval,
            tooltip:
                'How often the lights update their color and brightness '
                'throughout the day and night.',
            onSliderChanged: (v) => setState(() {
              _intervalSecs = v;
              _markDirty();
            }),
            onAuto: () => setState(() {
              _intervalAuto = true;
              _markDirty();
            }),
            onManual: () => setState(() {
              _intervalSecs = effectiveIntervalSecs;
              _intervalAuto = false;
              _markDirty();
            }),
          ),
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

  Widget _buildRoomDefaultsSection({required bool canUseStandby}) {
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
                    Expanded(
                      child: Row(
                        children: [
                          const Flexible(
                            child: Text(
                              'Room Defaults',
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                color: _Palette.textPrimary,
                                fontSize: 15,
                                fontWeight: FontWeight.w600,
                                letterSpacing: -0.1,
                              ),
                            ),
                          ),
                          const SizedBox(width: 4),
                          InfoTooltip(
                            message:
                                'Override the default state for each room '
                                'while this profile is active. Useful for '
                                'keeping certain rooms always on, off, or in '
                                'standby regardless of the curve.',
                            iconSize: 13,
                          ),
                        ],
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
                        padding: const EdgeInsets.only(top: 14),
                        child: Column(
                          children: [
                            for (int i = 0; i < rooms.length; i++) ...[
                              if (i > 0) const SizedBox(height: 6),
                              _RoomDefaultCard(
                                key: ValueKey(rooms[i].id),
                                roomId: rooms[i].id,
                                roomName: rooms[i].name,
                                state: defaults[rooms[i].id],
                                canUseStandby: canUseStandby,
                                onStateChanged: (newState) =>
                                    _onRoomDefaultChanged(
                                        rooms[i].id, newState),
                              ),
                            ],
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
  // Advanced — Pro-gated features collected under one expandable section.
  // Cards are always rendered; when the user lacks the relevant entitlement
  // the body is dimmed, pointer events are absorbed by an upsell tap target
  // that opens the PlanTierModal.
  // ---------------------------------------------------------------------------

  Widget _buildAdvancedSection({
    required bool canUseAdvancedDay,
    required bool canUseStandby,
    required bool canUseSleepPrimary,
  }) {
    final expanded = _advancedExpanded;
    final lockedCount = _isSleepProfile
        ? (canUseSleepPrimary ? 0 : 1) + (canUseStandby ? 0 : 1)
        : (canUseAdvancedDay ? 0 : 1) + (canUseStandby ? 0 : 1);
    final allUnlocked = lockedCount == 0;
    const accent = _Palette.amber;

    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      padding: EdgeInsets.fromLTRB(18, 18, 18, expanded ? 14 : 18),
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            _Palette.card,
            allUnlocked
                ? _Palette.card
                : Color.alphaBlend(
                    accent.withValues(alpha: 0.04), _Palette.card),
          ],
        ),
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: expanded
              ? accent.withValues(alpha: 0.30)
              : accent.withValues(alpha: 0.14),
        ),
        boxShadow: expanded
            ? [
                BoxShadow(
                  color: accent.withValues(alpha: 0.10),
                  blurRadius: 24,
                  spreadRadius: -4,
                ),
              ]
            : const [],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: () {
              HapticFeedback.selectionClick();
              setState(() => _advancedExpanded = !_advancedExpanded);
            },
            child: Row(
              children: [
                _AdvancedHeaderIcon(locked: !allUnlocked),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text(
                        'Advanced',
                        style: TextStyle(
                          color: _Palette.textPrimary,
                          fontSize: 15,
                          fontWeight: FontWeight.w600,
                          letterSpacing: -0.1,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        _isSleepProfile
                            ? 'Custom sleep colors & standby'
                            : 'Fine-tune timing & standby',
                        style: const TextStyle(
                          color: _Palette.textSecondary,
                          fontSize: 11,
                          fontWeight: FontWeight.w500,
                          letterSpacing: 0.1,
                        ),
                      ),
                    ],
                  ),
                ),
                if (!expanded && !allUnlocked)
                  _ProBadge(label: '$lockedCount locked'),
                if (!expanded && allUnlocked)
                  _ProBadge(label: 'Pro', solid: true),
                const SizedBox(width: 6),
                AnimatedRotation(
                  turns: expanded ? 0.5 : 0,
                  duration: const Duration(milliseconds: 250),
                  curve: Curves.easeOutCubic,
                  child: Icon(
                    Icons.keyboard_arrow_down_rounded,
                    color: _Palette.textSecondary.withValues(alpha: 0.4),
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
                    padding: const EdgeInsets.only(top: 14),
                    child: _isSleepProfile
                        ? Column(
                            children: [
                              _ProLockWrap(
                                unlocked: canUseSleepPrimary,
                                entitlement: Entitlement.sleepPrimarySettings,
                                child: _buildSleepPrimarySettingsCard(),
                              ),
                              const SizedBox(height: 10),
                              _ProLockWrap(
                                unlocked: canUseSleepPrimary,
                                entitlement: Entitlement.sleepPrimarySettings,
                                child: _buildTimingCard(),
                              ),
                              const SizedBox(height: 10),
                              _ProLockWrap(
                                unlocked: canUseStandby,
                                entitlement: Entitlement.standby,
                                child: _buildIdleSection(),
                              ),
                            ],
                          )
                        : Column(
                            children: [
                              _ProLockWrap(
                                unlocked: canUseAdvancedDay,
                                entitlement: Entitlement.advancedDayControls,
                                child: _buildTimingCard(),
                              ),
                              const SizedBox(height: 10),
                              _ProLockWrap(
                                unlocked: canUseStandby,
                                entitlement: Entitlement.standby,
                                child: _buildIdleSection(),
                              ),
                            ],
                          ),
                  )
                : const SizedBox.shrink(),
          ),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Pending changes — Revert | Save
  // ---------------------------------------------------------------------------

  Widget _buildPendingChangesActions() {
    final canRevert = !_isSaving && _curveConfigDirty;
    return Row(
      children: [
        Expanded(child: _buildRevertButton(enabled: canRevert)),
        const SizedBox(width: 10),
        Expanded(child: _buildSaveButton()),
      ],
    );
  }

  Widget _buildRevertButton({required bool enabled}) {
    final color = _Palette.textSecondary;
    return GestureDetector(
      onTap: enabled ? _revertChanges : null,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 14),
        decoration: BoxDecoration(
          color: color.withValues(alpha: enabled ? 0.08 : 0.04),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: color.withValues(alpha: enabled ? 0.20 : 0.10),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              Icons.undo_rounded,
              color: color.withValues(alpha: enabled ? 0.85 : 0.35),
              size: 18,
            ),
            const SizedBox(width: 8),
            Text(
              'Revert',
              style: TextStyle(
                color: color.withValues(alpha: enabled ? 0.95 : 0.4),
                fontSize: 14,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildSaveButton() {
    return GestureDetector(
      onTap: _isSaving ? null : _saveCurveConfig,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 14),
        decoration: BoxDecoration(
          color: _Palette.amber.withValues(alpha: _isSaving ? 0.06 : 0.1),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: _Palette.amber.withValues(alpha: _isSaving ? 0.18 : 0.25),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (_isSaving)
              const SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation<Color>(_Palette.amber),
                ),
              )
            else
              const Icon(Icons.save_rounded, color: _Palette.amber, size: 18),
            const SizedBox(width: 8),
            Text(
              _isSaving ? 'Applying…' : 'Save',
              style: const TextStyle(
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

  void _revertChanges() {
    final baseline = _profileConfigs[_selectedProfileId];
    if (baseline == null) return;
    setState(() {
      _applyProfileConfig(baseline);
      final idleProfileId = _customIdleProfileIdForProfile(_selectedProfileId);
      final idleConfig =
          idleProfileId != null ? _profileConfigs[idleProfileId] : null;
      if (idleConfig != null) {
        _applyIdleConfig(idleConfig);
      } else {
        _applyIdleFallback();
      }
      _curveConfigDirty = false;
    });
    unawaited(_loadCurveData(profileId: _selectedProfileId));
  }

  Future<void> _saveCurveConfig() async {
    if (_isSaving) return;
    final config = _buildDraftConfig();
    final serverSync = context.read<ServerSyncProvider>();
    final canUseStandby =
        context.read<SubscriptionProvider>().has(Entitlement.standby);
    final api = serverSync.api;
    final activeProfileId = serverSync.activeProfileId;
    setState(() {
      _curveConfigDirty = false;
      _isSaving = true;
    });
    try {
      final profileSaved = await api.configSet(
        config,
        id: _selectedProfileId,
        apply: activeProfileId != null && _selectedProfileId == activeProfileId,
      );
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

      // Also save idle config when standby is entitled. Free users keep
      // Power Save on and detach any custom standby profile on the next save.
      final idleConfig = canUseStandby ? _buildIdleDraftConfig() : null;
      bool idleSaved = true;
      if (idleConfig != null) {
        idleSaved = await api.configSet(
          idleConfig,
          id: idleConfig.id,
          apply: activeProfileId != null && idleConfig.id == activeProfileId,
        );
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
    } finally {
      if (mounted) {
        setState(() => _isSaving = false);
      } else {
        _isSaving = false;
      }
    }
  }

  Widget _buildResetToDefaultsButton() {
    return GestureDetector(
      onTap: _confirmResetToDefaults,
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

  Future<void> _confirmResetToDefaults() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        backgroundColor: _Palette.card,
        title: Text(
          'Reset $_profileTitle?',
          style: const TextStyle(color: _Palette.textPrimary),
        ),
        content: Text(
          'This restores every setting in the ${_profileTitle.toLowerCase()} '
          'to its factory default. Any customizations you have made will be '
          'lost.',
          style: const TextStyle(color: _Palette.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text(
              'Cancel',
              style: TextStyle(color: _Palette.textSecondary),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text(
              'Reset',
              style: TextStyle(color: Colors.redAccent),
            ),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    await _resetToDefaults();
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
  final bool canUseStandby;
  final ValueChanged<String?> onStateChanged;

  const _RoomDefaultCard({
    super.key,
    required this.roomId,
    required this.roomName,
    required this.state,
    required this.canUseStandby,
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
      _RoomDefaultMode.active => 'On',
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
          onLongPress: () {
            HapticFeedback.lightImpact();
            onStateChanged('hard_off');
          },
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeInOut,
            decoration: BoxDecoration(
              color: bgColor,
              borderRadius: BorderRadius.circular(12),
              border: Border.all(
                color: hasOverride
                    ? _Palette.border
                    : _Palette.border.withValues(alpha: 0.4),
              ),
            ),
            padding: const EdgeInsets.fromLTRB(12, 10, 10, 10),
            child: Row(
              children: [
                if (motionTimer != null)
                  Padding(
                    padding: const EdgeInsets.only(right: 10),
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
                    padding: const EdgeInsets.only(right: 10),
                    child: Icon(
                      Icons.sensors_rounded,
                      size: 16,
                      color: indicatorColor.withValues(alpha: 0.45),
                    ),
                  ),
                Expanded(
                  child: AnimatedOpacity(
                    opacity: hasOverride ? 1.0 : 0.55,
                    duration: const Duration(milliseconds: 300),
                    child: Text(
                      roomName,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        color: _Palette.textPrimary,
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ),
                if (hasOverride) ...[
                  const SizedBox(width: 8),
                  Text(
                    stateLabel,
                    style: TextStyle(
                      color: stateLabelColor,
                      fontSize: 11,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ],
                const SizedBox(width: 10),
                _DefaultStateToggle(
                  mode: mode,
                  canUseStandby: canUseStandby,
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
  final bool canUseStandby;
  final ValueChanged<_RoomDefaultMode> onModeChanged;

  const _DefaultStateToggle({
    required this.mode,
    required this.canUseStandby,
    required this.onModeChanged,
  });

  void _onTap() {
    if (!canUseStandby) {
      // Free tier: active → off → none (no override) → active.
      onModeChanged(switch (mode) {
        _RoomDefaultMode.active => _RoomDefaultMode.off,
        _RoomDefaultMode.off => _RoomDefaultMode.none,
        _ => _RoomDefaultMode.active,
      });
      return;
    }
    // Pro tier: active → idle → off → none (no override) → active.
    onModeChanged(switch (mode) {
      _RoomDefaultMode.active => _RoomDefaultMode.idle,
      _RoomDefaultMode.idle => _RoomDefaultMode.off,
      _RoomDefaultMode.off => _RoomDefaultMode.none,
      _RoomDefaultMode.none => _RoomDefaultMode.active,
    });
  }

  void _onLongPress() {
    if (mode != _RoomDefaultMode.off) {
      onModeChanged(_RoomDefaultMode.off);
    }
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: _onTap,
      onLongPress: _onLongPress,
      behavior: HitTestBehavior.opaque,
      child: SizedBox(
        width: 64,
        height: 28,
        child: AnimatedSwitcher(
          duration: const Duration(milliseconds: 220),
          switchInCurve: Curves.easeOut,
          switchOutCurve: Curves.easeIn,
          transitionBuilder: (child, animation) => FadeTransition(
            opacity: animation,
            child: ScaleTransition(
              scale: Tween<double>(begin: 0.92, end: 1).animate(animation),
              child: child,
            ),
          ),
          child: mode == _RoomDefaultMode.none
              ? _buildNoOverrideChip()
              : _buildToggle(),
        ),
      ),
    );
  }

  Widget _buildNoOverrideChip() {
    return Container(
      key: const ValueKey('none'),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: _Palette.textSecondary.withValues(alpha: 0.22),
          width: 1,
        ),
      ),
      alignment: Alignment.center,
      child: Text(
        'Auto',
        style: TextStyle(
          color: _Palette.textSecondary.withValues(alpha: 0.7),
          fontSize: 10,
          fontWeight: FontWeight.w600,
          letterSpacing: 0.4,
        ),
      ),
    );
  }

  Widget _buildToggle() {
    // For free tier, legacy `idle` collapses to `off` since standby isn't
    // available — the thumb only ever sits at left or right.
    final visualMode = switch (mode) {
      _RoomDefaultMode.idle when !canUseStandby => _RoomDefaultMode.off,
      _ => mode,
    };

    final alignment = switch (visualMode) {
      _RoomDefaultMode.off => Alignment.centerLeft,
      _RoomDefaultMode.idle => Alignment.center,
      _RoomDefaultMode.active => Alignment.centerRight,
      _RoomDefaultMode.none => Alignment.centerRight, // unreachable
    };

    final trackGradient = switch (visualMode) {
      _RoomDefaultMode.off || _RoomDefaultMode.none => const LinearGradient(
          colors: [Color(0xFF2A2F38), Color(0xFF30363D)],
        ),
      _RoomDefaultMode.idle => const LinearGradient(
          colors: [Color(0xFF221C14), Color(0xFF2E2518)],
        ),
      _RoomDefaultMode.active => const LinearGradient(
          colors: [Color(0xFF8B6B20), Color(0xFFD4A020)],
        ),
    };

    final thumbColor = switch (visualMode) {
      _RoomDefaultMode.off || _RoomDefaultMode.none => _Palette.textSecondary,
      _RoomDefaultMode.idle => const Color(0xFFCDBFAA),
      _RoomDefaultMode.active => Colors.white,
    };

    final thumbShadow = switch (visualMode) {
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

    return AnimatedContainer(
      key: const ValueKey('toggle'),
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeInOut,
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
  static const amberWarm = Color(0xFFFFB900);
  static const amberDeep = Color(0xFFFF8C00);
  static const blue = Color(0xFF58A6FF);
  static const teal = Color(0xFF4ADE80);
  static const idle = Color(0xFFB8A890);
}

// ---------------------------------------------------------------------------
// Advanced section — header icon, Pro badge, and per-card lock wrap.
// ---------------------------------------------------------------------------

class _AdvancedHeaderIcon extends StatelessWidget {
  const _AdvancedHeaderIcon({required this.locked});
  final bool locked;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 36,
      height: 36,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        gradient: const LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [_Palette.amberWarm, _Palette.amberDeep],
        ),
        boxShadow: [
          BoxShadow(
            color: _Palette.amber.withValues(alpha: locked ? 0.18 : 0.35),
            blurRadius: 14,
            spreadRadius: -2,
          ),
        ],
      ),
      child: Icon(
        locked ? Icons.workspace_premium_rounded : Icons.auto_awesome_rounded,
        color: Colors.white,
        size: 18,
      ),
    );
  }
}

class _ProBadge extends StatelessWidget {
  const _ProBadge({required this.label, this.solid = false});
  final String label;
  final bool solid;

  @override
  Widget build(BuildContext context) {
    if (!FeatureFlags.entitlementsEnabled) return const SizedBox.shrink();
    if (solid) {
      return Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
        decoration: BoxDecoration(
          gradient: const LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [_Palette.amberWarm, _Palette.amberDeep],
          ),
          borderRadius: BorderRadius.circular(6),
        ),
        child: Text(
          label.toUpperCase(),
          style: const TextStyle(
            color: Colors.white,
            fontSize: 10,
            fontWeight: FontWeight.w800,
            letterSpacing: 1.0,
          ),
        ),
      );
    }
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: _Palette.amber.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: _Palette.amber.withValues(alpha: 0.30)),
      ),
      child: Text(
        label,
        style: TextStyle(
          color: _Palette.amber.withValues(alpha: 0.92),
          fontSize: 11,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.1,
        ),
      ),
    );
  }
}

/// Wraps a Pro-only card so it always renders, but blocks interaction and
/// reveals an upsell tap target when [unlocked] is false. The visual
/// treatment — dimmed body, amber gloss, a floating PRO chip — is meant to
/// read as "you can see what you're missing" rather than "this is disabled".
class _ProLockWrap extends StatelessWidget {
  const _ProLockWrap({
    required this.child,
    required this.unlocked,
    required this.entitlement,
  });

  final Widget child;
  final bool unlocked;
  final Entitlement entitlement;

  @override
  Widget build(BuildContext context) {
    if (unlocked) return child;
    return _LockedCard(entitlement: entitlement, child: child);
  }
}

class _LockedCard extends StatefulWidget {
  const _LockedCard({required this.child, required this.entitlement});

  final Widget child;
  final Entitlement entitlement;

  @override
  State<_LockedCard> createState() => _LockedCardState();
}

class _LockedCardState extends State<_LockedCard>
    with SingleTickerProviderStateMixin {
  late final AnimationController _shimmer;

  @override
  void initState() {
    super.initState();
    _shimmer = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 3400),
    )..repeat();
  }

  @override
  void dispose() {
    _shimmer.dispose();
    super.dispose();
  }

  void _openUpsell() {
    HapticFeedback.lightImpact();
    PlanTierModal.show(context, highlightFeature: widget.entitlement);
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: _openUpsell,
      child: ClipRRect(
        borderRadius: BorderRadius.circular(18),
        child: Stack(
          children: [
            // 1. The real card, painted but inert.
            IgnorePointer(
              ignoring: true,
              child: ColorFiltered(
                colorFilter: const ColorFilter.matrix(<double>[
                  // De-saturate ~55% so cool greens/teals don't fight the
                  // warm amber lock veil.
                  0.55, 0.35, 0.10, 0, 0,
                  0.20, 0.65, 0.15, 0, 0,
                  0.20, 0.35, 0.45, 0, 0,
                  0, 0, 0, 0.55, 0,
                ]),
                child: child(),
              ),
            ),

            // 2. Diagonal amber veil — top-right glow.
            Positioned.fill(
              child: IgnorePointer(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.bottomLeft,
                      end: Alignment.topRight,
                      colors: [
                        _Palette.amberWarm.withValues(alpha: 0.02),
                        _Palette.amberWarm.withValues(alpha: 0.08),
                      ],
                    ),
                  ),
                ),
              ),
            ),

            // 3. Slow shimmer sweep that signals "tap me".
            Positioned.fill(
              child: IgnorePointer(
                child: AnimatedBuilder(
                  animation: _shimmer,
                  builder: (context, _) {
                    final t = _shimmer.value;
                    return ShaderMask(
                      shaderCallback: (rect) => LinearGradient(
                        begin: const Alignment(-1.4, -1),
                        end: const Alignment(1.4, 1),
                        stops: [
                          (t - 0.20).clamp(0.0, 1.0),
                          t.clamp(0.0, 1.0),
                          (t + 0.20).clamp(0.0, 1.0),
                        ],
                        colors: [
                          Colors.white.withValues(alpha: 0.00),
                          Colors.white.withValues(alpha: 0.04),
                          Colors.white.withValues(alpha: 0.00),
                        ],
                      ).createShader(rect),
                      blendMode: BlendMode.plus,
                      child: const ColoredBox(color: Colors.transparent),
                    );
                  },
                ),
              ),
            ),

            // 4. Floating PRO chip in the top-right.
            const Positioned(
              top: 10,
              right: 12,
              child: _ProBadge(label: 'Pro', solid: true),
            ),

            // 5. Bottom-center upsell hint.
            Positioned(
              left: 0,
              right: 0,
              bottom: 10,
              child: Center(
                child: Container(
                  padding: const EdgeInsets.symmetric(
                      horizontal: 12, vertical: 6),
                  decoration: BoxDecoration(
                    color: Colors.black.withValues(alpha: 0.38),
                    borderRadius: BorderRadius.circular(999),
                    border: Border.all(
                      color: _Palette.amber.withValues(alpha: 0.35),
                    ),
                  ),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      const Icon(
                        Icons.lock_open_rounded,
                        size: 12,
                        color: _Palette.amberWarm,
                      ),
                      const SizedBox(width: 6),
                      Text(
                        'Tap to unlock',
                        style: TextStyle(
                          color: Colors.white.withValues(alpha: 0.95),
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.2,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget child() => widget.child;
}
