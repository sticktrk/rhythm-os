import 'dart:async';
import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/models/hub.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import '../widgets/solar_orbit.dart';
import '../providers/server_sync_provider.dart';

/// Full-screen state shown when a paired rhythm-server is unreachable.
///
/// The same blue screen is used for connecting, reconnecting, and unreachable
/// states. It intentionally avoids endpoint details; the user only needs to
/// know whether the app is actively retrying or waiting for the next attempt.
class ServerDisconnectedScreen extends StatefulWidget {
  final Hub serverHub;
  final String? title;
  final FutureOr<void> Function()? onRetry;
  final bool autoRetry;

  /// Non-destructive escape hatch shown as the standard top-left Home icon.
  final VoidCallback? onChooseHome;

  final Duration retryInterval;

  const ServerDisconnectedScreen({
    super.key,
    required this.serverHub,
    this.title,
    this.onRetry,
    this.autoRetry = true,
    this.onChooseHome,
    this.retryInterval = const Duration(seconds: 6),
  });

  @override
  State<ServerDisconnectedScreen> createState() =>
      _ServerDisconnectedScreenState();
}

class _ServerDisconnectedScreenState extends State<ServerDisconnectedScreen>
    with TickerProviderStateMixin {
  late AnimationController _ringController;
  late AnimationController _pulseController;
  late AnimationController _fadeInController;
  Timer? _retryTimer;
  bool _retryInFlight = false;

  /// Accent that carries the screen: Rhythm's celestial blue in both loading
  /// and unreachable states.
  Color get _accent => CelestialColors.accentBlue;

  @override
  void initState() {
    super.initState();

    // Slow rotation for the server connection orbit.
    _ringController = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 24),
    )..repeat();

    // Breathing pulse for the center icon glow
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 3),
    )..repeat(reverse: true);

    // Initial fade-in
    _fadeInController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 600),
    )..forward();

    if (widget.autoRetry) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted) return;
        _startRetryLoop();
      });
    }
  }

  @override
  void dispose() {
    _ringController.dispose();
    _pulseController.dispose();
    _fadeInController.dispose();
    _retryTimer?.cancel();
    super.dispose();
  }

  void _startRetryLoop() {
    unawaited(_retryConnection());
    _retryTimer?.cancel();
    _retryTimer = Timer.periodic(
      widget.retryInterval,
      (_) => unawaited(_retryConnection()),
    );
  }

  Future<void> _retryConnection() async {
    if (_retryInFlight || !mounted) return;
    setState(() => _retryInFlight = true);
    try {
      final onRetry = widget.onRetry;
      if (onRetry != null) {
        await Future<void>.sync(onRetry);
      } else {
        await context.read<ServerSyncProvider>().retryActiveServerConnection(
              authoritative: true,
            );
      }
    } finally {
      if (mounted) {
        setState(() => _retryInFlight = false);
      } else {
        _retryInFlight = false;
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: FadeTransition(
        opacity: CurvedAnimation(
          parent: _fadeInController,
          curve: Curves.easeOut,
        ),
        child: Stack(
          children: [
            // Subtle radial glow behind center.
            Positioned.fill(
              child: AnimatedBuilder(
                animation: _pulseController,
                builder: (context, _) {
                  final p = _pulseController.value;
                  return Container(
                    decoration: BoxDecoration(
                      gradient: RadialGradient(
                        center: const Alignment(0, -0.25),
                        radius: 1.2,
                        colors: [
                          _accent.withValues(alpha: 0.03 + p * 0.02),
                          CelestialColors.backgroundDark,
                        ],
                      ),
                    ),
                  );
                },
              ),
            ),

            // Main content
            SafeArea(
              bottom: false,
              child: Center(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.symmetric(horizontal: 40),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      const SizedBox(height: 40),
                      _buildOrbit(),
                      const SizedBox(height: 36),
                      _buildTitle(),
                      const SizedBox(height: 8),
                      _buildRetryStatus(),
                      const SizedBox(height: 24),
                    ],
                  ),
                ),
              ),
            ),
            if (widget.onChooseHome != null)
              Positioned(
                top: MediaQuery.of(context).padding.top + 8,
                left: 12,
                child: IconButton(
                  tooltip: 'Choose Home',
                  onPressed: () {
                    HapticFeedback.lightImpact();
                    widget.onChooseHome?.call();
                  },
                  icon: Icon(
                    Icons.home_rounded,
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.82),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }

  /// The central orbit visual.
  Widget _buildOrbit() {
    return SizedBox(
      width: 200,
      height: 200,
      child: AnimatedBuilder(
        animation: Listenable.merge([_ringController, _pulseController]),
        builder: (context, _) {
          final pulse = _pulseController.value;
          return CustomPaint(
            painter: _ServerOrbitPainter(
              rotation: _ringController.value * 2 * math.pi,
              pulse: pulse,
              accent: _accent,
            ),
            child: Center(
              child: Container(
                width: 72,
                height: 72,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: _accent.withValues(alpha: 0.06 + pulse * 0.04),
                  border: Border.all(
                    color: _accent.withValues(alpha: 0.1 + pulse * 0.08),
                    width: 1,
                  ),
                  boxShadow: [
                    BoxShadow(
                      color: _accent.withValues(alpha: 0.04 + pulse * 0.04),
                      blurRadius: 28,
                      spreadRadius: 4,
                    ),
                  ],
                ),
                child: Icon(
                  Icons.hub_rounded,
                  color: _accent.withValues(alpha: 0.4 + pulse * 0.2),
                  size: 28,
                ),
              ),
            ),
          );
        },
      ),
    );
  }

  Widget _buildTitle() {
    final title = widget.title ??
        (widget.serverHub.remoteEndpoint != null
            ? 'Connecting to Your Home'
            : 'Server Unreachable');
    return Text(
      title,
      style: TextStyle(
        color: CelestialColors.textPrimary,
        fontSize: 22,
        fontWeight: FontWeight.w600,
        letterSpacing: 0.3,
      ),
    );
  }

  Widget _buildRetryStatus() {
    final providerState = _watchConnectionState();
    final connectionAttemptActive =
        providerState == RhythmConnectionState.connecting ||
            providerState == RhythmConnectionState.reconnecting;
    final retrying = _retryInFlight || connectionAttemptActive;

    return AnimatedSwitcher(
      duration: const Duration(milliseconds: 180),
      child: _RetryStatusChip(
        key: ValueKey(retrying),
        color: _accent,
        retrying: retrying,
      ),
    );
  }

  RhythmConnectionState? _watchConnectionState() {
    try {
      return context.watch<ServerSyncProvider>().connectionState;
    } on ProviderNotFoundException {
      return null;
    }
  }
}

class _RetryStatusChip extends StatelessWidget {
  final Color color;
  final bool retrying;

  const _RetryStatusChip({
    super.key,
    required this.color,
    required this.retrying,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 11),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: color.withValues(alpha: 0.2)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          SizedBox(
            width: 16,
            height: 16,
            child: retrying
                ? CircularProgressIndicator(
                    strokeWidth: 2,
                    valueColor: AlwaysStoppedAnimation<Color>(
                      color.withValues(alpha: 0.8),
                    ),
                  )
                : Center(child: _ReconnectDot(color: color)),
          ),
          const SizedBox(width: 10),
          Text(
            _statusText,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.76),
              fontSize: 15,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }

  String get _statusText {
    return retrying ? 'Connecting...' : 'Retrying soon...';
  }
}

/// Pulsing dot indicating active reconnection. Tinted by the screen mood.
class _ReconnectDot extends StatefulWidget {
  final Color color;
  const _ReconnectDot({required this.color});

  @override
  State<_ReconnectDot> createState() => _ReconnectDotState();
}

class _ReconnectDotState extends State<_ReconnectDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1200),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _controller,
      builder: (context, _) {
        final opacity = 0.3 + 0.7 * _controller.value;
        return Container(
          width: 8,
          height: 8,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: widget.color.withValues(alpha: opacity),
            boxShadow: [
              BoxShadow(
                color: widget.color.withValues(alpha: opacity * 0.4),
                blurRadius: 6,
              ),
            ],
          ),
        );
      },
    );
  }
}

