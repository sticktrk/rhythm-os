import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

/// Which kind of mood the sheet is editing.
enum MoodTab { color, scenes, hue }

typedef MoodSceneSelected = Future<bool> Function(
  RhythmSceneDefinition scene,
);

/// Slide-up "mood" card. A mood can be **either** a custom color or one of the
/// saved Rhythm scenes (presets). A segmented toggle switches between a color
/// wheel and a gallery of luminous scene tiles; the card's ambient glow tracks
/// whichever the user is shaping.
class MoodSheet extends StatefulWidget {
  final Color? initialColor;
  final String? initialSceneId;
  final MoodTab initialTab;
  final List<RhythmSceneDefinition> initialScenes;
  final Future<List<RhythmSceneDefinition>> Function() scenesLoader;
  final ValueChanged<Color> onColorChanged;
  final MoodSceneSelected onSceneSelected;
  final ValueChanged<MoodTab>? onTabChanged;
  final bool showHueTab;

  const MoodSheet({
    super.key,
    required this.scenesLoader,
    required this.onColorChanged,
    required this.onSceneSelected,
    this.initialColor,
    this.initialSceneId,
    this.initialTab = MoodTab.color,
    this.initialScenes = const [],
    this.onTabChanged,
    this.showHueTab = false,
  });

  /// Show the mood picker as a modal bottom sheet.
  static Future<void> show(
    BuildContext context, {
    required Future<List<RhythmSceneDefinition>> Function() scenesLoader,
    required ValueChanged<Color> onColorChanged,
    required MoodSceneSelected onSceneSelected,
    Color? initialColor,
    String? initialSceneId,
    MoodTab initialTab = MoodTab.color,
    List<RhythmSceneDefinition> initialScenes = const [],
    ValueChanged<MoodTab>? onTabChanged,
    bool showHueTab = false,
  }) {
    return showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      barrierColor: Colors.black54,
      builder: (_) => MoodSheet(
        scenesLoader: scenesLoader,
        onColorChanged: onColorChanged,
        onSceneSelected: onSceneSelected,
        initialColor: initialColor,
        initialSceneId: initialSceneId,
        initialTab: initialTab,
        initialScenes: initialScenes,
        onTabChanged: onTabChanged,
        showHueTab: showHueTab,
      ),
    );
  }

  @override
  State<MoodSheet> createState() => _MoodSheetState();
}

class _MoodSheetState extends State<MoodSheet> with TickerProviderStateMixin {
  late MoodTab _tab;

  // Color state.
  late double _hue;
  late double _saturation;

  // Scene state.
  late List<RhythmSceneDefinition> _scenes;
  late bool _loadingScenes;
  bool _scenesLoadStarted = false;
  bool _showMoreHueScenes = false;
  int _sceneSelectionGeneration = 0;
  String? _selectedSceneId;

  late final AnimationController _glowPulse;
  late final AnimationController _enter;

  static const _bg = Color(0xFF12151D);

