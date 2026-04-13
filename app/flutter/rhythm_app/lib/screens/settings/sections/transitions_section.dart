import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../../providers/home_provider.dart';
import '../../../providers/server_sync_provider.dart';
import '../../../widgets/settings_row.dart';
import '../default_transition_editor_screen.dart';

class TransitionsSection extends StatelessWidget {
  const TransitionsSection({super.key});

  @override
  Widget build(BuildContext context) {
    return Consumer2<ServerSyncProvider, HomeProvider>(
      builder: (context, serverSync, homeProvider, _) {
        final profileColors = _resolveProfileColors(
          serverSync.modeConfigs,
          serverSync.profiles,
        );
        final summary = _summaryValue(
          context,
          transitions: serverSync.modeTransitions,
          home: homeProvider.currentHome,
        );

        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const SettingsSectionHeader(title: 'Rhythm'),
            SettingsGroup(
              children: [
                SettingsRow(
                  icon: Icons.wb_twilight_rounded,
                  iconColor: const Color(0xFFFFB74D),
                  label: 'Daily Rhythm',
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

String _summaryValue(
  BuildContext context, {
  required List<RhythmModeTransitionConfig> transitions,
  required Home? home,
}) {
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

  final solarContext = _resolveSolarContext(home);
  final labels = [
    _triggerSummaryLabel(
      context,
      mode: RhythmMode.day,
      trigger: dayTransition?.trigger,
      sunTimes: solarContext?.sunTimes,
      twilightTimes: solarContext?.twilightTimes,
    ),
    _triggerSummaryLabel(
      context,
      mode: RhythmMode.sleep,
      trigger: sleepTransition?.trigger,
      sunTimes: solarContext?.sunTimes,
      twilightTimes: solarContext?.twilightTimes,
    ),
  ].whereType<String>().where((label) => label.isNotEmpty).toList();
  return labels.join(' / ');
}

({SunTimesDto sunTimes, TwilightTimesDto twilightTimes})? _resolveSolarContext(
  Home? home,
) {
  final loc = home?.location;
  if (loc == null) return null;

  try {
    final now = DateTime.now();
    final timezone =
        home?.timezone ?? SolarUtils.timezoneFromLongitude(loc.longitude);
    return (
      sunTimes: getSunTimes(
        latitude: loc.latitude,
        longitude: loc.longitude,
        year: now.year,
        month: now.month,
        day: now.day,
        timezone: timezone,
      ),
      twilightTimes: getTwilightTimes(
        latitude: loc.latitude,
        longitude: loc.longitude,
        year: now.year,
        month: now.month,
        day: now.day,
        timezone: timezone,
      ),
    );
  } catch (_) {
    return null;
  }
}

String? _triggerSummaryLabel(
  BuildContext context, {
  required RhythmMode mode,
  required RhythmTransitionTrigger? trigger,
  required SunTimesDto? sunTimes,
  required TwilightTimesDto? twilightTimes,
}) {
  if (trigger == null) return null;
  if (trigger.isScheduled) return trigger.time;
  if (trigger.kind == 'manual') return '';

  final event = trigger.event;
  if (trigger.kind == 'solar' && event != null) {
    final hour = _eventTimeHours(
      mode: mode,
      event: event,
      sunTimes: sunTimes,
      twilightTimes: twilightTimes,
    );
    if (hour != null) {
      return SolarUtils.formatHour(
        hour,
        use24: MediaQuery.alwaysUse24HourFormatOf(context),
      );
    }
  }

  return _triggerLabel(trigger);
}

double? _eventTimeHours({
  required RhythmMode mode,
  required String event,
  required SunTimesDto? sunTimes,
  required TwilightTimesDto? twilightTimes,
}) {
  final isDawn = mode == RhythmMode.day;
  return switch (event) {
    'sunrise' => sunTimes?.sunrise,
    'sunset' => sunTimes?.sunset,
    'civil_twilight' =>
      isDawn ? twilightTimes?.dawn.civil : twilightTimes?.dusk.civil,
    'nautical_twilight' =>
      isDawn ? twilightTimes?.dawn.nautical : twilightTimes?.dusk.nautical,
    'astronomical_twilight' => isDawn
        ? twilightTimes?.dawn.astronomical
        : twilightTimes?.dusk.astronomical,
    _ => null,
  };
}

String? _triggerLabel(RhythmTransitionTrigger trigger) {
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
