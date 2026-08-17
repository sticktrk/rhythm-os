import 'dart:math';
import 'dart:ui' as ui;
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:rhythm_core/rhythm_core.dart';

// ═══════════════════════════════════════════════════════════════════
// MATH — handle positioning & drag logic ONLY
//
// IMPORTANT: These Dart functions are ONLY used for:
//   1. Computing handle positions (where to draw / hit-test handles)
//   2. Handle drag logic (converting pixel position → config values)
//
// The actual CURVE RENDERING uses FFI-computed CurveData from Rust
// (via getCurveDataHighRes) to guarantee the displayed curve is
// IDENTICAL to what drives the lights. No room for drift.
// ═══════════════════════════════════════════════════════════════════

const double _epsilon = 0.02;

double? _widthFromDrag(
    double hour, String side, CurveConfigDto c, double sr, double ss) {
  final noon = (sr + ss) / 2;
  final dist = (hour - noon).abs();
  if (dist < 0.3) return null;
  final halfDay = side == 'left' ? noon - sr : ss - noon;
  return (halfDay / dist).clamp(0.2, 2.0);
}

Color _kelvinToColor(double k) {
  final t = k / 100;
  double r, g, b;
  if (t <= 66) {
    r = 255;
    g = max(0, min(255, 99.4708 * log(t) - 161.1196));
    b = t <= 19 ? 0 : max(0, min(255, 138.5177 * log(t - 10) - 305.0448));
  } else {
    r = max(0, min(255, 329.6987 * pow(t - 60, -0.1332).toDouble()));
    g = max(0, min(255, 288.1222 * pow(t - 60, -0.0755).toDouble()));
    b = 255;
  }
  return Color.fromARGB(255, r.round(), g.round(), b.round());
}

// ═══════════════════════════════════════════════════════════════════
// FFI CURVE INTERPOLATION
// ═══════════════════════════════════════════════════════════════════

double _interpBri(double hour, CurveData? cd, double fallback) {
  if (cd == null || cd.hours.isEmpty) return fallback;
  final hours = cd.hours;
  final bri = cd.brightness;
  if (hour <= hours.first) return bri.first.toDouble();
  if (hour >= hours.last) return bri.last.toDouble();
  int lo = 0, hi = hours.length - 1;
  while (hi - lo > 1) {
    final mid = (lo + hi) ~/ 2;
    if (hours[mid] <= hour) {
      lo = mid;
    } else {
      hi = mid;
    }
  }
  final t = (hour - hours[lo]) / (hours[hi] - hours[lo]);
  return bri[lo] + (bri[hi] - bri[lo]) * t;
}

double _interpKelvin(double hour, CurveData? cd, double fallback) {
  if (cd == null || cd.hours.isEmpty) return fallback;
  final hours = cd.hours;
  final kel = cd.kelvin;
  if (hour <= hours.first) return kel.first.toDouble();
  if (hour >= hours.last) return kel.last.toDouble();
  int lo = 0, hi = hours.length - 1;
  while (hi - lo > 1) {
    final mid = (lo + hi) ~/ 2;
    if (hours[mid] <= hour) {
      lo = mid;
    } else {
      hi = mid;
    }
  }
  final t = (hour - hours[lo]) / (hours[hi] - hours[lo]);
  return kel[lo] + (kel[hi] - kel[lo]) * t;
}

// ═══════════════════════════════════════════════════════════════════
// LAYER MODE
// ═══════════════════════════════════════════════════════════════════

enum ChartLayer { brightness, colorTemp }

// ═══════════════════════════════════════════════════════════════════
// HANDLE METADATA — 5 chart handles + 4 edge slider thumbs
// ═══════════════════════════════════════════════════════════════════

// Chart handles (on the curve area)
const _briHandleIds = ['widthLeftBri', 'widthRightBri'];
const _cctHandleIds = ['widthLeftCCT', 'widthRightCCT'];
// Edge slider thumbs
const _edgeSliderIds = ['minBri', 'maxBri', 'minCCT', 'maxCCT'];

const _handleLayer = {
  'widthLeftBri': ChartLayer.brightness,
  'widthRightBri': ChartLayer.brightness,
  'widthLeftCCT': ChartLayer.colorTemp,
  'widthRightCCT': ChartLayer.colorTemp,
};

// ═══════════════════════════════════════════════════════════════════
// CHART LAYOUT
// ═══════════════════════════════════════════════════════════════════

class _ChartLayout {
  static const double briAxisMax = 120.0;
  static const double cctAxisMin = 1500;
  static const double cctAxisMax = 7000;
  static const double cctScaleW = 14;
  static const double cctScaleGap = 8;

  final double left, right, top, bottom, chartWidth, chartHeight;
  final double briSliderX;
  final double cctScaleLeft;

  _ChartLayout(Size size)
      : left = 50,
        right = size.width - 66,
        top = 28,
        bottom = size.height - 30,
        chartWidth = size.width - 66 - 50,
        chartHeight = size.height - 30 - 28,
        briSliderX = 18,
        cctScaleLeft = size.width - 66 + cctScaleGap;

  double get cctScaleRight => cctScaleLeft + cctScaleW;
  double get cctScaleCenterX => cctScaleLeft + cctScaleW / 2;

  double hourToX(double h) => left + (h / 24) * chartWidth;
  double xToHour(double x) => ((x - left) / chartWidth * 24).clamp(0.0, 24.0);
  double briToY(double v) => bottom - (v / briAxisMax) * chartHeight;
  double yToBri(double y) =>
      ((bottom - y) / chartHeight * briAxisMax).clamp(0.0, 100.0);
  double cctToY(double k) =>
      bottom -
      ((k - cctAxisMin) / (cctAxisMax - cctAxisMin)) * chartHeight;
  double yToCCT(double y) =>
      cctAxisMin + ((bottom - y) / chartHeight) * (cctAxisMax - cctAxisMin);
}

// ═══════════════════════════════════════════════════════════════════
// RHYTHM TUNER CHART — public widget
// ═══════════════════════════════════════════════════════════════════

class RhythmTunerChart extends StatefulWidget {
  final CurveConfigDto config;
  final CurveData? curveData;
  final double sunrise;
  final double sunset;
  final ValueChanged<CurveConfigDto> onConfigChanged;
  final VoidCallback? onDragStart;
  final VoidCallback? onDragEnd;
  final bool showSolarContext;

  const RhythmTunerChart({
    super.key,
    required this.config,
    this.curveData,
    required this.sunrise,
    required this.sunset,
    required this.onConfigChanged,
    this.onDragStart,
    this.onDragEnd,
    this.showSolarContext = false,
  });

  @override
  State<RhythmTunerChart> createState() => _RhythmTunerChartState();
}

