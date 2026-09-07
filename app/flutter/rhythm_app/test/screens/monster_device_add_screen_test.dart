import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/monster_device_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/monster_cloud_service.dart';

import '../helpers/capturing_analytics_backend.dart';

const _dsn = 'ACFIXTURE123456';
const _address = 'AA:BB:CC:DD:EE:FF';
const _setupToken = '0123456789abcdef0123456789abcdef';

class _Fixture {
  _Fixture({
    this.provisionFailure,
    this.lanPendingRounds = 0,
    this.candidates = 1,
  });

  final Map<String, dynamic>? provisionFailure;
  final int lanPendingRounds;
  final int candidates;
  final pairStages = <String>[];
  final pairParams = <Map<String, dynamic>>[];
  final cloudActions = <String>[];
  int completeCalls = 0;

  Future<Map<String, dynamic>?> pair({
    required String hubType,
    required Map<String, dynamic> params,
    required Duration receiveTimeout,
    required String sessionId,
  }) async {
    expect(hubType, 'monster');
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
              'device_id': 'monster-acfixture123456',
              'name': 'Monster Neon Flow 3456',
              'device_type': 'light',
              'manufacturer': 'Monster',
              'model': 'xt-16ft-hw-neon-led-rgbic',
            },
          ],
        };
    }
    return null;
  }

  MonsterCloudService get cloud => MonsterCloudService(
        canUseOverride: true,
        invoke: (body) async {
          final action = body['action'] as String;
          cloudActions.add(action);
          switch (action) {
            case 'begin':
              expect(body['dsn'], _dsn);
              return const MonsterCloudResponse(
                status: 200,
                setupToken: _setupToken,
                ticket: 'fixture-ticket',
              );
            case 'complete':
              expect(body['ticket'], 'fixture-ticket');
              completeCalls += 1;
              if (completeCalls <= lanPendingRounds) {
                return const MonsterCloudResponse(
                  status: 409,
                  error: 'device_not_on_lan',
                );
              }
              return const MonsterCloudResponse(
                status: 200,
                dsn: _dsn,
                ip: '192.168.4.20',
                localKey: 'synthetic-fixture-key',
                localKeyId: 7,
              );
          }
          return const MonsterCloudResponse(status: 400, error: 'invalid');
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

  Future<MonsterDevicePairingResult?> pumpScreen(
    WidgetTester tester,
    _Fixture fixture,
  ) async {
    // Tall enough that every action below the timeline is built.
    await tester.binding.setSurfaceSize(const Size(430, 1400));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    MonsterDevicePairingResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<MonsterDevicePairingResult>(
                  builder: (_) => MonsterDeviceAddScreen(
                    analyticsSource: 'test',
                    journeyId: 'monster-test',
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

  testWidgets('runs discover, begin, provision, complete, adopt in order', (
    tester,
  ) async {
    final fixture = _Fixture(lanPendingRounds: 2);
    final result = await pumpScreen(tester, fixture);

    expect(fixture.pairStages, ['discover', 'provision', 'adopt']);
    expect(fixture.cloudActions, ['begin', 'complete', 'complete', 'complete']);
    expect(fixture.pairParams[1]['dsn'], _dsn);
    expect(fixture.pairParams[1]['address'], _address);
    expect(fixture.pairParams[1]['setup_token'], _setupToken);
    expect(fixture.pairParams[2]['local_key'], 'synthetic-fixture-key');
    expect(fixture.pairParams[2]['local_key_id'], 7);
    expect(result?.device.nativeDeviceId, 'monster-acfixture123456');
    expect(result?.device.name, 'Monster Neon Flow 3456');

    final completed = analyticsBackend.events.where(
      (event) => event.name == 'monster_pairing_completed',
    );
    expect(completed.single.properties['outcome'], 'succeeded');
    final serialized = analyticsBackend.events
        .map((event) => event.properties.toString())
        .join();
    expect(serialized, isNot(contains('synthetic-fixture-key')));
    expect(serialized, isNot(contains(_setupToken)));
    expect(serialized, isNot(contains(_dsn)));
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
    expect(find.byKey(const ValueKey('monster-pairing-error')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('monster-continue-registration')),
      findsOneWidget,
    );
    expect(find.text('Scan Again'), findsOneWidget);

    // Continuing skips the radio entirely and finishes with the retained
    // ticket: no second discover, no second provision.
    await tester
        .tap(find.byKey(const ValueKey('monster-continue-registration')));
    await settle(tester);
    expect(fixture.pairStages, ['discover', 'provision', 'adopt']);
    expect(fixture.cloudActions, ['begin', 'complete']);
    final attempts = analyticsBackend.events
        .where((event) => event.name == 'monster_pairing_attempted')
        .toList();
    expect(attempts.last.properties['resumed'], true);
  });

  testWidgets('a definite Wi-Fi failure offers only a fresh scan', (
    tester,
  ) async {
    final fixture = _Fixture(
      provisionFailure: {
        'status': 'failed',
        'error': 'The strip could not join Wi-Fi.',
        'details': {'stage': 'provision', 'uncertain': false},
      },
    );
    await pumpScreen(tester, fixture);

    expect(find.text('The strip could not join Wi-Fi.'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('monster-continue-registration')),
      findsNothing,
    );
    final completed = analyticsBackend.events.where(
      (event) => event.name == 'monster_pairing_completed',
    );
    expect(completed.single.properties['failure_stage'], 'provision');
  });

  testWidgets('multiple strips require an explicit choice', (tester) async {
    final fixture = _Fixture(candidates: 2);
    await tester.pumpWidget(
      MaterialApp(
        home: MonsterDeviceAddScreen(
          analyticsSource: 'test',
          journeyId: 'monster-test',
          pairingRequest: fixture.pair,
          cloudService: fixture.cloud,
          completeRetryDelay: const Duration(milliseconds: 10),
        ),
      ),
    );
    await settle(tester);

    expect(find.text('Choose the strip to add'), findsOneWidget);
    expect(fixture.cloudActions, isEmpty);
    await tester.tap(find.byKey(const ValueKey('monster-candidate-$_dsn')));
    await settle(tester);
    expect(fixture.cloudActions.first, 'begin');
  });

  testWidgets('signed-out users are told to sign in before any radio work', (
    tester,
  ) async {
    var pairs = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: MonsterDeviceAddScreen(
          analyticsSource: 'test',
          journeyId: 'monster-test',
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) async {
            pairs += 1;
            return null;
          },
          cloudService: MonsterCloudService(
            canUseOverride: false,
            invoke: (_) async => const MonsterCloudResponse(status: 401),
          ),
        ),
      ),
    );
    await settle(tester);

    expect(pairs, 0);
    expect(
      find.text('Sign in to your Rhythm account to add a Monster strip.'),
      findsOneWidget,
    );
  });
}
