import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;

import '../api/hybrid_client.dart';
import '../models/config_model.dart';
import '../models/plan_tier.dart';
import '../providers/server_sync_provider.dart';
import '../providers/subscription_provider.dart';
import '../services/analytics_service.dart';
import 'pro_lock.dart';

// Local palette — mirrors the Light profile surfaces.
const Color _amber = Color(0xFFF9A825);
const Color _card = Color(0xFF13171E);
const Color _border = Color(0xFF232A35);
const Color _textSecondary = Color(0xFF8A919C);
const double _brightnessChartPlotLeft = 34;
const double _brightnessChartPlotTop = 14;
const double _brightnessChartPlotRightInset = 12;
const double _brightnessChartPlotBottomInset = 26;

/// The Time Simulator — a compressed spectrum "ribbon" you scrub to preview
/// what the Day curve looks like at any hour, with the option to push that
/// preview to the real lights or **absorb** the offset into the profile.
///
/// Self-contained: it loads the Day profile's curve from [ServerSyncProvider]
/// and owns all time-offset state. Lives on the Light tab (Day mode only),
/// independent of the per-profile editors.
class TimeSimulator extends StatefulWidget {
  /// Profile whose curve the simulator scrubs and absorbs into. Defaults to the
  /// Day ("rhythm") profile.
  final String profileId;

  const TimeSimulator({super.key, this.profileId = 'rhythm'});

  @override
  State<TimeSimulator> createState() => _TimeSimulatorState();
}

enum _DispatchAction { preview, reset, absorb }