class _RhythmTunerChartState extends State<RhythmTunerChart>
    with TickerProviderStateMixin {
  // Layer mode
  ChartLayer _activeLayer = ChartLayer.colorTemp;

  // Interaction state
  String? _activeHandleId; // chart handle or edge slider being dragged
  String? _hoveredHandleId;
  bool _showOnboarding = true;
  bool _showSolarLegend = false;
  _ChartLayout? _layout;

  // Animation controllers
  late final AnimationController _entranceCtrl;
  late final AnimationController _pulseCtrl;
  late final AnimationController _layerFadeCtrl;

  @override
  void initState() {
    super.initState();
    _entranceCtrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1200),
    )..addListener(() => setState(() {}));
    _pulseCtrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2000),
    )
      ..addListener(() => setState(() {}))
      ..repeat(reverse: true);
    _layerFadeCtrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 300),
      value: 1.0, // 0 = brightness, 1 = colorTemp
    )..addListener(() => setState(() {}));
    _entranceCtrl.forward();
  }

  @override
  void dispose() {
    _entranceCtrl.dispose();
    _pulseCtrl.dispose();
    _layerFadeCtrl.dispose();
    super.dispose();
  }

  // ── animation values ──
  double get _animProgress {
    final t = _entranceCtrl.value;
    if (t < 0.667) return Curves.easeOutCubic.transform(t / 0.667);
    return 1.0;
  }

  double get _handleOpacity {
    final t = _entranceCtrl.value;
    if (t < 0.667) return 0.0;
    return Curves.easeOutCubic.transform((t - 0.667) / 0.333);
  }

  /// 0.0 = BRI fully active, 1.0 = CCT fully active
  double get _layerBlend => _layerFadeCtrl.value;

  void _switchLayer(ChartLayer layer) {
    if (layer == _activeLayer) return;
    setState(() => _activeLayer = layer);
    if (layer == ChartLayer.colorTemp) {
      _layerFadeCtrl.forward();
    } else {
      _layerFadeCtrl.reverse();
    }
  }

  // ── handle position computation ──
  Map<String, Offset> _computeHandlePositions() {
    final l = _layout;
    if (l == null) return {};
    final c = widget.config;
    final sr = widget.sunrise;
    final ss = widget.sunset;
    final noon = (sr + ss) / 2;
    final pos = <String, Offset>{};
    final cd = widget.curveData;
    final halfDayL = noon - sr;
    final halfDayR = ss - noon;

    final edgeBri =
        c.minBrightness + _epsilon * (c.maxBrightness - c.minBrightness);

    double edgeFallback(String side, double halfDay, double w) {
      final dist = halfDay / max(w, 0.2);
      return side == 'left' ? noon - dist : noon + dist;
    }

    // BRI width handles — use edgeFallback (inverse of _widthFromDrag)
    // so handle position exactly matches drag math with no FFI drift
    final leftBriH = edgeFallback('left', halfDayL, c.widthLeftBri);
    pos['widthLeftBri'] = Offset(
      l.hourToX(leftBriH),
      l.briToY(_interpBri(leftBriH, cd, edgeBri)),
    );
    final rightBriH = edgeFallback('right', halfDayR, c.widthRightBri);
    pos['widthRightBri'] = Offset(
      l.hourToX(rightBriH),
      l.briToY(_interpBri(rightBriH, cd, edgeBri)),
    );

    // CCT width handles — use edgeFallback (inverse of _widthFromDrag)
    // so handle position exactly matches drag math with no FFI drift
    final leftCctH = edgeFallback('left', halfDayL, c.widthLeftCct);
    pos['widthLeftCCT'] = Offset(
        l.hourToX(leftCctH),
        l.briToY(_interpBri(leftCctH, cd, edgeBri)));
    final rightCctH = edgeFallback('right', halfDayR, c.widthRightCct);
    pos['widthRightCCT'] = Offset(
        l.hourToX(rightCctH),
        l.briToY(_interpBri(rightCctH, cd, edgeBri)));

    // Edge slider thumbs
    pos['maxBri'] = Offset(l.briSliderX, l.briToY(c.maxBrightness.toDouble()));
    pos['minBri'] = Offset(l.briSliderX, l.briToY(c.minBrightness.toDouble()));
    pos['maxCCT'] = Offset(l.cctScaleCenterX, l.cctToY(c.maxColorTemp.toDouble()));
    pos['minCCT'] = Offset(l.cctScaleCenterX, l.cctToY(c.minColorTemp.toDouble()));

    return pos;
  }

  Color _handleColor(String id) {
    switch (id) {
      case 'maxCCT':
        return _kelvinToColor(widget.config.maxColorTemp.toDouble());
      case 'minCCT':
        return _kelvinToColor(widget.config.minColorTemp.toDouble());
      case 'widthLeftCCT':
      case 'widthRightCCT':
        return const Color(0xFF4DD0E1);
      default:
        return const Color(0xFFFFB74D);
    }
  }

  String _handleLabel(String id) {
    final c = widget.config;
    switch (id) {
      case 'maxBri':
        return '${c.maxBrightness}%';
      case 'minBri':
        return '${c.minBrightness}%';
      case 'widthLeftBri':
        return '\u2190${c.widthLeftBri.toStringAsFixed(2)}';
      case 'widthRightBri':
        return '${c.widthRightBri.toStringAsFixed(2)}\u2192';
      case 'maxCCT':
        return '${c.maxColorTemp}K';
      case 'minCCT':
        return '${c.minColorTemp}K';
      case 'widthLeftCCT':
        return '\u2190${c.widthLeftCct.toStringAsFixed(2)}';
      case 'widthRightCCT':
        return '${c.widthRightCct.toStringAsFixed(2)}\u2192';
      default:
        return '';
    }
  }

  // ── hit testing ──
  String? _findHandle(Offset position) {
    final positions = _computeHandlePositions();
    const hitR = 28.0;
    String? best;
    double bestDist = hitR;

    // Determine which chart handles are active based on layer
    final activeChartHandles = _activeLayer == ChartLayer.brightness
        ? _briHandleIds
        : _cctHandleIds;

    // Check active chart handles first (highest priority)
    for (final id in activeChartHandles) {
      final p = positions[id];
      if (p == null) continue;
      final d = (position - p).distance;
      if (d < bestDist) {
        best = id;
        bestDist = d;
      }
    }

    // Check edge sliders (always available)
    for (final id in _edgeSliderIds) {
      final p = positions[id];
      if (p == null) continue;
      final d = (position - p).distance;
      if (d < bestDist) {
        best = id;
        bestDist = d;
      }
    }

    // Check inactive layer handles (can auto-switch)
    if (best == null) {
      final inactiveHandles = _activeLayer == ChartLayer.brightness
          ? _cctHandleIds
          : _briHandleIds;
      for (final id in inactiveHandles) {
        final p = positions[id];
        if (p == null) continue;
        final d = (position - p).distance;
        if (d < bestDist) {
          best = id;
          bestDist = d;
        }
      }
    }

    return best;
  }

  // ── drag handlers ──
  void _onPointerDown(PointerDownEvent event) {
    final hit = _findHandle(event.localPosition);
    if (hit != null) {
      widget.onDragStart?.call();
      // Auto-switch layer if touching inactive layer handle
      final handleLyr = _handleLayer[hit];
      if (handleLyr != null && handleLyr != _activeLayer) {
        _switchLayer(handleLyr);
      }
      setState(() {
        _activeHandleId = hit;
        _showOnboarding = false;
        _showSolarLegend = false;
      });
    } else if (widget.showSolarContext) {
      // Tap on empty chart area toggles solar legend
      setState(() {
        _showSolarLegend = !_showSolarLegend;
        _showOnboarding = false;
      });
    }
  }

  void _onPointerMove(PointerMoveEvent event) {
    if (_activeHandleId != null) {
      _handleDrag(_activeHandleId!, event.localPosition);
    }
  }

  void _onPointerUp(PointerUpEvent event) {
    if (_activeHandleId != null) {
      setState(() => _activeHandleId = null);
      widget.onDragEnd?.call();
    }
  }

  void _onHover(PointerHoverEvent event) {
    final hit = _findHandle(event.localPosition);
    if (hit != _hoveredHandleId) {
      setState(() => _hoveredHandleId = hit);
    }
  }

  void _handleDrag(String id, Offset pos) {
    final l = _layout;
    if (l == null) return;
    final c = widget.config;
    final sr = widget.sunrise;
    final ss = widget.sunset;
    CurveConfigDto? nc;

    switch (id) {
      // Edge sliders
      case 'maxBri':
        nc = c.copyWith(
            maxBrightness:
                l.yToBri(pos.dy).round().clamp(c.minBrightness + 5, 100));
        break;
      case 'minBri':
        nc = c.copyWith(
            minBrightness:
                l.yToBri(pos.dy).round().clamp(0, c.maxBrightness - 5));
        break;
      case 'maxCCT':
        final v = (l.yToCCT(pos.dy) / 100).round() * 100;
        nc = c.copyWith(
            maxColorTemp: v.clamp(c.minColorTemp, 6500));
        break;
      case 'minCCT':
        final v = (l.yToCCT(pos.dy) / 100).round() * 100;
        nc = c.copyWith(
            minColorTemp: v.clamp(500, c.maxColorTemp));
        break;
      // Chart handles
      case 'widthLeftBri':
        final w = _widthFromDrag(l.xToHour(pos.dx), 'left', c, sr, ss);
        if (w != null) nc = c.copyWith(widthLeftBri: w);
        break;
      case 'widthRightBri':
        final w = _widthFromDrag(l.xToHour(pos.dx), 'right', c, sr, ss);
        if (w != null) nc = c.copyWith(widthRightBri: w);
        break;
      case 'widthLeftCCT':
        final w = _widthFromDrag(l.xToHour(pos.dx), 'left', c, sr, ss);
        if (w != null) nc = c.copyWith(widthLeftCct: w);
        break;
      case 'widthRightCCT':
        final w = _widthFromDrag(l.xToHour(pos.dx), 'right', c, sr, ss);
        if (w != null) nc = c.copyWith(widthRightCct: w);
        break;
    }
    if (nc != null) widget.onConfigChanged(nc);
  }

  // ── build ──
  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Expanded(child: _buildChartArea()),
        _buildReadout(),
      ],
    );
  }

  // ── Chart area ──
  Widget _buildChartArea() {
    return LayoutBuilder(
      builder: (context, constraints) {
        final size = Size(constraints.maxWidth, constraints.maxHeight);
        _layout = _ChartLayout(size);
        final positions = _computeHandlePositions();
        final currentHour =
            DateTime.now().hour + DateTime.now().minute / 60.0;

        return Stack(
          children: [
            Container(
              decoration: const BoxDecoration(
                gradient: RadialGradient(
                  center: Alignment(0, -0.3),
                  radius: 0.65,
                  colors: [Color(0xFF0E1028), Color(0xFF080910)],
                ),
              ),
            ),
            Listener(
              onPointerDown: _onPointerDown,
              onPointerMove: _onPointerMove,
              onPointerUp: _onPointerUp,
              child: MouseRegion(
                onHover: _onHover,
                cursor: _hoveredHandleId != null
                    ? SystemMouseCursors.grab
                    : SystemMouseCursors.basic,
                child: CustomPaint(
                  painter: _TunerPainter(
                    layout: _layout!,
                    config: widget.config,
                    curveData: widget.curveData,
                    sunrise: widget.sunrise,
                    sunset: widget.sunset,
                    currentHour: currentHour,
                    handlePositions: positions,
                    activeHandleId: _activeHandleId,
                    hoveredHandleId: _hoveredHandleId,
                    handleOpacity: _handleOpacity,
                    animProgress: _animProgress,
                    pulseValue: _pulseCtrl.value,
                    layerBlend: _layerBlend,
                    activeLayer: _activeLayer,
                    handleColorFn: _handleColor,
                    handleLabelFn: _handleLabel,
                    showSolarContext: widget.showSolarContext,
                    showSolarLegend: _showSolarLegend,
                  ),
                  size: size,
                ),
              ),
            ),
            if (_showOnboarding)
              Positioned.fill(
                child: IgnorePointer(
                  child: AnimatedOpacity(
                    opacity: _showOnboarding ? 1.0 : 0.0,
                    duration: const Duration(milliseconds: 800),
                    child: const Center(
                      child: Text(
                        'drag handles to shape the curve',
                        style: TextStyle(
                          color: Color(0x4DFFFFFF),
                          fontSize: 13,
                          fontWeight: FontWeight.w500,
                          letterSpacing: 0.5,
                        ),
                      ),
                    ),
                  ),
                ),
              ),
          ],
        );
      },
    );
  }

  // ── Readout bar ──
  Widget _buildReadout() {
    final c = widget.config;
    final isActive = _activeHandleId != null;
    final activeId = _activeHandleId;

    // Context-sensitive: show active parameter large, or summary when idle
    if (isActive && activeId != null) {
      final label = _handleLabel(activeId);
      final color = _handleColor(activeId);
      final name = _readoutName(activeId);
      return _readoutContainer(
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Text(
              name,
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.8,
                color: color.withValues(alpha: 0.6),
              ),
            ),
            const SizedBox(width: 10),
            Text(
              label,
              style: TextStyle(
                fontSize: 18,
                fontWeight: FontWeight.w600,
                fontFamily: 'monospace',
                color: color,
              ),
            ),
          ],
        ),
      );
    }

    // Idle summary
    final briText =
        '${c.minBrightness}\u2013${c.maxBrightness}%  '
        '\u2190${c.widthLeftBri.toStringAsFixed(2)} '
        '${c.widthRightBri.toStringAsFixed(2)}\u2192';
    final cctText =
        '${c.minColorTemp}\u2013${c.maxColorTemp}K  '
        '\u2190${c.widthLeftCct.toStringAsFixed(2)} '
        '${c.widthRightCct.toStringAsFixed(2)}\u2192';
    return _readoutContainer(
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          _rdGroup('BRI', briText, false, const Color(0xFFFFB74D)),
          const SizedBox(width: 20),
          _rdGroup('CCT', cctText, false, const Color(0xFF4DD0E1)),
        ],
      ),
    );
  }

  String _readoutName(String id) {
    switch (id) {
      case 'maxBri': return 'PEAK BRI';
      case 'minBri': return 'FLOOR BRI';
      case 'widthLeftBri': return 'MORNING RAMP';
      case 'widthRightBri': return 'EVENING RAMP';
      case 'maxCCT': return 'PEAK CCT';
      case 'minCCT': return 'FLOOR CCT';
      case 'widthLeftCCT': return 'MORNING CCT';
      case 'widthRightCCT': return 'EVENING CCT';
      default: return '';
    }
  }

  Widget _readoutContainer({required Widget child}) {
    return AnimatedSwitcher(
      duration: const Duration(milliseconds: 200),
      child: Container(
        key: ValueKey(_activeHandleId ?? 'idle'),
        height: 44,
        padding: const EdgeInsets.symmetric(horizontal: 16),
        decoration: const BoxDecoration(
          color: Color(0xD90C0D18),
          border: Border(top: BorderSide(color: Color(0x0FFFFFFF))),
        ),
        child: child,
      ),
    );
  }

  Widget _rdGroup(String label, String value, bool active, Color hlColor) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(label,
            style: const TextStyle(
              fontSize: 10,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.8,
              color: Color(0xFF5A6070),
            )),
        const SizedBox(width: 5),
        Text(value,
            style: TextStyle(
              fontSize: 11,
              fontFamily: 'monospace',
              color: active ? hlColor : const Color(0xFF8890A0),
            )),
      ],
    );
  }

}

