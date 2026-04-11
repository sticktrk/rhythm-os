import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/utils/color_utils.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../../providers/server_sync_provider.dart';
import '../../../widgets/settings_row.dart';
import '../../../widgets/solar_orbit.dart';
import '../transition_edit_screen.dart';

/// Transitions section — displays and allows editing of mode transition configs.
class TransitionsSection extends StatelessWidget {
  const TransitionsSection({super.key});

  @override
  Widget build(BuildContext context) {
    return Consumer<ServerSyncProvider>(
      builder: (context, serverSync, _) {
        final transitions = serverSync.modeTransitions;
        if (transitions.isEmpty) return const SizedBox.shrink();

        // Resolve profile colors per mode from actual curve data.
        final profileColors = _resolveProfileColors(
          serverSync.modeConfigs,
          serverSync.profiles,
        );

        // Sort: sunrise (sleep→day) first, twilight (day→sleep) second.
        final sorted = [...transitions]..sort((a, b) {
            if (_isSunriseTrigger(a.trigger)) return -1;
            if (_isSunriseTrigger(b.trigger)) return 1;
            return 0;
          });

        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const SettingsSectionHeader(title: 'Transitions'),
            for (int i = 0; i < sorted.length; i++) ...[
              _TransitionCard(
                transition: sorted[i],
                profileColors: profileColors,
              ),
              if (i < sorted.length - 1) const SizedBox(height: 10),
            ],
          ],
        );
      },
    );
  }
}

// ---------------------------------------------------------------------------
// Transition card
// ---------------------------------------------------------------------------

class _TransitionCard extends StatelessWidget {
  final RhythmModeTransitionConfig transition;
  final Map<RhythmMode, Color> profileColors;

  const _TransitionCard({
    required this.transition,
    required this.profileColors,
  });

  @override
  Widget build(BuildContext context) {
    final accentColor = _triggerColor(transition.trigger);

    return GestureDetector(
      onTap: () => TransitionEditScreen.show(
        context,
        transition: transition,
        profileColors: profileColors,
      ),
      child: Container(
        decoration: BoxDecoration(
          color: CelestialColors.backgroundCard,
          borderRadius: BorderRadius.circular(14),
        ),
        clipBehavior: Clip.antiAlias,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            _FlowRow(
              transition: transition,
              accentColor: accentColor,
              profileColors: profileColors,
            ),
            _cardDivider(),
            _InfoRow(
              label: transition.label,
              duration: transition.duration,
            ),
          ],
        ),
      ),
    );
  }
}

Widget _cardDivider() {
  return Padding(
    padding: const EdgeInsets.only(left: 16),
    child: Container(
      height: 0.5,
      color: CelestialColors.orbitRing.withValues(alpha: 0.2),
    ),
  );
}

// ---------------------------------------------------------------------------
// Flow visualization: [From mode] ── trigger ──▶ [To mode]
// ---------------------------------------------------------------------------

class _FlowRow extends StatelessWidget {
  final RhythmModeTransitionConfig transition;
  final Color accentColor;
  final Map<RhythmMode, Color> profileColors;

  const _FlowRow({
    required this.transition,
    required this.accentColor,
    required this.profileColors,
  });

