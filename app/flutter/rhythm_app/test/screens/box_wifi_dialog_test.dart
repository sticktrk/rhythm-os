import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/network/box_wifi_dialog.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

const _catalog = RhythmWifiProfiles(
  revision: 3,
  defaultId: 'iot',
  boxProfileId: 'home',
  profiles: [
    RhythmWifiProfile(id: 'home', ssid: 'Home fixture'),
    RhythmWifiProfile(id: 'iot', ssid: 'IoT fixture'),
  ],
);

Future<BoxWifiChoice?> _open(
  WidgetTester tester,
  RhythmWifiProfiles? catalog,
  Future<void> Function() interact,
) async {
  BoxWifiChoice? result;
  var closed = false;
  await tester.pumpWidget(MaterialApp(
    home: Builder(
      builder: (context) => TextButton(
        onPressed: () async {
          result = await showDialog<BoxWifiChoice>(
            context: context,
            builder: (_) => BoxWifiDialog(catalog: catalog),
          );
          closed = true;
        },
        child: const Text('Open'),
      ),
    ),
  ));
  await tester.tap(find.text('Open'));
  await tester.pumpAndSettle();
  await interact();
  await tester.pumpAndSettle();
  expect(closed, isTrue);
  return result;
}

TextButton _change(WidgetTester tester) =>
    tester.widget(find.byKey(const ValueKey('box-wifi-change')));

void main() {
  testWidgets('a saved network moves the Box without any password',
      (tester) async {
    final choice = await _open(tester, _catalog, () async {
      // The network the Box is already on is not a destination.
      expect(find.text('Home fixture'), findsNothing);
      expect(find.byKey(const ValueKey('box-wifi-ssid')), findsNothing);
      expect(_change(tester).onPressed, isNull);
      await tester.tap(find.text('IoT fixture'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('box-wifi-change')));
    });
    expect(choice?.profileId, 'iot');
    expect(choice?.ssid, 'IoT fixture');
    expect(choice?.password, isNull);
  });

  testWidgets('another network is still typed', (tester) async {
    final choice = await _open(tester, _catalog, () async {
      await tester.tap(find.text('Other network'));
      await tester.pumpAndSettle();
      expect(_change(tester).onPressed, isNull);
      await tester.enterText(
          find.byKey(const ValueKey('box-wifi-ssid')), ' New fixture ');
      await tester.enterText(
          find.byKey(const ValueKey('box-wifi-password')), 'fixture-secret');
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('box-wifi-change')));
    });
    expect(choice?.profileId, isNull);
    expect(choice?.ssid, 'New fixture');
    expect(choice?.password, 'fixture-secret');
  });

  testWidgets('without saved networks the dialog is typed entry only',
      (tester) async {
    final choice = await _open(tester, null, () async {
      expect(find.text('Other network'), findsNothing);
      await tester.enterText(
          find.byKey(const ValueKey('box-wifi-ssid')), 'Only fixture');
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('box-wifi-change')));
    });
    expect(choice?.ssid, 'Only fixture');
    expect(choice?.password, '');
  });
}
