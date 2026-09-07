import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/ble_wifi_device_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/device_cloud_broker_service.dart';
import 'package:rhythm_app/services/nearby_ble_discovery_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

const _dsn = 'ACFIXTURE123456';
const _address = 'AA:BB:CC:DD:EE:FF';
const _setupToken = '0123456789abcdef0123456789abcdef';

/// A profile-described family; the screen must never assume a vendor.
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

class _Fixture {
  _Fixture({
    this.provisionFailure,
    this.lanPendingRounds = 0,
    this.candidates = 1,
  });

  final Map<String, dynamic>? provisionFailure;
  final int lanPendingRounds;
  final int candidates;
  final pairHubTypes = <String>[];
  final pairStages = <String>[];
  final pairParams = <Map<String, dynamic>>[];
  final cloudCalls = <String>[];
  int completeCalls = 0;

  Future<Map<String, dynamic>?> pair({
    required String hubType,
    required Map<String, dynamic> params,
    required Duration receiveTimeout,
    required String sessionId,
  }) async {
    pairHubTypes.add(hubType);
    final stage = params['stage'] as String;
    pairStages.add(stage);
    pairParams.add(params);
    switch (stage) {
      case 'discover':
        return {
          'status': 'complete',
          'details': {
            'stage': 'discover',
            'candidates': [
              for (var i = 0; i < candidates; i++)
                {'dsn': i == 0 ? _dsn : 'ACOTHER00000$i', 'address': _address},
            ],
          },
        };
      case 'provision':
        return provisionFailure ??
            {
              'status': 'complete',
              'details': {'stage': 'provision', 'dsn': _dsn},
            };
      case 'adopt':
        return {
          'status': 'complete',
          'details': {'stage': 'adopt', 'dsn': _dsn},
          'devices': [
            {
              'device_id': 'vendor-acfixture123456',
              'name': 'Vendor strip 3456',
              'device_type': 'light',
              'manufacturer': 'Vendor',
              'model': 'strip-1',
            },
          ],
        };
    }
    return null;
  }

  DeviceCloudBrokerService get cloud => DeviceCloudBrokerService(
        canUseOverride: true,
        invoke: (functionName, body) async {
          final action = body['action'] as String;
          cloudCalls.add('$functionName:$action');
          switch (action) {
            case 'begin':
              expect(body['dsn'], _dsn);
              return const DeviceCloudBrokerResponse(
                status: 200,
                setupToken: _setupToken,
                ticket: 'fixture-ticket',
              );
            case 'complete':
              expect(body['ticket'], 'fixture-ticket');
              completeCalls += 1;
              if (completeCalls <= lanPendingRounds) {
                return const DeviceCloudBrokerResponse(
                  status: 409,
                  error: 'device_not_on_lan',
                );
              }
              return const DeviceCloudBrokerResponse(
                status: 200,
                dsn: _dsn,
                ip: '192.168.4.20',
                localKey: 'synthetic-fixture-key',
                localKeyId: 7,
              );
          }
          return const DeviceCloudBrokerResponse(status: 400, error: 'bad');
        },
      );
}