  @override
  Widget build(BuildContext context) {
    final fromColor = profileColors[transition.fromMode] ??
        _fallbackModeColor(transition.fromMode);
    final toColor = profileColors[transition.toMode] ??
        _fallbackModeColor(transition.toMode);

    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 14),
      child: Row(
        children: [
          // From mode pill
          _ModePill(mode: transition.fromMode, color: fromColor),
          const SizedBox(width: 10),
          // Arrow + trigger label
          Expanded(
            child: Row(
              children: [
                Expanded(
                  child: Container(
                    height: 1,
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                        colors: [
                          fromColor.withValues(alpha: 0.4),
                          accentColor.withValues(alpha: 0.6),
                        ],
                      ),
                    ),
                  ),
                ),
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  child: Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                    decoration: BoxDecoration(
                      color: accentColor.withValues(alpha: 0.12),
                      borderRadius: BorderRadius.circular(10),
                      border: Border.all(
                        color: accentColor.withValues(alpha: 0.25),
                        width: 0.5,
                      ),
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Icon(
                          _triggerIcon(transition.trigger),
                          size: 13,
                          color: accentColor,
                        ),
                        const SizedBox(width: 5),
                        Text(
                          _triggerLabel(transition.trigger),
                          style: TextStyle(
                            color: accentColor,
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            letterSpacing: 0.3,
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
                Expanded(
                  child: Container(
                    height: 1,
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                        colors: [
                          accentColor.withValues(alpha: 0.6),
                          toColor.withValues(alpha: 0.4),
                        ],
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 10),
          // To mode pill
          _ModePill(mode: transition.toMode, color: toColor),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Mode pill chip
// ---------------------------------------------------------------------------

class _ModePill extends StatelessWidget {
  final RhythmMode mode;
  final Color color;

  const _ModePill({required this.mode, required this.color});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(
          color: color.withValues(alpha: 0.25),
          width: 0.5,
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(_modeIcon(mode), size: 14, color: color),
          const SizedBox(width: 5),
          Text(
            _modeLabel(mode),
            style: TextStyle(
              color: color,
              fontSize: 12,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.2,
            ),
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Info row (read-only summary with chevron)
// ---------------------------------------------------------------------------

class _InfoRow extends StatelessWidget {
  final String label;
  final TransitionDuration duration;

  const _InfoRow({required this.label, required this.duration});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          if (label.isNotEmpty) ...[
            Flexible(
              child: Text(
                label,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
            if (!duration.isAuto)
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 8),
                child: Container(
                  width: 3,
                  height: 3,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                  ),
                ),
              ),
          ],
          if (!duration.isAuto) ...[
            Icon(
              Icons.timer_outlined,
              size: 13,
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            ),
            const SizedBox(width: 4),
            Text(
              _formatDuration(duration.ms),
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                fontSize: 13,
              ),
            ),
          ],
          const Spacer(),
          Icon(
            Icons.chevron_right,
            color: CelestialColors.textSecondary.withValues(alpha: 0.4),
            size: 20,
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

String _formatDuration(int ms) {
  final seconds = ms ~/ 1000;
  if (seconds < 60) return '${seconds}s';
  final minutes = seconds ~/ 60;
  final remainingSeconds = seconds % 60;
  if (remainingSeconds == 0) return '$minutes min';
  return '${minutes}m ${remainingSeconds}s';
}

bool _isSunriseTrigger(RhythmTransitionTrigger trigger) {
  return trigger.kind == 'solar' && trigger.event == 'sunrise';
}

String _triggerLabel(RhythmTransitionTrigger trigger) {
  if (trigger.kind == 'manual') return 'Manual';

  final event = trigger.event;
  if (event == null || event.isEmpty) return trigger.kind.replaceAll('_', ' ');

  return switch (event) {
    'sunrise' => 'Sunrise',
    'civil_twilight' => 'Civil',
    'nautical_twilight' => 'Nautical',
    'astronomical_twilight' => 'Astro',
    'sunset' => 'Sunset',
    _ => event.replaceAll('_', ' '),
  };
}

IconData _triggerIcon(RhythmTransitionTrigger trigger) {
  if (trigger.kind == 'manual') return Icons.schedule;

  return switch (trigger.event) {
    'sunrise' => Icons.wb_sunny_rounded,
    'civil_twilight' => Icons.wb_twilight_rounded,
    'nautical_twilight' => Icons.nights_stay_rounded,
    'astronomical_twilight' => Icons.dark_mode_rounded,
    'sunset' => Icons.wb_twilight_rounded,
    _ => Icons.schedule,
  };
}

Color _triggerColor(RhythmTransitionTrigger trigger) {
  if (trigger.kind == 'manual') return CelestialColors.accentBlue;

  return switch (trigger.event) {
    'sunrise' => const Color(0xFFF9A825),
    'civil_twilight' => const Color(0xFFFFB74D),
    'nautical_twilight' => const Color(0xFF7C4DFF),
    'astronomical_twilight' => const Color(0xFF5C6BC0),
    'sunset' => const Color(0xFFFF7043),
    _ => CelestialColors.accentBlue,
  };
}

/// Resolve a representative display color for each mode from actual profile
/// curve data. Falls back to generic mode colors if data isn't available.
Map<RhythmMode, Color> _resolveProfileColors(
  List<RhythmModeConfig> modeConfigs,
  List<RhythmCurveConfig> profiles,
) {
  final colors = <RhythmMode, Color>{};
  for (final mc in modeConfigs) {
    final profile = profiles.cast<RhythmCurveConfig?>().firstWhere(
          (p) => p!.id == mc.activeProfileId,
          orElse: () => null,
        );
    if (profile == null) continue;
    colors[mc.mode] = _colorFromProfile(profile);
  }
  return colors;
}

/// Extract a representative color from a profile's curve config.
Color _colorFromProfile(RhythmCurveConfig profile) {
  final curve = profile.curve;
  // Constant curves (sleep/idle) — use directColor RGB if present.
  if (curve is RhythmConstantCurve && curve.directColor != null) {
    final rgb = curve.directColor!.rgb;
    return Color.fromARGB(255, rgb.r, rgb.g, rgb.b);
  }
  // Super-gaussian curves — use directColor if set, otherwise derive from CCT.
  if (curve is RhythmSuperGaussianCurve && curve.directColor != null) {
    final rgb = curve.directColor!.rgb;
    return Color.fromARGB(255, rgb.r, rgb.g, rgb.b);
  }
  // No direct color — derive from midpoint of color temp range.
  final midCct = (profile.minColorTemp + profile.maxColorTemp) ~/ 2;
  return ColorUtils.cctToColor(midCct);
}

Color _fallbackModeColor(RhythmMode mode) => switch (mode) {
      RhythmMode.day => const Color(0xFFF9A825),
      RhythmMode.sleep => const Color(0xFF7C4DFF),
    };

IconData _modeIcon(RhythmMode mode) => switch (mode) {
      RhythmMode.day => Icons.wb_sunny_rounded,
      RhythmMode.sleep => Icons.bedtime_rounded,
    };

String _modeLabel(RhythmMode mode) => switch (mode) {
      RhythmMode.day => 'Day',
      RhythmMode.sleep => 'Sleep',
    };
