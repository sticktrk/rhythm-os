import 'dart:async';
import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/models/hub.dart';
import '../widgets/solar_orbit.dart';
import '../providers/home_provider.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnectionState;

/// Full-screen state shown when a paired rhythm-server is unreachable.
///
/// The same blue screen is used for connecting, reconnecting, and unreachable
/// states. Callers can adjust copy and decide whether Retry/Forget actions are
/// appropriate for that state.
class ServerDisconnectedScreen extends StatefulWidget {
  final Hub serverHub;
  final String? title;
  final String? description;
  final String? addressLabel;
  final String? statusLabel;
  final bool? statusActive;
  final String retryLabel;
  final VoidCallback? onRetry;

  /// Whether to show the Retry/Forget recovery actions.
  final bool showRecoveryActions;

  /// Non-destructive escape hatch. When provided, a "Choose a different Home"
  /// action is offered so the user is never trapped on this screen.
  final VoidCallback? onChooseHome;

  const ServerDisconnectedScreen({
    super.key,
    required this.serverHub,
    this.title,
    this.description,
    this.addressLabel,
    this.statusLabel,
    this.statusActive,
    this.retryLabel = 'Retry Now',
    this.onRetry,
    this.showRecoveryActions = true,
    this.onChooseHome,
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
  }

  @override
  void dispose() {
    _ringController.dispose();
    _pulseController.dispose();
    _fadeInController.dispose();
    super.dispose();
  }

  void _retry() {
    HapticFeedback.mediumImpact();
    final onRetry = widget.onRetry;
    if (onRetry != null) {
      onRetry();
      return;
    }
    unawaited(
      context.read<ServerSyncProvider>().retryActiveServerConnection(
            authoritative: true,
          ),
    );
  }

  Future<void> _forgetServer() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: const Color(0xFF1C2333),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
        title: Text(
          'Forget Server?',
          style: TextStyle(color: CelestialColors.textPrimary, fontSize: 18),
        ),
        content: Text(
          'This will remove the server and all synced rooms.',
          style: TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 14,
            height: 1.5,
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text(
              'Cancel',
              style: TextStyle(color: CelestialColors.textSecondary),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('Forget', style: TextStyle(color: Colors.red)),
          ),
        ],
      ),
    );

    if (confirmed == true && mounted) {
      await context.read<RoomProvider>().clearAllRooms();
      if (!mounted) return;
      final serverSync = context.read<ServerSyncProvider>();
      serverSync.cancelHomeEntryRefresh();
      serverSync.connection.disconnect();
      await context.read<HomeProvider>().deleteHub(widget.serverHub.id);
    }
  }

  @override
  Widget build(BuildContext context) {
    final serverSync = context.watch<ServerSyncProvider>();
    final state = serverSync.connectionState;
    final isActivelyReconnecting =
        state == RhythmConnectionState.reconnecting ||
            state == RhythmConnectionState.connecting;
    final statusActive = widget.statusActive ?? isActivelyReconnecting;

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
                      _buildServerAddress(),
                      const SizedBox(height: 24),
                      _buildReconnectStatus(statusActive),
                      if (widget.showRecoveryActions) ...[
                        const SizedBox(height: 36),
                        _buildRetryButton(),
                        const SizedBox(height: 16),
                        _buildForgetButton(),
                      ],
                      if (widget.onChooseHome != null) _buildEscapeHatch(),
                      const SizedBox(height: 24),
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
    return Text(
      widget.title ?? 'Server Unreachable',
      style: TextStyle(
        color: CelestialColors.textPrimary,
        fontSize: 22,
        fontWeight: FontWeight.w600,
        letterSpacing: 0.3,
      ),
    );
  }

  Widget _buildServerAddress() {
    final activeEndpoint =
        context.watch<ServerSyncProvider>().activeConnectionEndpoint;
    final ep = activeEndpoint ?? widget.serverHub.endpoint;
    final address = widget.addressLabel ?? '${ep.host}:${ep.port}';

    return Column(
      children: [
        Text(
          address,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.45),
            fontSize: 14,
            fontFamily: widget.addressLabel == null ? 'monospace' : null,
            letterSpacing: widget.addressLabel == null ? 0.5 : 0.1,
          ),
        ),
        if (widget.description != null) ...[
          const SizedBox(height: 10),
          Text(
            widget.description!,
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.62),
              fontSize: 14,
              height: 1.45,
            ),
          ),
        ],
      ],
    );
  }

  String _retryLabel() {
    return widget.retryLabel;
  }

  Widget _buildRetryButton() {
    return GestureDetector(
      onTap: _retry,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 32, vertical: 14),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: _accent.withValues(alpha: 0.1),
          border: Border.all(
            color: _accent.withValues(alpha: 0.2),
            width: 1,
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.refresh_rounded,
                color: _accent.withValues(alpha: 0.8), size: 18),
            const SizedBox(width: 10),
            Text(
              _retryLabel(),
              style: TextStyle(
                color: _accent.withValues(alpha: 0.9),
                fontSize: 15,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildReconnectStatus(bool isReconnecting) {
    final label = widget.statusLabel ??
        (isReconnecting ? 'Reconnecting\u2026' : 'Disconnected');
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (isReconnecting) ...[
          _ReconnectDot(color: _accent),
          const SizedBox(width: 8),
        ],
        Text(
          label,
          style: TextStyle(
            color: isReconnecting
                ? _accent.withValues(alpha: 0.6)
                : CelestialColors.textSecondary.withValues(alpha: 0.45),
            fontSize: 14,
            fontWeight: FontWeight.w500,
          ),
        ),
      ],
    );
  }

  Widget _buildForgetButton() {
    return GestureDetector(
      onTap: _forgetServer,
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 8),
        child: Text(
          'Forget Server',
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.4),
            fontSize: 14,
          ),
        ),
      ),
    );
  }

  /// Non-destructive way off this screen.
  Widget _buildEscapeHatch() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        const SizedBox(height: 16),
        _buildChooseHomeButton(),
      ],
    );
  }

  Widget _buildChooseHomeButton() {
    return GestureDetector(
      onTap: () {
        HapticFeedback.mediumImpact();
        widget.onChooseHome?.call();
      },
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 28, vertical: 13),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: _accent.withValues(alpha: 0.08),
          border: Border.all(
            color: _accent.withValues(alpha: 0.22),
            width: 1,
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.swap_horiz_rounded,
              color: _accent.withValues(alpha: 0.85),
              size: 18,
            ),
            const SizedBox(width: 10),
            Text(
              'Choose a different Home',
              style: TextStyle(
                color: _accent.withValues(alpha: 0.9),
                fontSize: 15,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
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
