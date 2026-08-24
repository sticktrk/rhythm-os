import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/rhythm_tuner_chart.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  for (final size in <Size>[
    const Size(390, 844),
    const Size(320, 568),
  ]) {
    testWidgets(
        'tuner chart readout fits ${size.width.toInt()}x${size.height.toInt()}',
        (tester) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: RhythmTunerChart(
            config: defaultCurveConfig,
            sunrise: 6.5,
            sunset: 18.5,
            onConfigChanged: (_) {},
          ),
        ),
      ));
      await tester.pump(const Duration(milliseconds: 100));

      expect(find.text('BRI'), findsOneWidget);
      expect(find.text('CCT'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  }
}
