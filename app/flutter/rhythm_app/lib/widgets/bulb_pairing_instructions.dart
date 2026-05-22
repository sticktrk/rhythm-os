import 'dart:math' as math;

import 'package:flutter/material.dart';

import 'solar_orbit.dart';

/// Editorial-style readiness diagram for Matter bulbs.
///
/// Two parts:
///   • A "powered on / listening" status panel that affirms a fresh bulb
///     auto-enters pairing on first power-up. Always shown. No off→on
///     cycle is implied, because asking a first-time user to cycle a
///     fresh bulb is wrong (and was previously misread that way).
///   • A five-pulse factory-reset row, shown only when [showResetSection]
///     is true — typically after a failed attempt — for bulbs that won't
///     engage and need to be reset.
///
/// The bulb illustration stays "on" and breathes subtly to feel alive.
class BulbPairingInstructions extends StatefulWidget {
  const BulbPairingInstructions({
    super.key,
    this.showResetSection = false,
  });

  /// When true, reveals the "IF PAIRING WON'T ENGAGE" reset section.
  final bool showResetSection;

  @override
  State<BulbPairingInstructions> createState() =>
      _BulbPairingInstructionsState();
}

class _BulbPairingInstructionsState extends State<BulbPairingInstructions>
    with TickerProviderStateMixin {
  static const _teal = Color(0xFF00BCD4);
  static const _amber = CelestialColors.sunWarm;
  static const _ink = CelestialColors.textPrimary;
  static const _muted = CelestialColors.textSecondary;

  late final AnimationController _breath;
  late final AnimationController _pulseTrain;

  @override
  void initState() {
    super.initState();
    _breath = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2600),
    )..repeat(reverse: true);
    _pulseTrain = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2200),
    )..repeat();
  }

  @override
  void dispose() {
    _breath.dispose();
    _pulseTrain.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(22, 22, 22, 24),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard.withValues(alpha: 0.85),
        borderRadius: BorderRadius.circular(20),
        border: Border.all(
          color: _teal.withValues(alpha: 0.14),
          width: 1,
        ),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.35),
            blurRadius: 24,
            offset: const Offset(0, 12),
          ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _editorialHeader(
            kicker: 'READY THE BULB',
            counter: 'STATUS',
          ),
          const SizedBox(height: 18),
          _bulbStage(),
          const SizedBox(height: 14),
          _poweredStatus(),
          const SizedBox(height: 10),
          _poweredCopy(),
          if (widget.showResetSection) ...[
            const SizedBox(height: 22),
            _hairlineDivider(),
            const SizedBox(height: 18),
            _editorialHeader(
              kicker: 'IF PAIRING WON’T ENGAGE',
              counter: 'RESET × 5',
            ),
            const SizedBox(height: 14),
            _fivePulseReset(),
            const SizedBox(height: 12),
            _resetCopy(),
          ],
        ],
      ),
    );
  }

  Widget _editorialHeader({required String kicker, required String counter}) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        Container(
          width: 6,
          height: 6,
          decoration: BoxDecoration(
            color: _amber.withValues(alpha: 0.9),
            shape: BoxShape.circle,
            boxShadow: [
              BoxShadow(
                color: _amber.withValues(alpha: 0.55),
                blurRadius: 8,
                spreadRadius: 1,
              ),
            ],
          ),
        ),
        const SizedBox(width: 10),
        Expanded(
          child: Text(
            kicker,
            style: TextStyle(
              color: _ink.withValues(alpha: 0.95),
              fontSize: 11.5,
              fontWeight: FontWeight.w700,
              letterSpacing: 2.4,
            ),
          ),
        ),
        Text(
          counter,
          style: TextStyle(
            color: _muted.withValues(alpha: 0.7),
            fontSize: 11,
            fontWeight: FontWeight.w600,
            fontFamily: 'monospace',
            letterSpacing: 1.4,
          ),
        ),
      ],
    );
  }

  Widget _bulbStage() {
    return SizedBox(
      height: 132,
      child: AnimatedBuilder(
        animation: _breath,
        builder: (context, _) {
          // Keep the bulb steadily lit — gently pulse the glow without
          // ever swinging back through an "off" state.
          final t = 0.78 +
              Curves.easeInOutSine.transform(_breath.value) * 0.22;
          return CustomPaint(
            painter: _BulbDiagramPainter(
              progress: t,
              teal: _teal,
              amber: _amber,
              ink: _ink,
              muted: _muted,
            ),
            child: const SizedBox.expand(),
          );
        },
      ),
    );
  }

  Widget _poweredStatus() {
    return Row(
      children: [
        AnimatedBuilder(
          animation: _breath,
          builder: (context, _) {
            final t = Curves.easeInOutSine.transform(_breath.value);
            return Container(
              width: 8,
              height: 8,
              decoration: BoxDecoration(
                color: _amber.withValues(alpha: 0.95),
                shape: BoxShape.circle,
                boxShadow: [
                  BoxShadow(
                    color: _amber.withValues(alpha: 0.45 + t * 0.35),
                    blurRadius: 8 + t * 6,
                    spreadRadius: 1 + t * 1.2,
                  ),
                ],
              ),
            );
          },
        ),
        const SizedBox(width: 10),
        Expanded(
          child: Text(
            'POWERED · LISTENING',
            style: TextStyle(
              color: _ink.withValues(alpha: 0.95),
              fontSize: 11.5,
              fontWeight: FontWeight.w700,
              letterSpacing: 2.0,
              fontFamily: 'monospace',
            ),
          ),
        ),
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
          decoration: BoxDecoration(
            color: _amber.withValues(alpha: 0.12),
            borderRadius: BorderRadius.circular(4),
            border: Border.all(
              color: _amber.withValues(alpha: 0.35),
              width: 1,
            ),
          ),
          child: Text(
            'READY',
            style: TextStyle(
              color: _amber.withValues(alpha: 0.95),
              fontSize: 10,
              fontFamily: 'monospace',
              fontWeight: FontWeight.w700,
              letterSpacing: 1.4,
            ),
          ),
        ),
      ],
    );
  }

  Widget _poweredCopy() {
    return RichText(
      text: TextSpan(
        style: TextStyle(
          color: _muted.withValues(alpha: 0.85),
          fontSize: 12.5,
          height: 1.5,
        ),
        children: const [
          TextSpan(text: 'Fresh bulbs enter pairing mode automatically for '),
          TextSpan(
            text: 'about 15 minutes',
            style: TextStyle(fontWeight: FontWeight.w700),
          ),
          TextSpan(
            text: ' after first power-up. No need to flip the switch.',
          ),
        ],
      ),
    );
  }

  Widget _hairlineDivider() {
    return Row(
      children: [
        Container(
          width: 14,
          height: 1,
          color: _muted.withValues(alpha: 0.35),
        ),
        const SizedBox(width: 8),
        Container(
          width: 4,
          height: 4,
          decoration: BoxDecoration(
            color: _muted.withValues(alpha: 0.55),
            shape: BoxShape.circle,
          ),
        ),
        const SizedBox(width: 8),
        Expanded(
          child: Container(
            height: 1,
            color: _muted.withValues(alpha: 0.18),
          ),
        ),
      ],
    );
  }

  Widget _fivePulseReset() {
    return SizedBox(
      height: 64,
      child: AnimatedBuilder(
        animation: _pulseTrain,
        builder: (context, _) {
          return CustomPaint(
            painter: _PulseTrainPainter(
              progress: _pulseTrain.value,
              teal: _teal,
              amber: _amber,
              muted: _muted.withValues(alpha: 0.55),
            ),
            child: const SizedBox.expand(),
          );
        },
      ),
    );
  }

  Widget _resetCopy() {
    return RichText(
      text: TextSpan(
        style: TextStyle(
          color: _muted.withValues(alpha: 0.85),
          fontSize: 12.5,
          height: 1.5,
        ),
        children: [
          const TextSpan(text: 'Cycle the bulb '),
          TextSpan(
            text: 'off → on, five times',
            style: TextStyle(
              color: _amber.withValues(alpha: 0.95),
              fontWeight: FontWeight.w700,
            ),
          ),
          const TextSpan(
            text: ', pausing about a second between flips. The bulb pulses to '
                'confirm a factory reset — then try pairing again.',
          ),
        ],
      ),
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Painters
// ─────────────────────────────────────────────────────────────────────────────

/// Stylised bulb that breathes between an "off" cool slate state and an "on"
/// warm amber state. Drawn from primitives so it stays crisp at any size.
class _BulbDiagramPainter extends CustomPainter {
  _BulbDiagramPainter({
    required this.progress,
    required this.teal,
    required this.amber,
    required this.ink,
    required this.muted,
  });

  final double progress; // 0 = off, 1 = on
  final Color teal;
  final Color amber;
  final Color ink;
  final Color muted;

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height * 0.52);
    final bulbRadius = math.min(size.width, size.height) * 0.32;

    _drawSchematicGrid(canvas, size);
    _drawGlow(canvas, center, bulbRadius);
    _drawBulb(canvas, center, bulbRadius);
    _drawFilament(canvas, center, bulbRadius);
    _drawBase(canvas, center, bulbRadius);
    _drawAxisTicks(canvas, size);
    _drawStateLabel(canvas, size);
  }

  void _drawSchematicGrid(Canvas canvas, Size size) {
    // Faint corner cross-hairs to suggest an engineering diagram.
    final paint = Paint()
      ..color = muted.withValues(alpha: 0.18)
      ..strokeWidth = 0.8
      ..style = PaintingStyle.stroke;

    const armLength = 10.0;
    final corners = [
      Offset.zero,
      Offset(size.width, 0),
      Offset(0, size.height),
      Offset(size.width, size.height),
    ];
    for (final c in corners) {
      final sx = c.dx == 0 ? 1.0 : -1.0;
      final sy = c.dy == 0 ? 1.0 : -1.0;
      canvas.drawLine(c, c.translate(armLength * sx, 0), paint);
      canvas.drawLine(c, c.translate(0, armLength * sy), paint);
    }
  }

  void _drawGlow(Canvas canvas, Offset center, double r) {
    final intensity = progress;
    final glowRadius = r * (2.2 + intensity * 0.6);
    final paint = Paint()
      ..shader = RadialGradient(
        colors: [
          amber.withValues(alpha: 0.32 * intensity),
          amber.withValues(alpha: 0.12 * intensity),
          Colors.transparent,
        ],
        stops: const [0.0, 0.5, 1.0],
      ).createShader(Rect.fromCircle(center: center, radius: glowRadius));
    canvas.drawCircle(center, glowRadius, paint);
  }

  void _drawBulb(Canvas canvas, Offset center, double r) {
    // Gentle off→on color shift in the glass.
    final glassColor = Color.lerp(
      teal.withValues(alpha: 0.10),
      amber.withValues(alpha: 0.18),
      progress,
    )!;

    final glass = Paint()
      ..shader = RadialGradient(
        center: const Alignment(-0.3, -0.4),
        radius: 1.1,
        colors: [
          Color.lerp(
            Colors.white.withValues(alpha: 0.05),
            Colors.white.withValues(alpha: 0.18),
            progress,
          )!,
          glassColor,
        ],
      ).createShader(Rect.fromCircle(center: center, radius: r));
    canvas.drawCircle(center, r, glass);

    // Outline of the bulb (an A19-ish silhouette).
    final outline = Paint()
      ..color = Color.lerp(
        muted.withValues(alpha: 0.6),
        amber.withValues(alpha: 0.85),
        progress,
      )!
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.4;

    final path = Path();
    final top = center.translate(0, -r * 0.95);
    final bottomLeft = center.translate(-r * 0.42, r * 0.95);
    final bottomRight = center.translate(r * 0.42, r * 0.95);

    path.moveTo(bottomLeft.dx, bottomLeft.dy);
    path.cubicTo(
      center.dx - r * 1.05,
      center.dy + r * 0.2,
      center.dx - r * 1.05,
      center.dy - r * 0.5,
      top.dx,
      top.dy,
    );
    path.cubicTo(
      center.dx + r * 1.05,
      center.dy - r * 0.5,
      center.dx + r * 1.05,
      center.dy + r * 0.2,
      bottomRight.dx,
      bottomRight.dy,
    );
    canvas.drawPath(path, outline);
  }

  void _drawFilament(Canvas canvas, Offset center, double r) {
    // Two filament loops between the two posts.
    final filamentColor = Color.lerp(
      muted.withValues(alpha: 0.45),
      amber,
      progress,
    )!;
    final paint = Paint()
      ..color = filamentColor
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.2 + progress * 0.6
      ..strokeCap = StrokeCap.round;

    // Filament posts.
    final postY = center.dy + r * 0.55;
    final leftPost = Offset(center.dx - r * 0.22, postY);
    final rightPost = Offset(center.dx + r * 0.22, postY);
    canvas.drawLine(
      leftPost,
      Offset(leftPost.dx, center.dy + r * 0.05),
      paint,
    );
    canvas.drawLine(
      rightPost,
      Offset(rightPost.dx, center.dy + r * 0.05),
      paint,
    );

    // Filament arch made of two scallops.
    final filament = Path()
      ..moveTo(leftPost.dx, center.dy + r * 0.05)
      ..relativeQuadraticBezierTo(r * 0.11, -r * 0.55, r * 0.22, 0)
      ..relativeQuadraticBezierTo(r * 0.11, -r * 0.55, r * 0.22, 0);

    canvas.drawPath(filament, paint);

    // When "on", add a hot core.
    if (progress > 0.05) {
      final core = Paint()
        ..color = Colors.white.withValues(alpha: 0.45 * progress)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 0.6
        ..strokeCap = StrokeCap.round;
      canvas.drawPath(filament, core);
    }
  }

  void _drawBase(Canvas canvas, Offset center, double r) {
    final basePaint = Paint()
      ..color = muted.withValues(alpha: 0.55)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.2;

    final baseTop = center.dy + r * 0.95;
    final baseWidth = r * 0.62;

    // Trapezoidal screw base.
    final basePath = Path()
      ..moveTo(center.dx - baseWidth / 2, baseTop)
      ..lineTo(center.dx - baseWidth / 2.4, baseTop + r * 0.32)
      ..lineTo(center.dx + baseWidth / 2.4, baseTop + r * 0.32)
      ..lineTo(center.dx + baseWidth / 2, baseTop);
    canvas.drawPath(basePath, basePaint);

    // Three thread lines for E26-ish detail.
    for (var i = 1; i <= 3; i++) {
      final y = baseTop + r * 0.06 * i + 2;
      canvas.drawLine(
        Offset(center.dx - baseWidth / 2.2, y),
        Offset(center.dx + baseWidth / 2.2, y),
        basePaint,
      );
    }
  }

  void _drawAxisTicks(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = muted.withValues(alpha: 0.35)
      ..strokeWidth = 1;

    // Tiny tick marks left + right to anchor the bulb on a "stage".
    const tickLen = 6.0;
    final y = size.height * 0.52;
    for (final x in [16.0, size.width - 16.0]) {
      canvas.drawLine(
          Offset(x, y), Offset(x + (x < 20 ? tickLen : -tickLen), y), paint);
    }
  }

  void _drawStateLabel(Canvas canvas, Size size) {
    final label = progress < 0.5 ? 'OFF' : 'ON';
    final color = Color.lerp(teal, amber, progress)!;
    final tp = TextPainter(
      text: TextSpan(
        text: label,
        style: TextStyle(
          color: color.withValues(alpha: 0.85),
          fontSize: 10.5,
          fontWeight: FontWeight.w800,
          letterSpacing: 2.2,
          fontFamily: 'monospace',
        ),
      ),
      textDirection: TextDirection.ltr,
    )..layout();
    tp.paint(
      canvas,
      Offset(size.width / 2 - tp.width / 2, size.height - tp.height - 2),
    );
  }

  @override
  bool shouldRepaint(covariant _BulbDiagramPainter old) =>
      old.progress != progress;
}

