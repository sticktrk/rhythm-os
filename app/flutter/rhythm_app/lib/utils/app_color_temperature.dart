import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:rhythm_core/rhythm_core.dart' as rhythm_colors show ColorUtils;
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmLightCapabilities;

/// Safe editor envelope selected from the node capability contract.
class AppColorTemperatureRange {
  static const legacy = AppColorTemperatureRange(
    minKelvin: 1500,
    maxKelvin: 6500,
  );

  final int minKelvin;
  final int maxKelvin;

  const AppColorTemperatureRange({
    required this.minKelvin,
    required this.maxKelvin,
  });

  /// Resolve the range a profile editor may expose.
  ///
  /// Global and legacy/unknown scopes retain the conservative historical
  /// range. Only a node-scoped, server-proven range can widen it. An explicit
  /// empty capability object returns null, which means the node is known not
  /// to support adjustable color temperature.
  static AppColorTemperatureRange? forEditor({
    required bool nodeScoped,
    required RhythmLightCapabilities? capabilities,
  }) {
    if (!nodeScoped || capabilities == null) return legacy;
    final colorTemperature = capabilities.colorTemperature;
    if (colorTemperature == null ||
        colorTemperature.maxKelvin <= colorTemperature.minKelvin) {
      return null;
    }
    return AppColorTemperatureRange(
      minKelvin: colorTemperature.minKelvin,
      maxKelvin: colorTemperature.maxKelvin,
    );
  }
}

/// App-facing color-temperature rendering across the full range reported by
/// modern lights.
///
/// The shared legacy helper intentionally clamps at 6500 K. Keep its familiar
/// curve tint through that range, then extend smoothly toward the physical
/// black-body approximation for cooler-capable lights.
abstract final class AppColorTemperature {
  static const int minimumRenderKelvin = 500;
  static const int maximumRenderKelvin = 40000;
  static const int legacyMaximumKelvin = 6500;

  static Color toColor(int kelvin) {
    final t =
        kelvin.clamp(minimumRenderKelvin, maximumRenderKelvin).toDouble() / 100;

    final double red;
    if (t <= 66) {
      red = 255;
    } else {
      red = (329.698727446 * math.pow(t - 60, -0.1332047592)).clamp(0, 255);
    }

    final double green;
    if (t <= 66) {
      green = (99.4708025861 * math.log(t) - 161.1195681661).clamp(0, 255);
    } else {
      green = (288.1221695283 * math.pow(t - 60, -0.0755148492)).clamp(0, 255);
    }

    final double blue;
    if (t >= 66) {
      blue = 255;
    } else if (t <= 19) {
      blue = 0;
    } else {
      blue = (138.5177312231 * math.log(t - 10) - 305.0447927307).clamp(0, 255);
    }

    return Color.fromRGBO(
      red.round(),
      green.round(),
      blue.round(),
      1,
    );
  }

  static Color curveColor(int kelvin) {
    if (kelvin <= legacyMaximumKelvin) {
      return rhythm_colors.ColorUtils.curveColorForCCT(kelvin);
    }

    final legacyEdge =
        rhythm_colors.ColorUtils.curveColorForCCT(legacyMaximumKelvin);
    final target = toColor(kelvin);
    final progress =
        ((kelvin - legacyMaximumKelvin) / (20000 - legacyMaximumKelvin))
            .clamp(0.0, 1.0);
    return Color.lerp(legacyEdge, target, progress)!;
  }
}