  @override
  void initState() {
    super.initState();
    _tab = widget.initialTab == MoodTab.hue && !widget.showHueTab
        ? MoodTab.scenes
        : widget.initialTab;
    final initialHsv = widget.initialColor == null
        ? const HSVColor.fromAHSV(1, 35, 0.85, 1)
        : HSVColor.fromColor(widget.initialColor!);
    _hue = initialHsv.hue;
    _saturation = initialHsv.saturation.clamp(0.0, 1.0);
    _scenes = widget.initialScenes;
    _selectedSceneId = widget.initialSceneId;
    _loadingScenes = _tab != MoodTab.color && widget.initialScenes.isEmpty;

    _glowPulse = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 4),
    )..repeat(reverse: true);
    _enter = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 650),
    )..forward();

    if (_tab != MoodTab.color) {
      _ensureScenesLoaded();
    }
  }

  Future<void> _ensureScenesLoaded() async {
    if (_scenesLoadStarted) return;
    _scenesLoadStarted = true;
    if (_scenes.isEmpty && !_loadingScenes) {
      setState(() => _loadingScenes = true);
    }

    try {
      final scenes = await widget.scenesLoader();
      if (!mounted) return;
      setState(() {
        _scenes = scenes;
        _loadingScenes = false;
      });
    } catch (_) {
      if (!mounted) return;
      setState(() => _loadingScenes = false);
    }
  }

  @override
  void dispose() {
    _glowPulse.dispose();
    _enter.dispose();
    super.dispose();
  }

  Color get _selectedColor =>
      HSVColor.fromAHSV(1, _hue, _saturation, 1).toColor();

  RhythmSceneDefinition? get _selectedScene {
    for (final s in _scenes) {
      if (s.id == _selectedSceneId) return s;
    }
    return null;
  }

  Color get _ambientColor {
    if (_tab == MoodTab.color) return _selectedColor;
    final tabScenes = _tab == MoodTab.hue
        ? _scenes.where((scene) => scene.isImportedHueScene)
        : _scenes.where((scene) => !scene.isImportedHueScene);
    final scene =
        _selectedScene ?? (tabScenes.isNotEmpty ? tabScenes.first : null);
    return scene == null ? const Color(0xFFFF9500) : rhythmSceneColor(scene);
  }

  // --- Color interactions --------------------------------------------------

  void _setColor(double hue, double saturation, {bool commit = true}) {
    setState(() {
      _hue = hue % 360;
      _saturation = saturation.clamp(0.0, 1.0);
      _selectedSceneId = null;
    });
    if (commit) widget.onColorChanged(_selectedColor);
  }

  void _commitColor() {
    setState(() => _selectedSceneId = null);
    widget.onColorChanged(_selectedColor);
  }

  void _onColorWheel(Offset localPosition, Size size,
      {bool haptic = false, bool commit = true}) {
    final shortestSide = math.min(size.width, size.height);
    if (shortestSide <= 0) return;
    final center = Offset(size.width / 2, size.height / 2);
    final radius = shortestSide / 2;
    final vector = localPosition - center;
    final distance = vector.distance;
    final saturation = (distance / radius).clamp(0.0, 1.0);
    final hue = distance <= 0.5
        ? _hue
        : ((math.atan2(vector.dy, vector.dx) * 180 / math.pi) + 360) % 360;
    if (haptic) HapticFeedback.selectionClick();
    _setColor(hue, saturation, commit: commit);
  }

  Future<void> _onSceneTap(RhythmSceneDefinition scene) async {
    HapticFeedback.mediumImpact();
    final previousSceneId = _selectedSceneId;
    final generation = ++_sceneSelectionGeneration;
    setState(() => _selectedSceneId = scene.id);
    var applied = false;
    try {
      applied = await widget.onSceneSelected(scene);
    } catch (_) {
      applied = false;
    }
    if (!mounted ||
        applied ||
        generation != _sceneSelectionGeneration ||
        _selectedSceneId != scene.id) {
      return;
    }
    setState(() => _selectedSceneId = previousSceneId);
  }

  void _switchTab(MoodTab tab) {
    if (_tab == tab) return;
    HapticFeedback.selectionClick();
    setState(() => _tab = tab);
    widget.onTabChanged?.call(tab);
    if (tab != MoodTab.color) {
      _ensureScenesLoaded();
    }
  }

  @override
  Widget build(BuildContext context) {
    final bottomPad = MediaQuery.of(context).padding.bottom;
    final maxHeight = MediaQuery.of(context).size.height * 0.72;
    final ambient = _ambientColor;

    return ConstrainedBox(
      constraints: BoxConstraints(maxHeight: maxHeight),
      child: Container(
        decoration: const BoxDecoration(
          color: _bg,
          borderRadius: BorderRadius.vertical(top: Radius.circular(28)),
        ),
        child: Stack(
          children: [
            Positioned.fill(
              child: IgnorePointer(
                child: AnimatedBuilder(
                  animation: _glowPulse,
                  builder: (context, _) {
                    final pulse = 0.75 + _glowPulse.value * 0.25;
                    return DecoratedBox(
                      decoration: BoxDecoration(
                        borderRadius: const BorderRadius.vertical(
                            top: Radius.circular(28)),
                        gradient: RadialGradient(
                          center: const Alignment(0.0, -1.0),
                          radius: 1.5,
                          colors: [
                            ambient.withValues(alpha: 0.22 * pulse),
                            ambient.withValues(alpha: 0.06 * pulse),
                            Colors.transparent,
                          ],
                          stops: const [0.0, 0.4, 1.0],
                        ),
                      ),
                    );
                  },
                ),
              ),
            ),
            Padding(
              padding: EdgeInsets.fromLTRB(20, 12, 20, 16 + bottomPad),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Center(child: _dragHandle()),
                  const SizedBox(height: 18),
                  _MoodTabToggle(
                    tab: _tab,
                    accent: ambient,
                    onChanged: _switchTab,
                    showHueTab: widget.showHueTab,
                  ),
                  const SizedBox(height: 22),
                  Flexible(
                    child: AnimatedSize(
                      duration: const Duration(milliseconds: 300),
                      curve: Curves.easeOutCubic,
                      alignment: Alignment.topCenter,
                      child: AnimatedSwitcher(
                        duration: const Duration(milliseconds: 280),
                        switchInCurve: Curves.easeOutCubic,
                        switchOutCurve: Curves.easeIn,
                        transitionBuilder: (child, anim) => FadeTransition(
                          opacity: anim,
                          child: child,
                        ),
                        layoutBuilder: (current, previous) => Stack(
                          alignment: Alignment.topCenter,
                          children: [
                            ...previous,
                            if (current != null) current,
                          ],
                        ),
                        child: switch (_tab) {
                          MoodTab.color => _colorPane(),
                          MoodTab.scenes => _scenePane(),
                          MoodTab.hue => _huePane(),
                        },
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _dragHandle() => Container(
        width: 40,
        height: 4,
        decoration: BoxDecoration(
          color: Colors.white.withValues(alpha: 0.18),
          borderRadius: BorderRadius.circular(2),
        ),
      );

  // --- Color pane ----------------------------------------------------------

  Widget _colorPane() {
    return LayoutBuilder(
      key: const ValueKey(MoodTab.color),
      builder: (context, constraints) {
        final maxWheelSide = constraints.hasBoundedHeight
            ? math.min(252.0, math.max(140.0, constraints.maxHeight - 18))
            : 252.0;

        return Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const SizedBox(height: 8),
            _colorWheel(maxWheelSide),
            const SizedBox(height: 10),
          ],
        );
      },
    );
  }

  Widget _colorWheel(double maxSide) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final side = math.min(constraints.maxWidth, maxSide);
        final radius = side / 2;
        final markerAngle = _hue * math.pi / 180;
        final markerOffset = Offset(
          radius + math.cos(markerAngle) * radius * _saturation,
          radius + math.sin(markerAngle) * radius * _saturation,
        );

        return Center(
          child: Semantics(
            label: 'Mood color wheel',
            value: 'Selected color',
            child: GestureDetector(
              key: const Key('mood_color_spectrum'),
              onTapDown: (d) => _onColorWheel(
                d.localPosition,
                Size.square(side),
                haptic: true,
              ),
              onPanStart: (d) => _onColorWheel(
                d.localPosition,
                Size.square(side),
                haptic: true,
                commit: false,
              ),
              onPanUpdate: (d) => _onColorWheel(
                d.localPosition,
                Size.square(side),
                commit: false,
              ),
              onPanEnd: (_) => _commitColor(),
              child: SizedBox.square(
                key: const Key('mood_color_wheel'),
                dimension: side,
                child: Stack(
                  clipBehavior: Clip.none,
                  children: [
                    Positioned.fill(
                      child: CustomPaint(
                        painter: const _RgbColorWheelPainter(),
                      ),
                    ),
                    Positioned(
                      left: markerOffset.dx - 14,
                      top: markerOffset.dy - 14,
                      child: _ColorWheelThumb(color: _selectedColor),
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

  // --- Scene pane ----------------------------------------------------------

  Widget _scenePane() {
    if (_loadingScenes) {
      return _scenesLoadingState(MoodTab.scenes);
    }
    final scenes = _scenes.where((scene) => !scene.isImportedHueScene).toList();
    if (scenes.isEmpty) {
      return _scenesEmptyState();
    }
    return _sceneGrid(scenes, MoodTab.scenes);
  }

  Widget _huePane() {
    if (_loadingScenes) {
      return _scenesLoadingState(MoodTab.hue);
    }
    final paletteScenes =
        _scenes.where((scene) => scene.isHuePaletteScene).toList();
    final otherScenes = _scenes
        .where((scene) => scene.isImportedHueScene && !scene.isHuePaletteScene)
        .toList();
    if (paletteScenes.isEmpty && otherScenes.isEmpty) {
      return _hueScenesEmptyState();
    }

    return CustomScrollView(
      key: const ValueKey(MoodTab.hue),
      physics: const BouncingScrollPhysics(),
      slivers: [
        if (paletteScenes.isNotEmpty)
          SliverGrid(
            delegate: SliverChildBuilderDelegate(
              (context, index) => _animatedSceneTile(
                paletteScenes[index],
                index,
              ),
              childCount: paletteScenes.length,
            ),
            gridDelegate: _sceneGridDelegate,
          ),
        if (otherScenes.isNotEmpty)
          SliverToBoxAdapter(
            child: Padding(
              padding: EdgeInsets.only(
                top: paletteScenes.isEmpty ? 0 : 14,
                bottom: _showMoreHueScenes ? 14 : 4,
              ),
              child: Semantics(
                button: true,
                expanded: _showMoreHueScenes,
                child: InkWell(
                  key: const Key('mood_hue_more_toggle'),
                  borderRadius: BorderRadius.circular(16),
                  onTap: () => setState(
                    () => _showMoreHueScenes = !_showMoreHueScenes,
                  ),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                        horizontal: 14, vertical: 12),
                    child: Row(
                      children: [
                        const Expanded(
                          child: Text(
                            'More',
                            style: TextStyle(
                              color: Colors.white,
                              fontSize: 14,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                        ),
                        Text(
                          '${otherScenes.length}',
                          style: TextStyle(
                            color: Colors.white.withValues(alpha: 0.5),
                            fontSize: 12,
                          ),
                        ),
                        const SizedBox(width: 8),
                        AnimatedRotation(
                          duration: const Duration(milliseconds: 200),
                          turns: _showMoreHueScenes ? 0.5 : 0,
                          child: Icon(
                            Icons.expand_more_rounded,
                            color: Colors.white.withValues(alpha: 0.7),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        if (_showMoreHueScenes)
          SliverGrid(
            delegate: SliverChildBuilderDelegate(
              (context, index) => _animatedSceneTile(
                otherScenes[index],
                paletteScenes.length + index,
              ),
              childCount: otherScenes.length,
            ),
            gridDelegate: _sceneGridDelegate,
          ),
        const SliverToBoxAdapter(child: SizedBox(height: 4)),
      ],
    );
  }

  static const _sceneGridDelegate = SliverGridDelegateWithFixedCrossAxisCount(
    crossAxisCount: 2,
    mainAxisSpacing: 14,
    crossAxisSpacing: 14,
    childAspectRatio: 1.5,
  );

  Widget _sceneGrid(
    List<RhythmSceneDefinition> scenes,
    MoodTab tab,
  ) {
    return GridView.builder(
      key: ValueKey(tab),
      shrinkWrap: true,
      padding: const EdgeInsets.only(bottom: 4),
      physics: const BouncingScrollPhysics(),
      gridDelegate: _sceneGridDelegate,
      itemCount: scenes.length,
      itemBuilder: (context, i) => _animatedSceneTile(scenes[i], i),
    );
  }

  Widget _animatedSceneTile(RhythmSceneDefinition scene, int index) {
    final start = (index * 0.06).clamp(0.0, 0.6);
    final anim = CurvedAnimation(
      parent: _enter,
      curve: Interval(
        start,
        (start + 0.5).clamp(0.0, 1.0),
        curve: Curves.easeOutCubic,
      ),
    );
    return AnimatedBuilder(
      animation: anim,
      builder: (context, child) => Opacity(
        opacity: anim.value,
        child: Transform.translate(
          offset: Offset(0, (1 - anim.value) * 16),
          child: child,
        ),
      ),
      child: _SceneTile(
        scene: scene,
        selected: scene.id == _selectedSceneId,
        pulse: _glowPulse,
        onTap: () => _onSceneTap(scene),
      ),
    );
  }

  Widget _scenesLoadingState(MoodTab tab) => Padding(
        key: ValueKey('${tab.name}-loading'),
        padding: const EdgeInsets.symmetric(vertical: 48),
        child: const Center(
          child: SizedBox(
            width: 26,
            height: 26,
            child: CircularProgressIndicator(strokeWidth: 2.4),
          ),
        ),
      );

  Widget _scenesEmptyState() {
    return Padding(
      key: const ValueKey('scenes-empty'),
      padding: const EdgeInsets.symmetric(vertical: 36, horizontal: 12),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.auto_awesome_outlined,
              size: 36, color: Colors.white.withValues(alpha: 0.3)),
          const SizedBox(height: 14),
          const Text(
            'No scenes yet',
            style: TextStyle(
              color: Colors.white,
              fontSize: 16,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 6),
          Text(
            'Create scenes on your Rhythm to use them as moods, '
            'or pick a color instead.',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: Colors.white.withValues(alpha: 0.5),
              fontSize: 13,
              height: 1.4,
            ),
          ),
        ],
      ),
    );
  }

  Widget _hueScenesEmptyState() {
    return Padding(
      key: const ValueKey('hue-scenes-empty'),
      padding: const EdgeInsets.symmetric(vertical: 36, horizontal: 12),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.lightbulb_outline_rounded,
            size: 36,
            color: Colors.white.withValues(alpha: 0.3),
          ),
          const SizedBox(height: 14),
          const Text(
            'No Hue scenes',
            style: TextStyle(
              color: Colors.white,
              fontSize: 16,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }
}

/// Segmented pill toggle for switching between Color and Scenes.
class _MoodTabToggle extends StatelessWidget {
  final MoodTab tab;
  final Color accent;
  final ValueChanged<MoodTab> onChanged;
  final bool showHueTab;

  const _MoodTabToggle({
    required this.tab,
    required this.accent,
    required this.onChanged,
    required this.showHueTab,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 46,
      padding: const EdgeInsets.all(4),
      decoration: BoxDecoration(
        color: Colors.white.withValues(alpha: 0.06),
        borderRadius: BorderRadius.circular(23),
        border: Border.all(color: Colors.white.withValues(alpha: 0.06)),
      ),
      child: Stack(
        children: [
          AnimatedAlign(
            duration: const Duration(milliseconds: 260),
            curve: Curves.easeOutCubic,
            alignment: switch (tab) {
              MoodTab.color => Alignment.centerLeft,
              MoodTab.scenes =>
                showHueTab ? Alignment.center : Alignment.centerRight,
              MoodTab.hue => Alignment.centerRight,
            },
            child: FractionallySizedBox(
              widthFactor: showHueTab ? 1 / 3 : 0.5,
              heightFactor: 1,
              child: Container(
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(19),
                  gradient: LinearGradient(
                    colors: [
                      accent.withValues(alpha: 0.85),
                      accent.withValues(alpha: 0.55),
                    ],
                  ),
                  boxShadow: [
                    BoxShadow(
                      color: accent.withValues(alpha: 0.4),
                      blurRadius: 12,
                      spreadRadius: -1,
                    ),
                  ],
                ),
              ),
            ),
          ),
          Positioned.fill(
            child: Row(
              children: [
                _segment(MoodTab.color, Icons.palette_rounded, 'Color'),
                _segment(MoodTab.scenes, Icons.auto_awesome_rounded, 'Scenes'),
                if (showHueTab)
                  _segment(MoodTab.hue, Icons.lightbulb_rounded, 'Hue'),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _segment(MoodTab value, IconData icon, String label) {
    final active = tab == value;
    final fg = active ? Colors.white : Colors.white.withValues(alpha: 0.55);
    return Expanded(
      child: GestureDetector(
        key: Key('mood_tab_${value.name}'),
        behavior: HitTestBehavior.opaque,
        onTap: () => onChanged(value),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          crossAxisAlignment: CrossAxisAlignment.center,
          children: [
            Icon(icon, size: 16, color: fg),
            const SizedBox(width: 7),
            AnimatedDefaultTextStyle(
              duration: const Duration(milliseconds: 200),
              style: TextStyle(
                color: fg,
                fontSize: 14,
                fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                letterSpacing: -0.2,
              ),
              child: Text(label),
            ),
          ],
        ),
      ),
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
  final Color color;

  const _ColorWheelThumb({required this.color});

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

/// A compact circular swatch representing a mood's colors.
///
/// One color renders as a solid disc; multiple colors render as hard pie
/// segments — a little palette wheel. Works for both a custom-color mood and a
/// scene's multi-color palette.
class MoodPaletteBadge extends StatelessWidget {
  final List<Color> colors;
  final double size;
  final bool glow;
  final double borderWidth;

  const MoodPaletteBadge({
    super.key,
    required this.colors,
    this.size = 28,
    this.glow = true,
    this.borderWidth = 1.5,
  });

  @override
  Widget build(BuildContext context) {
    final cols = colors.isEmpty ? const [Color(0xFFFF9500)] : colors;
    final primary = cols.first;

    Gradient? gradient;
    Color? solid;
    if (cols.length == 1) {
      solid = primary;
    } else {
      // Hard-edged pie segments around the wheel.
      final sweepColors = <Color>[];
      final stops = <double>[];
      for (var i = 0; i < cols.length; i++) {
        sweepColors
          ..add(cols[i])
          ..add(cols[i]);
        stops
          ..add(i / cols.length)
          ..add((i + 1) / cols.length);
      }
      gradient = SweepGradient(colors: sweepColors, stops: stops);
    }

    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: solid,
        gradient: gradient,
        border: Border.all(
          color: Colors.white.withValues(alpha: 0.85),
          width: borderWidth,
        ),
        boxShadow: glow
            ? [
                BoxShadow(
                  color: primary.withValues(alpha: 0.5),
                  blurRadius: size * 0.45,
                  spreadRadius: 0.5,
                ),
              ]
            : null,
      ),
    );
  }
}

/// A single luminous scene tile.
class _SceneTile extends StatelessWidget {
  final RhythmSceneDefinition scene;
  final bool selected;
  final Animation<double> pulse;
  final VoidCallback onTap;

  const _SceneTile({
    required this.scene,
    required this.selected,
    required this.pulse,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final swatch = rhythmSceneSwatch(scene);
    final primary = swatch.first;
    final lightCount = scene.light.entries.length;

    return GestureDetector(
      key: Key('mood_scene_${scene.id}'),
      onTap: onTap,
      child: AnimatedBuilder(
        animation: pulse,
        builder: (context, _) {
          final t = pulse.value;
          return AnimatedContainer(
            duration: const Duration(milliseconds: 220),
            curve: Curves.easeOut,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(20),
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: swatch.length == 1
                    ? [primary, _shade(primary, 0.6)]
                    : swatch,
              ),
              border: Border.all(
                color: selected
                    ? Colors.white.withValues(alpha: 0.9)
                    : Colors.white.withValues(alpha: 0.08),
                width: selected ? 2 : 1,
              ),
              boxShadow: [
                BoxShadow(
                  color: primary.withValues(
                      alpha: selected ? 0.55 + t * 0.15 : 0.3),
                  blurRadius: selected ? 22 : 12,
                  spreadRadius: selected ? 1 : 0,
                ),
              ],
            ),
            child: Stack(
              children: [
                Positioned.fill(
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(20),
                      gradient: LinearGradient(
                        begin: Alignment.topCenter,
                        end: Alignment.bottomCenter,
                        colors: [
                          Colors.white.withValues(alpha: 0.14),
                          Colors.transparent,
                          Colors.black.withValues(alpha: 0.28),
                        ],
                        stops: const [0.0, 0.45, 1.0],
                      ),
                    ),
                  ),
                ),
                // Palette of the scene's colors.
                Positioned(
                  top: 10,
                  left: 12,
                  child:
                      MoodPaletteBadge(colors: swatch, size: 22, glow: false),
                ),
                if (scene.isHuePaletteScene || selected)
                  Positioned(
                    top: 10,
                    right: 10,
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        if (scene.isHuePaletteScene)
                          _HueSceneBadge(sceneId: scene.id),
                        if (scene.isHuePaletteScene && selected)
                          const SizedBox(width: 6),
                        if (selected)
                          Container(
                            width: 22,
                            height: 22,
                            decoration: const BoxDecoration(
                              shape: BoxShape.circle,
                              color: Colors.white,
                            ),
                            child: Icon(
                              Icons.check_rounded,
                              size: 15,
                              color: _shade(primary, 0.5),
                            ),
                          ),
                      ],
                    ),
                  ),
                Positioned(
                  left: 14,
                  right: 14,
                  bottom: 12,
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        scene.name.isEmpty ? 'Untitled' : scene.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: Colors.white,
                          fontSize: 15,
                          fontWeight: FontWeight.w700,
                          letterSpacing: -0.2,
                          shadows: [
                            Shadow(color: Colors.black54, blurRadius: 6),
                          ],
                        ),
                      ),
                      if (lightCount > 0) ...[
                        const SizedBox(height: 2),
                        Text(
                          lightCount == 1 ? '1 light' : '$lightCount lights',
                          style: TextStyle(
                            color: Colors.white.withValues(alpha: 0.8),
                            fontSize: 11.5,
                            fontWeight: FontWeight.w500,
                            shadows: const [
                              Shadow(color: Colors.black54, blurRadius: 4),
                            ],
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          );
        },
      ),
    );
  }
}

class _HueSceneBadge extends StatelessWidget {
  final String sceneId;

  const _HueSceneBadge({required this.sceneId});

  @override
  Widget build(BuildContext context) {
    return Semantics(
      label: 'Philips Hue scene',
      child: Container(
        key: Key('mood_scene_hue_badge_$sceneId'),
        padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 4),
        decoration: BoxDecoration(
          color: const Color(0xFF111722).withValues(alpha: 0.78),
          borderRadius: BorderRadius.circular(10),
          border: Border.all(
            color: Colors.white.withValues(alpha: 0.24),
          ),
        ),
        child: const Text(
          'Hue',
          style: TextStyle(
            color: Colors.white,
            fontSize: 9.5,
            height: 1,
            fontWeight: FontWeight.w700,
            letterSpacing: 0.2,
          ),
        ),
      ),
    );
  }
}

// ===========================================================================
// Scene color extraction
// ===========================================================================

/// The single representative color for a scene (its first lit color).
Color rhythmSceneColor(RhythmSceneDefinition scene) =>
    rhythmSceneSwatch(scene).first;

/// Representative RGB tuple for a scene, for optimistic local state.
(int, int, int) rhythmSceneRgb(RhythmSceneDefinition scene) {
  final c = rhythmSceneColor(scene);
  return (
    (c.r * 255).round(),
    (c.g * 255).round(),
    (c.b * 255).round(),
  );
}

/// Up to three distinct colors sampled from a scene's lit outputs, for use as
/// a tile gradient. Always returns at least one color.
List<Color> rhythmSceneSwatch(RhythmSceneDefinition scene) {
  final colors = <Color>[];
  void add(RhythmLightSceneOutput? output) {
    if (output == null || output.isOff) return;
    final color = output.color;
    if (color == null) return;
    final c = _colorFromLightColor(color);
    if (c != null) colors.add(c);
  }

  add(scene.light.defaultOutput);
  for (final output in scene.light.palette) {
    add(output);
  }
  for (final entry in scene.light.entries) {
    add(entry.output);
  }

  final unique = <Color>[];
  for (final c in colors) {
    if (unique.length >= 3) break;
    if (!unique.contains(c)) unique.add(c);
  }
  if (unique.isEmpty) {
    return const [Color(0xFFFFB04A), Color(0xFFB35A1E)];
  }
  return unique;
}

Color? _colorFromLightColor(RhythmLightColor color) {
  switch (color.kind) {
    case RhythmLightColorKind.kelvin:
      return color.kelvin == null ? null : _kelvinToColor(color.kelvin!);
    case RhythmLightColorKind.rgb:
      return _rgbColor(color.rgb);
    case RhythmLightColorKind.rgbXy:
      return _rgbColor(color.rgb) ??
          (color.xy == null ? null : _xyToColor(color.xy!.x, color.xy!.y));
    case RhythmLightColorKind.xy:
      return color.xy == null ? null : _xyToColor(color.xy!.x, color.xy!.y);
  }
}

Color? _rgbColor(RhythmSceneRgbColor? rgb) => rgb == null
    ? null
    : Color.fromARGB(
        255, rgb.r.clamp(0, 255), rgb.g.clamp(0, 255), rgb.b.clamp(0, 255));

/// Darken a color toward black by [amount] (0..1).
Color _shade(Color c, double amount) => Color.fromARGB(
      255,
      (c.r * 255 * amount).round().clamp(0, 255),
      (c.g * 255 * amount).round().clamp(0, 255),
      (c.b * 255 * amount).round().clamp(0, 255),
    );

/// Approximate color temperature (Kelvin) to RGB — Tanner Helland's algorithm.
Color _kelvinToColor(int kelvin) {
  final temp = kelvin.clamp(1000, 40000) / 100.0;
  double r, g, b;
  if (temp <= 66) {
    r = 255;
    g = 99.4708025861 * math.log(temp) - 161.1195681661;
    b = temp <= 19 ? 0 : 138.5177312231 * math.log(temp - 10) - 305.0447927307;
  } else {
    r = 329.698727446 * math.pow(temp - 60, -0.1332047592).toDouble();
    g = 288.1221695283 * math.pow(temp - 60, -0.0755148492).toDouble();
    b = 255;
  }
  return Color.fromARGB(
    255,
    r.clamp(0, 255).round(),
    g.clamp(0, 255).round(),
    b.clamp(0, 255).round(),
  );
}

/// Approximate CIE 1931 xy chromaticity to sRGB (Y normalized to 1).
Color _xyToColor(double x, double y) {
  if (y <= 0) return const Color(0xFFFFFFFF);
  const yY = 1.0;
  final xX = (yY / y) * x;
  final zZ = (yY / y) * (1.0 - x - y);

  var r = xX * 1.656492 - yY * 0.354851 - zZ * 0.255038;
  var g = -xX * 0.707196 + yY * 1.655397 + zZ * 0.036152;
  var b = xX * 0.051713 - yY * 0.121364 + zZ * 1.011530;

  double gamma(double c) =>
      c <= 0.0031308 ? 12.92 * c : 1.055 * math.pow(c, 1 / 2.4) - 0.055;
  r = gamma(r);
  g = gamma(g);
  b = gamma(b);

  final maxC = math.max(r, math.max(g, b));
  if (maxC > 1) {
    r /= maxC;
    g /= maxC;
    b /= maxC;
  }
  return Color.fromARGB(
    255,
    (r.clamp(0.0, 1.0) * 255).round(),
    (g.clamp(0.0, 1.0) * 255).round(),
    (b.clamp(0.0, 1.0) * 255).round(),
  );
}
