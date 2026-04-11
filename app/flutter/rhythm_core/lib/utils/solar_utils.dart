import 'dart:math' as math;

import 'package:flutter/material.dart';

import 'color_utils.dart';

class SolarUtils {
  static double normalizeHour(double hour) {
    return ((hour % 24) + 24) % 24;
  }

  static double hourToAngle(double hour, double solarNoon) {
    var delta = normalizeHour(hour) - normalizeHour(solarNoon);
    if (delta > 12) delta -= 24;
    if (delta < -12) delta += 24;
    return -math.pi / 2 + (delta / 12.0) * math.pi;
  }

  static double angleToHour(double angle, double solarNoon) {
    final delta = (angle + math.pi / 2) * 12.0 / math.pi;
    return normalizeHour(solarNoon + delta);
  }

  static double interpolateValue(
    List<double> hours,
    List<int> values,
    double targetHour,
  ) {
    if (hours.isEmpty || values.isEmpty) return 50.0;
    if (hours.length == 1 || values.length == 1) return values.first.toDouble();

    var lowerIdx = 0;
    var upperIdx = hours.length - 1;

    for (int i = 0; i < hours.length - 1; i++) {
      if (hours[i] <= targetHour && hours[i + 1] >= targetHour) {
        lowerIdx = i;
        upperIdx = i + 1;
        break;
      }
    }

    if (targetHour < hours.first || targetHour > hours.last) {
      lowerIdx = hours.length - 1;
      upperIdx = 0;
    }

    final lowerHour = hours[lowerIdx];
    final upperHour = hours[upperIdx];
    final lowerValue = values[lowerIdx];
    final upperValue = values[upperIdx];

    if (lowerHour == upperHour) return lowerValue.toDouble();

    double t;
    if (upperIdx == 0 && lowerIdx == hours.length - 1) {
      final totalSpan = (24 - lowerHour) + upperHour;
      final position = targetHour >= lowerHour
          ? targetHour - lowerHour
          : (24 - lowerHour) + targetHour;
      t = position / totalSpan;
    } else {
      t = (targetHour - lowerHour) / (upperHour - lowerHour);
    }

    return lowerValue + (upperValue - lowerValue) * t;
  }

  static int brightnessAtHour(
    double hour, {
    required List<double> hours,
    required List<int> brightness,
  }) {
    return interpolateValue(hours, brightness, hour).round();
  }

  static int kelvinAtHour(
    double hour, {
    required List<double> hours,
    required List<int> kelvin,
  }) {
    return interpolateValue(hours, kelvin, hour).round();
  }

  static Color curveColorAt(
    double hour, {
    required List<double> hours,
    required List<int> kelvin,
    Color fallback = const Color(0xFFF0A830),
  }) {
    if (hours.isEmpty || kelvin.isEmpty) return fallback;
    return ColorUtils.curveColorForCCT(
      kelvinAtHour(hour, hours: hours, kelvin: kelvin),
    );
  }

  static String timezoneFromLongitude(double longitude) {
    final offset = (longitude / 15).round();
    const timezones = {
      -10: 'Pacific/Honolulu',
      -9: 'America/Anchorage',
      -8: 'America/Los_Angeles',
      -7: 'America/Denver',
      -6: 'America/Chicago',
      -5: 'America/New_York',
      -4: 'America/Halifax',
      -3: 'America/Sao_Paulo',
      -2: 'Atlantic/South_Georgia',
      -1: 'Atlantic/Azores',
      0: 'Europe/London',
      1: 'Europe/Paris',
      2: 'Europe/Helsinki',
      3: 'Europe/Moscow',
      4: 'Asia/Dubai',
      5: 'Asia/Karachi',
      6: 'Asia/Dhaka',
      7: 'Asia/Bangkok',
      8: 'Asia/Shanghai',
      9: 'Asia/Tokyo',
      10: 'Australia/Sydney',
      11: 'Pacific/Noumea',
      12: 'Pacific/Auckland',
    };
    return timezones[offset] ?? 'UTC';
  }

  static String formatHour(double hour, {bool use24 = false}) {
    final normalizedHour = normalizeHour(hour);
    var hr = normalizedHour.floor();
    var mn = ((normalizedHour - hr) * 60).round();

    if (mn >= 60) {
      hr = (hr + 1) % 24;
      mn = 0;
    }

    if (use24) {
      return '${hr.toString().padLeft(2, '0')}:${mn.toString().padLeft(2, '0')}';
    }

    final period = hr < 12 ? 'a' : 'p';
    final h12 = hr == 0 ? 12 : (hr > 12 ? hr - 12 : hr);
    return '$h12:${mn.toString().padLeft(2, '0')}$period';
  }
}
