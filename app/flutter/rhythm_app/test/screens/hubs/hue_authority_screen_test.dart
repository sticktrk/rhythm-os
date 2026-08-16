import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/hue_authority_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  testWidgets('unreviewed rooms default to Hue and takeover needs every room',
      (tester) async {
    const bridge = RhythmHueBridgeAuthority(
      address: 'bridge.local',
      revision: '0123456789abcdef',
      takeoverScope: 'bridge',
      bridgeTakeoverRequested: false,
      rooms: [
        RhythmHueRoomAuthority(
          roomId: 'office',
          name: 'Office',
          owner: RhythmHueRoomAuthorityOwner.unreviewed,
          rhythmAutomationEnabled: false,
        ),
        RhythmHueRoomAuthority(
          roomId: 'bedroom',
          name: 'Bedroom',
          owner: RhythmHueRoomAuthorityOwner.unreviewed,
          rhythmAutomationEnabled: false,
        ),
      ],
    );

    await tester.pumpWidget(
      const MaterialApp(
        home: HueAuthorityScreen(
          bridge: bridge,
          topologySyncSupported: true,
        ),
      ),
    );

    expect(find.text('Keep existing Hue automations'), findsNWidgets(2));
    expect(find.byIcon(Icons.radio_button_checked), findsNWidgets(2));

    await tester.tap(find.text('Rhythm').at(0));
    await tester.pump();
    expect(find.textContaining('All rooms chose Rhythm'), findsNothing);

    await tester.ensureVisible(find.text('Rhythm').at(1));
    await tester.tap(find.text('Rhythm').at(1));
    await tester.pump();
    await tester.drag(find.byType(ListView), const Offset(0, -400));
    await tester.pump();
    expect(find.textContaining('All rooms chose Rhythm'), findsOneWidget);
    expect(find.text('Sync Rhythm rooms to Hue'), findsOneWidget);
    expect(find.byType(SwitchListTile), findsOneWidget);
  });

  testWidgets('room scope explains forced mixed coexistence', (tester) async {
    const bridge = RhythmHueBridgeAuthority(
      address: 'bridge.local',
      revision: '0123456789abcdef',
      takeoverScope: 'room',
      bridgeTakeoverRequested: false,
      rooms: [
        RhythmHueRoomAuthority(
          roomId: 'lights',
          name: 'Living room',
          owner: RhythmHueRoomAuthorityOwner.hue,
          rhythmAutomationEnabled: false,
        ),
        RhythmHueRoomAuthority(
          roomId: 'power',
          name: 'Dust Collector',
          owner: RhythmHueRoomAuthorityOwner.hue,
          rhythmAutomationEnabled: false,
        ),
      ],
    );

    await tester.pumpWidget(
      const MaterialApp(home: HueAuthorityScreen(bridge: bridge)),
    );
    await tester.tap(find.text('Rhythm').first);
    await tester.pump();
    await tester.drag(find.byType(ListView), const Offset(0, -300));
    await tester.pump();

    expect(
      find.textContaining('Rhythm will automate the selected rooms'),
      findsOneWidget,
    );
    expect(find.textContaining('Hue-owned rooms'), findsOneWidget);
    expect(find.textContaining('can conflict with Rhythm'), findsOneWidget);
  });
}