// ═══════════════════════════════════════════════════════════════════
// TUNER PAINTER
// ═══════════════════════════════════════════════════════════════════

class _TunerPainter extends CustomPainter {
  final _ChartLayout layout;
  final CurveConfigDto config;
  final CurveData? curveData;
  final double sunrise, sunset;
  final double currentHour;
  final Map<String, Offset> handlePositions;
  final String? activeHandleId;
  final String? hoveredHandleId;
  final double handleOpacity;
  final double animProgress;
  final double pulseValue;
  final double layerBlend;
  final ChartLayer activeLayer;
  final Color Function(String id) handleColorFn;
  final String Function(String id) handleLabelFn;
  final bool showSolarContext;
  final bool showSolarLegend;

  _TunerPainter({
    required this.layout,
    required this.config,
    this.curveData,
    required this.sunrise,
    required this.sunset,
    required this.currentHour,
    required this.handlePositions,
    required this.activeHandleId,
    required this.hoveredHandleId,
    required this.handleOpacity,
    required this.animProgress,
    required this.pulseValue,
    required this.layerBlend,
    required this.activeLayer,
    required this.handleColorFn,
    required this.handleLabelFn,
    required this.showSolarContext,
    required this.showSolarLegend,
  });

  double get _solarNoon => (sunrise + sunset) / 2;

