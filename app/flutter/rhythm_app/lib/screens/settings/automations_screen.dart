import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../utils/app_color_temperature.dart';
import '../../widgets/automation_section_header.dart';
import '../../widgets/celestial_segmented_control.dart';
import '../../widgets/header_close_button.dart';
import '../../widgets/mode_room_behavior_section.dart';
import '../../widgets/settings_row.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;
import 'default_transition_editor_screen.dart';

/// The Automations tab — a list of preset automations, each a row that taps
/// into its own detail screen. New automation types slot in as additional rows
/// without disturbing the details.
class AutomationsScreen extends StatelessWidget {
  const AutomationsScreen({super.key, this.onClose});

  /// Dismisses this menu back to Home (shown as a ✕ in the header).
  final VoidCallback? onClose;

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
                        const AutomationSectionHeader(
                          step: 1,
                          title: 'Schedule',
                          subtitle:
                              'Trigger Wake / Sleep presets automatically '
                              'or with a button.',
                          accent: CelestialColors.accentBlue,
                        ),
                        SettingsGroup(
                          children: [
                            _automaticRow(context, sync, profileColors),
                            _buttonRow(context, sync, profileColors),
                          ],
                        ),
                        const AutomationSectionHeader(
                          step: 2,
                          title: 'Wake / Sleep Presets',
                          subtitle: 'Set what your rooms do once Wake or Sleep '
                              'is triggered.',
                          accent: CelestialColors.sunWarm,
                        ),
                        SettingsGroup(
                          children: [
                            _modeRow(context, sync, RhythmMode.day),
                            _modeRow(context, sync, RhythmMode.sleep),
                          ],
                        ),
                        const AutomationSectionHeader(
                          step: 3,
                          title: 'Test your presets',
                          subtitle: 'Manually trigger a Wake or Sleep preset.',
                          accent: Color(0xFF9C8CFF),
                        ),
                        const _ManualModeToggle(),
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
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 12),
      child: Row(
        children: [
          if (onClose != null)
            HeaderCloseButton(onTap: onClose!)
          else
            const SizedBox(width: 40),
          const Expanded(
            child: Text(
              'Presets',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
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
              (t.fromMode == RhythmMode.day && t.toMode == RhythmMode.sleep)) &&
          !t.trigger.isManual &&
          t.triggerEnabled,
    );
    return SettingsRow(
      icon: Icons.alarm_rounded,
      iconColor: const Color(0xFF58A6FF),
      label: 'Alarm',
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
      label: 'Wake/Sleep Button',
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
      label: isDay ? 'Wake Presets' : 'Sleep Presets',
      value: overrides > 0
          ? '$overrides room${overrides == 1 ? '' : 's'} set'
          : 'All Auto',
      onTap: () => _pushDetail(context, ModeBehaviorDetailScreen(mode: mode)),
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

/// Standalone manual Day/Sleep switch — a segmented control that dispatches the
/// mode transition immediately. This is the temporary home for the manual
/// override that used to live on the home screen's top bar.
class _ManualModeToggle extends StatefulWidget {
  const _ManualModeToggle();

  @override
  State<_ManualModeToggle> createState() => _ManualModeToggleState();
}

class _ManualModeToggleState extends State<_ManualModeToggle> {
  bool _busy = false;

  Future<void> _select(RhythmMode target) async {
    final sync = context.read<ServerSyncProvider>();
    if (_busy || sync.activeMode == target || !sync.canDispatchActions) return;

    setState(() => _busy = true);
    try {
      final current = sync.activeMode;
      // Prefer a proper mode transition when we know the current mode; fall
      // back to a direct set on a cold start (no known current mode).
      final transitionId = current == null
          ? null
          : (target == RhythmMode.day ? 'sleep_to_day' : 'day_to_sleep');
      if (transitionId == null) {
        await sync.dispatchSetActiveMode(target);
      } else {
        await sync.dispatchRunTransition(transitionId);
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final active = context.select<ServerSyncProvider, RhythmMode?>(
      (s) => s.activeMode,
    );
    return CelestialSegmentedControl<RhythmMode>(
      segments: const [
        CelestialSegment(
          value: RhythmMode.day,
          label: 'Wake',
          icon: Icons.wb_sunny_rounded,
          accent: Color(0xFFF9A825),
        ),
        CelestialSegment(
          value: RhythmMode.sleep,
          label: 'Sleep',
          icon: Icons.bedtime_rounded,
          accent: Color(0xFF7C83FF),
        ),
      ],
      selected: active,
      onTap: _select,
      enabled: !_busy,
      trackColor: CelestialColors.backgroundCard,
      mediumHaptic: true,
    );
  }
}

/// Detail screen for a mode's per-room On / Low glow / Off behavior — what each
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
  return AppColorTemperature.toColor(midCct);
}
