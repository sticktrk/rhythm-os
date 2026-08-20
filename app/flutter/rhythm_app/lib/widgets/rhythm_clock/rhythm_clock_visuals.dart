import 'package:flutter/material.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../api/hybrid_client.dart' show sdkCurveConfigToDto;
import '../solar_clock/solar_clock_exports.dart';
import 'rhythm_schedule_clock.dart';

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
