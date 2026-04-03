import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Data for a single fan menu item.
class _FanMenuEntry {
  final IconData icon;
  final VoidCallback onTap;

  const _FanMenuEntry({required this.icon, required this.onTap});
}

/// Pre-computed animations for a single fan item.
class _FanItemAnimations {
  final Animation<double> scale;
  final Animation<double> opacity;
  final Animation<double> sizeReveal;

  const _FanItemAnimations({
    required this.scale,
    required this.opacity,
    required this.sizeReveal,
  });
}

/// Bottom navigation overlay with gear fan-out menu.
///
/// Floats over page content with transparent background.
/// - Gear fan menu (left): Expands to show settings + power usage icons
/// - Fix My Lights pill (right): Quick-action to reset all lights
class BottomNavOverlay extends StatefulWidget {
  final int currentPage;
  final int totalPages;
  final bool editMode;
  final VoidCallback onSettingsTap;
  final PageController? pageController;
  /// Sun position callback routed to gear fan menu.
  final VoidCallback? onSunPositionTap;
  /// Notifies parent when the gear fan menu expands/collapses (settings mode).
  final ValueChanged<bool>? onSettingsModeChanged;
  /// Callback to reset all on-lights to current adaptive values.
  final VoidCallback? onFixMyLights;
  /// Whether the fix-my-lights operation is in progress.
  final bool isFixing;

  const BottomNavOverlay({
    super.key,
    required this.currentPage,
    required this.totalPages,
    this.editMode = false,
    required this.onSettingsTap,
    this.pageController,
    this.onSunPositionTap,
    this.onSettingsModeChanged,
    this.onFixMyLights,
    this.isFixing = false,
  });

  @override
  State<BottomNavOverlay> createState() => _BottomNavOverlayState();
}

class _BottomNavOverlayState extends State<BottomNavOverlay> {
  final _fanMenuKey = GlobalKey<_GearFanMenuState>();
  bool _fanExpanded = false;

