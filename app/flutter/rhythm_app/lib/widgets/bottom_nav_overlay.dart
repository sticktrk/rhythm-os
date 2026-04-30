import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Data for a single fan menu item.
class _FanMenuEntry {
  final IconData icon;
  final VoidCallback onTap;

  const _FanMenuEntry({
    required this.icon,
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

/// Bottom navigation overlay with direct settings/sun controls.
///
/// Floats over page content with transparent background.
/// - Left side: settings gear (or [_GearFanMenu] when [useFanOutMenus])
/// - Right side: fan-out menu (+, bug, sun) when callbacks are wired,
///   otherwise the legacy single sun button
class BottomNavOverlay extends StatefulWidget {
  final int currentPage;
  final int totalPages;
  final bool editMode;
  final VoidCallback onSettingsTap;
  final PageController? pageController;

  final VoidCallback? onSunPositionTap;
  final VoidCallback? onAddDevicesTap;
  final VoidCallback? onReportBugTap;

  /// Notifies parent when the left gear fan menu expands/collapses.
  final ValueChanged<bool>? onSettingsModeChanged;

  /// Preserves the legacy left-side fan-out for future reuse.
  final bool useFanOutMenus;
  const BottomNavOverlay({
    super.key,
    required this.currentPage,
    required this.totalPages,
    this.editMode = false,
    required this.onSettingsTap,
    this.pageController,
    this.onSunPositionTap,
    this.onAddDevicesTap,
    this.onReportBugTap,
    this.onSettingsModeChanged,
    this.useFanOutMenus = false,
  });

  @override
  State<BottomNavOverlay> createState() => _BottomNavOverlayState();
}

class _BottomNavOverlayState extends State<BottomNavOverlay> {
  final _gearFanKey = GlobalKey<_GearFanMenuState>();
  final _rightFanKey = GlobalKey<_RightFanMenuState>();
  bool _gearExpanded = false;
  bool _rightExpanded = false;

  bool get _anyFanExpanded => _gearExpanded || _rightExpanded;
  bool get _showRightFan =>
      widget.onAddDevicesTap != null && widget.onReportBugTap != null;

  @override
  void didUpdateWidget(covariant BottomNavOverlay oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.editMode && !oldWidget.editMode) {
      _collapseAllFanMenus();
    }
  }

  void _collapseAllFanMenus() {
    _gearFanKey.currentState?.collapse();
    _rightFanKey.currentState?.collapse();
  }

  void _onGearExpandedChanged(bool expanded) {
    if (_gearExpanded != expanded) {
      setState(() => _gearExpanded = expanded);
      widget.onSettingsModeChanged?.call(expanded);
    }
    if (expanded) {
      _rightFanKey.currentState?.collapse();
    }
  }

  void _onRightExpandedChanged(bool expanded) {
    if (_rightExpanded != expanded) {
      setState(() => _rightExpanded = expanded);
    }
    if (expanded) {
      _gearFanKey.currentState?.collapse();
    }
  }

  Widget _buildRightAction({
    required bool showRightFan,
    required bool showLeftFan,
  }) {
    if (showRightFan) {
      return _RightFanMenu(
        key: _rightFanKey,
        onAddDevicesTap: widget.onAddDevicesTap!,
        onReportBugTap: widget.onReportBugTap!,
        onSunPositionTap: widget.onSunPositionTap,
        onExpandedChanged: _onRightExpandedChanged,
      );
    }
    if (showLeftFan) return const SizedBox.shrink();
    if (widget.onSunPositionTap != null) {
      return _OverlayActionButton(
        icon: Icons.wb_sunny_rounded,
        onTap: widget.onSunPositionTap!,
      );
    }
    return const SizedBox.shrink();
  }

  @override
  Widget build(BuildContext context) {
    final showLeftFan = widget.useFanOutMenus;
    final showRightFan = _showRightFan;

    return Stack(
      clipBehavior: Clip.none,
      children: [
        // Dismiss barrier
        if (!widget.editMode && (showLeftFan || showRightFan))
          _FanMenuBarrier(
            isVisible: _anyFanExpanded,
            onTap: _collapseAllFanMenus,
          ),
        // Nav bar content
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 12),
          child: Row(
            children: [
              // Left action
              if (!widget.editMode)
                showLeftFan
                    ? _GearFanMenu(
                        key: _gearFanKey,
                        onSettingsTap: widget.onSettingsTap,
                        onSunPositionTap: widget.onSunPositionTap,
                        onExpandedChanged: _onGearExpandedChanged,
                      )
                    : _OverlayActionButton(
                        icon: Icons.settings,
                        onTap: widget.onSettingsTap,
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
              // Right action
              if (!widget.editMode)
                _buildRightAction(
                  showRightFan: showRightFan,
                  showLeftFan: showLeftFan,
                ),
            ],
          ),
        ),
      ],
    );
  }
}

class _OverlayActionButton extends StatelessWidget {
  final IconData icon;
  final VoidCallback onTap;

