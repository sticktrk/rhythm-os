import 'package:flutter/material.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../api/hybrid_client.dart' show sdkCurveConfigToDto;
import '../../utils/app_color_temperature.dart';
import '../solar_clock/solar_clock_exports.dart';
import 'rhythm_schedule_clock.dart';

/// Fallback mode colors when no profile color can be resolved — warm amber
/// for Day, warm red for Sleep (never the cool UI accents: the clock's halves
/// represent light output, and sleep light is warm/dim).
const Color fallbackDayColor = Color(0xFFF9A825);
const Color fallbackSleepColor = Color(0xFFE57373);

/// Maps each [RhythmMode] to the dominant color of its active profile, used
/// to tint the orbital clock's ring, orbs, and summary chips. Shared by the
/// whole-house Presets/Alarm screens and the room Schedule tab so the same
/// profile renders the same color everywhere.
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

/// Today's solar clock data (sun + twilight times) for a location, or null
/// when the computation fails. Shared by the alarm editor and the room
/// Schedule tab so both clocks agree on the day's solar geometry.
SolarClockData? computeSolarClockData({
  required double latitude,
  required double longitude,
  required String timezone,
}) {
  try {
    final now = DateTime.now();
    final sunTimes = getSunTimes(
      latitude: latitude,
      longitude: longitude,
      year: now.year,
      month: now.month,
      day: now.day,
      timezone: timezone,
    );
    final twilightTimes = getTwilightTimes(
      latitude: latitude,
      longitude: longitude,
      year: now.year,
      month: now.month,
      day: now.day,
      timezone: timezone,
    );
    return SolarClockData(sunTimes: sunTimes, twilightTimes: twilightTimes);
  } catch (_) {
    return null;
  }
}

/// Ring styling for one mode's active lighting profile: full curve samples
/// when the profile is a solar curve, otherwise a flat fallback carrying the
/// profile's brightness.
ModeCurveVisual buildProfileCurveVisual({
  required RhythmCurveConfig? profile,
  required Color fallbackColor,
  required double latitude,
  required double longitude,
  required String timezone,
}) {
  if (profile == null) {
    return ModeCurveVisual(
      fallbackColor: fallbackColor,
      fallbackBrightness: 50,
    );
  }

  final curve = profile.curve;
  final fallbackBrightness = switch (curve) {
    RhythmConstantCurve() => curve.brightness,
    _ => ((profile.minBrightness + profile.maxBrightness) / 2).round(),
  };

  if (curve is! RhythmSuperGaussianCurve) {
    return ModeCurveVisual(
      fallbackColor: fallbackColor,
      fallbackBrightness: fallbackBrightness,
    );
  }

  try {
    final now = DateTime.now();
    final curveData = generateCurveDataWithSunTimes(
      config: sdkCurveConfigToDto(profile),
      latitude: latitude,
      longitude: longitude,
      year: now.year,
      month: now.month,
      day: now.day,
      timezone: timezone,
    );
    return ModeCurveVisual(
      samples: SolarCurveSamples.fromCurveDataDto(curveData),
      fallbackColor: fallbackColor,
      fallbackBrightness: fallbackBrightness,
    );
  } catch (_) {
    return ModeCurveVisual(
      fallbackColor: fallbackColor,
      fallbackBrightness: fallbackBrightness,
    );
  }
}