  @override
  void didUpdateWidget(covariant BottomNavOverlay oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.editMode && !oldWidget.editMode) {
      _collapseFanMenu();
    }
  }

  void _collapseFanMenu() {
    _fanMenuKey.currentState?.collapse();
  }

  void _onFanExpandedChanged(bool expanded) {
    if (_fanExpanded != expanded) {
      setState(() => _fanExpanded = expanded);
      widget.onSettingsModeChanged?.call(expanded);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Stack(
      clipBehavior: Clip.none,
      children: [
        // Dismiss barrier
        if (!widget.editMode)
          _FanMenuBarrier(
            fanMenuKey: _fanMenuKey,
            onTap: _collapseFanMenu,
          ),
        // Nav bar content
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 12),
          child: Stack(
            alignment: Alignment.centerLeft,
            children: [
              // Fix My Lights pill (right-aligned)
              if (!widget.editMode && widget.onFixMyLights != null)
                Align(
                  alignment: Alignment.centerRight,
                  child: _FixMyLightsPill(
                    onTap: widget.onFixMyLights!,
                    isFixing: widget.isFixing,
                  ),
                ),
              // Page dots (centered)
              if (widget.totalPages > 1)
                Center(
                  child: _PageDots(
                    currentPage: widget.currentPage,
                    totalPages: widget.totalPages,
                    pageController: widget.pageController,
                  ),
                ),
              // Gear fan menu (left)
              if (!widget.editMode)
                _GearFanMenu(
                  key: _fanMenuKey,
                  onSettingsTap: widget.onSettingsTap,
                  onSunPositionTap: widget.onSunPositionTap,
                  onExpandedChanged: _onFanExpandedChanged,
                ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Transparent barrier that covers the screen when fan menu is expanded.
/// Tapping it collapses the fan menu.
class _FanMenuBarrier extends StatefulWidget {
  final GlobalKey<_GearFanMenuState> fanMenuKey;
  final VoidCallback onTap;

  const _FanMenuBarrier({required this.fanMenuKey, required this.onTap});

  @override
  State<_FanMenuBarrier> createState() => _FanMenuBarrierState();
}

class _FanMenuBarrierState extends State<_FanMenuBarrier> {
  bool _isExpanded = false;

  void _checkExpanded() {
    final expanded = widget.fanMenuKey.currentState?._isExpanded ?? false;
    if (expanded != _isExpanded) {
      setState(() => _isExpanded = expanded);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (!_isExpanded) return const SizedBox.shrink();

    return Positioned(
      left: 0,
      right: 0,
      bottom: 0,
      top: -MediaQuery.of(context).size.height,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: widget.onTap,
        child: const ColoredBox(color: Colors.transparent),
      ),
    );
  }
}

/// Gear icon that fans out action buttons to the right on tap.
class _GearFanMenu extends StatefulWidget {
  final VoidCallback onSettingsTap;
  final VoidCallback? onSunPositionTap;
  final ValueChanged<bool>? onExpandedChanged;

  const _GearFanMenu({
    super.key,
    required this.onSettingsTap,
    this.onSunPositionTap,
    this.onExpandedChanged,
  });

  @override
  State<_GearFanMenu> createState() => _GearFanMenuState();
}

class _GearFanMenuState extends State<_GearFanMenu>
    with SingleTickerProviderStateMixin {
  late AnimationController _controller;
  late Animation<double> _gearRotation;
  late Animation<double> _crossfade;
  late List<_FanItemAnimations> _fanAnimations;
  bool _isExpanded = false;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      duration: const Duration(milliseconds: 450),
      vsync: this,
    );

    _gearRotation = Tween<double>(begin: 0.0, end: math.pi / 4).animate(
      CurvedAnimation(parent: _controller, curve: Curves.easeOut),
    );

    _crossfade = Tween<double>(begin: 0.0, end: 1.0).animate(
      CurvedAnimation(
        parent: _controller,
        curve: const Interval(0.0, 0.4, curve: Curves.easeOut),
      ),
    );

    _fanAnimations = List.generate(2, (i) {
      final start = 0.05 + i * 0.12;
      final end = math.min(start + 0.55, 1.0);

      return _FanItemAnimations(
        scale: Tween<double>(begin: 0.0, end: 1.0).animate(
          CurvedAnimation(
            parent: _controller,
            curve: Interval(start, end, curve: Curves.elasticOut),
          ),
        ),
        opacity: Tween<double>(begin: 0.0, end: 1.0).animate(
          CurvedAnimation(
            parent: _controller,
            curve: Interval(start, math.min(start + 0.3, 1.0), curve: Curves.easeOut),
          ),
        ),
        sizeReveal: Tween<double>(begin: 0.0, end: 1.0).animate(
          CurvedAnimation(
            parent: _controller,
            curve: Interval(start, end, curve: Curves.easeOutCubic),
          ),
        ),
      );
    });
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _toggle() {
    HapticFeedback.mediumImpact();
    setState(() => _isExpanded = !_isExpanded);
    if (_isExpanded) {
      _controller.forward();
    } else {
      _controller.reverse();
    }
    widget.onExpandedChanged?.call(_isExpanded);
    _notifyBarrier();
  }

  void collapse() {
    if (!_isExpanded) return;
    HapticFeedback.lightImpact();
    setState(() => _isExpanded = false);
    _controller.reverse();
    widget.onExpandedChanged?.call(false);
    _notifyBarrier();
  }

  void _notifyBarrier() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final overlayState = context.findAncestorStateOfType<_BottomNavOverlayState>();
      if (overlayState == null) return;
      void visitor(Element element) {
        if (element is StatefulElement && element.state is _FanMenuBarrierState) {
          (element.state as _FanMenuBarrierState)._checkExpanded();
          return;
        }
        element.visitChildren(visitor);
      }
      overlayState.context.visitChildElements(visitor);
    });
  }

  void _handleFanItemTap(VoidCallback callback) {
    HapticFeedback.selectionClick();
    setState(() => _isExpanded = false);
    _controller.reverse();
    widget.onExpandedChanged?.call(false);
    _notifyBarrier();
    callback();
  }

  List<_FanMenuEntry> _buildEntries() {
    return [
      _FanMenuEntry(icon: Icons.settings, onTap: widget.onSettingsTap),
      _FanMenuEntry(icon: Icons.wb_sunny_rounded, onTap: widget.onSunPositionTap ?? () {}),
    ];
  }

  @override
  Widget build(BuildContext context) {
    final entries = _buildEntries();

    return AnimatedBuilder(
      animation: _controller,
      builder: (context, _) {
        return Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            // Toggle button: gear ↔ X
            GestureDetector(
              onTap: _toggle,
              child: Transform.rotate(
                angle: _gearRotation.value,
                child: Container(
                  width: 44,
                  height: 44,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: CelestialColors.backgroundCard.withValues(alpha: 0.8),
                    border: Border.all(
                      color: CelestialColors.orbitRing.withValues(alpha: 0.5),
                      width: 1,
                    ),
                  ),
                  child: Stack(
                    alignment: Alignment.center,
                    children: [
                      Opacity(
                        opacity: (1.0 - _crossfade.value).clamp(0.0, 1.0),
                        child: const Icon(
                          Icons.settings,
                          color: CelestialColors.textSecondary,
                          size: 22,
                        ),
                      ),
                      Transform.rotate(
                        angle: -_gearRotation.value,
                        child: Opacity(
                          opacity: _crossfade.value.clamp(0.0, 1.0),
                          child: const Icon(
                            Icons.close,
                            color: CelestialColors.textSecondary,
                            size: 22,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
            // Fan items
            for (int i = 0; i < entries.length; i++)
              _buildFanItem(entries[i], _fanAnimations[i]),
          ],
        );
      },
    );
  }

  Widget _buildFanItem(_FanMenuEntry entry, _FanItemAnimations anims) {
    return Align(
      alignment: Alignment.centerLeft,
      widthFactor: anims.sizeReveal.value,
      child: Padding(
        padding: const EdgeInsets.only(left: 8),
        child: Opacity(
          opacity: anims.opacity.value.clamp(0.0, 1.0),
          child: Transform.scale(
            scale: anims.scale.value.clamp(0.0, 1.5),
            child: GestureDetector(
              onTap: () => _handleFanItemTap(entry.onTap),
              child: Container(
                width: 44,
                height: 44,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: CelestialColors.backgroundCard.withValues(alpha: 0.8),
                  border: Border.all(
                    color: CelestialColors.orbitRing.withValues(alpha: 0.5),
                    width: 1,
                  ),
                ),
                child: Icon(
                  entry.icon,
                  color: CelestialColors.textSecondary,
                  size: 22,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Pill-shaped button that triggers "Fix My Lights" action.
///
/// Features a warm ambient glow, press-scale animation, and an orbital
/// spinner during the loading state — consistent with the celestial theme.
class _FixMyLightsPill extends StatefulWidget {
  final VoidCallback onTap;
  final bool isFixing;

  const _FixMyLightsPill({required this.onTap, required this.isFixing});

  @override
  State<_FixMyLightsPill> createState() => _FixMyLightsPillState();
}

class _FixMyLightsPillState extends State<_FixMyLightsPill>
    with TickerProviderStateMixin {
  late AnimationController _pressController;
  late AnimationController _glowController;
  late AnimationController _fixingController;
  late Animation<double> _pressScale;
  late Animation<double> _glowPulse;

  @override
  void initState() {
    super.initState();

    // Press feedback: quick scale-down and bounce back
    _pressController = AnimationController(
      duration: const Duration(milliseconds: 120),
      reverseDuration: const Duration(milliseconds: 200),
      vsync: this,
    );
    _pressScale = Tween<double>(begin: 1.0, end: 0.92).animate(
      CurvedAnimation(parent: _pressController, curve: Curves.easeInOut),
    );

    // Ambient glow pulse (idle state)
    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2400),
      vsync: this,
    )..repeat(reverse: true);
    _glowPulse = Tween<double>(begin: 0.0, end: 1.0).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );

    // Fixing spinner rotation
    _fixingController = AnimationController(
      duration: const Duration(milliseconds: 1200),
      vsync: this,
    );
    if (widget.isFixing) _fixingController.repeat();
  }

  @override
  void didUpdateWidget(covariant _FixMyLightsPill oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.isFixing && !oldWidget.isFixing) {
      _fixingController.repeat();
      _glowController.stop();
    } else if (!widget.isFixing && oldWidget.isFixing) {
      _fixingController.stop();
      _fixingController.reset();
      _glowController.repeat(reverse: true);
    }
  }

  @override
  void dispose() {
    _pressController.dispose();
    _glowController.dispose();
    _fixingController.dispose();
    super.dispose();
  }

  void _handleTapDown(TapDownDetails _) {
    if (!widget.isFixing) _pressController.forward();
  }

  void _handleTapUp(TapUpDetails _) {
    _pressController.reverse();
  }

  void _handleTapCancel() {
    _pressController.reverse();
  }

  void _handleTap() {
    if (!widget.isFixing) widget.onTap();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: Listenable.merge([_pressScale, _glowPulse, _fixingController]),
      builder: (context, child) {
        final glowOpacity = widget.isFixing ? 0.5 : 0.15 + 0.15 * _glowPulse.value;
        final glowSpread = widget.isFixing ? 12.0 : 4.0 + 4.0 * _glowPulse.value;
        final glowBlur = widget.isFixing ? 20.0 : 8.0 + 8.0 * _glowPulse.value;
        final borderColor = widget.isFixing
            ? CelestialColors.sunWarm.withValues(alpha: 0.6)
            : Color.lerp(
                CelestialColors.orbitRing.withValues(alpha: 0.5),
                CelestialColors.sunWarm.withValues(alpha: 0.35),
                _glowPulse.value,
              )!;

        return Transform.scale(
          scale: _pressScale.value,
          child: GestureDetector(
            onTapDown: _handleTapDown,
            onTapUp: _handleTapUp,
            onTapCancel: _handleTapCancel,
            onTap: _handleTap,
            child: Container(
              height: 44,
              padding: const EdgeInsets.symmetric(horizontal: 16),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(22),
                color: CelestialColors.backgroundCard.withValues(alpha: 0.85),
                border: Border.all(color: borderColor, width: 1),
                boxShadow: [
                  BoxShadow(
                    color: CelestialColors.sunWarm.withValues(alpha: glowOpacity),
                    blurRadius: glowBlur,
                    spreadRadius: glowSpread,
                  ),
                ],
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  _buildIcon(),
                  const SizedBox(width: 8),
                  Text(
                    widget.isFixing ? 'Fixing...' : 'Fix My Lights',
                    style: TextStyle(
                      color: widget.isFixing
                          ? CelestialColors.sunWarm
                          : CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight: FontWeight.w500,
                      letterSpacing: 0.3,
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  Widget _buildIcon() {
    if (widget.isFixing) {
      return Transform.rotate(
        angle: _fixingController.value * 2 * math.pi,
        child: SizedBox(
          width: 20,
          height: 20,
          child: CustomPaint(
            painter: _OrbitalSpinnerPainter(
              color: CelestialColors.sunWarm,
              progress: _fixingController.value,
            ),
          ),
        ),
      );
    }
    return const Icon(
      Icons.auto_fix_high,
      color: CelestialColors.sunWarm,
      size: 20,
    );
  }
}

/// Draws a tapered arc that looks like a small sun orbiting — matching
/// the app's orbital ring motif rather than a generic spinner.
class _OrbitalSpinnerPainter extends CustomPainter {
  final Color color;
  final double progress;

  _OrbitalSpinnerPainter({required this.color, required this.progress});

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = size.width / 2 - 1.5;

    // Track ring (faint)
    final trackPaint = Paint()
      ..color = color.withValues(alpha: 0.15)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2.0;
    canvas.drawCircle(center, radius, trackPaint);

    // Sweeping arc with tapered ends
    const sweepAngle = math.pi * 0.8;
    final startAngle = progress * 2 * math.pi - math.pi / 2;
    final arcPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2.0
      ..strokeCap = StrokeCap.round
      ..shader = SweepGradient(
        startAngle: startAngle,
        endAngle: startAngle + sweepAngle,
        colors: [color.withValues(alpha: 0.0), color],
        stops: const [0.0, 1.0],
      ).createShader(Rect.fromCircle(center: center, radius: radius));
    canvas.drawArc(
      Rect.fromCircle(center: center, radius: radius),
      startAngle,
      sweepAngle,
      false,
      arcPaint,
    );

    // Leading dot (the "sun" on the orbit)
    final dotAngle = startAngle + sweepAngle;
    final dotCenter = Offset(
      center.dx + radius * math.cos(dotAngle),
      center.dy + radius * math.sin(dotAngle),
    );
    final dotPaint = Paint()..color = color;
    canvas.drawCircle(dotCenter, 2.5, dotPaint);
  }

  @override
  bool shouldRepaint(_OrbitalSpinnerPainter oldDelegate) =>
      progress != oldDelegate.progress;
}

/// Page indicator dots for multi-screen room layout.
class _PageDots extends StatelessWidget {
  final int currentPage;
  final int totalPages;
  final PageController? pageController;

  const _PageDots({
    required this.currentPage,
    required this.totalPages,
    this.pageController,
  });

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: List.generate(totalPages, (index) {
        final isActive = index == currentPage;
        return GestureDetector(
          onTap: () {
            pageController?.animateToPage(
              index,
              duration: const Duration(milliseconds: 300),
              curve: Curves.easeInOut,
            );
          },
          behavior: HitTestBehavior.opaque,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 8),
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 200),
              width: isActive ? 8 : 6,
              height: isActive ? 8 : 6,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: isActive
                    ? CelestialColors.textPrimary
                    : CelestialColors.textSecondary.withValues(alpha: 0.4),
              ),
            ),
          ),
        );
      }),
    );
  }
}
