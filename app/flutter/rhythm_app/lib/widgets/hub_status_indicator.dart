import 'package:flutter/material.dart';
import '../providers/hub_connection_provider.dart';

/// Industrial control panel color palette.
class IndustrialColors {
  // Background colors
  static const backgroundDeep = Color(0xFF0A0C10);
  static const backgroundPanel = Color(0xFF12151C);
  static const backgroundCard = Color(0xFF1A1E28);

  // Status colors
  static const statusGreen = Color(0xFF22C55E);
  static const statusAmber = Color(0xFFE8A54B);
  static const statusRed = Color(0xFFEF4444);
  static const statusBlue = Color(0xFF3B82F6);

  // Text colors
  static const textPrimary = Color(0xFFF5F5F7);
  static const textSecondary = Color(0xFF9CA3AF);
  static const textMuted = Color(0xFF4B5563);

  // Accent colors
  static const accentAmber = Color(0xFFE8A54B);
  static const accentAmberDim = Color(0xFF7A5A2E);

  // Grid pattern color
  static const gridLine = Color(0xFF1F242F);
}

/// Animated hub status indicator with industrial aesthetic.
///
/// Shows connection status with:
/// - Pulsing glow when connecting
/// - Solid glow when connected
/// - Warning flash on error
/// - Beveled edges with subtle inner shadow
class HubStatusIndicator extends StatefulWidget {
  final ConnectionStatus status;
  final double size;
  final bool showLabel;
  final String? label;

  const HubStatusIndicator({
    super.key,
    required this.status,
    this.size = 12,
    this.showLabel = false,
    this.label,
  });

  @override
  State<HubStatusIndicator> createState() => _HubStatusIndicatorState();
}

class _HubStatusIndicatorState extends State<HubStatusIndicator>
    with SingleTickerProviderStateMixin {
  late AnimationController _controller;
  late Animation<double> _pulseAnimation;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      duration: const Duration(milliseconds: 1500),
      vsync: this,
    );

    _pulseAnimation = Tween<double>(begin: 0.4, end: 1.0).animate(
      CurvedAnimation(parent: _controller, curve: Curves.easeInOut),
    );

    _updateAnimation();
  }

  @override
  void didUpdateWidget(HubStatusIndicator oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.status != widget.status) {
      _updateAnimation();
    }
  }

  void _updateAnimation() {
    switch (widget.status) {
      case ConnectionStatus.connecting:
        _controller.repeat(reverse: true);
        break;
      case ConnectionStatus.error:
        _controller.repeat(reverse: true);
        break;
      case ConnectionStatus.connected:
      case ConnectionStatus.disconnected:
        _controller.stop();
        _controller.value = 1.0;
        break;
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  Color get _statusColor {
    switch (widget.status) {
      case ConnectionStatus.connected:
        return IndustrialColors.statusGreen;
      case ConnectionStatus.connecting:
        return IndustrialColors.statusAmber;
      case ConnectionStatus.error:
        return IndustrialColors.statusRed;
      case ConnectionStatus.disconnected:
        return IndustrialColors.textMuted;
    }
  }

  String get _statusText {
    if (widget.label != null) return widget.label!;
    switch (widget.status) {
      case ConnectionStatus.connected:
        return 'Connected';
      case ConnectionStatus.connecting:
        return 'Connecting...';
      case ConnectionStatus.error:
        return 'Error';
      case ConnectionStatus.disconnected:
        return 'Offline';
    }
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _pulseAnimation,
      builder: (context, child) {
        final opacity = widget.status == ConnectionStatus.connected
            ? 1.0
            : _pulseAnimation.value;

        return Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            // Status dot with glow effect
            Container(
              width: widget.size,
              height: widget.size,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _statusColor.withValues(alpha: opacity),
                boxShadow: [
                  // Inner highlight (beveled effect)
                  BoxShadow(
                    color: Colors.white.withValues(alpha: 0.1 * opacity),
                    blurRadius: 1,
                    offset: const Offset(0, -1),
                  ),
                  // Outer glow
                  if (widget.status != ConnectionStatus.disconnected)
                    BoxShadow(
                      color: _statusColor.withValues(alpha: 0.5 * opacity),
                      blurRadius: widget.size * 0.8,
                      spreadRadius: widget.size * 0.2,
                    ),
                ],
                border: Border.all(
                  color: _statusColor.withValues(alpha: 0.3),
                  width: 1,
                ),
              ),
            ),
            if (widget.showLabel) ...[
              const SizedBox(width: 8),
              Text(
                _statusText,
                style: TextStyle(
                  fontSize: 12,
                  color: _statusColor.withValues(alpha: opacity),
                  fontWeight: FontWeight.w500,
                ),
              ),
            ],
          ],
        );
      },
    );
  }
}
