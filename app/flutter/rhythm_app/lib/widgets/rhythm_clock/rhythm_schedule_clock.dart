import 'dart:math' as math;

import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmMode;

import '../solar_clock/solar_clock_exports.dart';

/// A schedule point the orb can pin to — a solar event (sunrise, civil dusk,
/// …) with its resolved hour for the current day and location.
class TriggerAnchor {
  final String event;
  final String label;
  final double hour;

  const TriggerAnchor({
    required this.event,
    required this.label,
    required this.hour,
  });
}

/// Per-mode ring styling: samples of the mode's lighting curve (color +
/// brightness across the day) with a flat fallback when no curve is available.
class ModeCurveVisual {
  final SolarCurveSamples? samples;
  final Color fallbackColor;
  final int fallbackBrightness;

  const ModeCurveVisual({
    this.samples,
    required this.fallbackColor,
    this.fallbackBrightness = 50,
  });

  (Color, double) styleAt(double hour) {
    final curveSamples = samples;
    if (curveSamples == null || curveSamples.isEmpty) {
      final opacity =
          0.18 + (fallbackBrightness.clamp(0, 100).toDouble() / 100) * 0.55;
      return (fallbackColor, opacity);
    }

    final brightness = SolarUtils.interpolateValue(
      curveSamples.hours,
      curveSamples.brightness,
      hour,
    );
    final color = SolarUtils.curveColorAt(
      hour,
      hours: curveSamples.hours,
      kelvin: curveSamples.kelvin,
      fallback: fallbackColor,
    );
    final opacity = 0.18 + (brightness / 100) * 0.55;
    return (color, opacity);
  }
}

/// Live drag state reported to the host while an orb is being dragged, so
/// surrounding chrome (e.g. summary chips) can echo the preview time and any
/// solar anchor the orb is locking onto. `null` is reported when the drag
/// ends.
class RhythmClockDragPreview {
  final RhythmMode mode;
  final double hour;
  final TriggerAnchor? proximateAnchor;
  final double proximity;

  const RhythmClockDragPreview({
    required this.mode,
    required this.hour,
    required this.proximateAnchor,
    required this.proximity,
  });
}

/// The dawn/dusk anchor ladder for a mode, resolved from today's solar data.
List<TriggerAnchor> solarAnchorsForMode(RhythmMode mode, SolarClockData data) {
  final tw = data.twilightTimes;
  final st = data.sunTimes;

  if (mode == RhythmMode.day) {
    return [
      if (tw.dawn.astronomical != null)
        TriggerAnchor(
          event: 'astronomical_twilight',
          label: 'Astro Dawn',
          hour: tw.dawn.astronomical!,
        ),
      if (tw.dawn.nautical != null)
        TriggerAnchor(
          event: 'nautical_twilight',
          label: 'Nautical Dawn',
          hour: tw.dawn.nautical!,
        ),
      if (tw.dawn.civil != null)
        TriggerAnchor(
          event: 'civil_twilight',
          label: 'Civil Dawn',
          hour: tw.dawn.civil!,
        ),
      TriggerAnchor(
        event: 'sunrise',
        label: 'Sunrise',
        hour: st.sunrise,
      ),
    ];
  }

  return [
    TriggerAnchor(
      event: 'sunset',
      label: 'Sunset',
      hour: st.sunset,
    ),
    if (tw.dusk.civil != null)
      TriggerAnchor(
        event: 'civil_twilight',
        label: 'Civil Dusk',
        hour: tw.dusk.civil!,
      ),
    if (tw.dusk.nautical != null)
      TriggerAnchor(
        event: 'nautical_twilight',
        label: 'Nautical Dusk',
        hour: tw.dusk.nautical!,
      ),
    if (tw.dusk.astronomical != null)
      TriggerAnchor(
        event: 'astronomical_twilight',
        label: 'Astro Dusk',
        hour: tw.dusk.astronomical!,
      ),
  ];
}

/// The interactive orbital schedule editor shared by the whole-house Alarm
/// Schedule screen and the per-room Schedule tab: the 24-hour rhythm ring
/// painted from each mode's lighting curve, two draggable Day/Sleep orbs, and
/// the solar marker ladder. Only what a commit *means* differs between hosts
/// (solar/scheduled trigger vs. fixed room times), decided in
/// [onHourCommitted].
///
/// Markers are first-class targets: dragging near one snaps onto it, and
/// **tapping a marker pins the selected orb to it exactly** — no drag
/// precision needed. The pinned marker renders with a filled label pill.
///
/// Performance: the widget is fully static at rest — no animation tickers,
/// no repaints. During a drag it repaints a single-pass ring and a cached
/// label ladder (no blur filters, no per-frame text layout).
class RhythmScheduleClock extends StatefulWidget {
  const RhythmScheduleClock({
    super.key,
    required this.data,
    required this.dayHour,
    required this.sleepHour,
    required this.dayColor,
    required this.sleepColor,
    required this.dayVisual,
    required this.sleepVisual,
    required this.selectedMode,
    required this.onModeSelected,
    required this.onHourCommitted,
    this.dayAnchors = const [],
    this.sleepAnchors = const [],
    this.activeDayEvent,
    this.activeSleepEvent,
    this.onDragPreview,
    this.enabled = true,
    this.enforceDayBeforeSleep = true,
    this.dragStepHours = 5.0 / 60.0,
    this.snapThresholdHours = 0.25,
    this.minimumGapHours = 0.25,
    this.maxEditableHour = 23.99,
  });

  final SolarClockData data;