/// Five-dot reset visualization. Each dot represents one off→on flip;
/// they light in sequence (with a small overlap) then fade together,
/// communicating "do this five times in a row".
class _PulseTrainPainter extends CustomPainter {
  _PulseTrainPainter({
    required this.progress,
    required this.teal,
    required this.amber,
    required this.muted,
  });

  final double progress;
  final Color teal;
  final Color amber;
  final Color muted;

  @override
  void paint(Canvas canvas, Size size) {
    const count = 5;
    final spacing = size.width / (count + 1);
    final y = size.height / 2;

    // Underline rail.
    canvas.drawLine(
      Offset(spacing * 0.6, y + 18),
      Offset(size.width - spacing * 0.6, y + 18),
      Paint()
        ..color = muted.withValues(alpha: 0.5)
        ..strokeWidth = 0.8,
    );

    for (var i = 0; i < count; i++) {
      final cx = spacing * (i + 1);
      final center = Offset(cx, y);

      // Each dot has its own time slot inside [0,1].
      // Slot length 0.2, with light overlap so the sequence feels continuous.
      final slotStart = i * 0.16;
      const slotLen = 0.32;
      double local = (progress - slotStart) / slotLen;
      local = local.clamp(0.0, 1.0);
      // Triangular envelope: 0 → peak at 0.5 → 0.
      final intensity = local < 0.5 ? local * 2 : (1 - local) * 2;

      final dotColor = Color.lerp(
        teal.withValues(alpha: 0.55),
        amber,
        intensity,
      )!;

      // Halo.
      canvas.drawCircle(
        center,
        9 + intensity * 6,
        Paint()
          ..color = amber.withValues(alpha: 0.18 * intensity)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 2,
      );

      // Core dot.
      canvas.drawCircle(
        center,
        5 + intensity * 1.5,
        Paint()..color = dotColor,
      );

      // Index numeral underneath.
      final tp = TextPainter(
        text: TextSpan(
          text: '0${i + 1}',
          style: TextStyle(
            color: Color.lerp(
              muted.withValues(alpha: 0.6),
              amber.withValues(alpha: 0.95),
              intensity,
            ),
            fontSize: 10,
            fontFamily: 'monospace',
            fontWeight: FontWeight.w700,
            letterSpacing: 1.2,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      tp.paint(canvas, Offset(cx - tp.width / 2, y + 22));
    }
  }

  @override
  bool shouldRepaint(covariant _PulseTrainPainter old) =>
      old.progress != progress;
}
