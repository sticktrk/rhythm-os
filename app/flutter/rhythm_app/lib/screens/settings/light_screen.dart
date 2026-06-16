import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;
import '../../widgets/time_simulator.dart';
import 'light_profile_screen.dart';

/// The "Light" tab — the *look* of each mode, presented as a stack of
/// collapsible **profile layers**.
///
/// Each profile (Day, Sleep, …) is one "layer" card: collapsed it shows a live
/// gradient preview + a one-line summary; tapping expands its full editor
/// inline while the others stay tucked away. New profiles slot in by adding a
/// single entry to [_kProfileLayers] — the screen scales without redesign.
class LightScreen extends StatefulWidget {
  const LightScreen({super.key});

  @override
  State<LightScreen> createState() => _LightScreenState();
}

/// Static description of a profile layer. The ordered list here is the single
/// place new profiles are introduced on the Light tab.
class _ProfileLayer {
  final String id;
  final String name;
  final IconData icon;
  final Color accent;

  const _ProfileLayer({
    required this.id,
    required this.name,
    required this.icon,
    required this.accent,
  });
}

const List<_ProfileLayer> _kProfileLayers = [
  _ProfileLayer(
    id: 'rhythm',
    name: 'Day',
    icon: Icons.wb_sunny_rounded,
    accent: Color(0xFFFFB74D),
  ),
  _ProfileLayer(
    id: 'sleep',
    name: 'Sleep',
    icon: Icons.bedtime_rounded,
    accent: Color(0xFF7C83FF),
  ),
];

class _LightScreenState extends State<LightScreen> {
  // Accordion: a single layer is expanded at a time. Defaults to the first.
  String? _expandedId = _kProfileLayers.first.id;

  void _toggle(String id) {
    HapticFeedback.selectionClick();
    setState(() => _expandedId = _expandedId == id ? null : id);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(16, 4, 16, 28),
                child: Consumer<ServerSyncProvider>(
                  builder: (context, sync, _) {
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        for (final layer in _kProfileLayers)
                          _ProfileLayerCard(
                            layer: layer,
                            preview: _previewFor(_configFor(sync, layer.id)),
                            expanded: _expandedId == layer.id,
                            onToggle: () => _toggle(layer.id),
                            child: LightProfileScreen(
                              initialProfile: layer.id,
                              embedded: true,
                            ),
                          ),
                        // The Time Simulator scrubs the Day curve, so it only
                        // belongs here when Day is the active mode.
                        if (sync.hasBeenSynced &&
                            sync.activeMode == RhythmMode.day) ...[
                          const SizedBox(height: 4),
                          const TimeSimulator(),
                        ],
                      ],
                    );
                  },
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
      alignment: Alignment.center,
      child: const Text(
        'Light',
        style: TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 18,
          fontWeight: FontWeight.w600,
          letterSpacing: 0.3,
        ),
      ),
    );
  }

  RhythmCurveConfig? _configFor(ServerSyncProvider sync, String id) {
    for (final profile in sync.profiles) {
      if (profile.id == id) return profile;
    }
    return null;
  }
}

/// A compact, live preview of a profile's look: a gradient swatch plus a short
/// value summary, derived from the profile's curve config.
class _LayerPreview {
  final List<Color> gradient;
  final String summary;

  const _LayerPreview(this.gradient, this.summary);
}

Color? _directColor(RhythmCurveConfig config) {
  final curve = config.curve;
  final direct = switch (curve) {
    RhythmSuperGaussianCurve(:final directColor) => directColor,
    RhythmConstantCurve(:final directColor) => directColor,
    _ => null,
  };
  if (direct == null) return null;
  return Color.fromARGB(255, direct.rgb.r, direct.rgb.g, direct.rgb.b);
}

_LayerPreview _previewFor(RhythmCurveConfig? config) {
  if (config == null) {
    final grey = CelestialColors.orbitRing.withValues(alpha: 0.6);
    return _LayerPreview([grey, grey], '—');
  }

  // Fixed-color profiles (e.g. a Sleep "warm glow") preview as that color, with
  // brightness as the headline value.
  final fixed = _directColor(config);
  if (fixed != null) {
    return _LayerPreview(
      [Color.lerp(fixed, Colors.black, 0.4)!, fixed],
      '${config.maxBrightness}%',
    );
  }

  // Curve profiles preview as their color-temperature ramp.
  final minK = config.minColorTemp;
  final maxK = config.maxColorTemp;
  final gradient = [
    ColorUtils.cctToColor(minK),
    ColorUtils.cctToColor((minK + maxK) ~/ 2),
    ColorUtils.cctToColor(maxK),
  ];
  final summary = minK == maxK ? '$minK K' : '$minK–$maxK K';
  return _LayerPreview(gradient, summary);
}