class _TimeSimulatorState extends State<TimeSimulator>
    with SingleTickerProviderStateMixin {
  static const Duration _defaultBatchDispatchSpacing =
      Duration(milliseconds: 500);

  late final ServerSyncProvider _serverSync;

  // Curve + slider state.
  CurveData? _curveData;
  List<double>? _compressedPositions;
  Color? _fixedColor;
  String? _loadedConfigKey;
  int _curvePreviewRequestId = 0;

  double _timeOffsetMinutes = 0;
  double _sliderFraction = 0.5;
  bool _isDraggingTime = false;
  bool _timeOffsetApplied = false;
  bool _timeOffsetPreviewActive = false;
  _DispatchAction? _dispatchAction;

  // Glow animation for the slider thumb.
  late final AnimationController _glowController;
  late final Animation<double> _glowAnimation;

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

    _serverSync = context.read<ServerSyncProvider>();
    _serverSync.addListener(_handleSyncChanged);
    unawaited(_loadCurve());
  }

  @override
  void dispose() {
    _serverSync.removeListener(_handleSyncChanged);
    _glowController.dispose();
    super.dispose();
  }

  void _handleSyncChanged() {
    if (!mounted) return;
    // Don't clobber the curve mid-interaction.
    if (_isDraggingTime || _dispatchAction != null || _hasTimeOffset) return;
    if (!_serverSync.hasBeenSynced) return;
    if (_dayConfig()?._key != _loadedConfigKey) {
      unawaited(_loadCurve());
    }
  }

  // ---------------------------------------------------------------------------
  // Curve loading
  // ---------------------------------------------------------------------------

  sdk.RhythmCurveConfig? _dayConfig() {
    for (final profile in _serverSync.profiles) {
      if (profile.id == widget.profileId) return profile;
    }
    return null;
  }

  Future<void> _loadCurve() async {
    if (!_serverSync.hasBeenSynced) return;
    final api = _serverSync.api;
    try {
      var config = _dayConfig();
      config ??= await api.getConfig(id: widget.profileId);
      if (config == null || !mounted) return;

      final requestId = ++_curvePreviewRequestId;
      final fixedColor = _directColorOf(config);
      final data = await _resolveCurveData(config);
      if (data == null || !mounted || requestId != _curvePreviewRequestId) {
        return;
      }
      setState(() {
        _fixedColor = fixedColor;
        _curveData = data;
        _compressedPositions = _computeCompressedMapping(data);
        _loadedConfigKey = config!._key;
        if (!_hasTimeOffset && !_isDraggingTime) {
          _sliderFraction = _hourToNowFraction();
        }
      });
    } catch (e) {
      debugPrint('TimeSimulator: Failed to load curve: $e');
    }
  }

  Future<CurveData?> _resolveCurveData(sdk.RhythmCurveConfig config) async {
    final local = _tryBuildLocalGaussianCurveData(config);
    if (local != null) return local;

    final preview =
        await _serverSync.api.getCurveData(id: config.id, overrides: null);
    if (preview == null) return null;
    return _curveDataFromSdk(preview);
  }

  CurveData? _tryBuildLocalGaussianCurveData(sdk.RhythmCurveConfig config) {
    final curve = config.curve;
    if (curve is! sdk.RhythmSuperGaussianCurve || curve.directColor != null) {
      return null;
    }
    try {
      final api = context.read<RhythmApi>();
      if (api is! HybridApiClient) return null;
      final dto = _toCurveConfigDto(config);
      if (dto == null) return null;
      return api.getCurveDataHighRes(config: dto, samplesPerHour: 4);
    } catch (e) {
      debugPrint('TimeSimulator: Local Gaussian preview unavailable: $e');
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

  Color? _directColorOf(sdk.RhythmCurveConfig config) {
    final curve = config.curve;
    final direct = switch (curve) {
      sdk.RhythmSuperGaussianCurve(:final directColor) => directColor,
      sdk.RhythmConstantCurve(:final directColor) => directColor,
      _ => null,
    };
    if (direct == null) return null;
    return Color.fromARGB(255, direct.rgb.r, direct.rgb.g, direct.rgb.b);
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

  // ---------------------------------------------------------------------------
  // Compressed mapping — rate-of-change based position distribution
  // ---------------------------------------------------------------------------

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

      final dK = (k1 - k0).abs() / 5000;
      final dB = (b1 - b0).abs() / 100;
      weights.add(math.max(dK + dB, 0.003));
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

  double _positionToHour(double position) {
    final p = _compressedPositions;
    if (p == null) return position * 24;
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
  // Time helpers
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
    final fixedColor = _fixedColor;
    if (fixedColor != null) return fixedColor;
    final kelvin = _kelvinAtHour(hour);
    if (kelvin > 0) return ColorUtils.curveColorForCCT(kelvin);
    return _amber;
  }

  String _previewValueLabel(double hour) {
    final brightness = _brightnessAtHour(hour);
    final kelvin = _kelvinAtHour(hour);
    if (_fixedColor != null) {
      return '${_formatHour(hour)}  $brightness%  Fixed Color';
    }
    if (kelvin > 0) {
      return '${_formatHour(hour)}  $brightness%  ${kelvin}K';
    }
    return '${_formatHour(hour)}  $brightness%';
  }

  double _hourToNowFraction() {
    return _hourToPosition(_currentHour());
  }

  double _hourToPosition(double hour) {
    final p = _compressedPositions;
    final normalized = hour.clamp(0.0, 24.0).toDouble();
    if (p == null) return normalized / 24;
    final idx = (normalized / 24 * 96).clamp(0.0, 96.0);
    final lower = idx.floor().clamp(0, 95);
    final upper = (lower + 1).clamp(0, 96);
    final t = idx - lower;
    return p[lower] + t * (p[upper] - p[lower]);
  }

  double _selectedChartHour() {
    if (_hasTimeOffset || _isDraggingTime || _timeOffsetPreviewActive) {
      return _positionToHour(_sliderFraction).clamp(0.0, 24.0).toDouble();
    }
    return _currentHour();
  }

  void _onSliderInteraction(double dx, double trackWidth) {
    final fraction = (dx / trackWidth).clamp(0.0, 1.0);
    final tappedHour = _positionToHour(fraction);
    _setSelectedHour(tappedHour, sliderFraction: fraction);
  }

  void _onChartInteraction(Offset position, Size chartSize) {
    final plotLeft = _brightnessChartPlotLeft;
    final plotRight = math.max(
        plotLeft + 1, chartSize.width - _brightnessChartPlotRightInset);
    final hour =
        ((position.dx - plotLeft) / (plotRight - plotLeft)).clamp(0.0, 1.0) *
            24;
    _setSelectedHour(hour, sliderFraction: _hourToPosition(hour));
  }

  void _setSelectedHour(double tappedHour, {required double sliderFraction}) {
    double offset = (tappedHour - _currentHour()) * 60;
    offset = (offset / 5).roundToDouble() * 5;

    setState(() {
      _timeOffsetMinutes = offset;
      _sliderFraction = sliderFraction.clamp(0.0, 1.0).toDouble();
      _timeOffsetApplied = false;
    });
  }

  // ---------------------------------------------------------------------------
  // Dispatch — preview / reset / absorb
  // ---------------------------------------------------------------------------

  bool get _dispatching => _dispatchAction != null;

  bool _isDispatching(_DispatchAction action) => _dispatchAction == action;

  Future<sdk.RhythmDispatchResult> _sendTimeOffset({
    required double offsetMinutes,
  }) {
    return _serverSync.api.nodeOffsetPreviewResult(timeOffset: offsetMinutes);
  }

  Future<void> _waitForDispatch(sdk.RhythmDispatchResult result) async {
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

  Future<void> _applyTimeOffset() async {
    if (_dispatching) return;
    final previewOffset = _timeOffsetMinutes;
    setState(() => _dispatchAction = _DispatchAction.preview);
    try {
      final result = await _sendTimeOffset(offsetMinutes: previewOffset);
      await _waitForDispatch(result);
      if (!mounted) return;
      setState(() {
        _timeOffsetApplied =
            (_timeOffsetMinutes - previewOffset).abs() <= 0.5 &&
                _isSignificantTimeOffset(previewOffset);
        _timeOffsetPreviewActive = _isSignificantTimeOffset(previewOffset);
      });
      AnalyticsService().logLightProfilePreviewAction(
        profile: widget.profileId,
        action: 'apply',
        offsetMinutes: previewOffset,
      );
    } finally {
      if (mounted) setState(() => _dispatchAction = null);
    }
  }

  Future<void> _resetTimeOffset() async {
    if (_dispatching) return;
    final previousOffset = _timeOffsetMinutes;
    setState(() {
      _timeOffsetMinutes = 0;
      _sliderFraction = _hourToNowFraction();
      _timeOffsetApplied = false;
      _timeOffsetPreviewActive = false;
      _dispatchAction = _DispatchAction.reset;
    });
    try {
      final result = await _sendTimeOffset(offsetMinutes: 0);
      await _waitForDispatch(result);
      AnalyticsService().logLightProfilePreviewAction(
        profile: widget.profileId,
        action: 'reset',
        offsetMinutes: previousOffset,
      );
    } finally {
      if (mounted) setState(() => _dispatchAction = null);
    }
  }

  Future<void> _absorbTimeOffset() async {
    if (_dispatching) return;
    final absorbedOffset = _timeOffsetMinutes;
    setState(() => _dispatchAction = _DispatchAction.absorb);
    try {
      final result = await _serverSync.api
          .absorbTimeOffsetResult(absorbedOffset, id: widget.profileId);
      if (!mounted) return;
      final sdkConfig = result.config;
      if (sdkConfig != null) {
        await _syncActiveConfigModel(sdkConfig);
        await _loadCurve();
      }
      await _waitForDispatch(result.dispatch);
      if (!mounted) return;
      setState(() {
        _timeOffsetMinutes = 0;
        _sliderFraction = _hourToNowFraction();
        _timeOffsetApplied = false;
        _timeOffsetPreviewActive = false;
      });
      AnalyticsService().logLightProfilePreviewAction(
        profile: widget.profileId,
        action: 'absorb',
        offsetMinutes: absorbedOffset,
      );
    } finally {
      if (mounted) setState(() => _dispatchAction = null);
    }
  }

  Future<void> _syncActiveConfigModel(sdk.RhythmCurveConfig config) async {
    if (config.id == _serverSync.activeProfileId) {
      final dto = _toCurveConfigDto(config);
      if (dto != null && mounted) {
        context.read<ConfigModel>().updateConfig(dto);
      }
    }
    await _serverSync.fullRefresh();
  }

  // ---------------------------------------------------------------------------
  // Build
  // ---------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    final unlocked =
        context.watch<SubscriptionProvider>().has(Entitlement.timeSimulator);
    return ProLockWrap(
      unlocked: unlocked,
      entitlement: Entitlement.timeSimulator,
      child: _buildSimulator(),
    );
  }

  Widget _buildSimulator() {
    final selectedHour = _selectedHour();
    final chartHour = _selectedChartHour();
    final previewColor = _previewColorAtHour(selectedHour);
    final active = _hasTimeOffset;

    return Container(
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(20),
        gradient: const LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [Color(0xFF06080C), Color(0xFF0A0D13)],
        ),
        border: Border.all(
          color: active
              ? _amber.withValues(alpha: 0.30)
              : Colors.white.withValues(alpha: 0.04),
        ),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.35),
            blurRadius: 8,
            spreadRadius: -2,
            offset: const Offset(0, 2),
          ),
          if (active)
            BoxShadow(
              color: _amber.withValues(alpha: 0.12),
              blurRadius: 24,
              spreadRadius: -6,
            ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Container(
                width: 6,
                height: 6,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: active ? _amber : _amber.withValues(alpha: 0.35),
                  boxShadow: active
                      ? [
                          BoxShadow(
                            color: _amber.withValues(alpha: 0.6),
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
                    color: _amber.withValues(alpha: 0.75),
                    fontSize: 10,
                    fontWeight: FontWeight.w800,
                    letterSpacing: 2.0,
                  ),
                ),
              ),
              const SizedBox(width: 8),
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
                                FontFeature.tabularFigures()
                              ],
                            ),
                          )
                        : Text(
                            'Drag to simulate',
                            key: const ValueKey('hint'),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              color: _textSecondary.withValues(alpha: 0.45),
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
          _buildGradientSlider(),
          const SizedBox(height: 36),
          _buildBrightnessCurveChart(
            selectedHour: chartHour,
            previewColor: _previewColorAtHour(chartHour),
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeOutCubic,
            child: _showTimeOffsetActions
                ? Padding(
                    padding: const EdgeInsets.only(top: 14),
                    child: _buildActions(),
                  )
                : const SizedBox.shrink(),
          ),
        ],
      ),
    );
  }

  Widget _buildActions() {
    final children = <Widget>[];
    var showAbsorbNote = false;
    if (_timeOffsetPreviewActive) {
      children.add(_buildResetButton());
      if (_timeOffsetApplied) {
        children
          ..add(const SizedBox(width: 12))
          ..add(_buildAbsorbButton());
        showAbsorbNote = true;
      } else if (_hasTimeOffset) {
        children
          ..add(const SizedBox(width: 12))
          ..add(_buildApplyButton());
      }
    } else {
      children
        ..add(_buildClearButton())
        ..add(const SizedBox(width: 12))
        ..add(_buildApplyButton());
    }

    final row =
        Row(mainAxisAlignment: MainAxisAlignment.center, children: children);
    if (!showAbsorbNote) return row;

    return Column(
      children: [
        row,
        const SizedBox(height: 10),
        Text(
          'Absorb bakes this preview into the Day profile — it reshapes the '
          'curve so the current time keeps this look, then clears the offset.',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: _textSecondary.withValues(alpha: 0.6),
            fontSize: 11,
            height: 1.4,
            letterSpacing: 0.1,
          ),
        ),
      ],
    );
  }

  Widget _buildGradientSlider() {
    return LayoutBuilder(
      builder: (context, constraints) {
        final trackWidth = constraints.maxWidth;
        return GestureDetector(
          onTapDown: (details) =>
              _onSliderInteraction(details.localPosition.dx, trackWidth),
          onHorizontalDragStart: (details) {
            setState(() => _isDraggingTime = true);
            _onSliderInteraction(details.localPosition.dx, trackWidth);
          },
          onHorizontalDragUpdate: (details) =>
              _onSliderInteraction(details.localPosition.dx, trackWidth),
          onHorizontalDragEnd: (_) => setState(() => _isDraggingTime = false),
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
                  fixedColor: _fixedColor,
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

  Widget _buildBrightnessCurveChart({
    required double selectedHour,
    required Color previewColor,
  }) {
    final curveData = _curveData;
    final isSimulated = _hasTimeOffset || _isDraggingTime;
    final selectedBrightness = curveData == null
        ? null
        : _interpolateCurve(
            curveData.hours,
            curveData.brightness,
            selectedHour,
          ).round();
    final selectedKelvin =
        curveData == null || curveData.kelvin.isEmpty || _fixedColor != null
            ? null
            : _interpolateCurve(
                curveData.hours,
                curveData.kelvin,
                selectedHour,
              ).round();
    final readout = selectedBrightness == null
        ? 'LOADING'
        : selectedKelvin == null
            ? '${isSimulated ? 'SIM' : 'NOW'} ${_formatHour(selectedHour)}  BRI $selectedBrightness%'
            : '${isSimulated ? 'SIM' : 'NOW'} ${_formatHour(selectedHour)}  BRI $selectedBrightness%  CCT ${selectedKelvin}K';

    return Semantics(
      label: 'Brightness curve chart',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  'BRI CURVE',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: _textSecondary.withValues(alpha: 0.54),
                    fontSize: 9.5,
                    fontWeight: FontWeight.w800,
                    letterSpacing: 1.7,
                  ),
                ),
              ),
              const SizedBox(width: 10),
              Flexible(
                child: Align(
                  alignment: Alignment.centerRight,
                  child: Text(
                    readout,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: selectedBrightness == null
                          ? _textSecondary.withValues(alpha: 0.45)
                          : previewColor.withValues(alpha: 0.82),
                      fontSize: 10.5,
                      fontWeight: FontWeight.w700,
                      letterSpacing: 0.2,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          LayoutBuilder(
            builder: (context, constraints) {
              final chartSize = Size(constraints.maxWidth, 148);
              return MouseRegion(
                cursor: SystemMouseCursors.grab,
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTapDown: (details) =>
                      _onChartInteraction(details.localPosition, chartSize),
                  onHorizontalDragStart: (details) {
                    setState(() => _isDraggingTime = true);
                    _onChartInteraction(details.localPosition, chartSize);
                  },
                  onHorizontalDragUpdate: (details) =>
                      _onChartInteraction(details.localPosition, chartSize),
                  onHorizontalDragEnd: (_) =>
                      setState(() => _isDraggingTime = false),
                  onHorizontalDragCancel: () =>
                      setState(() => _isDraggingTime = false),
                  child: SizedBox(
                    height: chartSize.height,
                    child: CustomPaint(
                      painter: _BrightnessCurvePainter(
                        curveData: curveData,
                        currentHour: _currentHour(),
                        selectedHour: selectedHour,
                        accent: previewColor,
                        hasOffset: _hasTimeOffset,
                        isDragging: _isDraggingTime,
                        showCct: _fixedColor == null,
                      ),
                    ),
                  ),
                ),
              );
            },
          ),
        ],
      ),
    );
  }

  Widget _buildClearButton() {
    return GestureDetector(
      onTap: _dispatching
          ? null
          : () => setState(() {
                _timeOffsetMinutes = 0;
                _sliderFraction = _hourToNowFraction();
                _timeOffsetApplied = false;
                _timeOffsetPreviewActive = false;
              }),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        decoration: BoxDecoration(
          color: _card,
          borderRadius: BorderRadius.circular(20),
          border: Border.all(color: _border.withValues(alpha: 0.6)),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.close_rounded,
                color: _textSecondary.withValues(alpha: 0.5), size: 14),
            const SizedBox(width: 6),
            Text(
              'Clear',
              style: TextStyle(
                color: _textSecondary.withValues(alpha: 0.6),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildApplyButton() {
    final busy = _isDispatching(_DispatchAction.preview);
    return GestureDetector(
      onTap: _dispatching ? null : _applyTimeOffset,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 10),
        decoration: BoxDecoration(
          color: const Color(0xFF2A2520),
          borderRadius: BorderRadius.circular(20),
          border: Border.all(color: _amber.withValues(alpha: 0.4)),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            busy
                ? _buildSpinner(_amber, size: 16)
                : Icon(Icons.play_arrow_rounded,
                    color: _amber.withValues(alpha: 0.8), size: 16),
            const SizedBox(width: 6),
            Text(
              busy ? 'Updating Lights' : 'Preview on Lights',
              style: TextStyle(
                color: _amber.withValues(alpha: 0.8),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildResetButton() {
    final busy = _isDispatching(_DispatchAction.reset);
    return GestureDetector(
      onTap: _dispatching ? null : _resetTimeOffset,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        decoration: BoxDecoration(
          color: _card,
          borderRadius: BorderRadius.circular(20),
          border: Border.all(color: _border.withValues(alpha: 0.6)),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            busy
                ? _buildSpinner(_textSecondary, size: 14)
                : Icon(Icons.refresh_rounded,
                    color: _textSecondary.withValues(alpha: 0.5), size: 14),
            const SizedBox(width: 6),
            Text(
              busy ? 'Resetting Lights' : 'Reset',
              style: TextStyle(
                color: _textSecondary.withValues(alpha: 0.6),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildAbsorbButton() {
    final busy = _isDispatching(_DispatchAction.absorb);
    const absorbColor = Color(0xFFD4A54A);
    return GestureDetector(
      onTap: _dispatching ? null : _absorbTimeOffset,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        decoration: BoxDecoration(
          color: const Color(0xFF2A2520),
          borderRadius: BorderRadius.circular(20),
          border: Border.all(color: absorbColor.withValues(alpha: 0.4)),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            busy
                ? _buildSpinner(absorbColor, size: 14)
                : Icon(Icons.check_rounded,
                    color: absorbColor.withValues(alpha: 0.8), size: 14),
            const SizedBox(width: 6),
            Text(
              busy ? 'Updating Lights' : 'Absorb',
              style: TextStyle(
                color: absorbColor.withValues(alpha: 0.8),
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildSpinner(Color color, {required double size}) {
    return SizedBox(
      width: size,
      height: size,
      child: CircularProgressIndicator(
        strokeWidth: 2,
        color: color.withValues(alpha: 0.8),
      ),
    );
  }
}

/// Convenience for change-detection: a cheap identity string for a config.
extension on sdk.RhythmCurveConfig {
  String get _key => '$id:$hashCode';
}

// -----------------------------------------------------------------------------
// Curve interpolation + the compressed spectrum painter
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

    canvas.drawRRect(rrect, Paint()..color = const Color(0xFF080A0E));

    canvas.save();
    canvas.clipRRect(rrect);
    _drawGradientFill(canvas, ribbonSize);
    canvas.restore();

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

    canvas.drawRRect(
      rrect,
      Paint()
        ..style = PaintingStyle.stroke
        ..color = const Color(0xFF1E2530)
        ..strokeWidth = 1,
    );

    _drawNowMarker(canvas, ribbonSize);

    if (hasOffset || isDragging) {
      _drawThumb(canvas, ribbonSize);
    }

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

    final glowIntensity = isDragging ? 0.25 : 0.15 + glowPhase * 0.06;
    canvas.drawLine(
      Offset(x, 0),
      Offset(x, size.height),
      Paint()
        ..color = cctColor.withValues(alpha: glowIntensity)
        ..strokeWidth = 20
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 12),
    );

    canvas.drawLine(
      Offset(x, 0),
      Offset(x, size.height),
      Paint()
        ..color = Colors.white.withValues(alpha: isDragging ? 0.2 : 0.1)
        ..strokeWidth = 8
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 4),
    );

    canvas.drawLine(
      Offset(x, 3),
      Offset(x, size.height - 3),
      Paint()
        ..color = Colors.white.withValues(alpha: 0.9)
        ..strokeWidth = 2
        ..strokeCap = StrokeCap.round,
    );

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

    final all = <({double x, String label, int priority})>[];
    for (int h = 0; h < 24; h++) {
      final x = _hourToX(h.toDouble(), size.width);
      final h12 = h == 0 ? 12 : (h > 12 ? h - 12 : h);
      final suffix = h >= 12 ? 'p' : 'a';
      final label = '$h12$suffix';
      final priority = h % 6 == 0
          ? 0
          : h % 3 == 0
              ? 1
              : h % 2 == 0
                  ? 2
                  : 3;
      all.add((x: x, label: label, priority: priority));
    }

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
      canvas.drawLine(
        Offset(e.x, _ribbonHeight + 2),
        Offset(e.x, _ribbonHeight + 6),
        Paint()
          ..color = Colors.white.withValues(alpha: 0.15)
          ..strokeWidth = 1
          ..strokeCap = StrokeCap.round,
      );

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

class _BrightnessCurvePainter extends CustomPainter {
  final CurveData? curveData;
  final double currentHour;
  final double selectedHour;
  final Color accent;
  final bool hasOffset;
  final bool isDragging;
  final bool showCct;

  _BrightnessCurvePainter({
    required this.curveData,
    required this.currentHour,
    required this.selectedHour,
    required this.accent,
    required this.hasOffset,
    required this.isDragging,
    required this.showCct,
  });

  static const double _axisMax = 100;

  double _hourToX(double hour, Rect plot) {
    if (hour <= 0) return plot.left;
    if (hour >= 24) return plot.right;
    return plot.left + hour / 24 * plot.width;
  }

  double _briToY(double brightness, Rect plot) {
    final clamped = brightness.clamp(0.0, _axisMax);
    return plot.bottom - clamped / _axisMax * plot.height;
  }

  @override
  void paint(Canvas canvas, Size size) {
    final panel = Offset.zero & size;
    final panelRRect = RRect.fromRectAndRadius(
      panel,
      const Radius.circular(14),
    );

    canvas.drawRRect(
      panelRRect,
      Paint()..color = const Color(0xFF090C12),
    );

    canvas.save();
    canvas.clipRRect(panelRRect);
    canvas.drawRect(
      panel,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [
            Colors.white.withValues(alpha: 0.035),
            Colors.transparent,
            Colors.black.withValues(alpha: 0.16),
          ],
        ).createShader(panel),
    );
    canvas.restore();

    canvas.drawRRect(
      panelRRect,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1
        ..color = const Color(0xFF1E2530).withValues(alpha: 0.85),
    );

    final plot = Rect.fromLTRB(
      _brightnessChartPlotLeft,
      _brightnessChartPlotTop,
      math.max(_brightnessChartPlotLeft + 1,
          size.width - _brightnessChartPlotRightInset),
      math.max(_brightnessChartPlotTop + 1,
          size.height - _brightnessChartPlotBottomInset),
    );
    if (plot.width <= 1 || plot.height <= 1) return;

    _drawGrid(canvas, plot, size.width);

    final data = curveData;
    if (data == null || data.hours.isEmpty || data.brightness.isEmpty) {
      _drawCenteredText(canvas, panel, 'Curve loading');
      return;
    }

    final curvePath = _buildCurvePath(data, plot);
    final fillPath = Path.from(curvePath)
      ..lineTo(plot.right, plot.bottom)
      ..lineTo(plot.left, plot.bottom)
      ..close();

    canvas.save();
    canvas.clipRect(plot.inflate(1));
    canvas.drawPath(
      fillPath,
      Paint()..shader = _timeColorShader(data, plot, alpha: 0.18),
    );

    canvas.drawPath(
      curvePath,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 5
        ..strokeCap = StrokeCap.round
        ..strokeJoin = StrokeJoin.round
        ..shader = _timeColorShader(data, plot, alpha: 0.2)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 5),
    );

    canvas.drawPath(
      curvePath,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2.5
        ..strokeCap = StrokeCap.round
        ..strokeJoin = StrokeJoin.round
        ..shader = _timeColorShader(data, plot, alpha: 0.9),
    );
    canvas.restore();

    _drawTimeMarker(
      canvas,
      plot,
      hour: currentHour,
      brightness: _interpolateCurve(data.hours, data.brightness, currentHour),
      color: Colors.white,
      alpha: hasOffset || isDragging ? 0.28 : 0.42,
      label: null,
    );

    final selectedBrightness =
        _interpolateCurve(data.hours, data.brightness, selectedHour);
    final selectedKelvin = showCct && data.kelvin.isNotEmpty
        ? _interpolateCurve(data.hours, data.kelvin, selectedHour).round()
        : null;
    final selectedColor = _colorAtHour(data, selectedHour);
    _drawTimeMarker(
      canvas,
      plot,
      hour: selectedHour,
      brightness: selectedBrightness,
      color: selectedColor,
      alpha: 0.95,
      label: selectedKelvin == null
          ? 'BRI ${selectedBrightness.round()}%'
          : 'BRI ${selectedBrightness.round()}%\nCCT ${selectedKelvin}K',
    );
  }

  Shader _timeColorShader(CurveData data, Rect plot, {required double alpha}) {
    const samples = 96;
    final colors = <Color>[];
    final stops = <double>[];

    for (int i = 0; i <= samples; i++) {
      final hour = i / samples * 24;
      colors.add(_colorAtHour(data, hour).withValues(alpha: alpha));
      stops.add(i / samples);
    }

    return LinearGradient(colors: colors, stops: stops).createShader(plot);
  }

  Color _colorAtHour(CurveData data, double hour) {
    if (!showCct) return accent;
    if (data.kelvin.isEmpty) return accent;
    final kelvin = _interpolateCurve(data.hours, data.kelvin, hour).round();
    if (kelvin <= 0) return accent;
    return ColorUtils.curveColorForCCT(kelvin);
  }

  void _drawGrid(Canvas canvas, Rect plot, double width) {
    final gridPaint = Paint()
      ..color = Colors.white.withValues(alpha: 0.075)
      ..strokeWidth = 1;
    final axisPaint = Paint()
      ..color = Colors.white.withValues(alpha: 0.18)
      ..strokeWidth = 1;

    for (final value in const [0, 50, 100]) {
      final y = _briToY(value.toDouble(), plot);
      canvas.drawLine(
        Offset(plot.left, y),
        Offset(plot.right, y),
        value == 0 ? axisPaint : gridPaint,
      );
      _drawText(
        canvas,
        '$value',
        Offset(plot.left - 8, y),
        color: Colors.white.withValues(alpha: 0.32),
        fontSize: 9,
        align: Alignment.centerRight,
      );
    }

    final hourTicks =
        width < 300 ? const <int>[0, 12, 24] : const <int>[0, 6, 12, 18, 24];
    for (final hour in hourTicks) {
      final x = hour == 24 ? plot.right : _hourToX(hour.toDouble(), plot);
      canvas.drawLine(
        Offset(x, plot.top),
        Offset(x, plot.bottom),
        gridPaint,
      );
      _drawText(
        canvas,
        _formatTickHour(hour),
        Offset(x, plot.bottom + 13),
        color: Colors.white.withValues(alpha: 0.32),
        fontSize: 9,
        align: hour == 0
            ? Alignment.centerLeft
            : hour == 24
                ? Alignment.centerRight
                : Alignment.center,
      );
    }

    _drawText(
      canvas,
      'bri',
      Offset(plot.left - 8, plot.top - 2),
      color: Colors.white.withValues(alpha: 0.35),
      fontSize: 9,
      align: Alignment.centerRight,
    );
  }

  Path _buildCurvePath(CurveData data, Rect plot) {
    const samples = 192;
    final path = Path();
    for (int i = 0; i <= samples; i++) {
      final hour = i / samples * 24;
      final brightness = _interpolateCurve(
        data.hours,
        data.brightness,
        hour,
      );
      final point = Offset(_hourToX(hour, plot), _briToY(brightness, plot));
      if (i == 0) {
        path.moveTo(point.dx, point.dy);
      } else {
        path.lineTo(point.dx, point.dy);
      }
    }
    return path;
  }

  void _drawTimeMarker(
    Canvas canvas,
    Rect plot, {
    required double hour,
    required double brightness,
    required Color color,
    required double alpha,
    String? label,
  }) {
    final x = _hourToX(hour, plot);
    final y = _briToY(brightness, plot);

    canvas.drawLine(
      Offset(x, plot.top),
      Offset(x, plot.bottom),
      Paint()
        ..color = color.withValues(alpha: alpha * 0.62)
        ..strokeWidth = 1.2
        ..strokeCap = StrokeCap.round,
    );

    canvas.drawCircle(
      Offset(x, y),
      7,
      Paint()
        ..color = color.withValues(alpha: alpha * 0.22)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 5),
    );
    canvas.drawCircle(
      Offset(x, y),
      4,
      Paint()..color = color.withValues(alpha: alpha),
    );
    canvas.drawCircle(
      Offset(x, y),
      4,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1
        ..color = Colors.white.withValues(alpha: 0.7),
    );

    if (label == null) return;
    final labelX = x.clamp(plot.left + 18, plot.right - 18).toDouble();
    final labelY = (y - 24).clamp(plot.top + 13, plot.bottom - 10).toDouble();
    _drawText(
      canvas,
      label,
      Offset(labelX, labelY),
      color: Colors.white.withValues(alpha: 0.82),
      fontSize: 10,
      fontWeight: FontWeight.w700,
      align: Alignment.center,
    );
  }

  void _drawCenteredText(Canvas canvas, Rect panel, String text) {
    _drawText(
      canvas,
      text,
      panel.center,
      color: _textSecondary.withValues(alpha: 0.45),
      fontSize: 11,
      fontWeight: FontWeight.w600,
      align: Alignment.center,
    );
  }

  void _drawText(
    Canvas canvas,
    String text,
    Offset anchor, {
    required Color color,
    required double fontSize,
    FontWeight fontWeight = FontWeight.w500,
    Alignment align = Alignment.centerLeft,
  }) {
    final painter = TextPainter(
      text: TextSpan(
        text: text,
        style: TextStyle(
          color: color,
          fontSize: fontSize,
          fontWeight: fontWeight,
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      ),
      textDirection: TextDirection.ltr,
    )..layout();

    final offset = Offset(
      anchor.dx - painter.width * (align.x + 1) / 2,
      anchor.dy - painter.height * (align.y + 1) / 2,
    );
    painter.paint(canvas, offset);
  }

  String _formatTickHour(int hour) {
    if (hour == 0 || hour == 24) return '12a';
    if (hour == 12) return '12p';
    if (hour < 12) return '${hour}a';
    return '${hour - 12}p';
  }

  @override
  bool shouldRepaint(covariant _BrightnessCurvePainter old) =>
      curveData != old.curveData ||
      currentHour != old.currentHour ||
      selectedHour != old.selectedHour ||
      accent != old.accent ||
      hasOffset != old.hasOffset ||
      isDragging != old.isDragging ||
      showCct != old.showCct;
}
