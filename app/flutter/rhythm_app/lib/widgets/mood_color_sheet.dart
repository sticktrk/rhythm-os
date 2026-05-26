import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

/// Compact bottom sheet for picking a mood color (hue).
///
/// Shows a full-spectrum hue bar, color preset dots, and a live preview
/// glow. Calls [onColorChanged] when the user commits a color (lift finger
/// off spectrum or tap a preset).
class MoodColorSheet extends StatefulWidget {
  final Color? initialColor;
  final ValueChanged<Color> onColorChanged;

  const MoodColorSheet({
    super.key,
    this.initialColor,
    required this.onColorChanged,
  });

  /// Show the mood color picker as a modal bottom sheet.
  static Future<void> show(
    BuildContext context, {
    Color? initialColor,
    required ValueChanged<Color> onColorChanged,
  }) {
    return showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      barrierColor: Colors.black54,
      builder: (_) => MoodColorSheet(
        initialColor: initialColor,
        onColorChanged: onColorChanged,
      ),
    );
  }

  @override
  State<MoodColorSheet> createState() => _MoodColorSheetState();
}

class _MoodColorSheetState extends State<MoodColorSheet>
    with SingleTickerProviderStateMixin {
  late double _hue;
  late AnimationController _glowPulse;

  static const _presets = <({Color color, double hue, String label})>[
    (color: Color(0xFFFF3B30), hue: 4, label: 'Red'),
    (color: Color(0xFFFF6B4A), hue: 14, label: 'Ember'),
    (color: Color(0xFFFF9500), hue: 35, label: 'Amber'),
    (color: Color(0xFF34C759), hue: 135, label: 'Green'),
    (color: Color(0xFF00BFFF), hue: 195, label: 'Cyan'),
    (color: Color(0xFF5856D6), hue: 241, label: 'Indigo'),
    (color: Color(0xFFFF2D92), hue: 333, label: 'Pink'),
  ];

  @override
  void initState() {
    super.initState();
    if (widget.initialColor != null) {
      _hue = HSVColor.fromColor(widget.initialColor!).hue;
    } else {
      _hue = 35; // warm amber default
    }
    _glowPulse = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 3),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _glowPulse.dispose();
    super.dispose();
  }

  Color get _selectedColor => HSVColor.fromAHSV(1, _hue, 0.85, 1).toColor();

  void _onHueChanged(double hue, {bool commit = true}) {
    setState(() => _hue = hue);
    if (commit) {
      final color = HSVColor.fromAHSV(1, hue, 0.85, 1).toColor();
      widget.onColorChanged(color);
    }
  }

  void _commitCurrentColor() {
    widget.onColorChanged(_selectedColor);
  }

  void _onSpectrumInteraction(double dx, double width,
      {bool haptic = false, bool commit = true}) {
    final fraction = (dx / width).clamp(0.0, 1.0);
    if (haptic) HapticFeedback.selectionClick();
    _onHueChanged(fraction * 360, commit: commit);
  }

  bool _isPresetSelected(Color presetColor) {
    final selected = _selectedColor;
    return (selected.r - presetColor.r).abs() < 0.06 &&
        (selected.g - presetColor.g).abs() < 0.06 &&
        (selected.b - presetColor.b).abs() < 0.06;
  }

  @override
  Widget build(BuildContext context) {
    final bottomPad = MediaQuery.of(context).padding.bottom;
    final color = _selectedColor;

    return Container(
      decoration: const BoxDecoration(
        color: Color(0xFF141820),
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      child: Stack(
        children: [
          // Ambient glow from the selected color
          Positioned.fill(
            child: AnimatedBuilder(
              animation: _glowPulse,
              builder: (context, _) {
                final pulse = 0.7 + _glowPulse.value * 0.3;
                return DecoratedBox(
                  decoration: BoxDecoration(
                    borderRadius:
                        const BorderRadius.vertical(top: Radius.circular(24)),
                    gradient: RadialGradient(
                      center: const Alignment(0.0, -0.8),
                      radius: 1.6,
                      colors: [
                        color.withValues(alpha: 0.15 * pulse),
                        color.withValues(alpha: 0.04 * pulse),
                        Colors.transparent,
                      ],
                      stops: const [0.0, 0.45, 1.0],
                    ),
                  ),
                );
              },
            ),
          ),
          // Content
          Padding(
            padding: EdgeInsets.fromLTRB(24, 12, 24, 16 + bottomPad),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                // Drag handle
                Container(
                  width: 36,
                  height: 4,
                  decoration: BoxDecoration(
                    color: Colors.white.withValues(alpha: 0.2),
                    borderRadius: BorderRadius.circular(2),
                  ),
                ),
                const SizedBox(height: 20),
                // Color preview orb
                _ColorPreviewOrb(color: color, pulse: _glowPulse),
                const SizedBox(height: 24),
                // Hue spectrum bar
                _buildSpectrumBar(),
                const SizedBox(height: 20),
                // Preset dots
                _buildPresets(),
                const SizedBox(height: 8),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildSpectrumBar() {
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        final thumbX = (_hue / 360) * width;

        return GestureDetector(
          onTapDown: (d) => _onSpectrumInteraction(
              d.localPosition.dx, width, haptic: true),
          onHorizontalDragStart: (d) => _onSpectrumInteraction(
              d.localPosition.dx, width, commit: false),
          onHorizontalDragUpdate: (d) => _onSpectrumInteraction(
              d.localPosition.dx, width, commit: false),
          onHorizontalDragEnd: (_) => _commitCurrentColor(),
          child: SizedBox(
            height: 48,
            child: Stack(
              clipBehavior: Clip.none,
              children: [
                // Spectrum track
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
                // Thumb
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

  Widget _buildPresets() {
    return Row(
      mainAxisAlignment: MainAxisAlignment.spaceEvenly,
      children: _presets.map((preset) {
        final selected = _isPresetSelected(preset.color);
        return GestureDetector(
          onTap: () {
            HapticFeedback.lightImpact();
            _onHueChanged(preset.hue);
          },
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 200),
            width: 38,
            height: 38,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: preset.color.withValues(alpha: selected ? 0.9 : 0.35),
              border: Border.all(
                color: selected
                    ? Colors.white.withValues(alpha: 0.8)
                    : Colors.white.withValues(alpha: 0.06),
                width: selected ? 2.5 : 1,
              ),
              boxShadow: selected
                  ? [
                      BoxShadow(
                        color: preset.color.withValues(alpha: 0.5),
                        blurRadius: 14,
                        spreadRadius: 1,
                      ),
                    ]
                  : null,
            ),
          ),
        );
      }).toList(),
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
