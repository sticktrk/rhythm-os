import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

/// Which kind of mood the sheet is editing.
enum MoodTab { color, scenes }

/// Slide-up "mood" card. A mood can be **either** a custom color or one of the
/// saved Rhythm scenes (presets). A segmented toggle switches between a hue
/// picker and a gallery of luminous scene tiles; the card's ambient glow tracks
/// whichever the user is shaping.
class MoodSheet extends StatefulWidget {
  final Color? initialColor;
  final String? initialSceneId;
  final MoodTab initialTab;
  final List<RhythmSceneDefinition> initialScenes;
  final Future<List<RhythmSceneDefinition>> Function() scenesLoader;
  final ValueChanged<Color> onColorChanged;
  final ValueChanged<RhythmSceneDefinition> onSceneSelected;

  const MoodSheet({
    super.key,
    required this.scenesLoader,
    required this.onColorChanged,
    required this.onSceneSelected,
    this.initialColor,
    this.initialSceneId,
    this.initialTab = MoodTab.color,
    this.initialScenes = const [],
  });

  /// Show the mood picker as a modal bottom sheet.
  static Future<void> show(
    BuildContext context, {
    required Future<List<RhythmSceneDefinition>> Function() scenesLoader,
    required ValueChanged<Color> onColorChanged,
    required ValueChanged<RhythmSceneDefinition> onSceneSelected,
    Color? initialColor,
    String? initialSceneId,
    MoodTab initialTab = MoodTab.color,
    List<RhythmSceneDefinition> initialScenes = const [],
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

  // Scene state.
  late List<RhythmSceneDefinition> _scenes;
  late bool _loadingScenes;
  bool _scenesLoadStarted = false;
  String? _selectedSceneId;

  late final AnimationController _glowPulse;
  late final AnimationController _enter;

  static const _bg = Color(0xFF12151D);
  static const _colorPresets = <({
    String id,
    String label,
    double hue,
    Color color,
  })>[
    (id: 'red', label: 'Red', hue: 4, color: Color(0xFFFF3B30)),
    (id: 'ember', label: 'Ember', hue: 14, color: Color(0xFFFF6B4A)),
    (id: 'amber', label: 'Amber', hue: 35, color: Color(0xFFFF9500)),
    (id: 'rose', label: 'Rose', hue: 345, color: Color(0xFFFF6E8A)),
    (id: 'blue', label: 'Blue', hue: 210, color: Color(0xFF3BA7FF)),
    (id: 'violet', label: 'Violet', hue: 275, color: Color(0xFF9B5CFF)),
  ];

  @override
  void initState() {
    super.initState();
    _tab = widget.initialTab;
    _hue = widget.initialColor != null
        ? HSVColor.fromColor(widget.initialColor!).hue
        : 35; // warm amber default
    _scenes = widget.initialScenes;
    _selectedSceneId = widget.initialSceneId;
    _loadingScenes = _tab == MoodTab.scenes && widget.initialScenes.isEmpty;

    _glowPulse = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 4),
    )..repeat(reverse: true);
    _enter = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 650),
    )..forward();

    if (_tab == MoodTab.scenes) {
      _ensureScenesLoaded();
    }
  }

  Future<void> _ensureScenesLoaded() async {
    if (_scenesLoadStarted || _scenes.isNotEmpty) return;
    _scenesLoadStarted = true;
    if (!_loadingScenes) {
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

  Color get _selectedColor => HSVColor.fromAHSV(1, _hue, 0.85, 1).toColor();

  RhythmSceneDefinition? get _selectedScene {
    for (final s in _scenes) {
      if (s.id == _selectedSceneId) return s;
    }
    return null;
  }

  Color get _ambientColor {
    if (_tab == MoodTab.color) return _selectedColor;
    final scene = _selectedScene ?? (_scenes.isNotEmpty ? _scenes.first : null);
    return scene == null ? const Color(0xFFFF9500) : rhythmSceneColor(scene);
  }

  // --- Color interactions --------------------------------------------------

  void _setHue(double hue, {bool commit = true}) {
    setState(() {
      _hue = hue;
      _selectedSceneId = null;
    });
    if (commit) widget.onColorChanged(_selectedColor);
  }

  void _commitColor() {
    setState(() => _selectedSceneId = null);
    widget.onColorChanged(_selectedColor);
  }

  void _onSpectrum(double dx, double width,
      {bool haptic = false, bool commit = true}) {
    final fraction = (dx / width).clamp(0.0, 1.0);
    if (haptic) HapticFeedback.selectionClick();
    _setHue(fraction * 360, commit: commit);
  }

  void _onSceneTap(RhythmSceneDefinition scene) {
    HapticFeedback.mediumImpact();
    setState(() => _selectedSceneId = scene.id);
    widget.onSceneSelected(scene);
  }

  void _switchTab(MoodTab tab) {
    if (_tab == tab) return;
    HapticFeedback.selectionClick();
    setState(() => _tab = tab);
    if (tab == MoodTab.scenes) {
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
                        child: _tab == MoodTab.color
                            ? _colorPane(ambient)
                            : _scenePane(),
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

  Widget _colorPane(Color color) {
    return Column(
      key: const ValueKey(MoodTab.color),
      mainAxisSize: MainAxisSize.min,
      children: [
        const SizedBox(height: 8),
        _ColorPreviewOrb(color: color, pulse: _glowPulse),
        const SizedBox(height: 28),
        _spectrumBar(),
        const SizedBox(height: 14),
        _presetSwatches(),
        const SizedBox(height: 10),
      ],
    );
  }

  Widget _presetSwatches() {
    return Wrap(
      alignment: WrapAlignment.center,
      spacing: 10,
      runSpacing: 10,
      children: [
        for (final preset in _colorPresets)
          _ColorPresetSwatch(
            gestureKey: Key('mood_color_preset_${preset.id}'),
            label: preset.label,
            color: preset.color,
            selected: (_hue - preset.hue).abs() < 0.5,
            onTap: () {
              HapticFeedback.selectionClick();
              _setHue(preset.hue);
            },
          ),
      ],
    );
  }

  Widget _spectrumBar() {
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        final thumbX = (_hue / 360) * width;

        return GestureDetector(
          key: const Key('mood_color_spectrum'),
          onTapDown: (d) =>
              _onSpectrum(d.localPosition.dx, width, haptic: true),
          onHorizontalDragStart: (d) =>
              _onSpectrum(d.localPosition.dx, width, commit: false),
          onHorizontalDragUpdate: (d) =>
              _onSpectrum(d.localPosition.dx, width, commit: false),
          onHorizontalDragEnd: (_) => _commitColor(),
          child: SizedBox(
            height: 48,
            child: Stack(
              clipBehavior: Clip.none,
              children: [
                Positioned(
                  left: 0,
                  right: 0,
                  top: 8,
                  child: Container(
                    height: 32,
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(16),
                      gradient: const LinearGradient(
                        colors: [
                          Color(0xFFFF0000),
                          Color(0xFFFF8800),
                          Color(0xFFFFFF00),
                          Color(0xFF00FF00),
                          Color(0xFF00FFFF),
                          Color(0xFF0088FF),
                          Color(0xFF0000FF),
                          Color(0xFF8800FF),
                          Color(0xFFFF00FF),
                          Color(0xFFFF0044),
                          Color(0xFFFF0000),
                        ],
                      ),
                      border: Border.all(
                        color: Colors.white.withValues(alpha: 0.08),
                      ),
                    ),
                  ),
                ),
                Positioned(
                  left: thumbX - 12,
                  top: 4,
                  child: Container(
                    width: 24,
                    height: 40,
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(12),
                      color: _selectedColor,
                      border: Border.all(
                        color: Colors.white.withValues(alpha: 0.9),
                        width: 3,
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: _selectedColor.withValues(alpha: 0.6),
                          blurRadius: 16,
                          spreadRadius: 2,
                        ),
                        BoxShadow(
                          color: Colors.black.withValues(alpha: 0.5),
                          blurRadius: 4,
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  // --- Scene pane ----------------------------------------------------------

  Widget _scenePane() {
    if (_loadingScenes) {
      return const Padding(
        key: ValueKey('scenes-loading'),
        padding: EdgeInsets.symmetric(vertical: 48),
        child: Center(
          child: SizedBox(
            width: 26,
            height: 26,
            child: CircularProgressIndicator(strokeWidth: 2.4),
          ),
        ),
      );
    }
    if (_scenes.isEmpty) {
      return _scenesEmptyState();
    }
    return GridView.builder(
      key: const ValueKey(MoodTab.scenes),
      shrinkWrap: true,
      padding: const EdgeInsets.only(bottom: 4),
      physics: const BouncingScrollPhysics(),
      gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
        crossAxisCount: 2,
        mainAxisSpacing: 14,
        crossAxisSpacing: 14,
        childAspectRatio: 1.5,
      ),
      itemCount: _scenes.length,
      itemBuilder: (context, i) {
        final scene = _scenes[i];
        final start = (i * 0.06).clamp(0.0, 0.6);
        final anim = CurvedAnimation(
          parent: _enter,
          curve: Interval(start, (start + 0.5).clamp(0.0, 1.0),
              curve: Curves.easeOutCubic),
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
      },
    );
  }

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
}

/// Segmented pill toggle for switching between Color and Scenes.
class _MoodTabToggle extends StatelessWidget {
  final MoodTab tab;
  final Color accent;
  final ValueChanged<MoodTab> onChanged;

  const _MoodTabToggle({
    required this.tab,
    required this.accent,
    required this.onChanged,
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
            alignment: tab == MoodTab.color
                ? Alignment.centerLeft
                : Alignment.centerRight,
            child: FractionallySizedBox(
              widthFactor: 0.5,
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

/// Glowing orb that previews the selected mood color.
class _ColorPreviewOrb extends StatelessWidget {
  final Color color;
  final Animation<double> pulse;

  const _ColorPreviewOrb({required this.color, required this.pulse});

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: pulse,
      builder: (context, _) {
        final t = pulse.value;
        final glowRadius = 32.0 + t * 8.0;
        return Container(
          width: 64,
          height: 64,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: color,
            boxShadow: [
              BoxShadow(
                color: color.withValues(alpha: 0.35 + t * 0.2),
                blurRadius: glowRadius,
                spreadRadius: 4 + t * 4,
              ),
              BoxShadow(
                color: color.withValues(alpha: 0.15),
                blurRadius: glowRadius * 2,
                spreadRadius: 8,
              ),
            ],
          ),
        );
      },
    );
  }
}

class _ColorPresetSwatch extends StatelessWidget {
  final Key gestureKey;
  final String label;
  final Color color;
  final bool selected;
  final VoidCallback onTap;

  const _ColorPresetSwatch({
    required this.gestureKey,
    required this.label,
    required this.color,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      key: gestureKey,
      behavior: HitTestBehavior.opaque,
      onTap: onTap,
      child: Semantics(
        button: true,
        label: '$label mood color',
        selected: selected,
        child: SizedBox(
          width: 48,
          height: 56,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              AnimatedContainer(
                duration: const Duration(milliseconds: 180),
                width: 34,
                height: 34,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: color,
                  border: Border.all(
                    color: selected
                        ? Colors.white.withValues(alpha: 0.95)
                        : Colors.white.withValues(alpha: 0.18),
                    width: selected ? 3 : 1,
                  ),
                  boxShadow: [
                    BoxShadow(
                      color: color.withValues(alpha: selected ? 0.55 : 0.25),
                      blurRadius: selected ? 14 : 8,
                      spreadRadius: selected ? 1 : 0,
                    ),
                  ],
                ),
              ),
              const SizedBox(height: 6),
              Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: Colors.white.withValues(alpha: selected ? 0.86 : 0.55),
                  fontSize: 10.5,
                  fontWeight: selected ? FontWeight.w700 : FontWeight.w500,
                ),
              ),
            ],
          ),
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
                if (selected)
                  Positioned(
                    top: 10,
                    right: 10,
                    child: Container(
                      width: 22,
                      height: 22,
                      decoration: const BoxDecoration(
                        shape: BoxShape.circle,
                        color: Colors.white,
                      ),
                      child: Icon(Icons.check_rounded,
                          size: 15, color: _shade(primary, 0.5)),
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
