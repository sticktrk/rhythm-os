import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;
import '../../config/feature_flags.dart';
import '../../models/config_model.dart';
import '../../models/plan_tier.dart';
import '../../providers/server_sync_provider.dart';
import '../../providers/subscription_provider.dart';
import '../../services/analytics_service.dart';
import '../../widgets/auto_slider_setting_row.dart';
import '../../widgets/info_tooltip.dart';
import '../../widgets/pro_lock.dart';

/// Full-screen modal for configuring the light profile.
///
/// Features a compressed color spectrum slider that emphasizes the dawn/dusk
/// ramps where color changes rapidly, compressing flat night/day regions.
class LightProfileScreen extends StatefulWidget {
  final String? initialProfile;

  /// When true, render only the scrollable content body — no [Scaffold],
  /// [SafeArea], header, or outer scroll view — so the screen can be stacked
  /// inside a host like `LightScreen` that supplies its own chrome and a single
  /// shared scroll view.
  final bool embedded;

  const LightProfileScreen({
    super.key,
    this.initialProfile,
    this.embedded = false,
  });

  @override
  State<LightProfileScreen> createState() => _LightProfileScreenState();
}

class _LightProfileScreenState extends State<LightProfileScreen> {
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
  bool _idleExpanded = false;
  bool _advancedExpanded = false;

  // Sleep profile state: optional fixed color + optional fixed brightness.
  // When neither toggle is on, sleep inherits CCT + brightness from the wake
  // (rhythm) profile's min_color_temp and min_brightness.
  double _sleepHue = 10;
  double _sleepBrightness = 20;
  bool _sleepCustomBri = false;
  bool _sleepCustomColor = false;

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

  @override
  void initState() {
    super.initState();
    _selectedProfileId = switch (widget.initialProfile) {
      'idle' || 'day_idle' => 'rhythm',
      'sleep_idle' => 'sleep',
      final profile? => profile,
      null => 'rhythm',
    };

    _serverSync = context.read<ServerSyncProvider>();
    _serverSync.addListener(_handleServerSyncChanged);
    unawaited(_loadConfig());
  }