  /// Committed handle hours. During a drag the widget previews its own
  /// ephemeral hour; the dragged value keeps showing until these inputs
  /// change (the host adopting — or reverting — the commit).
  final double dayHour;
  final double sleepHour;
  final Color dayColor;
  final Color sleepColor;
  final ModeCurveVisual dayVisual;
  final ModeCurveVisual sleepVisual;

  /// Which mode's half of the ring (and orb) renders emphasized. Owned by the
  /// host so surrounding chrome can share the selection.
  final RhythmMode selectedMode;
  final ValueChanged<RhythmMode> onModeSelected;

  /// Fired when a drag ends or a marker is tapped: the final hour and the
  /// solar anchor it snapped/pinned onto, if any.
  final void Function(RhythmMode mode, double hour, TriggerAnchor? snapped)
      onHourCommitted;

  /// Solar events the orb snaps to while dragging and pins to on tap. Empty
  /// lists disable both for that mode.
  final List<TriggerAnchor> dayAnchors;
  final List<TriggerAnchor> sleepAnchors;

  /// The solar event a mode is currently bound to, when the host's trigger is
  /// solar — renders as the pinned marker. Hosts with fixed times leave these
  /// null; a handle sitting exactly on a marker still renders pinned.
  final String? activeDayEvent;
  final String? activeSleepEvent;

  final ValueChanged<RhythmClockDragPreview?>? onDragPreview;
  final bool enabled;

  /// When true (the alarm editor), Day must stay before Sleep with at least
  /// [minimumGapHours] between them and Sleep capped at [maxEditableHour].
  /// When false (room schedules), orbs move freely around the full ring.
  final bool enforceDayBeforeSleep;

  final double dragStepHours;
  final double snapThresholdHours;
  final double minimumGapHours;
  final double maxEditableHour;

  @override
  State<RhythmScheduleClock> createState() => _RhythmScheduleClockState();
}

class _RhythmScheduleClockState extends State<RhythmScheduleClock> {
  /// A committed hour within one minute of a marker renders as pinned.
  static const double _pinEpsilonHours = 1.0 / 60.0;

  /// Tap radius around a marker's tick on the ring.
  static const double _anchorTapRadius = 30.0;

  RhythmMode? _dragMode;

  /// Whether the current gesture actually changed the hour. A press-and-lift
  /// on an orb is a selection, not an edit — it must not commit (on the
  /// alarm that would rewrite a solar trigger as a fixed time).
  bool _dragMoved = false;
  double _gestureStartHour = 0.0;
  double? _dragPreviewHour;
  TriggerAnchor? _proximateAnchor;
  double _anchorProximity = 0.0;
  final Map<RhythmMode, double> _dragHours = {};

  /// Laid-out label painters, keyed by text + style bucket. Text layout is
  /// the most expensive part of painting the ladder; labels are stable, so
  /// they are laid out exactly once.
  final Map<String, TextPainter> _labelCache = {};

  @override
  void dispose() {
    for (final painter in _labelCache.values) {
      painter.dispose();
    }
    super.dispose();
  }

  @override
  void didUpdateWidget(covariant RhythmScheduleClock oldWidget) {
    super.didUpdateWidget(oldWidget);
    // A committed input changing means the host adopted (or reverted) the
    // drag — drop the ephemeral hour so the input is authoritative again.
    if (widget.dayHour != oldWidget.dayHour) {
      _dragHours.remove(RhythmMode.day);
    }
    if (widget.sleepHour != oldWidget.sleepHour) {
      _dragHours.remove(RhythmMode.sleep);
    }
  }

  double _committedHour(RhythmMode mode) =>
      mode == RhythmMode.day ? widget.dayHour : widget.sleepHour;

  double _hourForMode(RhythmMode mode) =>
      _dragHours[mode] ?? _committedHour(mode);

  double _displayHourForMode(RhythmMode mode) {
    if (_dragMode == mode && _dragPreviewHour != null) {
      return _dragPreviewHour!;
    }
    return _hourForMode(mode);
  }

  Color _colorForMode(RhythmMode mode) =>
      mode == RhythmMode.day ? widget.dayColor : widget.sleepColor;

  List<TriggerAnchor> _anchorsForMode(RhythmMode mode) =>
      mode == RhythmMode.day ? widget.dayAnchors : widget.sleepAnchors;

  String? _activeEventForMode(RhythmMode mode) =>
      mode == RhythmMode.day ? widget.activeDayEvent : widget.activeSleepEvent;

  /// The marker this mode is pinned to: the host's solar trigger when it has
  /// one, otherwise any marker the committed hour sits exactly on (fixed-time
  /// hosts pin by value).
  String? _pinnedEventForMode(RhythmMode mode) {
    final declared = _activeEventForMode(mode);
    if (declared != null) return declared;
    final hour = _hourForMode(mode);
    for (final anchor in _anchorsForMode(mode)) {
      if ((hour - anchor.hour).abs() <= _pinEpsilonHours) {
        return anchor.event;
      }
    }
    return null;
  }

  String _fmtTime(double h) {
    return SolarUtils.formatHour(
      h,
      use24: MediaQuery.alwaysUse24HourFormatOf(context),
    );
  }