  @override
  void paint(Canvas canvas, Size size) {
    _drawGrid(canvas);
    _drawSolarMarkers(canvas);
    _drawAxisLabels(canvas);
    _drawCurves(canvas);
    _drawEdgeSliders(canvas);
    _drawNowMarker(canvas);
    _drawHandles(canvas);
    if (showSolarLegend) _drawSolarLegend(canvas);
  }

  @override
  bool shouldRepaint(_TunerPainter old) => true;

  // ── FFI data interpolation ──
  double _interpolateBri(double hour) =>
      _interpBri(hour, curveData, config.minBrightness.toDouble());
  double _interpolateKelvin(double hour) =>
      _interpKelvin(hour, curveData, config.minColorTemp.toDouble());

  // ── grid with subtle vignette ──
  void _drawGrid(Canvas canvas) {
    final paint = Paint()..strokeWidth = 1;
    // horizontal
    for (int v = 0; v <= 100; v += 25) {
      paint.color = Color(v % 50 == 0 ? 0x0AFFFFFF : 0x05FFFFFF);
      canvas.drawLine(
          Offset(layout.left, layout.briToY(v.toDouble())),
          Offset(layout.right, layout.briToY(v.toDouble())),
          paint);
    }
    // Emphasized 100% ceiling line
    canvas.drawLine(
      Offset(layout.left, layout.briToY(100)),
      Offset(layout.right, layout.briToY(100)),
      Paint()
        ..color = const Color(0x18FFB74D)
        ..strokeWidth = 1,
    );
    // vertical — every hour, 0–24
    for (int h = 0; h <= 24; h++) {
      final x = layout.hourToX(h.toDouble());
      if (x < layout.left || x > layout.right) continue;
      paint.color = Color(h % 6 == 0 ? 0x0AFFFFFF : h % 3 == 0 ? 0x07FFFFFF : 0x03FFFFFF);
      canvas.drawLine(Offset(x, layout.top), Offset(x, layout.bottom), paint);
    }

    // Subtle corner vignette
    final vigPaint = Paint();
    final corners = [
      Offset(layout.left, layout.top),
      Offset(layout.right, layout.top),
      Offset(layout.left, layout.bottom),
      Offset(layout.right, layout.bottom),
    ];
    for (final corner in corners) {
      vigPaint.shader = ui.Gradient.radial(
        corner, layout.chartWidth * 0.3,
        [const Color(0x00080910), const Color(0x18080910)],
      );
      canvas.drawRect(
        Rect.fromLTRB(layout.left, layout.top, layout.right, layout.bottom),
        vigPaint,
      );
    }
  }

  static String _fmtHour(double h) {
    final hh = h.floor();
    final mm = ((h - hh) * 60).round();
    return '$hh:${mm.toString().padLeft(2, '0')}';
  }

  // ── solar markers with gradient fade ──
  void _drawSolarMarkers(Canvas canvas) {
    if (showSolarContext) {
      _drawTwilightBands(canvas);
    }

    // Primary markers — always shown
    final marks = [
      (sunrise, const Color(0xFFFF8C5A), 'RISE'),
      (sunset, const Color(0xFFE06070), 'SET'),
      (_solarNoon, const Color(0xFFFFD54F), 'NOON'),
    ];
    for (final (hr, col, lbl) in marks) {
      final x = layout.hourToX(hr);
      final isNoon = lbl == 'NOON';

      final linePaint = Paint()
        ..strokeWidth = isNoon ? 1.5 : 1
        ..shader = ui.Gradient.linear(
          Offset(x, layout.top),
          Offset(x, layout.bottom),
          [
            col.withValues(alpha: 0.0),
            col.withValues(alpha: isNoon ? 0.25 : 0.15),
            col.withValues(alpha: isNoon ? 0.25 : 0.15),
            col.withValues(alpha: 0.0),
          ],
          [0.0, 0.15, 0.85, 1.0],
        );
      canvas.drawLine(
          Offset(x, layout.top), Offset(x, layout.bottom), linePaint);

      _paintText(canvas, lbl, x, layout.top - 6,
          TextStyle(
            fontSize: 9,
            fontWeight: FontWeight.w500,
            color: col.withValues(alpha: 0.35),
          ));

      // Time annotation below label
      if (showSolarContext) {
        _paintText(canvas, _fmtHour(hr), x, layout.top - 16,
            TextStyle(
              fontSize: 8,
              fontFamily: 'monospace',
              fontWeight: FontWeight.w400,
              color: col.withValues(alpha: 0.25),
            ));
      }
    }
  }