  const _OverlayActionButton({
    required this.icon,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: () {
        HapticFeedback.selectionClick();
        onTap();
      },
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
          icon,
          color: CelestialColors.textSecondary,
          size: 22,
        ),
      ),
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
      _FanMenuEntry(
          icon: Icons.wb_sunny_rounded,
          onTap: widget.onSunPositionTap ?? () {}),
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
                    color:
                        CelestialColors.backgroundCard.withValues(alpha: 0.8),
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

/// Right-side fan-out menu — toggles a (+), bug, sun trio leftward.
class _RightFanMenu extends StatefulWidget {
  final VoidCallback onAddDevicesTap;
  final VoidCallback onReportBugTap;
  final VoidCallback? onSunPositionTap;
  final ValueChanged<bool>? onExpandedChanged;

  const _RightFanMenu({
    super.key,
    required this.onAddDevicesTap,
    required this.onReportBugTap,
    this.onSunPositionTap,
    this.onExpandedChanged,
  });

  @override
  State<_RightFanMenu> createState() => _RightFanMenuState();
}

class _RightFanMenuState extends State<_RightFanMenu>
    with SingleTickerProviderStateMixin {
  late AnimationController _controller;
  late Animation<double> _toggleRotation;
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

    _toggleRotation = Tween<double>(begin: 0.0, end: math.pi / 4).animate(
      CurvedAnimation(parent: _controller, curve: Curves.easeOut),
    );

    _crossfade = Tween<double>(begin: 0.0, end: 1.0).animate(
      CurvedAnimation(
        parent: _controller,
        curve: const Interval(0.0, 0.4, curve: Curves.easeOut),
      ),
    );

    _fanAnimations = List.generate(3, (i) {
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

  /// Items are laid out left-to-right (first is the *outermost* leaf of the
  /// fan, closest to center; last sits next to the toggle).
  List<_FanMenuEntry> _buildEntries() {
    return [
      _FanMenuEntry(
        icon: Icons.bug_report_outlined,
        onTap: widget.onReportBugTap,
      ),
      _FanMenuEntry(icon: Icons.add, onTap: widget.onAddDevicesTap),
      _FanMenuEntry(
        icon: Icons.wb_sunny_rounded,
        onTap: widget.onSunPositionTap ?? () {},
      ),
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
            // Fan items (revealed leftward)
            for (int i = 0; i < entries.length; i++)
              _buildFanItem(entries[i], _fanAnimations[entries.length - 1 - i]),
            // Toggle button: more_horiz ↔ X
            GestureDetector(
              onTap: _toggle,
              child: Transform.rotate(
                angle: _toggleRotation.value,
                child: Container(
                  width: 44,
                  height: 44,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color:
                        CelestialColors.backgroundCard.withValues(alpha: 0.8),
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
                          Icons.more_horiz,
                          color: CelestialColors.textSecondary,
                          size: 22,
                        ),
                      ),
                      Transform.rotate(
                        angle: -_toggleRotation.value,
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
          ],
        );
      },
    );
  }

  Widget _buildFanItem(_FanMenuEntry entry, _FanItemAnimations anims) {
    return Align(
      alignment: Alignment.centerRight,
      widthFactor: anims.sizeReveal.value,
      child: Padding(
        padding: const EdgeInsets.only(right: 8),
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
