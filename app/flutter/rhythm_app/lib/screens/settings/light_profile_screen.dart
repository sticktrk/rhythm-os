import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmCurveConfig;
import '../../api/hybrid_client.dart';
import '../../models/config_model.dart';
import '../../providers/room_provider.dart';
import '../../providers/server_sync_provider.dart';

/// Full-screen modal for configuring the light profile.
///
/// Features a compressed color spectrum slider that emphasizes the dawn/dusk
/// ramps where color changes rapidly, compressing flat night/day regions.
class LightProfileScreen extends StatefulWidget {
  const LightProfileScreen({super.key});

  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const LightProfileScreen();
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
  // Light transition duration (from server settings).
  double _fadeMs = 500;

  // Curve parameters.
  bool _advancedOpen = false;
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

  Future<void> _loadConfig({bool checkConnection = true}) async {
    if (checkConnection) {
      final syncProvider = context.read<ServerSyncProvider>();
      _connected = syncProvider.synced;

      if (!_connected) {
        setState(() => _loading = false);
        return;
      }
    }

    if (!mounted) return;
    final curveConfig = context.read<ConfigModel>().config;
    _minColorTemp = curveConfig.minColorTemp.toDouble();
    _maxColorTemp = curveConfig.maxColorTemp.toDouble();
    _minBrightness = curveConfig.minBrightness.toDouble();
    _maxBrightness = curveConfig.maxBrightness.toDouble();
    _widthLeftBri = curveConfig.widthLeftBri;
    _widthRightBri = curveConfig.widthRightBri;
    _widthLeftCct = curveConfig.widthLeftCct;
    _widthRightCct = curveConfig.widthRightCct;
    _shapeP = curveConfig.shapeP;
    _maxDimSteps = curveConfig.maxDimSteps.toDouble();
    // Use effective values from server (auto-computed) when available,
    // otherwise fall back to curve config values.
    final syncProvider = context.read<ServerSyncProvider>();
    _fadeMs = syncProvider.effectiveFadeMs?.toDouble()
        ?? curveConfig.fadeMs.toDouble();
    _motionTimeoutSecs = syncProvider.effectiveMotionTimeoutSecs
        ?? curveConfig.motionTimeoutSecs;

    if (mounted) setState(() => _loading = false);

    _loadCurveData(curveConfig);
  }

