import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../widgets/mode_room_behavior_section.dart';
import '../../widgets/settings_row.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;
import 'default_transition_editor_screen.dart';

/// The Automations tab — a list of preset automations, each a row that taps
/// into its own detail screen. New automation types slot in as additional rows
/// without disturbing the details.
class AutomationsScreen extends StatelessWidget {
  const AutomationsScreen({super.key});

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
                padding: const EdgeInsets.symmetric(horizontal: 20),
                child: Consumer<ServerSyncProvider>(
                  builder: (context, sync, _) {
                    final profileColors =
                        resolveProfileColors(sync.modeConfigs, sync.profiles);
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        const _AutomationSectionHeader(
                          step: 1,
                          title: 'How it switches',
                          subtitle: 'Move between Day and Sleep automatically, '
                              'or with a button.',
                          accent: CelestialColors.accentBlue,
                        ),
                        SettingsGroup(
                          children: [
                            _automaticRow(context, sync, profileColors),
                            _buttonRow(context, sync, profileColors),
                          ],
                        ),
                        const _AutomationSectionHeader(
                          step: 2,
                          title: 'What each mode does',
                          subtitle: 'Set what your rooms do once Day or Sleep '
                              'begins.',
                          accent: CelestialColors.sunWarm,
                        ),
                        SettingsGroup(
                          children: [
                            _modeRow(context, sync, RhythmMode.day),
                            _modeRow(context, sync, RhythmMode.sleep),
                          ],
                        ),
                        const SizedBox(height: 40),
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
        'Automations',
        style: TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 18,
          fontWeight: FontWeight.w600,
          letterSpacing: 0.3,
        ),
      ),
    );
  }

  void _pushDetail(BuildContext context, Widget screen) {
    Navigator.of(context).push(MaterialPageRoute(builder: (_) => screen));
  }

  Widget _automaticRow(
    BuildContext context,
    ServerSyncProvider sync,
    Map<RhythmMode, Color> profileColors,
  ) {
    final enabled = sync.modeTransitions.any(
      (t) =>
          ((t.fromMode == RhythmMode.sleep && t.toMode == RhythmMode.day) ||
              (t.fromMode == RhythmMode.day &&
                  t.toMode == RhythmMode.sleep)) &&
          !t.trigger.isManual &&
          t.triggerEnabled,
    );
    return SettingsRow(
      icon: Icons.access_time_rounded,
      iconColor: const Color(0xFF58A6FF),
      label: 'Time Schedule',
      showChevron: false,
      trailing: _StatusPill(enabled: enabled),
      onTap: () => _pushDetail(
        context,
        DefaultTransitionEditorScreen(
          detail: AutomationDetail.automatic,
          profileColors: profileColors,
        ),
      ),
    );
  }

  Widget _buttonRow(
    BuildContext context,
    ServerSyncProvider sync,
    Map<RhythmMode, Color> profileColors,
  ) {
    final enabled = sync.daySleepToggleInputBinding?.enabled ?? false;
    return SettingsRow(
      icon: Icons.radio_button_checked_rounded,
      iconColor: const Color(0xFF9C8CFF),
      label: 'Button Toggle',
      showChevron: false,
      trailing: _StatusPill(enabled: enabled),
      onTap: () => _pushDetail(
        context,
        DefaultTransitionEditorScreen(
          detail: AutomationDetail.button,
          profileColors: profileColors,
        ),
      ),
    );
  }

  Widget _modeRow(
    BuildContext context,
    ServerSyncProvider sync,
    RhythmMode mode,
  ) {
    final config = sync.modeConfigs.cast<RhythmModeConfig?>().firstWhere(
          (c) => c!.mode == mode,
          orElse: () => null,
        );
    final overrides = config?.roomDefaults.length ?? 0;
    final isDay = mode == RhythmMode.day;
    return SettingsRow(
      icon: isDay ? Icons.wb_sunny_rounded : Icons.bedtime_rounded,
      iconColor: isDay ? const Color(0xFFF9A825) : const Color(0xFF7C83FF),
      label: isDay ? 'Day' : 'Sleep',
      value: overrides > 0
          ? '$overrides room${overrides == 1 ? '' : 's'} set'
          : 'All Auto',
      onTap: () => _pushDetail(context, ModeBehaviorDetailScreen(mode: mode)),
    );
  }
}