  @override
  Widget build(BuildContext context) {
    final use24 = MediaQuery.alwaysUse24HourFormatOf(context);
    final clock = SizedBox.expand(
      child: SolarClock(
        data: widget.data,
        use24: use24,
        showUpperArc: false,
        showEventMarkers: false,
        showHourLabels: false,
        showLowerArc: false,
        horizonFactor: 0.58,
        radiusWidthFactor: 0.35,
        radiusHeightFactor: 0.92,
        underlayBuilder: (context, geometry) =>
            _buildClockRing(geometry: geometry, use24: use24),
        overlayBuilder: (context, geometry) =>
            _buildClockOverlay(geometry: geometry),
      ),
    );
    if (widget.enabled) return clock;
    return IgnorePointer(
      child: Opacity(opacity: 0.6, child: clock),
    );
  }

  Widget _buildClockRing({
    required SolarClockGeometry geometry,
    required bool use24,
  }) {
    return RepaintBoundary(
      child: CustomPaint(
        painter: _RhythmClockRingPainter(
          geometry: geometry,
          dayStartHour: _displayHourForMode(RhythmMode.day),
          sleepStartHour: _displayHourForMode(RhythmMode.sleep),
          dayVisual: widget.dayVisual,
          sleepVisual: widget.sleepVisual,
          selectedMode: _dragMode ?? widget.selectedMode,
          use24: use24,
          labelCache: _labelCache,
        ),
      ),
    );
  }

