import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/observed_light_status.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  testWidgets('physical output and unavailable state render distinctly',
      (tester) async {
    Future<void> show(String availability) => tester.pumpWidget(MaterialApp(
          home: Scaffold(
              body: ObservedLightStatus(
            observation: RhythmObservedLight.maybeFromJson({
              'availability': availability,
              'lights_on': true,
              'brightness': 23,
              'kelvin': 2700,
              'received_at_epoch_ms': 1,
            })!,
          )),
        ));
    await show('available');
    expect(find.text('On · 23% · 2700 K'), findsOneWidget);
    await show('unavailable');
    expect(find.text('Unavailable in Home Assistant'), findsOneWidget);
    expect(find.textContaining('23%'), findsNothing);
    await show('unknown');
    expect(find.text('State unknown in Home Assistant'), findsOneWidget);
    expect(find.text('Off'), findsNothing);
  });
}
