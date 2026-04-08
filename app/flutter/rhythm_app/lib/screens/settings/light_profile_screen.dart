import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;
import '../../models/config_model.dart';
import '../../providers/room_provider.dart';
import '../../providers/server_sync_provider.dart';

/// Full-screen modal for configuring the light profile.
///
/// Features a compressed color spectrum slider that emphasizes the dawn/dusk
/// ramps where color changes rapidly, compressing flat night/day regions.
class LightProfileScreen extends StatefulWidget {
  final String? initialProfile;

  const LightProfileScreen({super.key, this.initialProfile});

  static Future<void> show(BuildContext context, {String? initialProfile}) {
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

class _LightProfileScreenState extends State<LightProfileScreen>
    with SingleTickerProviderStateMixin {
  static const List<String> _profileOrder = ['rhythm', 'sleep', 'idle'];

  late String _selectedProfileId;
  final Map<String, sdk.RhythmCurveConfig> _profileConfigs = {};

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

  // Sleep profile color state (directColor on super-gaussian).
  bool _sleepHasColor = false;
  double _sleepHue = 10;

  // Idle profile state (folded into day/sleep profiles).
  bool _idleCustomBri = false;
  bool _idleCustomColor = false;
  double _idleBrightness = 1;
  int _idleColorTemp = 2200;
  double _idleHue = 30; // hue angle for spectrum picker

  bool _loading = true;
  bool _connected = false;

  // Time simulator state.
  CurveData? _curveData;
  double _timeOffsetMinutes = 0;
  double _sliderFraction = 0.5; // raw 0..1 position on the track
  bool _isDraggingTime = false;
  bool _timeOffsetApplied = false; // true after user taps Apply

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
    _selectedProfileId = widget.initialProfile == 'idle'
        ? 'rhythm'
        : (widget.initialProfile ?? 'rhythm');

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
  void dispose() {
    _glowController.dispose();
    super.dispose();
  }

  bool get _isSleepProfile => _selectedProfileId == 'sleep';

  String get _profileTitle => switch (_selectedProfileId) {
        'sleep' => 'Sleep Profile',
        _ => 'Day Profile',
      };

  sdk.RhythmCurveConfig? get _selectedProfileConfig =>
      _profileConfigs[_selectedProfileId];

  sdk.RhythmSuperGaussianCurve? get _selectedSuperGaussianCurve {
    final curve = _selectedProfileConfig?.curve;
    return curve is sdk.RhythmSuperGaussianCurve ? curve : null;
  }

  Color? get _selectedFixedColor {
    final directColor = _selectedSuperGaussianCurve?.directColor;
    if (directColor == null) return null;
    return Color.fromARGB(
      255,
      directColor.rgb.r,
      directColor.rgb.g,
      directColor.rgb.b,
    );
  }

  Future<void> _loadConfig({bool checkConnection = true}) async {
    if (checkConnection) {
      final syncProvider = context.read<ServerSyncProvider>();
      _connected = syncProvider.synced;

      if (!_connected) {
        setState(() => _loading = false);
        return;
      }
    }

    final api = context.read<ServerSyncProvider>().api;
    final settings = await api.getSettings();
    if (!mounted) return;

    final settingsProfiles = [...?settings?.profiles];
    settingsProfiles.sort((a, b) {
      final ia = _profileOrder.indexOf(a.id);
      final ib = _profileOrder.indexOf(b.id);
      final orderA = ia == -1 ? _profileOrder.length : ia;
      final orderB = ib == -1 ? _profileOrder.length : ib;
      return orderA.compareTo(orderB);
    });
    final profileConfigs = <String, sdk.RhythmCurveConfig>{
      for (final profile in settingsProfiles)
        if (profile.id.isNotEmpty) profile.id: profile,
    };
    final availableProfiles =
        settingsProfiles.map(sdk.CurveModule.fromProfile).toList();

    final initialProfileId = profileConfigs.containsKey(_selectedProfileId)
        ? _selectedProfileId
        : settings?.activeLightProfile ??
            availableProfiles.firstOrNull?.id ??
            'rhythm';
    final selectedConfig = profileConfigs[initialProfileId] ??
        await api.getConfig(id: initialProfileId);
    if (selectedConfig == null) {
      setState(() {

        _selectedProfileId = initialProfileId;
        _loading = false;
      });
      return;
    }

    profileConfigs[selectedConfig.id] = selectedConfig;
    if (!availableProfiles.any((profile) => profile.id == selectedConfig.id)) {
      availableProfiles.add(sdk.CurveModule.fromProfile(selectedConfig));
      availableProfiles.sort((a, b) {
        final ia = _profileOrder.indexOf(a.id);
        final ib = _profileOrder.indexOf(b.id);
        final orderA = ia == -1 ? _profileOrder.length : ia;
        final orderB = ib == -1 ? _profileOrder.length : ib;
        return orderA.compareTo(orderB);
      });
    }
    _profileConfigs
      ..clear()
      ..addAll(profileConfigs);
    _applyProfileConfig(selectedConfig);
    final idleConfig = profileConfigs['idle'];
    if (idleConfig != null) _applyIdleConfig(idleConfig);
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
      if (mounted) {
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

      // Sleep profile direct color.
      if (curve.directColor != null) {
        _sleepHasColor = true;
        final rgb = curve.directColor!.rgb;
        _sleepHue = HSVColor.fromColor(
          Color.fromARGB(255, rgb.r, rgb.g, rgb.b),
        ).hue;
      } else {
        _sleepHasColor = false;
      }
    }

    _curveConfigDirty = false;
  }

  void _applyIdleConfig(sdk.RhythmCurveConfig config) {
    final curve = config.curve;
    if (curve is sdk.RhythmInheritActiveCurve) {
      _idleCustomBri = false;
      _idleCustomColor = false;
      _idleBrightness = config.maxBrightness.toDouble().clamp(1, 100);
    } else if (curve is sdk.RhythmConstantCurve) {
      _idleBrightness = config.maxBrightness.toDouble().clamp(1, 100);
      _idleCustomBri = true;
      _idleCustomColor = curve.directColor != null;
      _idleColorTemp = curve.directColor != null
          ? curve.colorTemp
          : (curve.colorTemp > 0 ? curve.colorTemp : 2200);
      if (curve.directColor != null) {
        final rgb = curve.directColor!.rgb;
        final c = Color.fromARGB(255, rgb.r, rgb.g, rgb.b);
        _idleHue = HSVColor.fromColor(c).hue;
        final matchingPreset = _idleColorPresets
            .where((p) =>
                (p.color.r * 255 - rgb.r).abs() < 10 &&
                (p.color.g * 255 - rgb.g).abs() < 10 &&
                (p.color.b * 255 - rgb.b).abs() < 10)
            .firstOrNull;
        if (matchingPreset != null) {
          _idleColorTemp = matchingPreset.colorTemp;
        }
      } else if (_idleColorTemp > 0) {
        final matchingPreset = _idleColorPresets
            .where((p) => p.colorTemp == _idleColorTemp)
            .firstOrNull;
        if (matchingPreset != null) {
          _idleHue = HSVColor.fromColor(matchingPreset.color).hue;
        }
      }
    }
  }

  sdk.RhythmCurveConfig _buildDraftConfig() {
    final base = _selectedProfileConfig;
    if (base == null) {
      return const sdk.RhythmCurveConfig();
    }

    final curve = base.curve;
    sdk.RhythmCurveShape nextCurve;
    if (curve is sdk.RhythmSuperGaussianCurve) {
      final sleepColor = (_isSleepProfile && _sleepHasColor)
          ? _sleepDirectColor
          : null;
      nextCurve = curve.copyWith(
        widthLeftBri: _widthLeftBri,
        widthRightBri: _widthRightBri,
        widthLeftCct: _widthLeftCct,
        widthRightCct: _widthRightCct,
        shapeP: _shapeP,
        directColor: sleepColor,
      );
      // Clear directColor when sleep has no fixed color.
      if (_isSleepProfile && !_sleepHasColor) {
        nextCurve = sdk.RhythmSuperGaussianCurve(
          widthLeftBri: _widthLeftBri,
          widthRightBri: _widthRightBri,
          widthLeftCct: _widthLeftCct,
          widthRightCct: _widthRightCct,
          shapeP: _shapeP,
        );
      }
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
    final base = _profileConfigs['idle'];
    if (base == null) return null;

    if (!_idleCustomBri && !_idleCustomColor) {
      return base.copyWith(
        curve: const sdk.RhythmInheritActiveCurve(),
      );
    }

    final bri =
        _idleCustomBri ? _idleBrightness.round() : _minBrightness.round();
    sdk.RhythmDirectColor? directColor;
    int colorTemp = 0;

    if (_idleCustomColor) {
      final color = _idleSelectedColor;
      directColor = _rgbToDirectColor(
        (color.r * 255).round(),
        (color.g * 255).round(),
        (color.b * 255).round(),
      );
      colorTemp = _idleColorTemp;
    }

    return base.copyWith(
      minBrightness: bri,
      maxBrightness: bri,
      curve: sdk.RhythmConstantCurve(
        brightness: bri,
        colorTemp: colorTemp,
        directColor: directColor,
      ),
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
    final activeProfileId =
        context.read<ServerSyncProvider>().activeCurveModule;
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
      fadeMs: config.fadeMs ?? CurveConfigDto.default_().fadeMs,
      motionTimeoutSecs: config.motionTimeoutSecs ??
          CurveConfigDto.default_().motionTimeoutSecs,
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

  void _applyTimeOffset() {
    _sendTimeOffset();
    setState(() => _timeOffsetApplied = true);
  }

  void _sendTimeOffset() {
    final api = context.read<ServerSyncProvider>().api;
    final rooms = context.read<RoomProvider>().rooms;
    if (rooms.isEmpty) return;
    api.roomOffsetBatch([
      for (final room in rooms)
        (roomId: room.id, timeOffset: _timeOffsetMinutes),
    ]);
  }

  void _resetTimeOffset() {
    setState(() {
      _timeOffsetMinutes = 0;
      _sliderFraction = _hourToNowFraction();
      _timeOffsetApplied = false;
    });
    _sendTimeOffset();
  }

  Future<void> _absorbTimeOffset() async {
    final sdkConfig = await context
        .read<ServerSyncProvider>()
        .api
        .absorbTimeOffset(_timeOffsetMinutes, id: _selectedProfileId);
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
      _timeOffsetApplied = false;
      _curveConfigDirty = false;
    });
  }

  Future<void> _resetToDefaults() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: _Palette.card,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(18),
          side: const BorderSide(color: _Palette.border),
        ),
        title: const Text(
          'Reset to Defaults?',
          style: TextStyle(color: _Palette.textPrimary, fontSize: 17),
        ),
        content: const Text(
          'This will reset the light curve to factory defaults on the server.',
          style: TextStyle(color: _Palette.textSecondary, fontSize: 14),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Cancel',
                style: TextStyle(color: _Palette.textSecondary)),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('Reset', style: TextStyle(color: _Palette.amber)),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

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
          ],
          _buildBrightnessRangeCard(),
          if (!_isSleepProfile) ...[
            const SizedBox(height: 14),
            _buildColorTempRangeCard(),
            const SizedBox(height: 24),
            _buildTimeSimulator(),
          ],
          const SizedBox(height: 24),
          _buildMotionTimeoutCard(),
          if (!_isSleepProfile) ...[
            const SizedBox(height: 14),
            _buildIntervalCard(),
          ],
          const SizedBox(height: 24),
          _buildIdleSection(),
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
    final activeColor = _sleepHasColor ? _sleepSelectedColor : _Palette.amber;

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
                child: Icon(Icons.palette_rounded,
                    color: activeColor, size: 18),
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
              SizedBox(
                height: 28,
                child: Switch.adaptive(
                  value: _sleepHasColor,
                  onChanged: (v) {
                    setState(() {
                      _sleepHasColor = v;
                      _curveConfigDirty = true;
                    });
                  },
                  activeTrackColor: activeColor,
                  activeThumbColor: _Palette.textPrimary,
                ),
              ),
            ],
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: !_sleepHasColor
                ? Padding(
                    padding: const EdgeInsets.only(left: 48, top: 8),
                    child: Text(
                      'Uses sun-based color temperature curve',
                      style: TextStyle(
                        color: _Palette.textSecondary.withValues(alpha: 0.45),
                        fontSize: 12,
                        height: 1.3,
                      ),
                    ),
                  )
                : Padding(
                    padding: const EdgeInsets.only(top: 18),
                    child: Column(
                      children: [
                        // Warm spectrum bar.
                        GestureDetector(
                          onTapDown: (d) =>
                              _onSleepSpectrumTap(d.localPosition.dx, context),
                          onHorizontalDragUpdate: (d) =>
                              _onSleepSpectrumTap(d.localPosition.dx, context),
                          child: LayoutBuilder(
                            builder: (context, constraints) {
                              final width = constraints.maxWidth;
                              final thumbX = (_sleepHue / 360) * width;
                              return SizedBox(
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
                                          color:
                                              Colors.white.withValues(alpha: 0.06),
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
                                            color: Colors.white
                                                .withValues(alpha: 0.9),
                                            width: 2.5,
                                          ),
                                          boxShadow: [
                                            BoxShadow(
                                              color: activeColor
                                                  .withValues(alpha: 0.5),
                                              blurRadius: 12,
                                              spreadRadius: 1,
                                            ),
                                            BoxShadow(
                                              color: Colors.black
                                                  .withValues(alpha: 0.4),
                                              blurRadius: 4,
                                            ),
                                          ],
                                        ),
                                      ),
                                    ),
                                  ],
                                ),
                              );
                            },
                          ),
                        ),
                        const SizedBox(height: 18),
                        // Preset swatches.
                        Row(
                          mainAxisAlignment: MainAxisAlignment.spaceBetween,
                          children: _sleepColorPresets.map((preset) {
                            final selected = _isSleepPresetSelected(preset.color);
                            return GestureDetector(
                              onTap: () {
                                setState(() {
                                  _sleepHue = preset.hue;
                                  _curveConfigDirty = true;
                                });
                              },
                              child: Column(
                                children: [
                                  Container(
                                    width: 40,
                                    height: 40,
                                    decoration: BoxDecoration(
                                      shape: BoxShape.circle,
                                      color: preset.color.withValues(
                                          alpha: selected ? 0.9 : 0.4),
                                      border: Border.all(
                                        color: selected
                                            ? Colors.white
                                                .withValues(alpha: 0.7)
                                            : Colors.white
                                                .withValues(alpha: 0.06),
                                        width: selected ? 2.5 : 1,
                                      ),
                                      boxShadow: selected
                                          ? [
                                              BoxShadow(
                                                color: preset.color
                                                    .withValues(alpha: 0.4),
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
                                          : _Palette.textSecondary
                                              .withValues(alpha: 0.5),
                                      fontSize: 10,
                                      fontWeight: selected
                                          ? FontWeight.w600
                                          : FontWeight.w500,
                                    ),
                                  ),
                                ],
                              ),
                            );
                          }).toList(),
                        ),
                      ],
                    ),
                  ),
          ),
        ],
      ),
    );
  }

  void _onSleepSpectrumTap(double dx, BuildContext context) {
    final box = context.findRenderObject() as RenderBox?;
    if (box == null) return;
    final width = box.size.width - 36;
    final fraction = (dx / width).clamp(0.0, 1.0);
    setState(() {
      _sleepHue = fraction * 360;
      _curveConfigDirty = true;
    });
  }

  // ---------------------------------------------------------------------------
  // Idle Section — folded into day/sleep profiles
  // ---------------------------------------------------------------------------

  Widget _buildIdleSection() {
    final minBri = _minBrightness.round();
    final isDefault = !_idleCustomBri && !_idleCustomColor;
    final briColor = _idleCustomBri ? _Palette.amber : _Palette.idle;
    final colorColor = _idleCustomColor ? _idleSelectedColor : _Palette.idle;

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
          // Header.
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
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    const Text(
                      'When Idle',
                      style: TextStyle(
                        color: _Palette.textPrimary,
                        fontSize: 15,
                        fontWeight: FontWeight.w600,
                        letterSpacing: -0.1,
                      ),
                    ),
                    if (isDefault) ...[
                      const SizedBox(height: 2),
                      Text(
                        'Dims to curve minimum ($minBri%)',
                        style: TextStyle(
                          color:
                              _Palette.textSecondary.withValues(alpha: 0.5),
                          fontSize: 12,
                        ),
                      ),
                    ],
                  ],
                ),
              ),
            ],
          ),
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
                        : _Palette.textSecondary.withValues(alpha: 0.5),
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
                      fontFeatures: const [FontFeature.tabularFigures()],
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
                        _idleBrightness = minBri.toDouble();
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
                    padding: const EdgeInsets.only(top: 8, left: 26),
                    child: SliderTheme(
                      data: SliderThemeData(
                        activeTrackColor: briColor,
                        inactiveTrackColor:
                            briColor.withValues(alpha: 0.12),
                        thumbColor: briColor,
                        overlayColor: briColor.withValues(alpha: 0.12),
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
                        : _Palette.textSecondary.withValues(alpha: 0.5),
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
      ),
    );
  }

  // Warm color presets for sleep profile.
  static const _sleepColorPresets = <({Color color, double hue, String label})>[
    (color: Color(0xFFFF3B30), hue: 4,   label: 'Red'),
    (color: Color(0xFFFF6B4A), hue: 14,  label: 'Ember'),
    (color: Color(0xFFFF9500), hue: 35,  label: 'Amber'),
    (color: Color(0xFFFFB347), hue: 33,  label: 'Peach'),
    (color: Color(0xFFFF6E8A), hue: 345, label: 'Rose'),
    (color: Color(0xFFE8A87C), hue: 24,  label: 'Sand'),
  ];

  sdk.RhythmDirectColor? get _sleepDirectColor {
    if (!_sleepHasColor) return null;
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
    if (!_sleepHasColor) return false;
    final selected = _sleepSelectedColor;
    return (selected.r - presetColor.r).abs() < 0.04 &&
        (selected.g - presetColor.g).abs() < 0.04 &&
        (selected.b - presetColor.b).abs() < 0.04;
  }

  // Full-spectrum color presets for idle custom-color mode.
  // Explicit hue ensures the spectrum thumb lands where expected.
  static const _idleColorPresets = <({Color color, double hue, String label, int colorTemp})>[
    (color: Color(0xFFFF4D6A), hue: 0,   label: 'Rose',    colorTemp: 1800),
    (color: Color(0xFFFF8A2D), hue: 28,  label: 'Ember',   colorTemp: 2200),
    (color: Color(0xFFFFD23F), hue: 47,  label: 'Gold',    colorTemp: 2700),
    (color: Color(0xFF3DDC84), hue: 145, label: 'Mint',    colorTemp: 4000),
    (color: Color(0xFF4FC3F7), hue: 199, label: 'Sky',     colorTemp: 5500),
    (color: Color(0xFFB388FF), hue: 262, label: 'Violet',  colorTemp: 3500),
  ];

  Widget _buildIdleColorPicker() {
    final activeColor = _idleSelectedColor;

    return Column(
      children: [
        // Full-spectrum hue bar.
        GestureDetector(
          onTapDown: (d) => _onSpectrumTap(d.localPosition.dx, context),
          onHorizontalDragUpdate: (d) =>
              _onSpectrumTap(d.localPosition.dx, context),
          child: LayoutBuilder(
            builder: (context, constraints) {
              final width = constraints.maxWidth;
              final thumbX = (_idleHue / 360) * width;

              return SizedBox(
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
                              color: activeColor.withValues(alpha: 0.5),
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
              );
            },
          ),
        ),
        const SizedBox(height: 18),
        // Preset swatches.
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: _idleColorPresets.map((preset) {
            final selected = _isPresetSelected(preset.color);
            return GestureDetector(
              onTap: () => _onIdleColorChanged(
                  preset.color, preset.hue, preset.colorTemp),
              child: AnimatedContainer(
                duration: const Duration(milliseconds: 200),
                curve: Curves.easeOutCubic,
                child: Column(
                  children: [
                    Container(
                      width: 40,
                      height: 40,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: preset.color.withValues(
                            alpha: selected ? 0.9 : 0.4),
                        border: Border.all(
                          color: selected
                              ? Colors.white.withValues(alpha: 0.7)
                              : Colors.white.withValues(alpha: 0.06),
                          width: selected ? 2.5 : 1,
                        ),
                        boxShadow: selected
                            ? [
                                BoxShadow(
                                  color: preset.color
                                      .withValues(alpha: 0.4),
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
                            : _Palette.textSecondary
                                .withValues(alpha: 0.5),
                        fontSize: 10,
                        fontWeight:
                            selected ? FontWeight.w600 : FontWeight.w500,
                      ),
                    ),
                  ],
                ),
              ),
            );
          }).toList(),
        ),
      ],
    );
  }

  /// The currently selected idle color — either a preset or from the hue bar.
  Color get _idleSelectedColor {
    // Check if current colorTemp matches a preset.
    if (_idleColorTemp > 0) {
      for (final p in _idleColorPresets) {
        if (_idleColorTemp == p.colorTemp) return p.color;
      }
    }
    // Custom hue from the spectrum bar.
    return HSVColor.fromAHSV(1, _idleHue, 0.85, 1).toColor();
  }

  bool _isPresetSelected(Color presetColor) {
    final selected = _idleSelectedColor;
    return (selected.r - presetColor.r).abs() < 0.04 &&
        (selected.g - presetColor.g).abs() < 0.04 &&
        (selected.b - presetColor.b).abs() < 0.04;
  }

  void _onIdleColorChanged(Color color, double hue, int colorTemp) {
    setState(() {
      _idleColorTemp = colorTemp;
      _idleHue = hue;
      _curveConfigDirty = true;
    });
  }

  void _onSpectrumTap(double dx, BuildContext context) {
    final box = context.findRenderObject() as RenderBox?;
    if (box == null) return;
    final width = box.size.width - 36; // account for card padding
    final fraction = (dx / width).clamp(0.0, 1.0);
    final hue = fraction * 360;
    setState(() {
      _idleHue = hue;
      _idleColorTemp = 0; // no CCT — direct_color carries the RGB/XY
      _curveConfigDirty = true;
    });
  }

  /// Convert sRGB (0-255) to CIE 1931 XY + build [RhythmDirectColor].
  static sdk.RhythmDirectColor _rgbToDirectColor(int r, int g, int b) {
    // 1. Linearise sRGB.
    double linearise(int c) {
      final s = c / 255.0;
      return s <= 0.04045 ? s / 12.92 : math.pow((s + 0.055) / 1.055, 2.4).toDouble();
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
                    color:
                        _Palette.textSecondary.withValues(alpha: 0.3),
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
          const SizedBox(height: 6),
          _buildInlineSlider(
            label: 'Min',
            value: _minBrightness,
            min: 1,
            max: 50,
            divisions: 49,
            format: (v) => '${v.round()}%',
            color: color.withValues(alpha: 0.5),
            onChanged: (v) => _onCurveChanged(() => _minBrightness = v),
          ),
          _buildInlineSlider(
            label: 'Max',
            value: _maxBrightness,
            min: 20,
            max: 100,
            divisions: 80,
            format: (v) => '${v.round()}%',
            color: color,
            onChanged: (v) => _onCurveChanged(() => _maxBrightness = v),
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
                  gradient: LinearGradient(
                    colors: [
                      warmColor.withValues(alpha: 0.18),
                      coolColor.withValues(alpha: 0.18),
                    ],
                  ),
                ),
                child: Icon(Icons.thermostat_rounded, color: warmColor, size: 18),
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
                    color:
                        _Palette.textSecondary.withValues(alpha: 0.3),
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
          const SizedBox(height: 14),
          // Kelvin gradient strip showing the selected range
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
          const SizedBox(height: 4),
          _buildInlineSlider(
            label: 'Min',
            value: _minColorTemp,
            min: 1500,
            max: 4000,
            divisions: 25,
            format: (v) => '${v.round()}K',
            color: warmColor,
            onChanged: (v) => _onCurveChanged(() => _minColorTemp = v),
          ),
          _buildInlineSlider(
            label: 'Max',
            value: _maxColorTemp,
            min: 2000,
            max: 6500,
            divisions: 45,
            format: (v) => '${v.round()}K',
            color: coolColor,
            onChanged: (v) => _onCurveChanged(() => _maxColorTemp = v),
          ),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Background Light Interval card
  // ---------------------------------------------------------------------------

  Widget _buildIntervalCard() {
    const color = _Palette.blue;
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 12),
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
                child: const Icon(Icons.update_rounded, color: color, size: 18),
              ),
              const SizedBox(width: 12),
              const Expanded(
                child: Text(
                  'Background Light Interval',
                  style: TextStyle(
                    color: _Palette.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                    letterSpacing: -0.1,
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
                  padding:
                      const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                  decoration: BoxDecoration(
                    color: _intervalAuto
                        ? color.withValues(alpha: 0.12)
                        : color.withValues(alpha: 0.06),
                    borderRadius: BorderRadius.circular(8),
                    border: Border.all(
                      color: _intervalAuto
                          ? color.withValues(alpha: 0.25)
                          : color.withValues(alpha: 0.12),
                    ),
                  ),
                  child: Text(
                    _intervalAuto ? 'Auto' : _formatInterval(_intervalSecs),
                    style: TextStyle(
                      color: _intervalAuto
                          ? color
                          : color.withValues(alpha: 0.7),
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      letterSpacing: 0.3,
                    ),
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Padding(
            padding: const EdgeInsets.only(left: 48),
            child: Text(
              _intervalAuto
                  ? 'Server will compute from curve rate-of-change'
                  : 'How often this profile updates the runtime loop',
              style: TextStyle(
                color: _Palette.textSecondary.withValues(alpha: 0.45),
                fontSize: 12,
                height: 1.3,
              ),
            ),
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 250),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: _intervalAuto
                ? const SizedBox.shrink()
                : Padding(
          padding: const EdgeInsets.only(top: 10),
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
                          color: _Palette.textSecondary.withValues(alpha: 0.4),
                          fontSize: 11,
                        ),
                      ),
                      Text(
                        _formatInterval(300),
                        style: TextStyle(
                          color: _Palette.textSecondary.withValues(alpha: 0.4),
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
      ),
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
                color: _motionTimeoutAuto
                    ? color
                    : color.withValues(alpha: 0.7),
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
          child: _hasTimeOffset
              ? Padding(
                  padding: const EdgeInsets.only(top: 16),
                  child: _timeOffsetApplied
                      ? Row(
                          mainAxisAlignment: MainAxisAlignment.center,
                          children: [
                            _buildResetTimeButton(),
                            if (!_isSleepProfile) ...[
                              const SizedBox(width: 12),
                              _buildAbsorbTimeButton(),
                            ],
                          ],
                        )
                      : Row(
                          mainAxisAlignment: MainAxisAlignment.center,
                          children: [
                            _buildClearTimeButton(),
                            const SizedBox(width: 12),
                            _buildApplyTimeButton(),
                          ],
                        ),
                )
              : const SizedBox.shrink(),
        ),
      ],
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
      onTap: () {
        setState(() {
          _timeOffsetMinutes = 0;
          _sliderFraction = _hourToNowFraction();
          _timeOffsetApplied = false;
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
    return GestureDetector(
      onTap: _applyTimeOffset,
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
            Icon(
              Icons.play_arrow_rounded,
              color: _Palette.amber.withValues(alpha: 0.8),
              size: 16,
            ),
            const SizedBox(width: 6),
            Text(
              'Preview on Lights',
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
    return GestureDetector(
      onTap: _resetTimeOffset,
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
              Icons.refresh_rounded,
              color: _Palette.textSecondary.withValues(alpha: 0.5),
              size: 14,
            ),
            const SizedBox(width: 6),
            Text(
              'Reset',
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
    return GestureDetector(
      onTap: _absorbTimeOffset,
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
            Icon(
              Icons.check_rounded,
              color: const Color(0xFFD4A54A).withValues(alpha: 0.8),
              size: 14,
            ),
            const SizedBox(width: 6),
            Text(
              'Absorb',
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
                padding:
                    const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
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
                thumbShape:
                    const RoundSliderThumbShape(enabledThumbRadius: 7),
                overlayShape:
                    const RoundSliderOverlayShape(overlayRadius: 16),
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
  // Save
  // ---------------------------------------------------------------------------

  Widget _buildSaveButton() {
    return GestureDetector(
      onTap: _confirmSaveCurveConfig,
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

  Future<void> _confirmSaveCurveConfig() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: _Palette.card,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(18),
          side: const BorderSide(color: _Palette.border),
        ),
        title: const Text(
          'Are you sure??',
          style: TextStyle(color: _Palette.textPrimary, fontSize: 17),
        ),
        content: const Text(
          'This will replace the stored profile config on the server.',
          style: TextStyle(color: _Palette.textSecondary, fontSize: 14),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Cancel',
                style: TextStyle(color: _Palette.textSecondary)),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('Save', style: TextStyle(color: _Palette.amber)),
          ),
        ],
      ),
    );

    if (confirmed == true) {
      await _saveCurveConfig();
    }
  }

  Future<void> _saveCurveConfig() async {
    final config = _buildDraftConfig();
    final api = context.read<ServerSyncProvider>().api;
    await api.configSet(config, id: _selectedProfileId);

    // Also save idle config.
    final idleConfig = _buildIdleDraftConfig();
    if (idleConfig != null) {
      await api.configSet(idleConfig, id: 'idle');
    }

    if (mounted) {
      _profileConfigs[_selectedProfileId] = config;
      if (idleConfig != null) _profileConfigs['idle'] = idleConfig;
      _applyProfileConfig(config);
      await _syncActiveConfigModel(config);
      await _loadCurveData(profileId: _selectedProfileId);
      setState(() => _curveConfigDirty = false);
    }
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
// Palette
// -----------------------------------------------------------------------------

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