/// A numbered, two-line section header for the Automations flow. The tinted
/// step badge plus subtitle turns two loose lists into a clear sequence:
/// ① decide how the home switches → ② define what each mode does.
class _AutomationSectionHeader extends StatelessWidget {
  final int step;
  final String title;
  final String subtitle;
  final Color accent;

  const _AutomationSectionHeader({
    required this.step,
    required this.title,
    required this.subtitle,
    required this.accent,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(left: 2, right: 8, top: 30, bottom: 14),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Tinted numeral badge — conveys "first, then" ordering.
          Container(
            width: 26,
            height: 26,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: accent.withValues(alpha: 0.16),
              border: Border.all(
                color: accent.withValues(alpha: 0.4),
                width: 1,
              ),
            ),
            child: Text(
              '$step',
              style: TextStyle(
                color: accent,
                fontSize: 13,
                fontWeight: FontWeight.w700,
                height: 1,
              ),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title.toUpperCase(),
                  style: TextStyle(
                    color: accent,
                    fontSize: 12.5,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.9,
                  ),
                ),
                const SizedBox(height: 3),
                Text(
                  subtitle,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.85),
                    fontSize: 13,
                    height: 1.35,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A colorful On/Off status pill (green when enabled, muted grey when off),
/// paired with a chevron to keep the row reading as tappable.
class _StatusPill extends StatelessWidget {
  final bool enabled;

  const _StatusPill({required this.enabled});

  @override
  Widget build(BuildContext context) {
    final color =
        enabled ? const Color(0xFF3FB950) : CelestialColors.textSecondary;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 4),
          decoration: BoxDecoration(
            color: color.withValues(alpha: 0.16),
            borderRadius: BorderRadius.circular(999),
          ),
          child: Text(
            enabled ? 'On' : 'Off',
            style: TextStyle(
              color: color,
              fontSize: 13,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.2,
            ),
          ),
        ),
        const SizedBox(width: 8),
        Icon(
          Icons.chevron_right,
          color: CelestialColors.textSecondary.withValues(alpha: 0.5),
          size: 22,
        ),
      ],
    );
  }
}

/// Detail screen for a mode's per-room On / Standby / Off behavior — what each
/// room does when Day or Sleep engages.
class ModeBehaviorDetailScreen extends StatelessWidget {
  final RhythmMode mode;

  const ModeBehaviorDetailScreen({super.key, required this.mode});

  @override
  Widget build(BuildContext context) {
    final title = mode == RhythmMode.day ? 'Day' : 'Sleep';
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            AutomationDetailHeader(title: title),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(20, 8, 20, 40),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Padding(
                      padding:
                          const EdgeInsets.only(left: 4, right: 4, bottom: 14),
                      child: Text(
                        'What each room does when $title engages. Rooms left on '
                        'Auto follow the lighting curve.',
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.8),
                          fontSize: 13,
                          height: 1.4,
                        ),
                      ),
                    ),
                    ModeRoomBehaviorSection(mode: mode),
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

/// Back-chevron + centered title header shared by automation detail screens.
class AutomationDetailHeader extends StatelessWidget {
  final String title;

  const AutomationDetailHeader({super.key, required this.title});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.accentBlue.withValues(alpha: 0.2),
              ),
              child: const Icon(
                Icons.chevron_left,
                color: CelestialColors.accentBlue,
                size: 24,
              ),
            ),
          ),
          Expanded(
            child: Text(
              title,
              textAlign: TextAlign.center,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }
}

// ── Profile color resolution (shared with the orbital-clock detail) ─────────

/// Maps each [RhythmMode] to the dominant color of its active profile, used to
/// tint the orbital clock in the Automatic detail.
Map<RhythmMode, Color> resolveProfileColors(
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

Color _colorFromProfile(RhythmCurveConfig profile) {
  final curve = profile.curve;
  if (curve is RhythmConstantCurve && curve.directColor != null) {
    final rgb = curve.directColor!.rgb;
    return Color.fromARGB(255, rgb.r, rgb.g, rgb.b);
  }
  if (curve is RhythmSuperGaussianCurve && curve.directColor != null) {
    final rgb = curve.directColor!.rgb;
    return Color.fromARGB(255, rgb.r, rgb.g, rgb.b);
  }
  final midCct = (profile.minColorTemp + profile.maxColorTemp) ~/ 2;
  return ColorUtils.cctToColor(midCct);
}