/// Draws two broken orbit rings that rotate in opposite directions.
///
/// Each ring is an arc with a gap and small dots at the endpoints,
/// giving the visual impression of a disrupted connection orbit.
class _ServerOrbitPainter extends CustomPainter {
  final double rotation;
  final double pulse;
  final Color accent;

  _ServerOrbitPainter({
    required this.rotation,
    required this.pulse,
    required this.accent,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);

    // Outer ring — rotates clockwise
    _drawBrokenRing(
      canvas,
      center,
      size.width / 2 - 6,
      rotation,
      0.10 + pulse * 0.05,
    );

    // Inner ring — rotates counter-clockwise, slower
    _drawBrokenRing(
      canvas,
      center,
      size.width / 2 - 28,
      -rotation * 0.6,
      0.06 + pulse * 0.03,
    );
  }

  void _drawBrokenRing(
    Canvas canvas,
    Offset center,
    double radius,
    double startRotation,
    double alpha,
  ) {
    final paint = Paint()
      ..color = accent.withValues(alpha: alpha)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.0
      ..strokeCap = StrokeCap.round;

    // Arc with a ~46-degree gap
    const gapAngle = 0.8;
    const sweepAngle = 2 * math.pi - gapAngle;

    final rect = Rect.fromCircle(center: center, radius: radius);
    canvas.drawArc(rect, startRotation, sweepAngle, false, paint);

    // Small dots at each endpoint of the arc
    final dotPaint = Paint()
      ..color = accent.withValues(alpha: (alpha * 1.5).clamp(0.0, 1.0))
      ..style = PaintingStyle.fill;

    final p1 = Offset(
      center.dx + radius * math.cos(startRotation),
      center.dy + radius * math.sin(startRotation),
    );
    final p2 = Offset(
      center.dx + radius * math.cos(startRotation + sweepAngle),
      center.dy + radius * math.sin(startRotation + sweepAngle),
    );

    canvas.drawCircle(p1, 2, dotPaint);
    canvas.drawCircle(p2, 2, dotPaint);
  }

  @override
  bool shouldRepaint(_ServerOrbitPainter old) =>
      old.rotation != rotation || old.pulse != pulse || old.accent != accent;
}