/// One collapsible profile "layer". Collapsed: icon + name + gradient preview +
/// summary. Expanded: the full embedded editor slides open beneath the header.
///
/// The editor [child] is always mounted (its draft + server state survive a
/// collapse); expansion is a clipped height reveal, not a remount.
class _ProfileLayerCard extends StatelessWidget {
  final _ProfileLayer layer;
  final _LayerPreview preview;
  final bool expanded;
  final VoidCallback onToggle;
  final Widget child;

  const _ProfileLayerCard({
    required this.layer,
    required this.preview,
    required this.expanded,
    required this.onToggle,
    required this.child,
  });

  @override
  Widget build(BuildContext context) {
    final accent = layer.accent;
    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      margin: const EdgeInsets.only(bottom: 12),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(
          color: expanded
              ? accent.withValues(alpha: 0.32)
              : CelestialColors.orbitRing.withValues(alpha: 0.5),
        ),
        boxShadow: expanded
            ? [
                BoxShadow(
                  color: accent.withValues(alpha: 0.12),
                  blurRadius: 26,
                  spreadRadius: -6,
                ),
              ]
            : const [],
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          _buildHeader(accent),
          // Keep the editor mounted; reveal it with a clipped height factor so
          // no state is lost when a layer collapses.
          ClipRect(
            child: AnimatedAlign(
              alignment: Alignment.topCenter,
              heightFactor: expanded ? 1 : 0,
              duration: const Duration(milliseconds: 300),
              curve: Curves.easeOutCubic,
              child: child,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildHeader(Color accent) {
    return GestureDetector(
      onTap: onToggle,
      behavior: HitTestBehavior.opaque,
      child: IntrinsicHeight(
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // Colored "layer spine" — the per-mode identity edge.
            Container(
              width: 4,
              color: accent.withValues(alpha: expanded ? 0.95 : 0.6),
            ),
            Expanded(
              child: Padding(
                padding: const EdgeInsets.fromLTRB(14, 14, 12, 14),
                child: Row(
                  children: [
                    Container(
                      width: 32,
                      height: 32,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: accent.withValues(alpha: 0.14),
                        border: Border.all(
                          color: accent.withValues(alpha: 0.28),
                        ),
                      ),
                      child: Icon(layer.icon, color: accent, size: 17),
                    ),
                    const SizedBox(width: 12),
                    // Fixed-width so every bar starts at the same x regardless
                    // of name length (Day vs Sleep).
                    SizedBox(
                      width: 64,
                      child: Text(
                        layer.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 16,
                          fontWeight: FontWeight.w600,
                          letterSpacing: -0.1,
                        ),
                      ),
                    ),
                    const SizedBox(width: 14),
                    Expanded(child: _GradientSwatch(colors: preview.gradient)),
                    const SizedBox(width: 12),
                    // Fixed-width, right-aligned so the swatches all end at the
                    // same x and read as a clean aligned column.
                    SizedBox(
                      width: 96,
                      child: Text(
                        preview.summary,
                        textAlign: TextAlign.right,
                        maxLines: 1,
                        softWrap: false,
                        overflow: TextOverflow.visible,
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.85),
                          fontSize: 12.5,
                          fontWeight: FontWeight.w500,
                          letterSpacing: 0.2,
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                    ),
                    const SizedBox(width: 6),
                    AnimatedRotation(
                      turns: expanded ? 0.5 : 0,
                      duration: const Duration(milliseconds: 250),
                      curve: Curves.easeOutCubic,
                      child: Icon(
                        Icons.keyboard_arrow_down_rounded,
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.5),
                        size: 22,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A small horizontal gradient bar previewing a profile's light — the visual
/// signature of the layer at a glance.
class _GradientSwatch extends StatelessWidget {
  final List<Color> colors;

  const _GradientSwatch({required this.colors});

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 10,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(6),
        gradient: LinearGradient(
          colors: colors.length == 1 ? [colors.first, colors.first] : colors,
        ),
        border: Border.all(
          color: Colors.white.withValues(alpha: 0.08),
        ),
        boxShadow: [
          BoxShadow(
            color: colors.last.withValues(alpha: 0.25),
            blurRadius: 8,
            spreadRadius: -3,
          ),
        ],
      ),
    );
  }
}