  @override
  void dispose() {
    _serverSync.removeListener(_handleServerSyncChanged);
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

  bool get _hasLocalDraft => _curveConfigDirty || _isSaving;

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
      mode == sdk.RhythmMode.sleep ? 'Sleep Mood' : 'Day Mood';

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
      if (!mounted) return;

      setState(() {
        _selectedProfileId = initialProfileId;
        _connected = syncProvider.hasBeenSynced;
        _loading = false;
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

  Future<void> _resetToDefaults() async {
    final api = context.read<ServerSyncProvider>().api;
    final sdkConfig = await api.resetConfig(id: _selectedProfileId);
    if (!mounted) return;
    if (sdkConfig != null) {
      _profileConfigs[_selectedProfileId] = sdkConfig;
      _applyProfileConfig(sdkConfig);
      await _syncActiveConfigModel(sdkConfig);
    }
    setState(() {
      _curveConfigDirty = false;
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

  // ---------------------------------------------------------------------------
  // Build
  // ---------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    if (widget.embedded) return _buildBody();
    return Scaffold(
      backgroundColor: _Palette.bg,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(child: _buildBody()),
          ],
        ),
      ),
    );
  }

  Widget _buildBody() {
    if (_loading) {
      if (widget.embedded) {
        return const Padding(
          padding: EdgeInsets.symmetric(vertical: 48),
          child: Center(
            child: CircularProgressIndicator(
              strokeWidth: 2,
              color: _Palette.amber,
            ),
          ),
        );
      }
      return const Center(
        child: CircularProgressIndicator(
          strokeWidth: 2,
          color: _Palette.amber,
        ),
      );
    }
    if (!_connected) return _buildDisconnected();
    return _buildContent();
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
    String? valueLabel,
    Widget? valueChild,
    required VoidCallback onAuto,
    required VoidCallback onManual,
  }) {
    assert(valueLabel != null || valueChild != null,
        'Either valueLabel or valueChild must be provided');

    Widget chipFrame({
      required bool active,
      required Widget child,
      required VoidCallback onTap,
    }) {
      return GestureDetector(
        onTap: onTap,
        behavior: HitTestBehavior.opaque,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          decoration: BoxDecoration(
            color: active ? color.withValues(alpha: 0.14) : Colors.transparent,
            borderRadius: BorderRadius.circular(6),
            border: Border.all(
              color: active
                  ? color.withValues(alpha: 0.30)
                  : color.withValues(alpha: 0.14),
            ),
          ),
          child: child,
        ),
      );
    }

    Widget textBody(String label, bool active) {
      return Text(
        label,
        style: TextStyle(
          color: active ? color : color.withValues(alpha: 0.45),
          fontSize: 11,
          fontWeight: FontWeight.w600,
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      );
    }

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        chipFrame(
          active: isAuto,
          child: textBody('Auto', isAuto),
          onTap: isAuto ? () {} : onAuto,
        ),
        const SizedBox(width: 6),
        chipFrame(
          active: !isAuto,
          child: valueChild ?? textBody(valueLabel!, !isAuto),
          onTap: !isAuto ? () {} : onManual,
        ),
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

    final children = <Widget>[
      if (_isSleepProfile) ...[
        // Sleep "look" — promoted out of Advanced so the Light tab shows the
        // sleep brightness + color directly alongside Day's. Per-room
        // on/off/standby behavior now lives on the Automations tab.
        ProLockWrap(
          unlocked: canUseSleepPrimary,
          entitlement: Entitlement.sleepPrimarySettings,
          child: _buildSleepBrightnessCard(),
        ),
        const SizedBox(height: 14),
        ProLockWrap(
          unlocked: canUseSleepPrimary,
          entitlement: Entitlement.sleepPrimarySettings,
          child: _buildSleepColorCard(),
        ),
        const SizedBox(height: 24),
      ] else ...[
        _buildBrightnessRangeCard(),
        const SizedBox(height: 14),
        _buildColorTempRangeCard(),
        const SizedBox(height: 24),
      ],
      // Advanced (timing fine-tune) is hidden for now behind a feature flag
      // while the Light tab moves to layered profiles — retained for later.
      if (FeatureFlags.showAdvancedLightSection)
        _buildAdvancedSection(
          canUseAdvancedDay: canUseAdvancedDay,
          canUseSleepPrimary: canUseSleepPrimary,
        ),
      if (_curveConfigDirty || _isSaving) ...[
        const SizedBox(height: 24),
        _buildPendingChangesActions(),
      ],
      const SizedBox(height: 32),
      _buildResetToDefaultsButton(),
    ];

    if (widget.embedded) {
      // Host (LightScreen layer card) supplies the outer inset and a header,
      // so keep the embedded body padding tight and let the card frame it.
      return Padding(
        padding: const EdgeInsets.fromLTRB(16, 6, 16, 18),
        child: Column(children: children),
      );
    }
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(20, 8, 20, 40),
      child: Column(children: children),
    );
  }

  // ---------------------------------------------------------------------------
  // Sleep Profile — Brightness & Color
  //
  // Each is its own card, but the header chips use the same `_buildAutoToggle`
  // segmented control as the Timing rows below: `[Auto] [value]`, one filled
  // / one outlined. Tapping a chip flips the mode; tapping the value chip in
  // AUTO state enters custom mode at the inherited value. The slider/picker
  // only renders in custom mode (mirroring how Timing rows hide their slider
  // in Auto mode).
  // ---------------------------------------------------------------------------