  Widget _buildClockOverlay({required SolarClockGeometry geometry}) {
    final handles = _handleSpecs();
    final handleByMode = {for (final handle in handles) handle.mode: handle};
    final anchorMode = _dragMode ?? widget.selectedMode;
    final anchorModeColor = _colorForMode(anchorMode);
    final anchorBase = anchorMode == RhythmMode.day
        ? SolarClockData.dawnColor
        : SolarClockData.duskColor;

    return RawGestureDetector(
      behavior: HitTestBehavior.translucent,
      gestures: <Type, GestureRecognizerFactory>{
        _ClockPanGestureRecognizer:
            GestureRecognizerFactoryWithHandlers<_ClockPanGestureRecognizer>(
          () => _ClockPanGestureRecognizer(),
          (recognizer) {
            recognizer.hitTestHandle = (position) =>
                _hitTestHandle(position, geometry, handleByMode) != null;
            recognizer.onStart = (details) {
              _handleClockPanStart(
                  details.localPosition, geometry, handleByMode);
            };
            recognizer.onUpdate = (details) {
              _handleClockPanUpdate(details.localPosition, geometry);
            };
            recognizer.onEnd = (_) {
              _handleClockPanEnd();
            };
            recognizer.onCancel = _handleClockPanEnd;
          },
        ),
        TapGestureRecognizer:
            GestureRecognizerFactoryWithHandlers<TapGestureRecognizer>(
          () => TapGestureRecognizer(),
          (recognizer) {
            recognizer.onTapUp = (details) {
              _handleClockTap(details.localPosition, geometry, handleByMode);
            };
          },
        ),
      },
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Positioned.fill(
            child: IgnorePointer(
              child: RepaintBoundary(
                child: CustomPaint(
                  painter: _ClockSolarAnchorPainter(
                    geometry: geometry,
                    anchors: _anchorsForMode(anchorMode),
                    accentColor: Color.lerp(anchorBase, anchorModeColor, 0.45)!,
                    pinnedEvent: _pinnedEventForMode(anchorMode),
                    proximateAnchor: _proximateAnchor,
                    anchorProximity: _anchorProximity,
                    handleHour: _displayHourForMode(anchorMode),
                    handleWidth:
                        anchorMode == widget.selectedMode ? 70.0 : 62.0,
                    isDragging: _dragMode != null,
                    labelCache: _labelCache,
                  ),
                ),
              ),
            ),
          ),
          Positioned.fill(
            child: IgnorePointer(
              child: CustomPaint(
                painter: _ClockHandlesPainter(
                  geometry: geometry,
                  handles: handles,
                  selectedMode: widget.selectedMode,
                ),
              ),
            ),
          ),
          for (final handle in handles)
            _buildPositionedHandle(handle: handle, geometry: geometry),
        ],
      ),
    );
  }

  Widget _buildPositionedHandle({
    required _TransitionHandleSpec handle,
    required SolarClockGeometry geometry,
  }) {
    final isSelected = handle.mode == widget.selectedMode;
    final center = geometry.positionForHour(handle.hour);
    final width = isSelected ? 70.0 : 62.0;
    final orbRadius = (isSelected ? 44.0 : 40.0) / 2;

    return Positioned(
      left: center.dx - width / 2,
      top: center.dy - orbRadius,
      child: _ClockModeOrb(
        mode: handle.mode,
        color: handle.color,
        timeLabel: _fmtTime(handle.hour),
        selected: isSelected,
      ),
    );
  }

  List<_TransitionHandleSpec> _handleSpecs() {
    return [
      for (final mode in [RhythmMode.day, RhythmMode.sleep])
        _TransitionHandleSpec(
          mode: mode,
          hour: _displayHourForMode(mode),
          color: _colorForMode(mode),
        ),
    ];
  }

  void _handleClockTap(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    final mode = _hitTestHandle(position, geometry, handleByMode);
    if (mode != null) {
      if (mode != widget.selectedMode) {
        HapticFeedback.selectionClick();
        widget.onModeSelected(mode);
      }
      return;
    }

    // Tap a marker to pin the selected orb to it exactly.
    final anchor = _hitTestAnchor(position, geometry);
    if (anchor != null &&
        _isHourAllowedForMode(widget.selectedMode, anchor.hour)) {
      HapticFeedback.mediumImpact();
      widget.onHourCommitted(widget.selectedMode, anchor.hour, anchor);
    }
  }

  TriggerAnchor? _hitTestAnchor(Offset position, SolarClockGeometry geometry) {
    TriggerAnchor? nearest;
    var nearestDistance = double.infinity;
    for (final anchor in _anchorsForMode(widget.selectedMode)) {
      final tick = geometry.positionForHour(anchor.hour);
      final distance = (position - tick).distance;
      if (distance < nearestDistance) {
        nearestDistance = distance;
        nearest = anchor;
      }
    }
    if (nearest != null && nearestDistance <= _anchorTapRadius) {
      return nearest;
    }
    return null;
  }

  void _handleClockPanStart(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    final mode = _hitTestHandle(position, geometry, handleByMode);
    if (mode == null) return;
    HapticFeedback.lightImpact();
    final initialHour = _hourForMode(mode);
    setState(() {
      _dragMode = mode;
      _dragMoved = false;
      _gestureStartHour = initialHour;
      _dragPreviewHour = initialHour;
      _computeDragProximity(mode, initialHour);
    });
    if (mode != widget.selectedMode) {
      widget.onModeSelected(mode);
    }
    _notifyDragPreview();
  }

  void _handleClockPanUpdate(Offset position, SolarClockGeometry geometry) {
    final mode = _dragMode;
    if (mode == null) return;

    final rawHour = geometry.hourFromPosition(position);
    // Quantize the drag to ratchet stops so small finger jitter doesn't
    // shift the time around.
    final steppedHour =
        (rawHour / widget.dragStepHours).round() * widget.dragStepHours;
    final draggedHour = _clampDraggedHour(mode, steppedHour);

    // Snap visually during drag so the orb locks onto solar events
    final snappedAnchor = _snappedAnchorForMode(mode, draggedHour);
    final displayHour = snappedAnchor?.hour ?? draggedHour;

    final previousProximate = _proximateAnchor;
    final previousDraggedHour = _dragHours[mode];
    final referenceHour = previousDraggedHour ?? _gestureStartHour;
    final stepChanged = (draggedHour - referenceHour).abs() > 0.0001;

    if (!stepChanged && _proximateAnchor == previousProximate) return;

    setState(() {
      if (stepChanged) _dragMoved = true;
      _dragPreviewHour = displayHour;
      _dragHours[mode] = draggedHour;
      _computeDragProximity(mode, draggedHour);
    });
    _notifyDragPreview();

    if (stepChanged && snappedAnchor == null && previousDraggedHour != null) {
      HapticFeedback.selectionClick();
    }
    if (_proximateAnchor != null &&
        _proximateAnchor != previousProximate &&
        _anchorProximity > 0.75) {
      HapticFeedback.selectionClick();
    }
  }

  void _handleClockPanEnd() {
    final mode = _dragMode;
    if (mode == null) return;

    // A press-and-lift that never moved the hour is a selection, not an
    // edit — clear the drag state and commit nothing.
    if (!_dragMoved) {
      setState(() {
        _dragMode = null;
        _dragPreviewHour = null;
        _proximateAnchor = null;
        _anchorProximity = 0.0;
        _dragHours.remove(mode);
      });
      widget.onDragPreview?.call(null);
      return;
    }

    final finalHour = _dragPreviewHour ?? _hourForMode(mode);
    final snappedAnchor = _snappedAnchorForMode(mode, finalHour);

    setState(() {
      _dragMode = null;
      _dragPreviewHour = null;
      _proximateAnchor = null;
      _anchorProximity = 0.0;
      // Keep showing the dragged hour until the host adopts or reverts the
      // commit (see didUpdateWidget).
      _dragHours[mode] = snappedAnchor?.hour ?? finalHour;
    });
    widget.onDragPreview?.call(null);

    widget.onHourCommitted(
      mode,
      snappedAnchor?.hour ?? finalHour,
      snappedAnchor,
    );
  }

  void _notifyDragPreview() {
    final mode = _dragMode;
    final hour = _dragPreviewHour;
    if (mode == null || hour == null) return;
    widget.onDragPreview?.call(
      RhythmClockDragPreview(
        mode: mode,
        hour: hour,
        proximateAnchor: _proximateAnchor,
        proximity: _anchorProximity,
      ),
    );
  }

  RhythmMode? _hitTestHandle(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    const threshold = 38.0;
    for (final entry in handleByMode.entries) {
      final center = geometry.positionForHour(entry.value.hour);
      if ((position - center).distance <= threshold) {
        return entry.key;
      }
    }
    return null;
  }

  double _clampDraggedHour(RhythmMode mode, double rawHour) {
    final normalized = SolarUtils.normalizeHour(rawHour);
    if (!widget.enforceDayBeforeSleep) {
      return normalized.clamp(0.0, widget.maxEditableHour).toDouble();
    }

    final dayHour = _hourForMode(RhythmMode.day);
    final sleepHour = _hourForMode(RhythmMode.sleep);

    if (mode == RhythmMode.day) {
      final maxDayHour = math.max(0.0, sleepHour - widget.minimumGapHours);
      return normalized.clamp(0.0, maxDayHour).toDouble();
    }

    final minSleepHour =
        math.min(widget.maxEditableHour, dayHour + widget.minimumGapHours);
    return normalized.clamp(minSleepHour, widget.maxEditableHour).toDouble();
  }

  void _computeDragProximity(RhythmMode mode, double dragHour) {
    final anchors = _anchorsForMode(mode)
        .where((a) => _isHourAllowedForMode(mode, a.hour))
        .toList();
    if (anchors.isEmpty) {
      _proximateAnchor = null;
      _anchorProximity = 0.0;
      return;
    }
    TriggerAnchor? nearest;
    var nearestDist = double.infinity;
    for (final a in anchors) {
      final dist = (dragHour - a.hour).abs();
      if (dist < nearestDist) {
        nearestDist = dist;
        nearest = a;
      }
    }
    const awarenessRadius = 1.5;
    if (nearest != null && nearestDist <= awarenessRadius) {
      _proximateAnchor = nearest;
      _anchorProximity = (1.0 - nearestDist / awarenessRadius).clamp(0.0, 1.0);
    } else {
      _proximateAnchor = null;
      _anchorProximity = 0.0;
    }
  }

  TriggerAnchor? _snappedAnchorForMode(RhythmMode mode, double hour) {
    final anchors = _anchorsForMode(mode)
        .where((anchor) => _isHourAllowedForMode(mode, anchor.hour))
        .toList();
    if (anchors.isEmpty) return null;

    TriggerAnchor? bestAnchor;
    var bestDistance = double.infinity;

    for (final anchor in anchors) {
      final distance = (hour - anchor.hour).abs();
      if (distance < bestDistance) {
        bestDistance = distance;
        bestAnchor = anchor;
      }
    }

    if (bestDistance <= widget.snapThresholdHours) {
      return bestAnchor;
    }
    return null;
  }

  bool _isHourAllowedForMode(RhythmMode mode, double hour) {
    if (!widget.enforceDayBeforeSleep) return true;
    final normalized = SolarUtils.normalizeHour(hour);
    final dayHour = _hourForMode(RhythmMode.day);
    final sleepHour = _hourForMode(RhythmMode.sleep);

    if (mode == RhythmMode.day) {
      return normalized <= sleepHour - widget.minimumGapHours;
    }
    return normalized >= dayHour + widget.minimumGapHours;
  }
}