/// The stage timeline animates continuously, so pump bounded frames instead
/// of waiting for the tree to settle.
Future<void> settle(WidgetTester tester, {int frames = 24}) async {
  for (var i = 0; i < frames; i++) {
    await tester.pump(const Duration(milliseconds: 50));
  }
}

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

  Future<BleWifiDevicePairingResult?> pumpScreen(
    WidgetTester tester,
    _Fixture fixture, {
    NearbyBleFamily? family,
  }) async {
    // Tall enough that every action below the timeline is built.
    await tester.binding.setSurfaceSize(const Size(430, 1400));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    BleWifiDevicePairingResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<BleWifiDevicePairingResult>(
                  builder: (_) => BleWifiDeviceAddScreen(
                    family: family ?? stripFamily,
                    analyticsSource: 'test',
                    journeyId: 'ble-wifi-test',
                    pairingRequest: fixture.pair,
                    cloudService: fixture.cloud,
                    completeRetryDelay: const Duration(milliseconds: 10),
                    completeRetryLimit: 3,
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await settle(tester);
    return result;
  }

  testWidgets(
    'runs discover, begin, provision, complete, adopt against the family\'s '
    'hub and broker',
    (tester) async {
      final fixture = _Fixture(lanPendingRounds: 2);
      final result = await pumpScreen(tester, fixture);

      expect(fixture.pairStages, ['discover', 'provision', 'adopt']);
      expect(fixture.pairHubTypes.toSet(), {'vendor_hub'});
      expect(fixture.cloudCalls, [
        'vendor-device:begin',
        'vendor-device:complete',
        'vendor-device:complete',
        'vendor-device:complete',
      ]);
      expect(fixture.pairParams[0]['profile_id'], 'vendor.strip.light.v1');
      expect(fixture.pairParams[1]['dsn'], _dsn);
      expect(fixture.pairParams[1]['address'], _address);
      expect(fixture.pairParams[1]['setup_token'], _setupToken);
      expect(fixture.pairParams[2]['local_key'], 'synthetic-fixture-key');
      expect(fixture.pairParams[2]['local_key_id'], 7);
      expect(result?.device.nativeDeviceId, 'vendor-acfixture123456');
      expect(result?.device.name, 'Vendor strip 3456');

      final completed = analyticsBackend.events.where(
        (event) => event.name == 'ble_wifi_pairing_completed',
      );
      expect(completed.single.properties['outcome'], 'succeeded');
      expect(completed.single.properties['family'], 'vendor.strip.light.v1');
      final serialized = analyticsBackend.events
          .map((event) => event.properties.toString())
          .join();
      expect(serialized, isNot(contains('synthetic-fixture-key')));
      expect(serialized, isNot(contains(_setupToken)));
      expect(serialized, isNot(contains(_dsn)));
    },
  );

  testWidgets('copy names the advertised family, not a vendor', (tester) async {
    final fixture = _Fixture(
      provisionFailure: {
        'status': 'failed',
        'error': 'The device could not join Wi-Fi.',
        'details': {'stage': 'provision', 'uncertain': false},
      },
    );
    await pumpScreen(tester, fixture);
    expect(find.text('Add Vendor strip'), findsOneWidget);
    expect(find.text('Adding a Vendor strip'), findsOneWidget);
    expect(find.textContaining('Monster'), findsNothing);
  });

  testWidgets('an uncertain Wi-Fi write is never retried automatically', (
    tester,
  ) async {
    final fixture = _Fixture(
      provisionFailure: {
        'status': 'failed',
        'error': 'Wi-Fi setup did not complete.',
        'details': {'stage': 'provision', 'uncertain': true},
      },
    );
    await pumpScreen(tester, fixture);

    expect(fixture.pairStages, ['discover', 'provision']);
    expect(
      find.byKey(const ValueKey('ble-wifi-pairing-error')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('ble-wifi-continue-registration')),
      findsOneWidget,
    );
    expect(find.text('Scan Again'), findsOneWidget);

    await tester.tap(
      find.byKey(const ValueKey('ble-wifi-continue-registration')),
    );
    await settle(tester);
    expect(fixture.pairStages, ['discover', 'provision', 'adopt']);
    expect(fixture.cloudCalls, [
      'vendor-device:begin',
      'vendor-device:complete',
    ]);
    final attempts = analyticsBackend.events
        .where((event) => event.name == 'ble_wifi_pairing_attempted')
        .toList();
    expect(attempts.last.properties['resumed'], true);
  });

  testWidgets('a definite Wi-Fi failure offers only a fresh scan', (
    tester,
  ) async {
    final fixture = _Fixture(
      provisionFailure: {
        'status': 'failed',
        'error': 'The device could not join Wi-Fi.',
        'details': {'stage': 'provision', 'uncertain': false},
      },
    );
    await pumpScreen(tester, fixture);

    expect(find.text('The device could not join Wi-Fi.'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('ble-wifi-continue-registration')),
      findsNothing,
    );
    final completed = analyticsBackend.events.where(
      (event) => event.name == 'ble_wifi_pairing_completed',
    );
    expect(completed.single.properties['failure_stage'], 'provision');
  });

  testWidgets('multiple devices require an explicit choice', (tester) async {
    final fixture = _Fixture(candidates: 2);
    await tester.pumpWidget(
      MaterialApp(
        home: BleWifiDeviceAddScreen(
          family: stripFamily,
          analyticsSource: 'test',
          journeyId: 'ble-wifi-test',
          pairingRequest: fixture.pair,
          cloudService: fixture.cloud,
          completeRetryDelay: const Duration(milliseconds: 10),
        ),
      ),
    );
    await settle(tester);

    expect(find.text('Choose the device to add'), findsOneWidget);
    expect(fixture.cloudCalls, isEmpty);
    await tester.tap(find.byKey(const ValueKey('ble-wifi-candidate-$_dsn')));
    await settle(tester);
    expect(fixture.cloudCalls.first, 'vendor-device:begin');
  });

  testWidgets('a family without a broker fails before any radio work', (
    tester,
  ) async {
    final fixture = _Fixture();
    final noBroker = NearbyBleFamily.fromProfile(
      hubType: 'vendor_hub',
      profile: const RhythmDeviceProfile(
        id: 'vendor.cloudless.light.v1',
        deviceType: 'light',
        displayName: 'Cloudless strip',
        inputOnly: false,
        nearbyServiceUuids: ['0000fe28-0000-1000-8000-00805f9b34fb'],
      ),
    );
    await pumpScreen(tester, fixture, family: noBroker);
    expect(fixture.pairStages, isEmpty);
    expect(
      find.text('This device type needs a cloud step this app cannot run yet.'),
      findsOneWidget,
    );
  });

  testWidgets('signed-out users are told to sign in before any radio work', (
    tester,
  ) async {
    var pairs = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: BleWifiDeviceAddScreen(
          family: stripFamily,
          analyticsSource: 'test',
          journeyId: 'ble-wifi-test',
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) async {
            pairs += 1;
            return null;
          },
          cloudService: DeviceCloudBrokerService(
            canUseOverride: false,
            invoke: (_, __) async =>
                const DeviceCloudBrokerResponse(status: 401),
          ),
        ),
      ),
    );
    await settle(tester);

    expect(pairs, 0);
    expect(
      find.text('Sign in to your Rhythm account to add this device.'),
      findsOneWidget,
    );
  });
}
