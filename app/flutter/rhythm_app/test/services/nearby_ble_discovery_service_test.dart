import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/nearby_ble_discovery_service.dart';

import '../helpers/capturing_analytics_backend.dart';

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

  test('classifies Hue FE0F and Ayla FE28 advertisements by family', () {
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['0000FE0F-0000-1000-8000-00805F9B34FB'],
        connectable: true,
      ),
      NearbyBleFamily.hueBle,
    );
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['0000fe28-0000-1000-8000-00805f9b34fb'],
        connectable: true,
      ),
      NearbyBleFamily.monster,
    );
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['0000fe28-0000-1000-8000-00805f9b34fb'],
        connectable: false,
      ),
      isNull,
    );
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['0000fe28-0000-1000-8000-00805f9b34fb'],
        connectable: true,
        families: const {NearbyBleFamily.hueBle},
      ),
      isNull,
      reason: 'families the appliance cannot onboard are never reported',
    );
    expect(
      nearbyBleFamilyForAdvertisement(
        serviceUuids: const ['72797468-6d00-1000-8000-00805f9b34fb'],
        connectable: true,
      ),
      isNull,
    );
  });

  test('reports bounded per-family counts and logs only counts', () async {
    Set<NearbyBleFamily>? requested;
    final service = NearbyBleDiscoveryService(
      platformScan: (families, timeout) async {
        requested = families;
        return const NearbyBleDiscoveryResult(
          NearbyBleDiscoveryOutcome.found,
          counts: {NearbyBleFamily.monster: 1, NearbyBleFamily.hueBle: 42},
        );
      },
    );

    final result = await service.discover(
      source: 'test',
      families: {NearbyBleFamily.monster, NearbyBleFamily.hueBle},
    );

    expect(requested, {NearbyBleFamily.monster, NearbyBleFamily.hueBle});
    expect(result.found, isTrue);
    expect(result.families, [NearbyBleFamily.hueBle, NearbyBleFamily.monster]);
    expect(NearbyBleFamily.monster.countLabel(1), 'Monster Neon Flow');
    expect(NearbyBleFamily.hueBle.countLabel(3), '3 Hue Bluetooth bulbs');

    final event = analyticsBackend.events.singleWhere(
      (event) => event.name == 'device_pairing_nearby_scan_completed',
    );
    expect(event.properties['outcome'], 'found');
    expect(event.properties['monster_count'], 1);
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
      families: {NearbyBleFamily.monster},
    );
    expect(failed.outcome, NearbyBleDiscoveryOutcome.failed);
    expect(scans, 1);
  });
}