  Widget _buildSleepBrightnessCard() {
    const amber = _Palette.amber;
    final isCustom = _sleepCustomBri;
    final autoValue = _wakeMinBrightness;
    final displayValue = isCustom ? _sleepBrightness.round() : autoValue;

    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      padding: EdgeInsets.fromLTRB(18, 18, 18, isCustom ? 10 : 18),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: isCustom ? amber.withValues(alpha: 0.25) : _Palette.border,
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
                  color: amber.withValues(alpha: 0.12),
                ),
                child: const Icon(
                  Icons.brightness_medium_rounded,
                  color: amber,
                  size: 18,
                ),
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
              _buildAutoToggle(
                color: amber,
                isAuto: !isCustom,
                valueLabel: '$displayValue%',
                onAuto: () => setState(() {
                  _sleepCustomBri = false;
                  // Reset slider value so toggling back to custom starts
                  // fresh at the inherited AUTO value, not a stale custom.
                  _sleepBrightness =
                      _wakeMinBrightness.toDouble().clamp(1, 100);
                  _markDirty();
                }),
                onManual: () => setState(() {
                  _sleepCustomBri = true;
                  _markDirty();
                }),
              ),
            ],
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 250),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: isCustom
                ? Padding(
                    padding: const EdgeInsets.only(top: 8),
                    child: _buildInlineSlider(
                      label: 'Level',
                      value: _sleepBrightness.clamp(1, 100).toDouble(),
                      min: 1,
                      max: 100,
                      divisions: 99,
                      format: (v) => '${v.round()}%',
                      color: amber,
                      onChanged: (v) => setState(() {
                        _sleepBrightness = v;
                        _markDirty();
                      }),
                    ),
                  )
                : const SizedBox.shrink(),
          ),
        ],
      ),
    );
  }

  Widget _buildSleepColorCard() {
    const amber = _Palette.amber;
    final isCustom = _sleepCustomColor;
    final swatch = _sleepSelectedColor;
    final wakeCctColor = ColorUtils.curveColorForCCT(_wakeMinColorTemp);
    final dotColor = isCustom ? swatch : wakeCctColor;

    // Small swatch dot used in place of the value chip's text label.
    final swatchDot = Container(
      width: 12,
      height: 12,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: dotColor.withValues(alpha: isCustom ? 1.0 : 0.55),
        boxShadow: isCustom
            ? [
                BoxShadow(
                  color: dotColor.withValues(alpha: 0.55),
                  blurRadius: 6,
                  spreadRadius: -1,
                ),
              ]
            : null,
      ),
    );

    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      padding: EdgeInsets.fromLTRB(18, 18, 18, isCustom ? 10 : 18),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: isCustom ? swatch.withValues(alpha: 0.30) : _Palette.border,
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
                      dotColor.withValues(alpha: 0.18),
                      dotColor.withValues(alpha: 0.32),
                    ],
                  ),
                ),
                child: Icon(
                  Icons.palette_outlined,
                  color: dotColor,
                  size: 18,
                ),
              ),
              const SizedBox(width: 12),
              const Expanded(
                child: Text(
                  'Color',
                  style: TextStyle(
                    color: _Palette.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                    letterSpacing: -0.1,
                  ),
                ),
              ),
              _buildAutoToggle(
                color: amber,
                isAuto: !isCustom,
                valueChild: swatchDot,
                onAuto: () => setState(() {
                  _sleepCustomColor = false;
                  // Reset hue to match the inherited wake CCT color so
                  // toggling back to custom doesn't surface a stale hue.
                  _sleepHue = HSVColor.fromColor(wakeCctColor).hue;
                  _markDirty();
                }),
                onManual: () => setState(() {
                  _sleepCustomColor = true;
                  _markDirty();
                }),
              ),
            ],
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 250),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: isCustom
                ? Padding(
                    padding: const EdgeInsets.only(top: 14),
                    child: _buildFixedColorPicker(
                      hue: _sleepHue,
                      selectedColor: _sleepSelectedColor,
                      isPresetSelected: _isSleepPresetSelected,
                      showSelection: _sleepCustomColor,
                      onHueChanged: (hue) {
                        setState(() {
                          _sleepHue = hue;
                          _sleepCustomColor = true;
                          _markDirty();
                        });
                      },
                    ),
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

  // ignore: unused_element
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
                        'Mood Settings',
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
    bool showSelection = true,
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
                    if (showSelection)
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
                final selected =
                    showSelection && isPresetSelected(preset.color);
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

    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 10),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: color.withValues(alpha: 0.25)),
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
              Expanded(
                child: Row(
                  children: [
                    const Flexible(
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
                    const SizedBox(width: 4),
                    InfoTooltip(
                      accentColor: color,
                      message:
                          'The lowest and highest brightness your lights reach '
                          'through the day. The curve eases between these two '
                          'values as it rises and falls.',
                    ),
                  ],
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
            ],
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(2, 10, 2, 4),
            child: _DualRangeBar(
              minValue: _minBrightness,
              maxValue: _maxBrightness,
              hardMin: 1,
              hardMax: 100,
              minThumbMax: 50,
              maxThumbMin: 20,
              tint: color,
              divisions: 99,
              onMinChanged: (v) =>
                  _onCurveChanged(() => _minBrightness = v),
              onMaxChanged: (v) =>
                  _onCurveChanged(() => _maxBrightness = v),
            ),
          ),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Color Temperature Card — kelvin range with gradient preview
  // ---------------------------------------------------------------------------

  Widget _buildColorTempRangeCard() {
    final warmColor = ColorUtils.curveColorForCCT(_minColorTemp.round());
    final coolColor = ColorUtils.curveColorForCCT(_maxColorTemp.round());

    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 10),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: warmColor.withValues(alpha: 0.2)),
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
              Expanded(
                child: Row(
                  children: [
                    const Flexible(
                      child: Text(
                        'Sun Hue',
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
                      accentColor: warmColor,
                      message:
                          'The warmest and coolest white your lights reach '
                          'through the day — warm at dawn and dusk, cool around '
                          'midday.',
                    ),
                  ],
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
            ],
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(2, 10, 2, 4),
            child: _DualRangeBar(
              minValue: _minColorTemp,
              maxValue: _maxColorTemp,
              hardMin: 1500,
              hardMax: 6500,
              minThumbMax: 4000,
              maxThumbMin: 2000,
              tint: warmColor,
              minThumbColor: warmColor,
              maxThumbColor: coolColor,
              gradient: LinearGradient(
                colors: List.generate(12, (i) {
                  final k = 1500 + (i / 11) * 5000;
                  return ColorUtils.curveColorForCCT(k.round());
                }),
              ),
              divisions: 50,
              onMinChanged: (v) =>
                  _onCurveChanged(() => _minColorTemp = v),
              onMaxChanged: (v) =>
                  _onCurveChanged(() => _maxColorTemp = v),
            ),
          ),
        ],
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
    return AutoSliderSettingRow(
      icon: icon,
      title: title,
      color: color,
      isAuto: isAuto,
      sliderValue: sliderValue,
      effectiveValue: effectiveValue,
      sliderMin: sliderMin,
      sliderMax: sliderMax,
      divisions: divisions,
      format: format,
      onSliderChanged: onSliderChanged,
      onAuto: onAuto,
      onManual: onManual,
      tooltip: tooltip,
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
    final effectiveFadeMs = _serverSync.effectiveFadeMs?.toDouble() ?? _fadeMs;
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
                'before timing out to mood.',
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
            tooltip: 'How often the lights update their color and brightness '
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
  // Advanced — Pro-gated features collected under one expandable section.
  // Cards are always rendered; when the user lacks the relevant entitlement
  // the body is dimmed, pointer events are absorbed by an upsell tap target
  // that opens the PlanTierModal.
  // ---------------------------------------------------------------------------

  Widget _buildAdvancedSection({
    required bool canUseAdvancedDay,
    required bool canUseSleepPrimary,
  }) {
    final expanded = _advancedExpanded;
    final lockedCount = _isSleepProfile
        ? (canUseSleepPrimary ? 0 : 1)
        : (canUseAdvancedDay ? 0 : 1);
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
                      const Text(
                        'Fine-tune timing',
                        style: TextStyle(
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
                  ProBadge(label: '$lockedCount locked'),
                if (!expanded && allUnlocked)
                  ProBadge(label: 'Pro', solid: true),
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
                        ? ProLockWrap(
                            unlocked: canUseSleepPrimary,
                            entitlement: Entitlement.sleepPrimarySettings,
                            child: _buildTimingCard(),
                          )
                        : ProLockWrap(
                            unlocked: canUseAdvancedDay,
                            entitlement: Entitlement.advancedDayControls,
                            child: _buildTimingCard(),
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
  }

  Future<void> _saveCurveConfig() async {
    if (_isSaving) return;
    final config = _buildDraftConfig();
    final serverSync = context.read<ServerSyncProvider>();
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

      final currentIdleProfileId = _selectedCustomIdleProfileId;
      const String? targetIdleProfileId = null;
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
            'Saved ${_profileTitle.toLowerCase()}, but failed to clear legacy standby settings.',
            error: true,
          );
          return;
        }
      }

      _profileConfigs[_selectedProfileId] = config;
      if (updatedModeConfigs != null) {
        _modeConfigs = updatedModeConfigs;
      }
      _applyProfileConfig(config);
      _applyIdleFallback();
      await _syncActiveConfigModel(config);
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

// ---------------------------------------------------------------------------
// Dual-thumb range bar
//
// A single horizontal track that holds two draggable handles — one for the
// min, one for the max. Used on the Day Profile screen so Brightness and
// Color Temperature are each tuned as a single "window" you stretch and
// shrink with two thumbs instead of two separate sliders.
//
// The track always shows the full hardMin..hardMax span: a gradient (CCT) or
// a tint ramp (brightness). The inactive regions outside the selected window
// are dimmed with a scrim, so the active slice reads as a luminous opening
// cut into the spectrum. Thumbs are aperture-style discs with a tinted ring
// and an inner iris — distinctive, on-brand for a lighting app.
// ---------------------------------------------------------------------------

enum _DualRangeThumb { min, max }

class _DualRangeBar extends StatefulWidget {
  const _DualRangeBar({
    required this.minValue,
    required this.maxValue,
    required this.hardMin,
    required this.hardMax,
    required this.minThumbMax,
    required this.maxThumbMin,
    required this.tint,
    required this.onMinChanged,
    required this.onMaxChanged,
    this.minThumbColor,
    this.maxThumbColor,
    this.gradient,
    this.divisions,
  });

  /// Smallest allowed gap (in value units) between the two thumbs.
  static const double minSeparation = 1.0;

  final double minValue;
  final double maxValue;
  final double hardMin;
  final double hardMax;

  /// The min thumb cannot move above this value.
  final double minThumbMax;

  /// The max thumb cannot move below this value.
  final double maxThumbMin;

  /// Tint used for the active fill (when no gradient is supplied) and as the
  /// default thumb color.
  final Color tint;

  /// Optional per-thumb tint override; falls back to [tint].
  final Color? minThumbColor;
  final Color? maxThumbColor;

  /// Optional gradient painted across the full track. When null the track is
  /// rendered as a single-tint alpha ramp (low → high).
  final Gradient? gradient;

  /// Snap to this many equally-spaced divisions across [hardMin, hardMax].
  final int? divisions;

  final ValueChanged<double> onMinChanged;
  final ValueChanged<double> onMaxChanged;

  @override
  State<_DualRangeBar> createState() => _DualRangeBarState();
}

class _DualRangeBarState extends State<_DualRangeBar> {
  static const double _trackHeight = 10;
  static const double _thumbDiameter = 22;
  static const double _verticalSlack = 12;

  _DualRangeThumb? _active;

  /// Pixel offset between the active thumb's position and the finger at
  /// pan-down. Preserved for the life of the drag so the thumb follows the
  /// finger's *motion* rather than snapping to its absolute position — a tap
  /// near a thumb shouldn't make it leap onto the fingertip before any drag.
  double _grabOffsetPx = 0;

  double _normalize(double v) {
    final span = widget.hardMax - widget.hardMin;
    if (span <= 0) return 0;
    return ((v - widget.hardMin) / span).clamp(0.0, 1.0);
  }

  double _denormalize(double frac) =>
      widget.hardMin + frac.clamp(0.0, 1.0) * (widget.hardMax - widget.hardMin);

  double _snap(double v) {
    final divisions = widget.divisions;
    if (divisions == null || divisions <= 0) return v;
    final step = (widget.hardMax - widget.hardMin) / divisions;
    return ((v - widget.hardMin) / step).round() * step + widget.hardMin;
  }

  void _dispatch(double localX, double usableWidth) {
    if (usableWidth <= 0) return;
    final frac = (localX / usableWidth).clamp(0.0, 1.0);
    final snapped = _snap(_denormalize(frac));

    if (_active == _DualRangeThumb.min) {
      final cap = math.min(
          widget.minThumbMax, widget.maxValue - _DualRangeBar.minSeparation);
      final clamped = snapped
          .clamp(widget.hardMin, math.max(widget.hardMin, cap))
          .toDouble();
      if (clamped != widget.minValue) widget.onMinChanged(clamped);
    } else if (_active == _DualRangeThumb.max) {
      final floor = math.max(
          widget.maxThumbMin, widget.minValue + _DualRangeBar.minSeparation);
      final clamped = snapped
          .clamp(math.min(widget.hardMax, floor), widget.hardMax)
          .toDouble();
      if (clamped != widget.maxValue) widget.onMaxChanged(clamped);
    }
  }

  _DualRangeThumb _pickThumb(double localX, double usableWidth) {
    final minX = _normalize(widget.minValue) * usableWidth;
    final maxX = _normalize(widget.maxValue) * usableWidth;
    final dMin = (localX - minX).abs();
    final dMax = (localX - maxX).abs();
    // Tie-break: if values are equal, the side of the tap decides.
    if (dMin == dMax) {
      return localX < minX ? _DualRangeThumb.min : _DualRangeThumb.max;
    }
    return dMin <= dMax ? _DualRangeThumb.min : _DualRangeThumb.max;
  }

  @override
  Widget build(BuildContext context) {
    final height = _thumbDiameter + _verticalSlack * 2;
    return SizedBox(
      height: height,
      child: LayoutBuilder(
        builder: (context, constraints) {
          final width = constraints.maxWidth;
          final inset = _thumbDiameter / 2;
          final usableWidth = math.max(0.0, width - _thumbDiameter);

          return GestureDetector(
            behavior: HitTestBehavior.opaque,
            onPanDown: (d) {
              final x = d.localPosition.dx - inset;
              final thumb = _pickThumb(x, usableWidth);
              final thumbX = (thumb == _DualRangeThumb.min
                      ? _normalize(widget.minValue)
                      : _normalize(widget.maxValue)) *
                  usableWidth;
              setState(() => _active = thumb);
              _grabOffsetPx = thumbX - x;
              HapticFeedback.selectionClick();
              // Intentionally no dispatch here — wait for actual motion so a
              // tap near a thumb doesn't yank it to the touch point.
            },
            onPanUpdate: (d) {
              _dispatch(
                  d.localPosition.dx - inset + _grabOffsetPx, usableWidth);
            },
            onPanEnd: (_) {
              if (_active != null) HapticFeedback.selectionClick();
              setState(() => _active = null);
            },
            onPanCancel: () => setState(() => _active = null),
            child: CustomPaint(
              size: Size(width, height),
              painter: _DualRangePainter(
                minFrac: _normalize(widget.minValue),
                maxFrac: _normalize(widget.maxValue),
                trackHeight: _trackHeight,
                thumbDiameter: _thumbDiameter,
                inset: inset,
                tint: widget.tint,
                minThumbColor: widget.minThumbColor ?? widget.tint,
                maxThumbColor: widget.maxThumbColor ?? widget.tint,
                gradient: widget.gradient,
                draggingMin: _active == _DualRangeThumb.min,
                draggingMax: _active == _DualRangeThumb.max,
              ),
            ),
          );
        },
      ),
    );
  }
}

class _DualRangePainter extends CustomPainter {
  _DualRangePainter({
    required this.minFrac,
    required this.maxFrac,
    required this.trackHeight,
    required this.thumbDiameter,
    required this.inset,
    required this.tint,
    required this.minThumbColor,
    required this.maxThumbColor,
    required this.gradient,
    required this.draggingMin,
    required this.draggingMax,
  });

  final double minFrac;
  final double maxFrac;
  final double trackHeight;
  final double thumbDiameter;
  final double inset;
  final Color tint;
  final Color minThumbColor;
  final Color maxThumbColor;
  final Gradient? gradient;
  final bool draggingMin;
  final bool draggingMax;

  @override
  void paint(Canvas canvas, Size size) {
    final cy = size.height / 2;
    final usableWidth = size.width - thumbDiameter;
    final minX = inset + minFrac * usableWidth;
    final maxX = inset + maxFrac * usableWidth;
    final leftEdge = inset;
    final rightEdge = size.width - inset;
    final radius = Radius.circular(trackHeight / 2);

    final trackRect = Rect.fromLTRB(
      leftEdge,
      cy - trackHeight / 2,
      rightEdge,
      cy + trackHeight / 2,
    );

    // 1. Full-width spectrum / luminance ramp. When no explicit gradient is
    // supplied (brightness use case) we derive a value ramp from `tint`: a
    // near-black tinted dark on the dim end through to a near-white warm glow
    // on the bright end. The hue stays tied to the brand color while the
    // *luminance* actually communicates "more light" across the track.
    final basePaint = Paint();
    if (gradient != null) {
      basePaint.shader = gradient!.createShader(trackRect);
    } else {
      final hsl = HSLColor.fromColor(tint);
      final dark = hsl
          .withLightness(0.07)
          .withSaturation((hsl.saturation - 0.15).clamp(0.0, 1.0))
          .toColor();
      final bright = hsl
          .withLightness(0.84)
          .withSaturation((hsl.saturation - 0.30).clamp(0.0, 1.0))
          .toColor();
      basePaint.shader = LinearGradient(
        colors: [dark, bright],
      ).createShader(trackRect);
    }
    canvas.drawRRect(RRect.fromRectAndRadius(trackRect, radius), basePaint);

    // 2. Scrim over the inactive regions. The scrim deepens the surrounding
    // spectrum so the active window reads as a luminous slice cut out of it.
    final scrim = Paint()..color = _Palette.bg.withValues(alpha: 0.74);
    if (minX > leftEdge + 0.5) {
      final r = Rect.fromLTRB(leftEdge, trackRect.top, minX, trackRect.bottom);
      canvas.drawRRect(
        RRect.fromRectAndCorners(r, topLeft: radius, bottomLeft: radius),
        scrim,
      );
    }
    if (maxX < rightEdge - 0.5) {
      final r = Rect.fromLTRB(maxX, trackRect.top, rightEdge, trackRect.bottom);
      canvas.drawRRect(
        RRect.fromRectAndCorners(r, topRight: radius, bottomRight: radius),
        scrim,
      );
    }

    // 3. Hairline highlight along the top of the active slice — gives it the
    // sense of a polished, recessed light.
    if (maxX > minX) {
      canvas.drawRect(
        Rect.fromLTRB(minX, trackRect.top, maxX, trackRect.top + 1.2),
        Paint()..color = Colors.white.withValues(alpha: 0.16),
      );
    }

    // 4. Thumbs.
    _drawThumb(canvas, Offset(minX, cy), minThumbColor, draggingMin);
    _drawThumb(canvas, Offset(maxX, cy), maxThumbColor, draggingMax);
  }

  void _drawThumb(Canvas canvas, Offset center, Color color, bool active) {
    // Match the simple filled-circle look of the rest of the screen's
    // sliders: a flat 7px-radius dot, with a soft overlay halo when active.
    const visibleRadius = 7.0;
    if (active) {
      canvas.drawCircle(
        center,
        16,
        Paint()..color = color.withValues(alpha: 0.10),
      );
    }
    canvas.drawCircle(center, visibleRadius, Paint()..color = color);
  }

  @override
  bool shouldRepaint(_DualRangePainter old) =>
      old.minFrac != minFrac ||
      old.maxFrac != maxFrac ||
      old.draggingMin != draggingMin ||
      old.draggingMax != draggingMax ||
      old.tint != tint ||
      old.minThumbColor != minThumbColor ||
      old.maxThumbColor != maxThumbColor;
}
