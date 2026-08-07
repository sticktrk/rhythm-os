import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/models/config_model.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('ConfigModel', () {
    test('Home reconciliation preserves an unsaved local edit', () {
      final model = ConfigModel();
      final saved = defaultCurveConfig.copyWith(maxBrightness: 80);
      final edited = saved.copyWith(maxBrightness: 70);
      final lateHomeUpdate = saved.copyWith(maxBrightness: 90);

      model.updateFromHomeCurveConfig(saved);
      model.updateConfig(edited);
      model.updateFromHomeCurveConfig(lateHomeUpdate);

      expect(model.config, edited);
      expect(model.canResetToSaved, isTrue);
    });

    test('starts with shared default config and solar context', () {
      final model = ConfigModel();

      expect(model.rawConfig.minColorTemp, defaultCurveConfig.minColorTemp);
      expect(model.rawConfig.maxColorTemp, defaultCurveConfig.maxColorTemp);
      expect(model.config, defaultCurveConfig);
      expect(model.solar, SolarContext.defaults());
      expect(model.selectedHour, inInclusiveRange(0.0, 24.0));
      expect(model.activeHalf, 'morning');
      expect(model.calloutAutoFollow, isTrue);
    });

    test('updateConfig syncs rawConfig fields', () {
      final model = ConfigModel();
      final updated = model.config.copyWith(
        minBrightness: 12,
        maxBrightness: 92,
        widthLeftBri: 1.25,
      );

      model.updateConfig(updated);

      expect(model.config, updated);
      expect(model.rawConfig.minBrightness, 12);
      expect(model.rawConfig.maxBrightness, 92);
      expect(model.rawConfig.widthLeftBri, 1.25);
    });

    test('resetToDefaults restores shared defaults after edits', () {
      final model = ConfigModel();
      model.updateConfig(model.config.copyWith(
        minColorTemp: 2400,
        shapeP: 4.0,
      ));

      model.resetToDefaults();

      expect(model.config, defaultCurveConfig);
      expect(model.rawConfig.minColorTemp, defaultCurveConfig.minColorTemp);
      expect(model.rawConfig.shapeP, defaultCurveConfig.shapeP);
    });

    test('setSelectedHour clamps range and updates activeHalf', () {
      final model = ConfigModel();

      model.setSelectedHour(30.0, solarNoon: 12.0);
      expect(model.selectedHour, 24.0);
      expect(model.activeHalf, 'evening');
      expect(model.calloutAutoFollow, isFalse);

      model.setSelectedHour(-2.0, solarNoon: 12.0);
      expect(model.selectedHour, 0.0);
      expect(model.activeHalf, 'morning');
    });
  });
}
