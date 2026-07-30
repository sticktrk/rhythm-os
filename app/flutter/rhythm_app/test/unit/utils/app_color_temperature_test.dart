import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/utils/app_color_temperature.dart';
import 'package:rhythm_core/rhythm_core.dart' show ColorUtils;
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  test('preserves the established rendering at the legacy maximum', () {
    expect(
      AppColorTemperature.toColor(6500),
      ColorUtils.cctToColor(6500),
    );
    expect(
      AppColorTemperature.curveColor(6500),
      ColorUtils.curveColorForCCT(6500),
    );
  });

  test('renders 20000 K without clamping it to 6500 K', () {
    final legacy = AppColorTemperature.toColor(6500);
    final extended = AppColorTemperature.toColor(20000);

    expect(extended, isNot(legacy));
    expect(extended.b, 1);
    expect(extended.r, lessThan(legacy.r));
    expect(extended.g, lessThan(legacy.g));
  });

  test('extended curve tint changes smoothly beyond 6500 K', () {
    final legacyEdge = AppColorTemperature.curveColor(6500);
    final justBeyond = AppColorTemperature.curveColor(6501);
    final extended = AppColorTemperature.curveColor(20000);

    expect(justBeyond.r, closeTo(legacyEdge.r, 1 / 255));
    expect(justBeyond.g, closeTo(legacyEdge.g, 1 / 255));
    expect(justBeyond.b, closeTo(legacyEdge.b, 1 / 255));
    expect(extended, isNot(legacyEdge));
  });

  group('profile editor range', () {
    const extended = RhythmLightCapabilities(
      colorTemperature: RhythmColorTemperatureCapabilities(
        minKelvin: 1000,
        maxKelvin: 20000,
      ),
    );

    test('widens only a node-scoped editor with a proven range', () {
      final range = AppColorTemperatureRange.forEditor(
        nodeScoped: true,
        capabilities: extended,
      );

      expect(range?.minKelvin, 1000);
      expect(range?.maxKelvin, 20000);
    });

    test('keeps global and unknown editors on conservative bounds', () {
      final global = AppColorTemperatureRange.forEditor(
        nodeScoped: false,
        capabilities: extended,
      );
      final legacyNode = AppColorTemperatureRange.forEditor(
        nodeScoped: true,
        capabilities: null,
      );

      expect(global?.minKelvin, 1500);
      expect(global?.maxKelvin, 6500);
      expect(legacyNode?.minKelvin, 1500);
      expect(legacyNode?.maxKelvin, 6500);
    });

    test('returns no editor range for known non-CT lights or rooms', () {
      final range = AppColorTemperatureRange.forEditor(
        nodeScoped: true,
        capabilities: const RhythmLightCapabilities(),
      );

      expect(range, isNull);
    });
  });
}
