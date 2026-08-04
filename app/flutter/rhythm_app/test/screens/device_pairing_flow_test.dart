import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/device_pairing_flow.dart';
import 'package:rhythm_app/screens/hubs/device_pairing_scanner_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  testWidgets('universal intake exposes nearby Hue separately from codes', (
    tester,
  ) async {
    DevicePairingTarget? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await showUniversalDevicePairingIntakeChooser(context);
            },
            child: const Text('Add device'),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Add device'));
    await tester.pumpAndSettle();

    expect(
      find.byKey(const ValueKey('pairing-method-hue-ble')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('pairing-method-code')), findsOneWidget);
    expect(find.text('No QR code or serial required'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('pairing-method-hue-ble')));
    await tester.pumpAndSettle();
    expect(result, DevicePairingTarget.hueBle);
  });

  testWidgets('Bridge chooser returns the exact selected address', (
    tester,
  ) async {
    String? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await showHueBridgePairingTargetChooser(
                context,
                const [
                  HueBridgePairingTarget(
                    address: '192.0.2.10:443',
                    label: 'Upstairs · 192.0.2.10:443',
                  ),
                  HueBridgePairingTarget(
                    address: '192.0.2.11:443',
                    label: 'Downstairs · 192.0.2.11:443',
                  ),
                ],
              );
            },
            child: const Text('Choose bridge'),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Choose bridge'));
    await tester.pumpAndSettle();
    expect(find.text('Choose a Hue Bridge'), findsOneWidget);

    await tester.tap(
      find.byKey(const ValueKey('hue-bridge-choice-192.0.2.11:443')),
    );
    await tester.pumpAndSettle();
    expect(result, '192.0.2.11:443');
  });

  group('paired light room resolution', () {
    test('prefers the canonical device room over stale topology', () {
      final parentNodeId = resolvePairedDeviceParentNodeId(
        {'id': 'light-1', 'room_id': 'room-canonical'},
        topologyParentId: 'room-topology',
      );

      expect(parentNodeId, 'room-canonical');
    });

    test('falls back to topology when canonical room is absent', () {
      final parentNodeId = resolvePairedDeviceParentNodeId(
        {'id': 'light-1', 'room_id': '  '},
        topologyParentId: 'room-topology',
      );

      expect(parentNodeId, 'room-topology');
    });

    test('accepts canonical parent for an already-known scanned device', () {
      final parentNodeId = resolvePairedDeviceParentNodeId(
        {'id': 'button-1', 'parent_id': 'room-existing'},
        topologyParentId: 'room-stale',
      );

      expect(parentNodeId, 'room-existing');
    });
  });

  group('Hue Bridge pairing targets', () {
    test('preserves the universal intake journey for Bridge pairing', () {
      const intake = DevicePairingScannerResult.hueBridge(
        'E277DA',
        journeyId: 'device-pair-outer',
      );

      expect(
        pairingJourneyIdForIntake(
          intake,
          fallbackPrefix: 'hue-bridge-add',
        ),
        'device-pair-outer',
      );
    });

    test('creates a scoped journey when intake has none', () {
      const intake = DevicePairingScannerResult.hueBridge('E277DA');

      expect(
        pairingJourneyIdForIntake(
          intake,
          fallbackPrefix: 'hue-bridge-add',
        ),
        startsWith('hue-bridge-add-'),
      );
    });

    test('keeps every connected Bridge exact and uses configured labels', () {
      final targets = connectedHueBridgePairingTargets(
        serverHubs: const [
          RhythmHubInfo(
            type: 'hue',
            address: '192.0.2.11:443',
            connected: true,
          ),
          RhythmHubInfo(
            type: 'hue',
            address: '192.0.2.10:443',
            connected: true,
          ),
          RhythmHubInfo(
            type: 'hue',
            address: '192.0.2.12:443',
            connected: false,
          ),
          RhythmHubInfo(
            type: 'matter',
            address: 'local',
            connected: true,
          ),
        ],
        serverHubInfos: const [
          {
            'type': 'hue',
            'address': '192.0.2.10:443',
            'connected': true,
            'name': 'Downstairs Bridge',
          },
          {
            'type': 'hue',
            'address': '192.0.2.11:443',
            'connected': true,
          },
        ],
      );

      expect(targets, hasLength(2));
      expect(
        targets.map((target) => target.address).toSet(),
        {'192.0.2.10:443', '192.0.2.11:443'},
      );
      expect(
        targets
            .singleWhere((target) => target.address == '192.0.2.10:443')
            .label,
        'Downstairs Bridge · 192.0.2.10:443',
      );
    });

    test('matches only the selected Bridge endpoint for assignment', () {
      const firstBridgeEndpoint = {
        'native_id': 'hue-v2-device-1',
        'hub_key': {
          'hub_type': 'hue',
          'address': '192.0.2.10:443',
        },
      };

      expect(
        pairedEndpointMatchesTarget(
          firstBridgeEndpoint,
          hubType: 'hue',
          hubAddress: '192.0.2.10:443',
        ),
        isTrue,
      );
      expect(
        pairedEndpointMatchesTarget(
          firstBridgeEndpoint,
          hubType: 'hue',
          hubAddress: '192.0.2.11:443',
        ),
        isFalse,
      );
      expect(
        pairedEndpointMatchesTarget(
          firstBridgeEndpoint,
          hubType: 'hue_ble',
        ),
        isFalse,
      );
    });

    test('deduplicates repeated hello entries by exact address', () {
      final targets = connectedHueBridgePairingTargets(
        serverHubs: const [
          RhythmHubInfo(
            type: 'hue',
            address: 'BRIDGE.LOCAL:443',
            connected: true,
          ),
          RhythmHubInfo(
            type: 'hue',
            address: 'bridge.local:443',
            connected: true,
          ),
        ],
        serverHubInfos: const [],
      );

      expect(targets, hasLength(1));
      expect(targets.single.address, 'bridge.local:443');
    });
  });
}
