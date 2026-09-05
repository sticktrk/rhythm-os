import 'dart:async';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/app_startup_performance.dart';
import '../helpers/capturing_analytics_backend.dart';

void main() {
  test('cold and resume journeys separate phases and bucket device counts',
      () async {
    PackageInfo.setMockInitialValues(
        appName: 'Rhythm',
        packageName: 'test.rhythm',
        version: '4.0.1',
        buildNumber: '1',
        buildSignature: '');
    final backend = CapturingAnalyticsBackend();
    await backend.initialize();
    BackendProvider.setInstanceForTesting(
        auth: OfflineAuthBackend(), analytics: backend);
    final analytics = AnalyticsService()..resetForTesting();
    await analytics.initialize();
    addTearDown(() {
      analytics.resetForTesting();
      BackendProvider.resetForTesting();
    });
    final perf = AppStartupPerformance.instance;
    perf.start();
    perf.recordPhase(AppStartupPhase.settings, 12);
    perf.recordServer(
        nodes: 201,
        devices: 190,
        version: '0.6.632',
        responseBytes: 100000,
        transport: 'lan');
    perf.markAllRoomsVisible(fromCache: true, roomCount: 11);
    perf.markAllRoomsInteractive(roomCount: 11);
    perf.markAllRoomsInteractive(roomCount: 11);
    await Future<void>.delayed(Duration.zero);
    final cold =
        backend.events.where((e) => e.name == 'app_control_readiness').single;
    expect(cold.properties, containsPair('settings_ms', 12));
    expect(cold.properties, containsPair('node_count_bucket', '201_plus'));
    expect(cold.properties, containsPair('device_count_bucket', '101_200'));
    expect(cold.properties, containsPair('room_count_bucket', '11_50'));
    expect(cold.properties, containsPair('payload_size_bucket', '64k_256k'));
    perf.start(resumed: true);
    final pending = Completer<void>();
    final stale = perf.measure(AppStartupPhase.auth, () => pending.future);
    perf.start(resumed: true);
    pending.complete();
    await stale;
    perf.recordPhase(AppStartupPhase.request, 30);
    perf.markAllRoomsInteractive(roomCount: 11);
    await Future<void>.delayed(Duration.zero);
    final journeys =
        backend.events.where((e) => e.name == 'app_control_readiness').toList();
    expect(journeys, hasLength(2));
    expect(journeys.last.properties, containsPair('journey_kind', 'resume'));
    expect(journeys.last.properties['journey_id'],
        isNot(cold.properties['journey_id']));
    expect(journeys.last.properties.containsKey('auth_ms'), isFalse);
    expect(journeys.last.properties.containsKey('settings_ms'), isFalse);
  });
}