  Future<void> _loadCurveData(CurveConfigDto config) async {
    try {
      final api = context.read<RhythmApi>();
      CurveData? data;
      if (api is HybridApiClient) {
        data = api.getCurveDataHighRes(config: config, samplesPerHour: 4);
      }
      data ??= await api.getCurveData(overrides: config);
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
    return _interpolateCurve(
            _curveData!.hours, _curveData!.kelvin, hour)
        .toInt();
  }

  int _brightnessAtHour(double hour) {
    if (_curveData == null || _curveData!.hours.isEmpty) return 50;
    return _interpolateCurve(
            _curveData!.hours, _curveData!.brightness, hour)
        .toInt();
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
    final sdkConfig = await context.read<ServerSyncProvider>().api.absorbTimeOffset(_timeOffsetMinutes);
    if (!mounted) return;
    if (sdkConfig != null) {
      context.read<ConfigModel>().updateConfig(sdkCurveConfigToDto(sdkConfig));
    }
    setState(() {
      _timeOffsetMinutes = 0;
      _sliderFraction = _hourToNowFraction();
      _timeOffsetApplied = false;
    });
    // Skip connectivity check — we just talked to the server successfully.
    _loadConfig(checkConnection: false);
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
            child: const Text('Reset',
                style: TextStyle(color: _Palette.amber)),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    final api = context.read<ServerSyncProvider>().api;
    final results = await Future.wait([
      api.resetConfig(),
      api.settingsSet(bulbFadeMs: 500),
    ]);
    if (!mounted) return;
    final sdkConfig = results[0] as RhythmCurveConfig?;
    if (sdkConfig != null) {
      context.read<ConfigModel>().updateConfig(sdkCurveConfigToDto(sdkConfig));
    }
    setState(() {
      _fadeMs = 500;
      _timeOffsetMinutes = 0;
      _sliderFraction = _hourToNowFraction();
      _curveConfigDirty = false;
    });
    _loadConfig(checkConnection: false);
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
              child:
                  const Icon(Icons.close, color: _Palette.amber, size: 20),
            ),
          ),
          const Expanded(
            child: Text(
              'Light Profile',
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
          _buildAutoSettingCard(
            icon: Icons.blur_on_rounded,
            color: _Palette.amber,
            title: 'Light Transition',
            subtitle: 'How quickly lights fade between brightness levels',
            value: _formatFade(_fadeMs),
          ),
          const SizedBox(height: 14),
          _buildAutoSettingCard(
            icon: Icons.motion_photos_on_rounded,
            color: _Palette.teal,
            title: 'Motion Timeout',
            subtitle: 'How long lights stay on after motion stops',
            value: _formatMotionTimeout(_motionTimeoutSecs),
          ),
          const SizedBox(height: 24),
          _buildTimeSimulator(),
          const SizedBox(height: 32),
          _buildAdvancedToggle(),
          _buildAdvancedSection(),
          const SizedBox(height: 32),
          _buildResetToDefaultsButton(),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Auto-managed setting card (read-only)
  // ---------------------------------------------------------------------------

  Widget _buildAutoSettingCard({
    required IconData icon,
    required Color color,
    required String title,
    required String subtitle,
    required String value,
  }) {
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 16),
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
                child: Icon(icon, color: color, size: 18),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  title,
                  style: const TextStyle(
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
                  border: Border.all(
                    color: color.withValues(alpha: 0.25),
                  ),
                ),
                child: Text(
                  'Auto',
                  style: TextStyle(
                    color: color,
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.3,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                decoration: BoxDecoration(
                  color: color.withValues(alpha: 0.06),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(
                    color: color.withValues(alpha: 0.12),
                  ),
                ),
                child: Text(
                  value,
                  style: TextStyle(
                    color: color.withValues(alpha: 0.7),
                    fontSize: 14,
                    fontWeight: FontWeight.w700,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Padding(
            padding: const EdgeInsets.only(left: 48),
            child: Text(
              subtitle,
              style: TextStyle(
                color: _Palette.textSecondary.withValues(alpha: 0.45),
                fontSize: 12,
                height: 1.3,
              ),
            ),
          ),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Time Simulator
  // ---------------------------------------------------------------------------

  Widget _buildTimeSimulator() {
    final selectedHour = _selectedHour();
    final kelvin = _kelvinAtHour(selectedHour);
    final brightness = _brightnessAtHour(selectedHour);
    final cctColor = ColorUtils.curveColorForCCT(kelvin);

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
                    '${_formatHour(selectedHour)}  $brightness%  ${kelvin}K',
                    style: TextStyle(
                      color: cctColor.withValues(alpha: 0.7),
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
                            const SizedBox(width: 12),
                            _buildAbsorbTimeButton(),
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
  // Advanced curve tuning
  // ---------------------------------------------------------------------------

  Widget _buildAdvancedToggle() {
    return GestureDetector(
      onTap: () => setState(() => _advancedOpen = !_advancedOpen),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 4),
        child: Text(
          _advancedOpen ? 'Hide Advanced' : 'Advanced',
          style: TextStyle(
            color: _Palette.textSecondary.withValues(alpha: 0.25),
            fontSize: 12,
            fontWeight: FontWeight.w500,
          ),
        ),
      ),
    );
  }

  Widget _buildAdvancedSection() {
    return AnimatedSize(
      duration: const Duration(milliseconds: 400),
      curve: Curves.easeInOutCubic,
      alignment: Alignment.topCenter,
      child: _advancedOpen
          ? Column(
              children: [
                const SizedBox(height: 28),
                Row(
                  children: [
                    Expanded(
                      child: Container(
                        height: 1,
                        color: _Palette.amber.withValues(alpha: 0.12),
                      ),
                    ),
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 14),
                      child: Text(
                        'CURVE PARAMETERS',
                        style: TextStyle(
                          color: _Palette.textSecondary.withValues(alpha: 0.45),
                          fontSize: 10,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 1.6,
                        ),
                      ),
                    ),
                    Expanded(
                      child: Container(
                        height: 1,
                        color: _Palette.amber.withValues(alpha: 0.12),
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 14),
                _buildGroup(
                  title: 'COLOR TEMPERATURE',
                  color: _Palette.amber,
                  children: [
                    _buildCompactSlider(
                      label: 'Min',
                      value: _minColorTemp,
                      min: 1500,
                      max: 4000,
                      divisions: 25,
                      format: (v) => '${v.round()}K',
                      color: _Palette.amber,
                      onChanged: (v) => _onCurveChanged(() => _minColorTemp = v),
                    ),
                    _buildCompactSlider(
                      label: 'Max',
                      value: _maxColorTemp,
                      min: 2000,
                      max: 6500,
                      divisions: 45,
                      format: (v) => '${v.round()}K',
                      color: _Palette.amber,
                      onChanged: (v) => _onCurveChanged(() => _maxColorTemp = v),
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                _buildGroup(
                  title: 'BRIGHTNESS RANGE',
                  color: _Palette.purple,
                  children: [
                    _buildCompactSlider(
                      label: 'Min',
                      value: _minBrightness,
                      min: 1,
                      max: 50,
                      divisions: 49,
                      format: (v) => '${v.round()}%',
                      color: _Palette.purple,
                      onChanged: (v) =>
                          _onCurveChanged(() => _minBrightness = v),
                    ),
                    _buildCompactSlider(
                      label: 'Max',
                      value: _maxBrightness,
                      min: 20,
                      max: 100,
                      divisions: 80,
                      format: (v) => '${v.round()}%',
                      color: _Palette.purple,
                      onChanged: (v) =>
                          _onCurveChanged(() => _maxBrightness = v),
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                _buildGroup(
                  title: 'RAMP SPEEDS',
                  color: _Palette.blue,
                  children: [
                    _buildCompactSlider(
                      label: 'AM Bri',
                      value: _widthLeftBri,
                      min: 0.3,
                      max: 2.0,
                      divisions: 34,
                      format: (v) => v.toStringAsFixed(2),
                      color: _Palette.blue,
                      onChanged: (v) =>
                          _onCurveChanged(() => _widthLeftBri = v),
                    ),
                    _buildCompactSlider(
                      label: 'PM Bri',
                      value: _widthRightBri,
                      min: 0.3,
                      max: 2.0,
                      divisions: 34,
                      format: (v) => v.toStringAsFixed(2),
                      color: _Palette.blue,
                      onChanged: (v) =>
                          _onCurveChanged(() => _widthRightBri = v),
                    ),
                    _buildCompactSlider(
                      label: 'AM CCT',
                      value: _widthLeftCct,
                      min: 0.3,
                      max: 2.0,
                      divisions: 34,
                      format: (v) => v.toStringAsFixed(2),
                      color: _Palette.blue,
                      onChanged: (v) =>
                          _onCurveChanged(() => _widthLeftCct = v),
                    ),
                    _buildCompactSlider(
                      label: 'PM CCT',
                      value: _widthRightCct,
                      min: 0.3,
                      max: 2.0,
                      divisions: 34,
                      format: (v) => v.toStringAsFixed(2),
                      color: _Palette.blue,
                      onChanged: (v) =>
                          _onCurveChanged(() => _widthRightCct = v),
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                _buildGroup(
                  title: 'CURVE SHAPE',
                  color: _Palette.teal,
                  children: [
                    _buildCompactSlider(
                      label: 'Exponent',
                      value: _shapeP,
                      min: 1.0,
                      max: 10.0,
                      divisions: 18,
                      format: (v) => v.toStringAsFixed(1),
                      color: _Palette.teal,
                      onChanged: (v) => _onCurveChanged(() => _shapeP = v),
                    ),
                    _buildCompactSlider(
                      label: 'Dim Steps',
                      value: _maxDimSteps,
                      min: 1,
                      max: 20,
                      divisions: 19,
                      format: (v) => '${v.round()}',
                      color: _Palette.teal,
                      onChanged: (v) =>
                          _onCurveChanged(() => _maxDimSteps = v),
                    ),
                  ],
                ),
                if (_curveConfigDirty) ...[
                  const SizedBox(height: 16),
                  _buildSaveButton(),
                ],
              ],
            )
          : const SizedBox.shrink(),
    );
  }

  // ---------------------------------------------------------------------------
  // Hero icon — color shifts to match simulated CCT
  // ---------------------------------------------------------------------------

  Widget _buildHeroIcon() {
    final selectedHour = _selectedHour();
    final kelvin = _kelvinAtHour(selectedHour);
    final cctColor =
        _hasTimeOffset ? ColorUtils.curveColorForCCT(kelvin) : _Palette.amber;

    return AnimatedBuilder(
      animation: _glowAnimation,
      builder: (context, child) {
        return Container(
          width: 72,
          height: 72,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: cctColor.withValues(alpha: 0.08),
            border: Border.all(
              color: cctColor.withValues(alpha: 0.18),
            ),
            boxShadow: [
              BoxShadow(
                color:
                    cctColor.withValues(alpha: _glowAnimation.value * 0.15),
                blurRadius: 32,
                spreadRadius: 0,
              ),
            ],
          ),
          child: Icon(
            Icons.lightbulb_rounded,
            color: cctColor.withValues(
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

  Widget _buildGroup({
    required String title,
    required Color color,
    required List<Widget> children,
  }) {
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 14, 10, 6),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: _Palette.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            title,
            style: TextStyle(
              color: color.withValues(alpha: 0.55),
              fontSize: 10,
              fontWeight: FontWeight.w700,
              letterSpacing: 1.2,
            ),
          ),
          const SizedBox(height: 4),
          ...children,
        ],
      ),
    );
  }

  Widget _buildCompactSlider({
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
      padding: const EdgeInsets.symmetric(vertical: 1),
      child: Row(
        children: [
          SizedBox(
            width: 60,
            child: Text(
              label,
              style: TextStyle(
                color: _Palette.textSecondary.withValues(alpha: 0.7),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
          Expanded(
            child: SliderTheme(
              data: SliderThemeData(
                activeTrackColor: color.withValues(alpha: 0.6),
                inactiveTrackColor: color.withValues(alpha: 0.08),
                thumbColor: color,
                overlayColor: color.withValues(alpha: 0.08),
                trackHeight: 3,
                thumbShape:
                    const RoundSliderThumbShape(enabledThumbRadius: 6),
                overlayShape:
                    const RoundSliderOverlayShape(overlayRadius: 14),
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
                fontSize: 12,
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
          'This will update the light curve on the server. All connected rooms will be affected.',
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
            child: const Text('Save',
                style: TextStyle(color: _Palette.amber)),
          ),
        ],
      ),
    );

    if (confirmed == true) {
      await _saveCurveConfig();
    }
  }

  Future<void> _saveCurveConfig() async {
    final config = CurveConfigDto(
      minColorTemp: _minColorTemp.round(),
      maxColorTemp: _maxColorTemp.round(),
      minBrightness: _minBrightness.round(),
      maxBrightness: _maxBrightness.round(),
      widthLeftBri: _widthLeftBri,
      widthRightBri: _widthRightBri,
      widthLeftCct: _widthLeftCct,
      widthRightCct: _widthRightCct,
      shapeP: _shapeP,
      maxDimSteps: _maxDimSteps.round(),
      fadeMs: _fadeMs.round(),
      motionTimeoutSecs: _motionTimeoutSecs,
    );

    await context.read<ServerSyncProvider>().api.configSet(RhythmCurveConfig(
      minColorTemp: config.minColorTemp, maxColorTemp: config.maxColorTemp,
      minBrightness: config.minBrightness, maxBrightness: config.maxBrightness,
      widthLeftBri: config.widthLeftBri, widthRightBri: config.widthRightBri,
      widthLeftCct: config.widthLeftCct, widthRightCct: config.widthRightCct,
      shapeP: config.shapeP, maxDimSteps: config.maxDimSteps,
      fadeMs: config.fadeMs, motionTimeoutSecs: config.motionTimeoutSecs,
    ));

    if (mounted) {
      context.read<ConfigModel>().updateConfig(config);
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
        kelvin = _interpolateCurve(
                curveData!.hours, curveData!.kelvin, hour)
            .toInt();
        brightness = _interpolateCurve(
                curveData!.hours, curveData!.brightness, hour)
            .toInt();
      } else {
        final t = 1 - ((hour - 12).abs() / 12);
        kelvin = (2000 + t * 3500).toInt();
        brightness = (5 + t * 95).toInt();
      }

      final color = ColorUtils.curveColorForCCT(kelvin);
      final opacity = 0.08 + (brightness / 100) * 0.92;
      colors.add(color.withValues(alpha: opacity));

      // Use compressed positions for the gradient stops.
      final stop = compressedPositions != null
          ? compressedPositions![i]
          : i / n;
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

    int kelvin;
    if (curveData != null && curveData!.hours.isNotEmpty) {
      kelvin = _interpolateCurve(
              curveData!.hours, curveData!.kelvin, selectedHour)
          .toInt();
    } else {
      final t = 1 - ((selectedHour - 12).abs() / 12);
      kelvin = (2000 + t * 3500).toInt();
    }
    final cctColor = ColorUtils.curveColorForCCT(kelvin);

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
      final priority = h % 6 == 0 ? 0 : h % 3 == 0 ? 1 : h % 2 == 0 ? 2 : 3;
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
  static const purple = Color(0xFFA78BFA);
}