  // ── twilight bands and phase marker lines ──
  void _drawTwilightBands(Canvas canvas) {
    final solar = curveData?.solar;
    if (solar == null) return;

    // Draw twilight zone fill bands
    _drawTwilightZones(canvas, solar);

    // Draw thin marker lines at each twilight boundary (no text — legend handles that)
    final phases = <(double, Color)>[];
    final dawn = solar.dawn;
    final dusk = solar.dusk;
    if (dawn != null) {
      if (dawn.astronomical != null) phases.add((dawn.astronomical!, const Color(0xFF5C6BC0)));
      if (dawn.nautical != null) phases.add((dawn.nautical!, const Color(0xFF7986CB)));
      if (dawn.civil != null) phases.add((dawn.civil!, const Color(0xFFFF8A65)));
    }
    if (dusk != null) {
      if (dusk.civil != null) phases.add((dusk.civil!, const Color(0xFFFF8A65)));
      if (dusk.nautical != null) phases.add((dusk.nautical!, const Color(0xFF7986CB)));
      if (dusk.astronomical != null) phases.add((dusk.astronomical!, const Color(0xFF5C6BC0)));
    }

    for (final (hr, col) in phases) {
      final x = layout.hourToX(hr);
      final linePaint = Paint()
        ..strokeWidth = 0.5
        ..shader = ui.Gradient.linear(
          Offset(x, layout.top),
          Offset(x, layout.bottom),
          [
            col.withValues(alpha: 0.0),
            col.withValues(alpha: 0.08),
            col.withValues(alpha: 0.08),
            col.withValues(alpha: 0.0),
          ],
          [0.0, 0.2, 0.8, 1.0],
        );
      canvas.drawLine(
          Offset(x, layout.top), Offset(x, layout.bottom), linePaint);
    }
  }

  void _drawTwilightZones(Canvas canvas, SolarInfo solar) {
    final dawn = solar.dawn;
    final dusk = solar.dusk;

    // Build ordered edge list for dawn side: astro → nautical → civil → sunrise
    final dawnEdges = <double>[];
    if (dawn?.astronomical != null) dawnEdges.add(dawn!.astronomical!);
    if (dawn?.nautical != null) dawnEdges.add(dawn!.nautical!);
    if (dawn?.civil != null) dawnEdges.add(dawn!.civil!);
    dawnEdges.add(sunrise);

    // Build ordered edge list for dusk side: sunset → civil → nautical → astro
    final duskEdges = <double>[sunset];
    if (dusk?.civil != null) duskEdges.add(dusk!.civil!);
    if (dusk?.nautical != null) duskEdges.add(dusk!.nautical!);
    if (dusk?.astronomical != null) duskEdges.add(dusk!.astronomical!);

    // Band colors: deepest twilight → shallowest
    const bandColors = [
      Color(0xFF1A1540), // astronomical (deep indigo)
      Color(0xFF1E2050), // nautical (navy)
      Color(0xFF2A3060), // civil (slate)
    ];

    // Draw dawn bands (right to left: deepest first)
    for (int i = 0; i < dawnEdges.length - 1 && i < bandColors.length; i++) {
      final left = layout.hourToX(dawnEdges[i]);
      final right = layout.hourToX(dawnEdges[i + 1]);
      final bandIdx = dawnEdges.length - 2 - i; // reverse: deepest band first
      final col = bandColors[bandIdx.clamp(0, bandColors.length - 1)];
      canvas.drawRect(
        Rect.fromLTRB(left, layout.top, right, layout.bottom),
        Paint()..color = col.withValues(alpha: 0.12),
      );
    }

    // Draw dusk bands (left to right: shallowest first)
    for (int i = 0; i < duskEdges.length - 1 && i < bandColors.length; i++) {
      final left = layout.hourToX(duskEdges[i]);
      final right = layout.hourToX(duskEdges[i + 1]);
      final col = bandColors[i.clamp(0, bandColors.length - 1)];
      canvas.drawRect(
        Rect.fromLTRB(left, layout.top, right, layout.bottom),
        Paint()..color = col.withValues(alpha: 0.12),
      );
    }
  }

