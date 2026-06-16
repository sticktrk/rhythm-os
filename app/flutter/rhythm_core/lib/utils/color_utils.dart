import 'dart:math' as math;
import 'dart:ui';

/// Utility functions for color temperature conversions.
class ColorUtils {
  /// Convert color temperature (Kelvin) to RGB color.
  ///
  /// Uses Tanner Helland's algorithm for Kelvin to RGB conversion.
  /// This matches the reference designer.html implementation exactly.
  /// Valid range: 500K - 6500K (typical lighting range)
  static Color cctToColor(int kelvin) {
    final k = kelvin.clamp(500, 6500);
    final t = k / 100;

    double r, g, b;

    // Red
    if (t <= 66) {
      r = 255;
    } else {
      r = 329.698727446 * math.pow(t - 60, -0.1332047592);
      r = r.clamp(0, 255);
    }

    // Green
    if (t <= 66) {
      g = 99.4708025861 * math.log(t) - 161.1195681661;
      g = g.clamp(0, 255);
    } else {
      g = 288.1221695283 * math.pow(t - 60, -0.0755148492);
      g = g.clamp(0, 255);
    }

    // Blue
    if (t >= 66) {
      b = 255;
    } else if (t <= 19) {
      b = 0;
    } else {
      b = 138.5177312231 * math.log(t - 10) - 305.0447927307;
      b = b.clamp(0, 255);
    }

    return Color.fromRGBO(r.round(), g.round(), b.round(), 1);
  }

  /// Get line color for the brightness curve based on CCT.
  ///
  /// Uses actual CCT→RGB conversion for realistic light simulation.
  /// For cooler temperatures (>6000K), adds a subtle blue tint.
  static Color lineColorForCCT(int kelvin) {
    final base = cctToColor(kelvin);

    // For temperatures above 6000K, blend toward blue for visual clarity
    if (kelvin <= 6000) {
      return base;
    }

    // Blend factor increases from 0 at 6000K to 0.6 at 6500K
    final f = ((kelvin - 6000) / 500).clamp(0.0, 1.0);
    final blendAmount = 0.6 * f;

    // Target blue-white for cool temperatures
    const blueWhite = Color.fromRGBO(150, 200, 255, 1);

    return Color.lerp(base, blueWhite, blendAmount)!;
  }

  /// Get a color for a curve segment based on CCT.
  ///
  /// Uses the actual CCT→RGB conversion for realistic light colors:
  /// - Very warm (500-2000K): Deep orange/red (like candlelight)
  /// - Warm (2700-3000K): Orange/amber (incandescent)
  /// - Neutral (4000-5000K): Warm white
  /// - Cool (5500-6500K): Blue-white (daylight)
  static Color curveColorForCCT(int kelvin) {
    // Use line color which has proper CCT conversion + blue enhancement
    return lineColorForCCT(kelvin);
  }

  /// Generate a list of colors for curve segments.
  ///
  /// Each color corresponds to the midpoint CCT of a segment.
  static List<Color> generateCurveColors(List<int> kelvinValues) {
    if (kelvinValues.isEmpty) return [];
    if (kelvinValues.length == 1) return [curveColorForCCT(kelvinValues[0])];

    final colors = <Color>[];
    for (int i = 0; i < kelvinValues.length - 1; i++) {
      final midK = (kelvinValues[i] + kelvinValues[i + 1]) ~/ 2;
      colors.add(curveColorForCCT(midK));
    }
    return colors;
  }
}
