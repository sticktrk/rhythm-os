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
/// - Default mode shows a settings gear on the left and sun screen on the right
/// - Fan-out menus remain available behind [useFanOutMenus] for later reuse
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

  /// Preserves the existing fan-out implementations for future reuse.
  final bool useFanOutMenus;
  const BottomNavOverlay({
    super.key,
    required this.currentPage,
    required this.totalPages,
    this.editMode = false,
    required this.onSettingsTap,
    this.pageController,
    this.onSunPositionTap,
    this.onSettingsModeChanged,
    this.useFanOutMenus = false,
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
    if (widget.useFanOutMenus && widget.editMode && !oldWidget.editMode) {
      _collapseAllFanMenus();
    }
  }

  void _collapseAllFanMenus() {
    _fanMenuKey.currentState?.collapse();
  }

  void _onGearExpandedChanged(bool expanded) {
    if (_fanExpanded != expanded) {
      setState(() => _fanExpanded = expanded);
      widget.onSettingsModeChanged?.call(expanded);
    }
  }

  @override
  Widget build(BuildContext context) {
    final showFanOutMenus = widget.useFanOutMenus;

    return Stack(
      clipBehavior: Clip.none,
      children: [
        // Dismiss barrier
        if (!widget.editMode && showFanOutMenus)
          _FanMenuBarrier(
            isVisible: _fanExpanded,
            onTap: _collapseAllFanMenus,
          ),
        // Nav bar content
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 12),
          child: Row(
            children: [
              // Left action
              if (!widget.editMode)
                showFanOutMenus
                    ? _GearFanMenu(
                        key: _fanMenuKey,
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
                showFanOutMenus
                    ? const SizedBox.shrink()
                    : (widget.onSunPositionTap != null
                        ? _OverlayActionButton(
                            icon: Icons.wb_sunny_rounded,
                            onTap: widget.onSunPositionTap!,
                          )
                        : const SizedBox.shrink()),
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