  // ── solar times legend (tap to reveal) ──
  void _drawSolarLegend(Canvas canvas) {
    final solar = curveData?.solar;
    if (solar == null) return;

    // Build rows: (label, time, color)
    final rows = <(String, double, Color)>[];
    final dawn = solar.dawn;
    final dusk = solar.dusk;

    if (dawn?.astronomical != null) {
      rows.add(('ASTRO DAWN', dawn!.astronomical!, const Color(0xFF5C6BC0)));
    }
    if (dawn?.nautical != null) {
      rows.add(('NAUT DAWN', dawn!.nautical!, const Color(0xFF7986CB)));
    }
    if (dawn?.civil != null) {
      rows.add(('CIVIL DAWN', dawn!.civil!, const Color(0xFFFF8A65)));
    }
    rows.add(('SUNRISE', sunrise, const Color(0xFFFF8C5A)));
    rows.add(('SOLAR NOON', _solarNoon, const Color(0xFFFFD54F)));
    rows.add(('SUNSET', sunset, const Color(0xFFE06070)));
    if (dusk?.civil != null) {
      rows.add(('CIVIL DUSK', dusk!.civil!, const Color(0xFFFF8A65)));
    }
    if (dusk?.nautical != null) {
      rows.add(('NAUT DUSK', dusk!.nautical!, const Color(0xFF7986CB)));
    }
    if (dusk?.astronomical != null) {
      rows.add(('ASTRO DUSK', dusk!.astronomical!, const Color(0xFF5C6BC0)));
    }

    // Layout constants
    const rowHeight = 16.0;
    const padH = 12.0;
    const padV = 10.0;
    const titleHeight = 18.0;
    final panelHeight = titleHeight + padV * 2 + rows.length * rowHeight;
    const panelWidth = 170.0;

    // Position: top-right of chart area, inset
    final px = layout.right - panelWidth - 8;
    final py = layout.top + 8;
    final panelRect = RRect.fromRectAndRadius(
      Rect.fromLTWH(px, py, panelWidth, panelHeight),
      const Radius.circular(6),
    );

    // Shadow
    canvas.drawRRect(
      panelRect.shift(const Offset(0, 2)),
      Paint()..color = const Color(0x50000000),
    );

    // Frosted background
    canvas.drawRRect(
      panelRect,
      Paint()..color = const Color(0xE80A0B16),
    );

    // Border
    canvas.drawRRect(
      panelRect,
      Paint()
        ..color = const Color(0x18FFFFFF)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 0.5,
    );

    // Title
    _paintText(canvas, 'SOLAR TIMES', px + panelWidth / 2, py + padV + 4,
        const TextStyle(
          fontSize: 9,
          fontWeight: FontWeight.w600,
          letterSpacing: 1.2,
          color: Color(0x60FFFFFF),
        ));

    // Rows
    final labelStyle = const TextStyle(
      fontSize: 9,
      fontWeight: FontWeight.w400,
      letterSpacing: 0.3,
      fontFamily: 'monospace',
    );
    final timeStyle = const TextStyle(
      fontSize: 9,
      fontWeight: FontWeight.w500,
      fontFamily: 'monospace',
    );

    for (int i = 0; i < rows.length; i++) {
      final (label, hour, color) = rows[i];
      final ry = py + padV + titleHeight + i * rowHeight + rowHeight / 2;

      // Color dot
      canvas.drawCircle(
        Offset(px + padH + 3, ry),
        2.5,
        Paint()..color = color.withValues(alpha: 0.7),
      );

      // Label (left-aligned)
      final ltp = TextPainter(
        text: TextSpan(
          text: label,
          style: labelStyle.copyWith(color: color.withValues(alpha: 0.55)),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      ltp.paint(canvas, Offset(px + padH + 10, ry - ltp.height / 2));

      // Time (right-aligned)
      final ttp = TextPainter(
        text: TextSpan(
          text: _fmtHour(hour),
          style: timeStyle.copyWith(color: color.withValues(alpha: 0.8)),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      ttp.paint(canvas,
          Offset(px + panelWidth - padH - ttp.width, ry - ttp.height / 2));
    }
  }

  // ── axis labels ──
  void _drawAxisLabels(Canvas canvas) {
    final briStyle = const TextStyle(
        fontSize: 10,
        fontFamily: 'monospace',
        color: Color(0x59FFB74D));
    for (int v = 0; v <= 100; v += 25) {
      _paintText(canvas, '$v%', layout.left - 8, layout.briToY(v.toDouble()),
          briStyle,
          alignX: 1.0);
    }

    // Right: kelvin labels along CCT scale
    final kStyle = const TextStyle(
        fontSize: 10, fontFamily: 'monospace', color: Color(0x2EFFFFFF));
    for (final k in [2000, 4000, 6000]) {
      _paintText(
          canvas,
          '${k ~/ 1000}k',
          layout.cctScaleRight + 4,
          layout.cctToY(k.toDouble()),
          kStyle,
          alignX: -1.0);
    }

    // Bottom: hours — every 3h across 0–24
    final hStyle = const TextStyle(
        fontSize: 10, fontFamily: 'monospace', color: Color(0x38FFFFFF));
    for (int h = 0; h <= 24; h += 3) {
      final x = layout.hourToX(h.toDouble());
      if (x < layout.left + 5 || x > layout.right - 5) continue;
      _paintText(canvas, '$h', x, layout.bottom + 10, hStyle);
    }
  }

  // ═══════════════════════════════════════════════════════════════════
  // 7-PASS LUMINOUS CURVE RENDERING
  // ═══════════════════════════════════════════════════════════════════

  void _drawCurves(Canvas canvas) {
    final cd = curveData;
    if (cd == null || cd.hours.length < 2) return;

    // Full 24h curve: (hour, brightness, kelvin)
    final pts = <(double, double, double)>[];
    for (int i = 0; i < cd.hours.length; i++) {
      pts.add((cd.hours[i], cd.brightness[i].toDouble(), cd.kelvin[i].toDouble()));
    }

    _drawLuminousCurve(canvas, pts);
  }

  void _drawLuminousCurve(Canvas canvas, List<(double, double, double)> pts) {
    if (pts.length < 2) return;

    // Build path for the BRI curve
    final path = Path();
    final fillPath = Path()..moveTo(layout.hourToX(pts.first.$1), layout.bottom);

    for (int i = 0; i < pts.length; i++) {
      final (h, b, _) = pts[i];
      final v = config.minBrightness +
          (b - config.minBrightness) * animProgress;
      final x = layout.hourToX(h);
      final y = layout.briToY(v);
      if (i == 0) {
        path.moveTo(x, y);
        fillPath.lineTo(x, y);
      } else {
        path.lineTo(x, y);
        fillPath.lineTo(x, y);
      }
    }
    fillPath
      ..lineTo(layout.hourToX(pts.last.$1), layout.bottom)
      ..close();

    // Gradient fill under the curve
    final warmColor = _kelvinToColor(_interpolateKelvin(_solarNoon));
    final coolColor = _kelvinToColor(config.minColorTemp.toDouble());
    canvas.drawPath(
      fillPath,
      Paint()
        ..shader = ui.Gradient.linear(
          Offset(0, layout.top),
          Offset(0, layout.bottom),
          [
            warmColor.withValues(alpha: 0.12),
            coolColor.withValues(alpha: 0.02),
          ],
        ),
    );

    // Solid CCT-colored stroke
    for (int i = 0; i < pts.length - 1; i++) {
      final (h1, b1, k1) = pts[i];
      final (h2, b2, k2) = pts[i + 1];
      final v1 = config.minBrightness + (b1 - config.minBrightness) * animProgress;
      final v2 = config.minBrightness + (b2 - config.minBrightness) * animProgress;
      final midK = (k1 + k2) / 2;
      canvas.drawLine(
        Offset(layout.hourToX(h1), layout.briToY(v1)),
        Offset(layout.hourToX(h2), layout.briToY(v2)),
        Paint()
          ..strokeWidth = 15
          ..strokeCap = StrokeCap.round
          ..color = _kelvinToColor(midK),
      );
    }
  }

  // ═══════════════════════════════════════════════════════════════════
  // EDGE SLIDERS
  // ═══════════════════════════════════════════════════════════════════

  void _drawEdgeSliders(Canvas canvas) {
    _drawBriSlider(canvas);
    _drawCCTScale(canvas);
  }

  /// Left-edge brightness min/max slider
  void _drawBriSlider(Canvas canvas) {
    final x = layout.briSliderX;

    // Track line
    canvas.drawLine(
      Offset(x, layout.top), Offset(x, layout.bottom),
      Paint()
        ..color = const Color(0x0DFFFFFF)
        ..strokeWidth = 2
        ..strokeCap = StrokeCap.round,
    );

    // Active range highlight
    final maxY = layout.briToY(config.maxBrightness.toDouble());
    final minY = layout.briToY(config.minBrightness.toDouble());
    canvas.drawLine(
      Offset(x, maxY), Offset(x, minY),
      Paint()
        ..color = const Color(0xFFFFB74D).withValues(alpha: 0.25)
        ..strokeWidth = 3
        ..strokeCap = StrokeCap.round,
    );

    // Thumbs
    _drawCapsuleThumb(canvas, Offset(x, maxY), 'maxBri', const Color(0xFFFFB74D));
    _drawCapsuleThumb(canvas, Offset(x, minY), 'minBri', const Color(0xFFFF8A65));
  }

  void _drawCapsuleThumb(Canvas canvas, Offset pos, String id, Color color) {
    final isActive = id == activeHandleId;
    final isHovered = id == hoveredHandleId;
    final w = isActive ? 18.0 : isHovered ? 16.0 : 14.0;
    final h = isActive ? 10.0 : isHovered ? 9.0 : 8.0;
    final alpha = handleOpacity;

    final rect = RRect.fromRectAndRadius(
      Rect.fromCenter(center: pos, width: w, height: h),
      Radius.circular(h / 2),
    );

    if (isActive || isHovered) {
      canvas.drawRRect(
        RRect.fromRectAndRadius(
          Rect.fromCenter(center: pos, width: w + 6, height: h + 6),
          Radius.circular((h + 6) / 2),
        ),
        Paint()..color = color.withValues(alpha: 0.2 * alpha),
      );
    }

    canvas.drawRRect(rect,
        Paint()..color = const Color(0xFF1A1B2E).withValues(alpha: 0.9 * alpha));
    canvas.drawRRect(rect, Paint()
      ..color = (isActive ? Colors.white : color).withValues(alpha: 0.5 * alpha)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5);
    canvas.drawLine(
      Offset(pos.dx - 3, pos.dy), Offset(pos.dx + 3, pos.dy),
      Paint()
        ..color = color.withValues(alpha: 0.6 * alpha)
        ..strokeWidth = 1.5
        ..strokeCap = StrokeCap.round,
    );

    if ((isActive || isHovered) && alpha > 0.3) {
      final label = handleLabelFn(id);
      final tp = TextPainter(
          text: TextSpan(text: label, style: TextStyle(
            fontSize: 11, fontWeight: FontWeight.w500, fontFamily: 'monospace',
            color: (isActive ? Colors.white : color).withValues(alpha: alpha),
          )),
          textDirection: TextDirection.ltr)
        ..layout();
      final lx = pos.dx + 14;
      final ly = pos.dy - 10;
      final bgRect = RRect.fromRectAndRadius(
          Rect.fromLTWH(lx - 4, ly, tp.width + 8, 20),
          const Radius.circular(4));
      canvas.drawRRect(bgRect, Paint()..color = const Color(0xE0080910));
      canvas.drawRRect(bgRect, Paint()
        ..color = color.withValues(alpha: 0.3 * alpha)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1);
      tp.paint(canvas, Offset(lx, ly + 10 - tp.height / 2));
    }
  }

  /// Right-edge CCT color scale bar (interactive)
  void _drawCCTScale(Canvas canvas) {
    final scaleLeft = layout.cctScaleLeft;
    final scaleRight = layout.cctScaleRight;
    final p = Paint();

    // Draw color scale
    for (double y = layout.top; y <= layout.bottom; y += 2) {
      final k = layout.yToCCT(y);
      final inRange =
          k >= config.minColorTemp && k <= config.maxColorTemp;
      p.color = _kelvinToColor(k).withValues(alpha: inRange ? 0.55 : 0.1);
      canvas.drawRect(
          Rect.fromLTWH(scaleLeft, y, scaleRight - scaleLeft, 2), p);
    }

    // Range bracket
    final yTop = layout.cctToY(config.maxColorTemp.toDouble());
    final yBot = layout.cctToY(config.minColorTemp.toDouble());
    canvas.drawLine(
      Offset(scaleLeft - 2, yTop),
      Offset(scaleLeft - 2, yBot),
      Paint()
        ..color = const Color(0x26FFFFFF)
        ..strokeWidth = 1,
    );

    // Diamond thumbs for max/min CCT
    _drawDiamondThumb(canvas, Offset(layout.cctScaleCenterX, yTop), 'maxCCT',
        _kelvinToColor(config.maxColorTemp.toDouble()));
    _drawDiamondThumb(canvas, Offset(layout.cctScaleCenterX, yBot), 'minCCT',
        _kelvinToColor(config.minColorTemp.toDouble()));
  }

  void _drawDiamondThumb(Canvas canvas, Offset pos, String id, Color color) {
    final isActive = id == activeHandleId;
    final isHovered = id == hoveredHandleId;
    final r = isActive ? 8.0 : isHovered ? 7.0 : 6.0;
    final alpha = handleOpacity;

    // Glow
    if (isActive || isHovered) {
      _drawDiamond(canvas, pos, r + 4,
          Paint()..color = color.withValues(alpha: 0.2 * alpha));
    }

    // Frosted body
    _drawDiamond(canvas, pos, r,
        Paint()..color = const Color(0xFF1A1B2E).withValues(alpha: 0.9 * alpha));

    // Border
    _drawDiamond(canvas, pos, r, Paint()
      ..color = (isActive ? Colors.white : color).withValues(alpha: 0.5 * alpha)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5);

    // Center dot
    _drawDiamond(canvas, pos, r * 0.3,
        Paint()..color = color.withValues(alpha: 0.7 * alpha));

    // Floating label
    if ((isActive || isHovered) && alpha > 0.3) {
      final label = handleLabelFn(id);
      final style = TextStyle(
        fontSize: 11,
        fontWeight: FontWeight.w500,
        fontFamily: 'monospace',
        color: (isActive ? Colors.white : color).withValues(alpha: alpha),
      );
      final tp = TextPainter(
          text: TextSpan(text: label, style: style),
          textDirection: TextDirection.ltr)
        ..layout();
      final lx = pos.dx - tp.width - 16;
      final ly = pos.dy - 10;
      final bgRect = RRect.fromRectAndRadius(
          Rect.fromLTWH(lx - 4, ly, tp.width + 8, 20),
          const Radius.circular(4));
      canvas.drawRRect(bgRect, Paint()..color = const Color(0xE0080910));
      canvas.drawRRect(
          bgRect,
          Paint()
            ..color = color.withValues(alpha: 0.3 * alpha)
            ..style = PaintingStyle.stroke
            ..strokeWidth = 1);
      tp.paint(canvas, Offset(lx, ly + 10 - tp.height / 2));
    }
  }

  // ── now marker (breathing pulse) ──
  void _drawNowMarker(Canvas canvas) {
    final x = layout.hourToX(currentHour);

    // Gradient-fade vertical line
    canvas.drawLine(
      Offset(x, layout.top),
      Offset(x, layout.bottom),
      Paint()
        ..strokeWidth = 1
        ..shader = ui.Gradient.linear(
          Offset(x, layout.top),
          Offset(x, layout.bottom),
          [
            const Color(0x00FFFFFF),
            const Color(0x1AFFFFFF),
            const Color(0x1AFFFFFF),
            const Color(0x00FFFFFF),
          ],
          [0.0, 0.2, 0.8, 1.0],
        ),
    );

    // Dot on BRI curve
    final bri = _interpolateBri(currentHour);
    final cct = _interpolateKelvin(currentHour);
    final pulse = 0.5 + 0.5 * sin(pulseValue * pi);
    final dotY = layout.briToY(bri);
    final dotColor = _kelvinToColor(cct);

    // Breathing glow ring
    canvas.drawCircle(
        Offset(x, dotY),
        6 + pulse * 3,
        Paint()
          ..color = dotColor.withValues(alpha: 0.06 + pulse * 0.06));
    canvas.drawCircle(
        Offset(x, dotY),
        4 + pulse * 1,
        Paint()
          ..color = dotColor.withValues(alpha: 0.12 + pulse * 0.08));

    // Solid dot
    canvas.drawCircle(Offset(x, dotY), 3, Paint()..color = dotColor);
    // White center
    canvas.drawCircle(Offset(x, dotY), 1.2,
        Paint()..color = Colors.white.withValues(alpha: 0.6));

    // Label
    _paintText(canvas, 'NOW', x, layout.bottom + 20,
        const TextStyle(
            fontSize: 8,
            fontWeight: FontWeight.w500,
            color: Color(0x40FFFFFF)));
  }

  // ═══════════════════════════════════════════════════════════════════
  // GLASSMORPHIC HANDLES
  // ═══════════════════════════════════════════════════════════════════

  void _drawHandles(Canvas canvas) {
    if (handleOpacity <= 0) return;

    // Draw inactive layer handles first (ghost)
    final inactiveHandles = activeLayer == ChartLayer.brightness
        ? _cctHandleIds
        : _briHandleIds;
    for (final id in inactiveHandles) {
      final pos = handlePositions[id];
      if (pos == null) continue;
      _drawGlassHandle(canvas, pos, id, isInactiveLayer: true);
    }

    // Draw active layer handles on top
    final activeHandles = activeLayer == ChartLayer.brightness
        ? _briHandleIds
        : _cctHandleIds;
    for (final id in activeHandles) {
      final pos = handlePositions[id];
      if (pos == null) continue;
      _drawGlassHandle(canvas, pos, id, isInactiveLayer: false);
    }
  }

  void _drawGlassHandle(Canvas canvas, Offset pos, String id,
      {required bool isInactiveLayer}) {
    final isActive = id == activeHandleId;
    final isHovered = id == hoveredHandleId;
    final color = handleColorFn(id);

    // Inactive layer: small, dim, no interaction visuals
    if (isInactiveLayer && !isActive) {
      final ghostAlpha = handleOpacity * 0.2;
      canvas.drawCircle(pos, 5,
          Paint()..color = const Color(0xFF1A1B2E).withValues(alpha: ghostAlpha));
      canvas.drawCircle(pos, 5, Paint()
        ..color = color.withValues(alpha: ghostAlpha * 0.5)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1);
      return;
    }

    // Size states: idle 10, hover 12, active 14
    final r = isActive ? 14.0 : isHovered ? 12.0 : 10.0;
    final alpha = handleOpacity;

    // Constraint guide line when active
    if (isActive) {
      final isHorizontal = id == 'widthLeftBri' || id == 'widthRightBri' ||
          id == 'widthLeftCCT' || id == 'widthRightCCT';
      final guidePaint = Paint()
        ..color = color.withValues(alpha: 0.1 * alpha)
        ..strokeWidth = 1;
      if (isHorizontal) {
        _drawDashedLine(canvas,
            Offset(layout.left, pos.dy), Offset(layout.right, pos.dy),
            guidePaint, dashLen: 3, gapLen: 3);
      }
      // Vertical guide for width handles
      if (id.startsWith('width')) {
        _drawDashedLine(canvas,
            Offset(pos.dx, layout.top), Offset(pos.dx, layout.bottom),
            guidePaint, dashLen: 3, gapLen: 3);
      }
    }

    // Accent glow (active/hovered)
    if (isActive || isHovered) {
      canvas.drawCircle(pos, r + 6,
          Paint()..color = color.withValues(alpha: 0.15 * alpha));
    }

    // Frosted glass body
    canvas.drawCircle(pos, r,
        Paint()..color = const Color(0xFF1A1B2E).withValues(alpha: 0.85 * alpha));

    // Frosted ring (white at low opacity)
    canvas.drawCircle(pos, r, Paint()
      ..color = (isActive ? Colors.white : Colors.white.withValues(alpha: 0.15))
          .withValues(alpha: alpha * (isActive ? 0.9 : isHovered ? 0.4 : 0.15))
      ..style = PaintingStyle.stroke
      ..strokeWidth = isActive ? 2.0 : 1.5);

    // Muted accent center
    canvas.drawCircle(pos, r * 0.35,
        Paint()..color = color.withValues(alpha: alpha * (isActive ? 0.9 : 0.4)));

    // Chevron direction indicators (fade in on hover/active)
    if ((isActive || isHovered) && alpha > 0.5) {
      final isHorizontal = id == 'widthLeftBri' || id == 'widthRightBri' ||
          id == 'widthLeftCCT' || id == 'widthRightCCT';
      final chevStyle = TextStyle(
          fontSize: 8,
          fontFamily: 'monospace',
          color: color.withValues(alpha: 0.4));
      if (isHorizontal) {
        _paintText(canvas, '\u25C0', pos.dx - r - 7, pos.dy, chevStyle);
        _paintText(canvas, '\u25B6', pos.dx + r + 7, pos.dy, chevStyle);
      } else {
        _paintText(canvas, '\u25B2', pos.dx, pos.dy - r - 6, chevStyle);
        _paintText(canvas, '\u25BC', pos.dx, pos.dy + r + 6, chevStyle);
      }
    }

    // Floating label with shadow
    if ((isActive || isHovered) && alpha > 0.3) {
      _drawHandleLabel(canvas, pos, id, r, color, isActive, alpha);
    }
  }

  void _drawHandleLabel(Canvas canvas, Offset pos, String id, double r,
      Color color, bool isActive, double alpha) {
    final label = handleLabelFn(id);
    final style = TextStyle(
      fontSize: 11,
      fontWeight: FontWeight.w500,
      fontFamily: 'monospace',
      color: (isActive ? Colors.white : color).withValues(alpha: alpha),
    );
    final tp = TextPainter(
        text: TextSpan(text: label, style: style),
        textDirection: TextDirection.ltr)
      ..layout();

    double lx = pos.dx - tp.width / 2 - 6;
    double ly = pos.dy - r - 26;

    // Keep label within chart bounds
    if (ly < layout.top) ly = pos.dy + r + 8;
    if (lx < layout.left) lx = layout.left;
    if (lx + tp.width + 12 > layout.right) lx = layout.right - tp.width - 12;

    final rect = RRect.fromRectAndRadius(
        Rect.fromLTWH(lx, ly, tp.width + 12, 20),
        const Radius.circular(4));

    // Shadow
    canvas.drawRRect(
        rect.shift(const Offset(0, 2)),
        Paint()..color = const Color(0x40000000));

    // Background
    canvas.drawRRect(rect, Paint()..color = const Color(0xE0080910));

    // Border
    canvas.drawRRect(
        rect,
        Paint()
          ..color = color.withValues(alpha: 0.33 * alpha)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1);

    tp.paint(canvas, Offset(lx + 6, ly + 10 - tp.height / 2));
  }

  // ── helpers ──
  void _drawDashedLine(Canvas canvas, Offset a, Offset b, Paint paint,
      {double dashLen = 4, double gapLen = 4}) {
    final dx = b.dx - a.dx;
    final dy = b.dy - a.dy;
    final total = sqrt(dx * dx + dy * dy);
    if (total == 0) return;
    final ux = dx / total;
    final uy = dy / total;
    double pos = 0;
    while (pos < total) {
      final end = min(pos + dashLen, total);
      canvas.drawLine(Offset(a.dx + ux * pos, a.dy + uy * pos),
          Offset(a.dx + ux * end, a.dy + uy * end), paint);
      pos += dashLen + gapLen;
    }
  }

  void _drawDiamond(Canvas canvas, Offset c, double r, Paint paint) {
    final path = Path()
      ..moveTo(c.dx, c.dy - r)
      ..lineTo(c.dx + r * 0.85, c.dy)
      ..lineTo(c.dx, c.dy + r)
      ..lineTo(c.dx - r * 0.85, c.dy)
      ..close();
    canvas.drawPath(path, paint);
  }

  void _paintText(Canvas canvas, String text, double x, double y,
      TextStyle style,
      {double alignX = 0, double alignY = 0}) {
    final tp = TextPainter(
        text: TextSpan(text: text, style: style),
        textDirection: TextDirection.ltr)
      ..layout();
    tp.paint(
        canvas,
        Offset(
          x - tp.width * (alignX + 1) / 2,
          y - tp.height * (alignY + 1) / 2,
        ));
  }
}
