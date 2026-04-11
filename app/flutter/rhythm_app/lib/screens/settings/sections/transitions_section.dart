import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/utils/color_utils.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../../providers/server_sync_provider.dart';
import '../../../widgets/settings_row.dart';
import '../default_transition_editor_screen.dart';

class TransitionsSection extends StatelessWidget {
  const TransitionsSection({super.key});

  @override
  Widget build(BuildContext context) {
    return Consumer<ServerSyncProvider>(
      builder: (context, serverSync, _) {
        final profileColors = _resolveProfileColors(
          serverSync.modeConfigs,
          serverSync.profiles,
        );
        final summary = _summaryValue(serverSync.modeTransitions);

        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const SettingsSectionHeader(title: 'Rhythm'),
            SettingsGroup(
              children: [
                SettingsRow(
                  icon: Icons.wb_twilight_rounded,
                  iconColor: const Color(0xFFFFB74D),
                  label: 'Default Transition',
                  value: summary.isEmpty ? null : summary,
                  onTap: () => DefaultTransitionEditorScreen.show(
                    context,
                    profileColors: profileColors,
                  ),
                ),
              ],
            ),
          ],
        );
      },
    );
  }
}

String _summaryValue(List<RhythmModeTransitionConfig> transitions) {
  RhythmModeTransitionConfig? dayTransition;
  RhythmModeTransitionConfig? sleepTransition;

  for (final transition in transitions) {
    if (transition.fromMode == RhythmMode.sleep &&
        transition.toMode == RhythmMode.day) {
      dayTransition = transition;
    } else if (transition.fromMode == RhythmMode.day &&
        transition.toMode == RhythmMode.sleep) {
      sleepTransition = transition;
    }
  }

  final labels = [
    _triggerLabel(dayTransition?.trigger),
    _triggerLabel(sleepTransition?.trigger),
  ].whereType<String>().where((label) => label.isNotEmpty).toList();
  return labels.join(' / ');
}

String? _triggerLabel(RhythmTransitionTrigger? trigger) {
  if (trigger == null) return null;
  if (trigger.isScheduled) return trigger.time;
  if (trigger.kind == 'manual') return '';

  return switch (trigger.event) {
    'sunrise' => 'Sunrise',
    'civil_twilight' => 'Civil',
    'nautical_twilight' => 'Nautical',
    'astronomical_twilight' => 'Astro',
    'sunset' => 'Sunset',
    _ => null,
  };
}

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
