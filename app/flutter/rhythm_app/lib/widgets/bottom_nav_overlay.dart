import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Data for a single fan menu item.
class _FanMenuEntry {
  final IconData icon;
  final Color? iconColor;
  final Color? borderColor;
  final VoidCallback onTap;

  const _FanMenuEntry({
    required this.icon,
    this.iconColor,
    this.borderColor,
    required this.onTap,
  });
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
  final _actionFanMenuKey = GlobalKey<_ActionFanMenuState>();
  bool _fanExpanded = false;
  bool _actionFanExpanded = false;

  @override
  void didUpdateWidget(covariant BottomNavOverlay oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.editMode && !oldWidget.editMode) {
      _collapseAllFanMenus();
    }
  }

  void _collapseAllFanMenus() {
    _fanMenuKey.currentState?.collapse();
    _actionFanMenuKey.currentState?.collapse();
  }

  void _onGearExpandedChanged(bool expanded) {
    if (expanded) _actionFanMenuKey.currentState?.collapse();
    if (_fanExpanded != expanded) {
      setState(() => _fanExpanded = expanded);
      widget.onSettingsModeChanged?.call(expanded);
    }
  }

  void _onActionExpandedChanged(bool expanded) {
    if (expanded) _fanMenuKey.currentState?.collapse();
    if (_actionFanExpanded != expanded) {
      setState(() => _actionFanExpanded = expanded);
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
            isVisible: _fanExpanded || _actionFanExpanded,
            onTap: _collapseAllFanMenus,
          ),
        // Nav bar content
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 12),
          child: Row(
            children: [
              // Gear fan menu (left)
              if (!widget.editMode)
                _GearFanMenu(
                  key: _fanMenuKey,
                  onSettingsTap: widget.onSettingsTap,
                  onSunPositionTap: widget.onSunPositionTap,
                  onExpandedChanged: _onGearExpandedChanged,
                ),
              // Center spacer (with optional page dots)
              Expanded(
                child: widget.totalPages > 1
                    ? Center(
                        child: _PageDots(
                          currentPage: widget.currentPage,
                          totalPages: widget.totalPages,
                          pageController: widget.pageController,
                        ),
                      )
                    : const SizedBox.shrink(),
              ),
              // Action fan menu (right)
              if (!widget.editMode && widget.onFixMyLights != null)
                _ActionFanMenu(
                  key: _actionFanMenuKey,
                  onFixMyLights: widget.onFixMyLights!,
                  isFixing: widget.isFixing,
                  onExpandedChanged: _onActionExpandedChanged,
                ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Transparent barrier that covers the screen when any fan menu is expanded.
/// Tapping it collapses all menus.
class _FanMenuBarrier extends StatelessWidget {
  final bool isVisible;
  final VoidCallback onTap;

  const _FanMenuBarrier({required this.isVisible, required this.onTap});

  @override
  Widget build(BuildContext context) {
    if (!isVisible) return const SizedBox.shrink();

    return Positioned(
      left: 0,
      right: 0,
      bottom: 0,
      top: -MediaQuery.of(context).size.height,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: onTap,
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
  }

  void collapse() {
    if (!_isExpanded) return;
    HapticFeedback.lightImpact();
    setState(() => _isExpanded = false);
    _controller.reverse();
    widget.onExpandedChanged?.call(false);
  }

  void _handleFanItemTap(VoidCallback callback) {
    HapticFeedback.selectionClick();
    setState(() => _isExpanded = false);
    _controller.reverse();
    widget.onExpandedChanged?.call(false);
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
                  color: entry.iconColor ?? CelestialColors.textSecondary,
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

/// Vertical fan-out menu for quick actions (right side of nav bar).
///
/// Trigger button fans out upward to reveal:
/// - Fix My Lights (reset all on-lights to adaptive curve)
/// - Sleep / Wake toggle (celestial moon/sun icons)
///
/// Mirrors [_GearFanMenu] animation style but with vertical layout.
class _ActionFanMenu extends StatefulWidget {
  final VoidCallback onFixMyLights;
  final bool isFixing;
  final ValueChanged<bool>? onExpandedChanged;

  const _ActionFanMenu({
    super.key,
    required this.onFixMyLights,
    required this.isFixing,
    this.onExpandedChanged,
  });

  @override
  State<_ActionFanMenu> createState() => _ActionFanMenuState();
}

class _ActionFanMenuState extends State<_ActionFanMenu>
    with TickerProviderStateMixin {
  late AnimationController _controller;
  late AnimationController _fixingController;
  late Animation<double> _triggerRotation;
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

    _triggerRotation = Tween<double>(begin: 0.0, end: math.pi / 4).animate(
      CurvedAnimation(parent: _controller, curve: Curves.easeOut),
    );

    _crossfade = Tween<double>(begin: 0.0, end: 1.0).animate(
      CurvedAnimation(
        parent: _controller,
        curve: const Interval(0.0, 0.4, curve: Curves.easeOut),
      ),
    );

    // Fan items with staggered timing (same as gear fan)
    _fanAnimations = List.generate(1, (i) {
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
            curve: Interval(start, math.min(start + 0.3, 1.0),
                curve: Curves.easeOut),
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

    // Fixing spinner rotation
    _fixingController = AnimationController(
      duration: const Duration(milliseconds: 1200),
      vsync: this,
    );
    if (widget.isFixing) _fixingController.repeat();
  }

  @override
  void didUpdateWidget(covariant _ActionFanMenu oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.isFixing && !oldWidget.isFixing) {
      _fixingController.repeat();
    } else if (!widget.isFixing && oldWidget.isFixing) {
      _fixingController.stop();
      _fixingController.reset();
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    _fixingController.dispose();
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
  }

  void collapse() {
    if (!_isExpanded) return;
    HapticFeedback.lightImpact();
    setState(() => _isExpanded = false);
    _controller.reverse();
    widget.onExpandedChanged?.call(false);
  }

  void _handleFanItemTap(VoidCallback callback) {
    HapticFeedback.selectionClick();
    setState(() => _isExpanded = false);
    _controller.reverse();
    widget.onExpandedChanged?.call(false);
    callback();
  }

  List<_FanMenuEntry> _buildEntries() {
    return [
      // Fix My Lights
      _FanMenuEntry(
        icon: Icons.auto_fix_high,
        iconColor: CelestialColors.sunWarm,
        borderColor: CelestialColors.sunWarm.withValues(alpha: 0.3),
        onTap: widget.onFixMyLights,
      ),
    ];
  }

  @override
  Widget build(BuildContext context) {
    final entries = _buildEntries();

    return AnimatedBuilder(
      animation: Listenable.merge([_controller, _fixingController]),
      builder: (context, _) {
        return Column(
          mainAxisSize: MainAxisSize.min,
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            // Fan items (expand upward — reverse order so furthest is first)
            for (int i = entries.length - 1; i >= 0; i--)
              _buildVerticalFanItem(entries[i], _fanAnimations[i]),
            // Trigger button (always visible, bottom of column)
            _buildTriggerButton(),
          ],
        );
      },
    );
  }

  Widget _buildVerticalFanItem(
      _FanMenuEntry entry, _FanItemAnimations anims) {
    return Align(
      alignment: Alignment.bottomCenter,
      heightFactor: anims.sizeReveal.value,
      child: Padding(
        padding: const EdgeInsets.only(bottom: 8),
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
                    color: entry.borderColor ??
                        CelestialColors.orbitRing.withValues(alpha: 0.5),
                    width: 1,
                  ),
                ),
                child: Icon(
                  entry.icon,
                  color: entry.iconColor ?? CelestialColors.textSecondary,
                  size: 22,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildTriggerButton() {
    return GestureDetector(
      onTap: _toggle,
      child: Transform.rotate(
        angle: _triggerRotation.value,
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
              // Wand icon (idle) or orbital spinner (fixing)
              Opacity(
                opacity: (1.0 - _crossfade.value).clamp(0.0, 1.0),
                child: widget.isFixing
                    ? Transform.rotate(
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
                      )
                    : const Icon(
                        Icons.auto_fix_high,
                        color: CelestialColors.sunWarm,
                        size: 22,
                      ),
              ),
              // Close icon (expanded)
              Transform.rotate(
                angle: -_triggerRotation.value,
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
