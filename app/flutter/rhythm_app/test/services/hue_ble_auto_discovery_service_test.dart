import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/hue_ble_auto_discovery_service.dart';

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

  test('matches only connectable Hue FE0F advertisements', () {
    expect(
      isHueBleDiscoveryAdvertisement(
        serviceUuids: const [
          '0000fe0f-0000-1000-8000-00805F9B34FB',
        ],
        connectable: true,
      ),
      isTrue,
    );
    expect(
      isHueBleDiscoveryAdvertisement(
        serviceUuids: const [
          '72797468-6d00-1000-8000-00805f9b34fb',
        ],
        connectable: true,
      ),
      isFalse,
    );
    expect(
      isHueBleDiscoveryAdvertisement(
        serviceUuids: const [
          '0000fe0f-0000-1000-8000-00805f9b34fb',
        ],
        connectable: false,
      ),
      isFalse,
    );
  });

  test('shares one in-flight scan and records one privacy-safe outcome',
      () async {
    final scanResult = Completer<HueBleDiscoveryResult>();
    var scanCalls = 0;
    final service = HueBleAutoDiscoveryService(
      platformScan: (timeout) {
        scanCalls += 1;
        expect(timeout, HueBleAutoDiscoveryService.scanTimeout);
        return scanResult.future;
      },
    );

    final fromReview = service.discover(source: 'add_review');
    final fromCamera = service.discover(source: 'device_camera');
    expect(identical(fromReview, fromCamera), isTrue);
    expect(scanCalls, 1);

    scanResult.complete(
      const HueBleDiscoveryResult(
        HueBleDiscoveryOutcome.found,
        deviceCount: 1,
      ),
    );
    expect((await fromReview).found, isTrue);
    expect((await fromCamera).deviceCount, 1);

    final event = analyticsBackend.events.singleWhere(
      (event) => event.name == 'hue_ble_nearby_discovery_completed',
    );
    expect(event.properties, {
      'source': 'add_review',
      'outcome': 'found',
      'device_count': 1,
    });
    expect(
      event.properties.keys,
      isNot(contains(anyOf('device_id', 'bluetooth_id', 'rssi', 'name'))),
    );
  });

  test('turns platform failures into a non-blocking bounded outcome', () async {
    final service = HueBleAutoDiscoveryService(
      platformScan: (timeout) => Future.error(
        StateError('raw adapter identity must not be captured'),
      ),
    );

    final result = await service.discover(source: 'add_review');

    expect(result.outcome, HueBleDiscoveryOutcome.failed);
    final event = analyticsBackend.events.singleWhere(
      (event) => event.name == 'hue_ble_nearby_discovery_completed',
    );
    expect(event.properties, {
      'source': 'add_review',
      'outcome': 'failed',
      'device_count': 0,
    });
    expect(
      event.properties.values,
      isNot(contains('raw adapter identity must not be captured')),
    );
  });
}