class _TransitionHandleSpec {
  final RhythmMode mode;
  final double hour;
  final Color color;

  const _TransitionHandleSpec({
    required this.mode,
    required this.hour,
    required this.color,
  });
}

/// Returns a laid-out label painter from [cache], building it once. The key
/// must uniquely describe both the text and its style.
TextPainter _cachedLabel(
  Map<String, TextPainter> cache,
  String key,
  TextSpan Function() build,
) {
  return cache.putIfAbsent(key, () {
    return TextPainter(
      text: build(),
      textDirection: TextDirection.ltr,
    )..layout();
  });
}

class _ClockModeOrb extends StatelessWidget {
  final RhythmMode mode;
  final Color color;
  final String timeLabel;
  final bool selected;

  const _ClockModeOrb({
    required this.mode,
    required this.color,
    required this.timeLabel,
    required this.selected,
  });

  @override
  Widget build(BuildContext context) {
    final width = selected ? 70.0 : 62.0;
    final orbSize = selected ? 44.0 : 40.0;

    return SizedBox(
      width: width,
      child: Container(
        width: orbSize,
        height: orbSize,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: Color.lerp(
            const Color(0xFF1A1A2E),
            color,
            selected ? 0.35 : 0.25,
          ),
          border: Border.all(
            color: color.withValues(alpha: selected ? 0.85 : 0.6),
            width: selected ? 2.0 : 1.6,
          ),
          boxShadow: [
            BoxShadow(
              color: color.withValues(alpha: selected ? 0.45 : 0.25),
              blurRadius: selected ? 18 : 12,
            ),
          ],
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              mode == RhythmMode.day
                  ? Icons.wb_sunny_rounded
                  : Icons.bedtime_rounded,
              size: selected ? 18 : 16,
              color: color,
            ),
            const SizedBox(height: 1),
            Text(
              timeLabel,
              style: TextStyle(
                color: Colors.white.withValues(alpha: selected ? 0.82 : 0.6),
                fontSize: 8.5,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.1,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ClockHandlesPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final List<_TransitionHandleSpec> handles;
  final RhythmMode selectedMode;

  const _ClockHandlesPainter({
    required this.geometry,
    required this.handles,
    required this.selectedMode,
  });

  @override
  void paint(Canvas canvas, Size size) {
    for (final handle in handles) {
      final isSelected = handle.mode == selectedMode;
      if (!isSelected) continue;

      final anchor = geometry.positionForHour(handle.hour);

      // Glow behind the selected orb
      canvas.drawCircle(
        anchor,
        24,
        Paint()
          ..color = handle.color.withValues(alpha: 0.22)
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 14),
      );
    }
  }

  @override
  bool shouldRepaint(covariant _ClockHandlesPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        !listEquals(handles, oldDelegate.handles) ||
        selectedMode != oldDelegate.selectedMode;
  }
}

/// The solar marker ladder: a tick on the ring per marker, a leader line to
/// its label, a filled pill on the pinned marker, and proximity emphasis
/// while an orb is dragged nearby. Labels come pre-laid-out from the shared
/// cache — this painter performs no text layout and no blur filters.
class _ClockSolarAnchorPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final List<TriggerAnchor> anchors;
  final Color accentColor;
  final String? pinnedEvent;
  final TriggerAnchor? proximateAnchor;
  final double anchorProximity;
  final double handleHour;
  final double handleWidth;
  final bool isDragging;
  final Map<String, TextPainter> labelCache;

  const _ClockSolarAnchorPainter({
    required this.geometry,
    required this.anchors,
    required this.accentColor,
    required this.pinnedEvent,
    required this.proximateAnchor,
    required this.anchorProximity,
    required this.handleHour,
    required this.handleWidth,
    required this.isDragging,
    required this.labelCache,
  });

  @override
  void paint(Canvas canvas, Size size) {
    if (anchors.isEmpty) return;

    final layouts = <_ClockSolarAnchorLayout>[];
    final tickColor = Color.lerp(accentColor, Colors.white, 0.20)!;

    for (final anchor in anchors) {
      final anchorPoint = geometry.positionForHour(anchor.hour);
      final radial = anchorPoint - geometry.center;
      final radialDistance = radial.distance;
      if (radialDistance == 0) continue;

      final normal = radial / radialDistance;
      final isPinned = !isDragging && pinnedEvent == anchor.event;
      final isProximate = isDragging && proximateAnchor?.event == anchor.event;
      final emphasis = isPinned ? 1.0 : (isProximate ? anchorProximity : 0.0);
      final tickExtent = 10.0 + emphasis * 6.0;
      final strokeWidth = 2.4 + emphasis * 1.8;

      final inner = geometry.center + normal * (geometry.radius - tickExtent);
      final outer = geometry.center + normal * (geometry.radius + tickExtent);
      final innerLeader =
          geometry.center + normal * (geometry.radius - tickExtent - 10);

      canvas.drawLine(
        inner,
        outer,
        Paint()
          ..color = tickColor.withValues(alpha: 0.55 + emphasis * 0.40)
          ..strokeWidth = strokeWidth
          ..strokeCap = StrokeCap.round,
      );
      canvas.drawCircle(
        outer,
        1.6 + emphasis * 2.2,
        Paint()..color = tickColor.withValues(alpha: 0.75 + emphasis * 0.25),
      );

      final label = _cachedLabel(
        labelCache,
        'anchor|${accentColor.toARGB32()}|${anchor.label}',
        () => TextSpan(
          text: anchor.label,
          style: TextStyle(
            color: Color.lerp(tickColor, Colors.white, 0.25)!
                .withValues(alpha: 0.9),
            fontSize: 9.2,
            fontWeight: FontWeight.w700,
            letterSpacing: 0.1,
            shadows: [
              Shadow(
                color: Colors.black.withValues(alpha: 0.45),
                blurRadius: 4,
              ),
            ],
          ),
        ),
      );

      layouts.add(
        _ClockSolarAnchorLayout(
          leaderStart: innerLeader,
          normal: normal,
          tickColor: tickColor,
          emphasis: emphasis,
          pinned: isPinned,
          textPainter: label,
          preferredTop: anchorPoint.dy - label.height / 2,
        ),
      );
    }

    if (layouts.isEmpty) return;

    final leftLayouts =
        layouts.where((layout) => layout.normal.dx < 0).toList();
    final rightLayouts =
        layouts.where((layout) => layout.normal.dx >= 0).toList();

    _paintClockSolarAnchorSide(
      canvas,
      size,
      geometry: geometry,
      layouts: leftLayouts,
      handleHour: handleHour,
      handleWidth: handleWidth,
      accentColor: accentColor,
      isRightSide: false,
    );
    _paintClockSolarAnchorSide(
      canvas,
      size,
      geometry: geometry,
      layouts: rightLayouts,
      handleHour: handleHour,
      handleWidth: handleWidth,
      accentColor: accentColor,
      isRightSide: true,
    );
  }

  @override
  bool shouldRepaint(covariant _ClockSolarAnchorPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        !listEquals(anchors, oldDelegate.anchors) ||
        accentColor != oldDelegate.accentColor ||
        pinnedEvent != oldDelegate.pinnedEvent ||
        proximateAnchor != oldDelegate.proximateAnchor ||
        anchorProximity != oldDelegate.anchorProximity ||
        handleHour != oldDelegate.handleHour ||
        handleWidth != oldDelegate.handleWidth ||
        isDragging != oldDelegate.isDragging;
  }
}

class _ClockSolarAnchorLayout {
  final Offset leaderStart;
  final Offset normal;
  final Color tickColor;
  final double emphasis;
  final bool pinned;
  final TextPainter textPainter;
  final double preferredTop;
  double top;

  _ClockSolarAnchorLayout({
    required this.leaderStart,
    required this.normal,
    required this.tickColor,
    required this.emphasis,
    required this.pinned,
    required this.textPainter,
    required this.preferredTop,
  }) : top = preferredTop;
}

void _paintClockSolarAnchorSide(
  Canvas canvas,
  Size size, {
  required SolarClockGeometry geometry,
  required List<_ClockSolarAnchorLayout> layouts,
  required double handleHour,
  required double handleWidth,
  required Color accentColor,
  required bool isRightSide,
}) {
  if (layouts.isEmpty) return;

  final maxTextWidth = layouts.fold<double>(
    0.0,
    (maxWidth, item) => math.max(maxWidth, item.textPainter.width),
  );
  final handleCenter = geometry.positionForHour(handleHour);
  final handleOnRight = handleCenter.dx >= geometry.center.dx;
  const handleGap = 16.0;
  final preferredColumnLeft = isRightSide
      ? geometry.center.dx + geometry.radius * 0.06
      : geometry.center.dx - geometry.radius * 0.06 - maxTextWidth;
  var columnLeft = preferredColumnLeft;

  if (isRightSide && handleOnRight) {
    columnLeft = math.min(
      columnLeft,
      handleCenter.dx - handleWidth / 2 - maxTextWidth - handleGap,
    );
  } else if (!isRightSide && !handleOnRight) {
    columnLeft = math.max(
      columnLeft,
      handleCenter.dx + handleWidth / 2 + handleGap,
    );
  }

  final minColumnLeft = isRightSide ? geometry.center.dx - 10.0 : 12.0;
  final maxColumnLeft = isRightSide
      ? size.width - maxTextWidth - 12.0
      : geometry.center.dx - maxTextWidth * 0.35;
  columnLeft = columnLeft.clamp(minColumnLeft, maxColumnLeft).toDouble();

  _resolveClockSolarAnchorLabelTops(
    layouts,
    minTop: math.max(14.0, geometry.center.dy - geometry.radius * 0.78),
    maxBottom: math.min(
      size.height - 14.0,
      geometry.center.dy + geometry.radius * 0.78,
    ),
    gap: 10.0,
  );

  for (final layout in layouts) {
    final labelLeft = isRightSide
        ? columnLeft
        : columnLeft + (maxTextWidth - layout.textPainter.width);
    final labelCenterY = layout.top + layout.textPainter.height / 2;
    final connectorEnd = Offset(
      isRightSide
          ? labelLeft + layout.textPainter.width + 6.0
          : labelLeft - 6.0,
      labelCenterY,
    );

    canvas.drawLine(
      layout.leaderStart,
      connectorEnd,
      Paint()
        ..color = layout.tickColor.withValues(
          alpha: 0.18 + layout.emphasis * 0.16,
        )
        ..strokeWidth = 1.2 + layout.emphasis * 0.7
        ..strokeCap = StrokeCap.round,
    );

    // The pinned marker's label sits in a filled pill so the bound event is
    // unmistakable at a glance.
    if (layout.pinned) {
      final pillRect = Rect.fromLTWH(
        labelLeft - 7,
        layout.top - 4,
        layout.textPainter.width + 14,
        layout.textPainter.height + 8,
      );
      final pill = RRect.fromRectAndRadius(
        pillRect,
        const Radius.circular(999),
      );
      canvas.drawRRect(
        pill,
        Paint()..color = accentColor.withValues(alpha: 0.18),
      );
      canvas.drawRRect(
        pill,
        Paint()
          ..color = accentColor.withValues(alpha: 0.55)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1,
      );
    }

    layout.textPainter.paint(canvas, Offset(labelLeft, layout.top));
  }
}

void _resolveClockSolarAnchorLabelTops(
  List<_ClockSolarAnchorLayout> layouts, {
  required double minTop,
  required double maxBottom,
  required double gap,
}) {
  layouts.sort((a, b) => a.preferredTop.compareTo(b.preferredTop));

  for (var i = 0; i < layouts.length; i++) {
    final layout = layouts[i];
    var nextTop = layout.preferredTop.clamp(
      minTop,
      maxBottom - layout.textPainter.height,
    );
    if (i > 0) {
      final previous = layouts[i - 1];
      final minAllowedTop = previous.top + previous.textPainter.height + gap;
      if (nextTop < minAllowedTop) nextTop = minAllowedTop;
    }
    layout.top = nextTop;
  }

  final overflow =
      layouts.last.top + layouts.last.textPainter.height - maxBottom;
  if (overflow > 0) {
    layouts.last.top -= overflow;
    for (var i = layouts.length - 2; i >= 0; i--) {
      final current = layouts[i];
      final next = layouts[i + 1];
      final maxAllowedTop = next.top - current.textPainter.height - gap;
      if (current.top > maxAllowedTop) current.top = maxAllowedTop;
    }
  }

  final underflow = minTop - layouts.first.top;
  if (underflow > 0) {
    for (final layout in layouts) {
      layout.top += underflow;
    }
  }
}

class _RhythmClockRingPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final double dayStartHour;
  final double sleepStartHour;
  final ModeCurveVisual dayVisual;
  final ModeCurveVisual sleepVisual;
  final RhythmMode selectedMode;
  final bool use24;
  final Map<String, TextPainter> labelCache;

  const _RhythmClockRingPainter({
    required this.geometry,
    required this.dayStartHour,
    required this.sleepStartHour,
    required this.dayVisual,
    required this.sleepVisual,
    required this.selectedMode,
    required this.use24,
    required this.labelCache,
  });

  @override
  void paint(Canvas canvas, Size size) {
    _drawRing(canvas);
    _drawClockTicks(canvas);
    _drawHourLabels(canvas);
  }

  /// Single-pass segmented ring. The curve colors ARE the visual — the old
  /// per-segment glow + accent passes tripled the draw calls and forced
  /// blur-filter compositing for a subtle effect, which made dragging laggy
  /// on device.
  void _drawRing(Canvas canvas) {
    const segments = 96;
    final rect = geometry.arcRect;
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 16
      ..strokeCap = StrokeCap.round;
    const hourStep = 24.0 / segments;
    final sweep = (2 * math.pi) / segments;
    final selectedIsDay = selectedMode == RhythmMode.day;

    for (int i = 0; i < segments; i++) {
      final startHour = i * hourStep;
      final midHour = startHour + hourStep / 2;
      final isDayHour = _isWithinDaySegment(
        midHour,
        dayStartHour,
        sleepStartHour,
      );
      final (color, opacity) =
          (isDayHour ? dayVisual : sleepVisual).styleAt(midHour);
      final baseOpacity = isDayHour ? opacity : math.max(opacity, 0.38);
      final isSelectedSegment = isDayHour == selectedIsDay;
      // Selected half reads brighter; the other half recedes slightly.
      final effectiveOpacity = isSelectedSegment
          ? math.min(0.98, baseOpacity * 1.05 + 0.10)
          : baseOpacity * 0.85;

      paint.color = color.withValues(alpha: effectiveOpacity);
      canvas.drawArc(
        rect,
        geometry.angleForHour(startHour),
        sweep + 0.02,
        false,
        paint,
      );
    }
  }

  /// Draws watch-style index marks around the ring perimeter.
  ///
  /// Hierarchy mirrors a traditional clock face:
  /// - Cardinal (0, 6, 12, 18) — tallest, boldest
  /// - 3-hour (3, 9, 15, 21) — medium
  /// - Hourly — short
  /// - Half-hour — subtle subdivisions
  void _drawClockTicks(Canvas canvas) {
    final ringOuter = geometry.radius + 8;
    final tickPaint = Paint()..strokeCap = StrokeCap.round;

    for (int i = 0; i < 48; i++) {
      final hour = i * 0.5;
      final isWholeHour = i.isEven;
      final hourInt = i ~/ 2;

      // Skip ticks at labeled positions — the number serves as the marker
      if (isWholeHour && hourInt % 3 == 0) continue;

      final angle = geometry.angleForHour(hour);
      final cosA = math.cos(angle);
      final sinA = math.sin(angle);

      double tickLen, strokeW, alpha;

      if (isWholeHour) {
        tickLen = 3.5;
        strokeW = 1.0;
        alpha = 0.18;
      } else {
        tickLen = 2.0;
        strokeW = 0.7;
        alpha = 0.10;
      }

      final innerR = ringOuter + 1.5;
      final outerR = ringOuter + 1.5 + tickLen;

      tickPaint
        ..color = Colors.white.withValues(alpha: alpha)
        ..strokeWidth = strokeW;

      canvas.drawLine(
        Offset(
          geometry.center.dx + innerR * cosA,
          geometry.center.dy + innerR * sinA,
        ),
        Offset(
          geometry.center.dx + outerR * cosA,
          geometry.center.dy + outerR * sinA,
        ),
        tickPaint,
      );
    }
  }

  void _drawHourLabels(Canvas canvas) {
    const labeledHours = [0, 3, 6, 9, 12, 15, 18, 21];

    for (final hour in labeledHours) {
      final angle = geometry.angleForHour(hour.toDouble());
      final cosA = math.cos(angle);
      final sinA = math.sin(angle);
      final isCardinal = hour % 6 == 0;

      final label = use24
          ? hour.toString().padLeft(2, '0')
          : hour == 0
              ? '12 AM'
              : hour == 12
                  ? '12 PM'
                  : hour < 12
                      ? '$hour AM'
                      : '${hour - 12} PM';

      final textPainter = _cachedLabel(
        labelCache,
        'hour|$isCardinal|$label',
        () => TextSpan(
          text: label,
          style: TextStyle(
            color: Colors.white.withValues(alpha: isCardinal ? 0.72 : 0.42),
            fontSize: isCardinal ? 12 : 10,
            fontWeight: isCardinal ? FontWeight.w600 : FontWeight.w500,
            letterSpacing: 0.3,
          ),
        ),
      );

      final distance = geometry.radius + 22;
      textPainter.paint(
        canvas,
        Offset(
          geometry.center.dx + distance * cosA - textPainter.width / 2,
          geometry.center.dy + distance * sinA - textPainter.height / 2,
        ),
      );
    }
  }

  bool _isWithinDaySegment(double hour, double dayStart, double sleepStart) {
    final normalizedHour = SolarUtils.normalizeHour(hour);
    final normalizedDayStart = SolarUtils.normalizeHour(dayStart);
    final normalizedSleepStart = SolarUtils.normalizeHour(sleepStart);

    if (normalizedDayStart <= normalizedSleepStart) {
      return normalizedHour >= normalizedDayStart &&
          normalizedHour < normalizedSleepStart;
    }

    return normalizedHour >= normalizedDayStart ||
        normalizedHour < normalizedSleepStart;
  }

  @override
  bool shouldRepaint(covariant _RhythmClockRingPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        dayStartHour != oldDelegate.dayStartHour ||
        sleepStartHour != oldDelegate.sleepStartHour ||
        dayVisual != oldDelegate.dayVisual ||
        sleepVisual != oldDelegate.sleepVisual ||
        selectedMode != oldDelegate.selectedMode ||
        use24 != oldDelegate.use24;
  }
}

/// A `PanGestureRecognizer` tuned for the orbital clock's gesture-arena
/// fight with ancestor scroll views.
///
/// A pointer that goes DOWN on one of the orbs claims the arena immediately —
/// before the scroll view can start moving — so a drag can never scroll the
/// page and rotate the orb at the same time (which made landing on a specific
/// time nearly impossible). A pointer that goes down anywhere else behaves
/// like a normal pan and loses to the scroll view, keeping the page
/// scrollable across the clock face.
class _ClockPanGestureRecognizer extends PanGestureRecognizer {
  /// Whether a pointer-down position grabs an orb. Wired by the overlay on
  /// every build so it always sees the current geometry.
  bool Function(Offset localPosition)? hitTestHandle;
  bool _downOnHandle = false;

  @override
  void addAllowedPointer(PointerDownEvent event) {
    _downOnHandle = hitTestHandle?.call(event.localPosition) ?? false;
    super.addAllowedPointer(event);
    if (_downOnHandle) {
      resolve(GestureDisposition.accepted);
    }
  }

  @override
  void rejectGesture(int pointer) {
    // Belt and braces: even if something else wins the arena first, a drag
    // that started on an orb still takes over.
    if (_downOnHandle) {
      acceptGesture(pointer);
    } else {
      super.rejectGesture(pointer);
    }
  }
}
