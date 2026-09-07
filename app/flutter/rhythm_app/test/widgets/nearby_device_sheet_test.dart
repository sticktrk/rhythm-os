import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/nearby_ble_discovery_service.dart';
import 'package:rhythm_app/widgets/nearby_device_sheet.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

final stripFamily = NearbyBleFamily.fromProfile(
  hubType: 'vendor_hub',
  profile: const RhythmDeviceProfile(
    id: 'vendor.strip.light.v1',
    deviceType: 'light',
    displayName: 'Vendor strip',
    inputOnly: false,
    onboardingMethods: [RhythmDeviceOnboardingMethod.bleWifiNearbyScan],
    nearbyServiceUuids: ['0000fe28-0000-1000-8000-00805f9b34fb'],
    cloudBroker: 'vendor-device',
  ),
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late CapturingAnalyticsBackend analyticsBackend;

  setUp(() async {
    analyticsBackend = CapturingAnalyticsBackend();
    await analyticsBackend.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: analyticsBackend,
    );
    await AnalyticsService().initialize();
  });

  tearDown(BackendProvider.resetForTesting);

  /// Opens the sheet and returns the future that completes with the choice.
  Future<Future<NearbyBleFamily?>> open(
    WidgetTester tester, {
    required Set<NearbyBleFamily> families,
    required NearbyBleDiscoveryRequest discoveryRequest,
  }) async {
    Future<NearbyBleFamily?>? sheet;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () {
              sheet = showNearbyDeviceSheet(
                context,
                families: families,
                source: 'test',
                discoveryRequest: discoveryRequest,
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(sheet, isNotNull);
    return sheet!;
  }

  testWidgets('lists only families with advertisers and returns the choice', (
    tester,
  ) async {
    Set<NearbyBleFamily>? requested;
    final chosenFuture = await open(
      tester,
      families: {NearbyBleFamily.hueBle, stripFamily},
      discoveryRequest: ({required source, required families}) async {
        requested = families;
        return NearbyBleDiscoveryResult(
          NearbyBleDiscoveryOutcome.found,
          counts: {stripFamily: 2},
        );
      },
    );
    await tester.pumpAndSettle();

    expect(requested, {NearbyBleFamily.hueBle, stripFamily});
    expect(
      find.byKey(const ValueKey('nearby-device-family-vendor.strip.light.v1')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('nearby-device-family-hue_ble')),
      findsNothing,
    );
    expect(find.text('Vendor strip (2 nearby)'), findsOneWidget);

    await tester.tap(
      find.byKey(const ValueKey('nearby-device-family-vendor.strip.light.v1')),
    );
    await tester.pumpAndSettle();
    expect(await chosenFuture, stripFamily);

    final selected = analyticsBackend.events.singleWhere(
      (event) => event.name == 'device_pairing_nearby_family_selected',
    );
    expect(selected.properties['family'], 'vendor.strip.light.v1');
    expect(selected.properties['device_count'], 2);
  });

  testWidgets('explains an empty scan and lets the person scan again', (
    tester,
  ) async {
    var scans = 0;
    final chosenFuture = await open(
      tester,
      families: {stripFamily},
      discoveryRequest: ({required source, required families}) async {
        scans += 1;
        return NearbyBleDiscoveryResult(
          scans == 1
              ? NearbyBleDiscoveryOutcome.bluetoothUnavailable
              : NearbyBleDiscoveryOutcome.none,
        );
      },
    );
    await tester.pumpAndSettle();

    expect(
      find.text('Turn on Bluetooth to look for nearby devices.'),
      findsOneWidget,
    );
    await tester.tap(find.byKey(const ValueKey('nearby-device-sheet-rescan')));
    await tester.pumpAndSettle();
    expect(scans, 2);
    expect(
      find.byKey(const ValueKey('nearby-device-sheet-empty')),
      findsOneWidget,
    );

    await tester.tap(find.byKey(const ValueKey('nearby-device-sheet-close')));
    await tester.pumpAndSettle();
    expect(await chosenFuture, isNull);
  });
}
