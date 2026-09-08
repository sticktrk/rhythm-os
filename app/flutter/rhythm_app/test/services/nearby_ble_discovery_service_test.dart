import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/nearby_ble_discovery_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

/// A profile-described family, exactly as an appliance would advertise it.
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
    AnalyticsService().resetForTesting();
    await AnalyticsService().initialize();
  });

  tearDown(() {
    AnalyticsService().resetForTesting();
    BackendProvider.resetForTesting();
  });

  test('families come from advertised profiles, not app knowledge', () {
    expect(stripFamily.kind, NearbyBleFamilyKind.bleWifi);
    expect(stripFamily.hubType, 'vendor_hub');
    expect(stripFamily.label, 'Vendor strip');
    expect(stripFamily.cloudBroker, 'vendor-device');
    expect(stripFamily.countLabel(1), 'Vendor strip');
    expect(stripFamily.countLabel(3), 'Vendor strip (3 nearby)');
    expect(NearbyBleFamily.hueBle.countLabel(1), 'Hue Bluetooth bulb');
    final unnamed = NearbyBleFamily.fromProfile(
      hubType: 'x',
      profile: const RhythmDeviceProfile(
        id: 'x.y.light.v1',
        deviceType: 'light',
        displayName: '  ',
        inputOnly: false,
      ),
    );
    expect(unnamed.label, 'Wi-Fi light');
  });

  test('classifies advertisements by each family\'s service UUIDs', () {
    final families = [NearbyBleFamily.hueBle, stripFamily];
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['0000FE0F-0000-1000-8000-00805F9B34FB'],
        connectable: true,
        families: families,
      ),
      NearbyBleFamily.hueBle,
    );
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['0000fe28-0000-1000-8000-00805f9b34fb'],
        connectable: true,
        families: families,
      ),
      stripFamily,
    );
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['FE28'],
        connectable: true,
        families: families,
      ),
      stripFamily,
      reason: 'flutter_blue_plus emits Bluetooth SIG UUIDs in compact form',
    );
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['0000fe28-0000-1000-8000-00805f9b34fb'],
        connectable: false,
        families: families,
      ),
      isNull,
    );
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['0000fe28-0000-1000-8000-00805f9b34fb'],
        connectable: true,
        families: const [NearbyBleFamily.hueBle],
      ),
      isNull,
      reason: 'families the appliance cannot onboard are never reported',
    );
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['72797468-6d00-1000-8000-00805f9b34fb'],
        connectable: true,
        families: families,
      ),
      isNull,
    );
  });

  test('reports bounded per-family counts and logs only counts', () async {
    Set<NearbyBleFamily>? requested;
    final service = NearbyBleDiscoveryService(
      platformScan: (families, timeout) async {
        requested = families;
        return NearbyBleDiscoveryResult(
          NearbyBleDiscoveryOutcome.found,
          counts: {stripFamily: 1, NearbyBleFamily.hueBle: 42},
        );
      },
    );

    final result = await service.discover(
      source: 'test',
      families: {stripFamily, NearbyBleFamily.hueBle},
    );

    expect(requested, {stripFamily, NearbyBleFamily.hueBle});
    expect(result.found, isTrue);
    expect(result.families, [NearbyBleFamily.hueBle, stripFamily]);

    final event = analyticsBackend.events.singleWhere(
      (event) => event.name == 'device_pairing_nearby_scan_completed',
    );
    expect(event.properties['outcome'], 'found');
    expect(event.properties['vendor.strip.light.v1_count'], 1);
    expect(event.properties['hue_ble_count'], 10, reason: 'bounded');
    expect(event.properties.keys, isNot(contains('address')));
  });

  test('empty family set never scans and scan errors become failed', () async {
    var scans = 0;
    final service = NearbyBleDiscoveryService(
      platformScan: (families, timeout) async {
        scans += 1;
        throw StateError('boom');
      },
    );
    final none = await service.discover(source: 'test', families: const {});
    expect(none.outcome, NearbyBleDiscoveryOutcome.none);
    expect(scans, 0);

    final failed = await service.discover(
      source: 'test',
      families: {stripFamily},
    );
    expect(failed.outcome, NearbyBleDiscoveryOutcome.failed);
    expect(scans, 1);
  });
}
