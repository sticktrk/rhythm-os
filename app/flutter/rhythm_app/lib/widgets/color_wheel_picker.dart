import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

typedef ColorWheelChanged = void Function(
  HSVColor color, {
  required bool commit,
});

/// The large circular hue-and-saturation picker shared by Mood and profile
/// editors.
class ColorWheelPicker extends StatefulWidget {
  const ColorWheelPicker({
    super.key,
    required this.value,
    required this.onChanged,
    required this.semanticsLabel,
    this.maxSide = 252,
    this.interactionKey,
    this.wheelKey,
  });

  final HSVColor value;
  final ColorWheelChanged onChanged;
  final String semanticsLabel;
  final double maxSide;
  final Key? interactionKey;
  final Key? wheelKey;

  @override
  State<ColorWheelPicker> createState() => _ColorWheelPickerState();
}

class _ColorWheelPickerState extends State<ColorWheelPicker> {
  late HSVColor _lastValue;
  bool _dragging = false;

  @override
  void initState() {
    super.initState();
    _lastValue = widget.value;
  }

  @override
  void didUpdateWidget(ColorWheelPicker oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!_dragging) _lastValue = widget.value;
  }

  HSVColor _valueForPosition(Offset localPosition, double side) {
    final center = Offset(side / 2, side / 2);
    final radius = side / 2;
    final vector = localPosition - center;
    final saturation = (vector.distance / radius).clamp(0.0, 1.0);
    final hue = vector.distance <= 0.5
        ? _lastValue.hue
        : ((math.atan2(vector.dy, vector.dx) * 180 / math.pi) + 360) % 360;
    return HSVColor.fromAHSV(1, hue, saturation, 1);
  }

  void _change(Offset localPosition, double side, {required bool commit}) {
    _lastValue = _valueForPosition(localPosition, side);
    widget.onChanged(_lastValue, commit: commit);
  }

  void _finishDrag() {
    if (!_dragging) return;
    _dragging = false;
    widget.onChanged(_lastValue, commit: true);
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final side = math.min(constraints.maxWidth, widget.maxSide);
        final radius = side / 2;
        final markerAngle = widget.value.hue * math.pi / 180;
        final markerOffset = Offset(
          radius + math.cos(markerAngle) * radius * widget.value.saturation,
          radius + math.sin(markerAngle) * radius * widget.value.saturation,
        );

        return Center(
          child: Semantics(
            label: widget.semanticsLabel,
            value: 'Selected color',
            child: GestureDetector(
              key: widget.interactionKey,
              onTapUp: (details) {
                HapticFeedback.selectionClick();
                _change(details.localPosition, side, commit: true);
              },
              onPanStart: (details) {
                _dragging = true;
                HapticFeedback.selectionClick();
                _change(details.localPosition, side, commit: false);
              },
              onPanUpdate: (details) {
                _change(details.localPosition, side, commit: false);
              },
              onPanEnd: (_) => _finishDrag(),
              onPanCancel: _finishDrag,
              child: SizedBox.square(
                key: widget.wheelKey,
                dimension: side,
                child: Stack(
                  clipBehavior: Clip.none,
                  children: [
                    const Positioned.fill(
                      child: CustomPaint(
                        painter: _RgbColorWheelPainter(),
                      ),
                    ),
                    Positioned(
                      left: markerOffset.dx - 14,
                      top: markerOffset.dy - 14,
                      child: _ColorWheelThumb(color: widget.value.toColor()),
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

class _RgbColorWheelPainter extends CustomPainter {
  const _RgbColorWheelPainter();

  static const _colors = [
    Color(0xFFFF0000),
    Color(0xFFFFFF00),
    Color(0xFF00FF00),
    Color(0xFF00FFFF),
    Color(0xFF0000FF),
    Color(0xFFFF00FF),
    Color(0xFFFF0000),
  ];

  @override
  void paint(Canvas canvas, Size size) {
    final side = math.min(size.width, size.height);
    if (side <= 0) return;
    final center = Offset(size.width / 2, size.height / 2);
    final radius = side / 2;
    final rect = Rect.fromCircle(center: center, radius: radius);

    canvas.save();
    canvas.clipPath(Path()..addOval(rect));
    canvas.drawCircle(
      center,
      radius,
      Paint()..shader = const SweepGradient(colors: _colors).createShader(rect),
    );
    canvas.drawCircle(
      center,
      radius,
      Paint()
        ..shader = const RadialGradient(
          colors: [Colors.white, Color(0x00FFFFFF)],
          stops: [0.0, 1.0],
        ).createShader(rect),
    );
    canvas.restore();

    canvas.drawCircle(
      center,
      radius - 0.5,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1
        ..color = Colors.white.withValues(alpha: 0.16),
    );
    canvas.drawCircle(
      center,
      radius - 1.5,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1
        ..color = Colors.black.withValues(alpha: 0.28),
    );
  }

  @override
  bool shouldRepaint(_RgbColorWheelPainter oldDelegate) => false;
}

class _ColorWheelThumb extends StatelessWidget {
  const _ColorWheelThumb({required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      child: Container(
        width: 28,
        height: 28,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: color,
          border: Border.all(
            color: Colors.white.withValues(alpha: 0.95),
            width: 3,
          ),
          boxShadow: [
            BoxShadow(
              color: color.withValues(alpha: 0.55),
              blurRadius: 16,
              spreadRadius: 1,
            ),
            BoxShadow(
              color: Colors.black.withValues(alpha: 0.55),
              blurRadius: 7,
              offset: const Offset(0, 2),
            ),
          ],
        ),
      ),
    );
  }
}
