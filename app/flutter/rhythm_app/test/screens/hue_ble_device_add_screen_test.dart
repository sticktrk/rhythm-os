import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/hue_ble_device_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

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
    await AnalyticsService().initialize();
  });

  tearDown(BackendProvider.resetForTesting);

  testWidgets('starts a code-less nearby scan through generic pairing', (
    tester,
  ) async {
    String? thisHubType;
    Map<String, dynamic>? thisParams;
    Duration? timeout;
    String? thisSessionId;

    await tester.pumpWidget(
      MaterialApp(
        home: HueBleDeviceAddScreen(
          analyticsSource: 'test',
          journeyId: 'hue-ble-test',
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) async {
            thisHubType = hubType;
            thisParams = params;
            timeout = receiveTimeout;
            thisSessionId = sessionId;
            return {
              'status': 'failed',
              'error': 'No pairable Hue bulb found.',
            };
          },
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 20));

    expect(thisHubType, 'hue_ble');
    expect(thisParams, isEmpty);
    expect(timeout, const Duration(minutes: 8));
    expect(thisSessionId, 'hue-ble-test');
    expect(find.text('No pairable Hue bulb found.'), findsOneWidget);
    expect(find.byKey(const ValueKey('hue-ble-retry')), findsOneWidget);
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('hue-ble-retry')),
          )
          .onPressed,
      isNotNull,
    );
    expect(
      tester
          .widget<PopScope<Object?>>(
            find.byWidgetPredicate((widget) => widget is PopScope),
          )
          .canPop,
      isTrue,
    );

    final attempted = analyticsBackend.events.singleWhere(
      (event) => event.name == 'hue_ble_pairing_attempted',
    );
    expect(attempted.properties, {
      'journey_id': 'hue-ble-test',
      'source': 'test',
      'input_method': 'nearby_scan',
      'attempt_number': 1,
    });
    expect(attempted.properties.values, isNot(contains('27F706')));
  });

  testWidgets(
    'keeps normal retries safe and confirms stale Bluetooth bond replacement',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(390, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final pairingParams = <Map<String, dynamic>>[];
      final pairingSessionIds = <String>[];

      await tester.pumpWidget(
        MaterialApp(
          home: HueBleDeviceAddScreen(
            analyticsSource: 'test',
            journeyId: 'hue-stale-bond-test',
            pairingRequest: ({
              required hubType,
              required params,
              required receiveTimeout,
              required sessionId,
            }) async {
              pairingParams.add(Map<String, dynamic>.from(params));
              pairingSessionIds.add(sessionId);
              if (params['list_stale_bond_candidates'] == true) {
                return {
                  'status': 'complete',
                  'details': {
                    'recovery_candidates': [
                      {
                        'candidate_address': 'EB:01:B4:6B:01:34',
                        'device_id': 'hue-ble-old',
                        'name': 'Hue white lamp',
                        'model': 'LWA003',
                      },
                      {
                        'candidate_address': 'EA:84:C2:50:A8:65',
                        'device_id': 'hue-ble-new',
                        'name': 'Hue color lamp',
                        'model': 'LCA013',
                      },
                    ],
                  },
                };
              }
              return {
                'status': 'failed',
                'error': 'No pairable Hue bulb found.',
              };
            },
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));

      expect(pairingParams, [isEmpty]);
      expect(
        find.byKey(const ValueKey('hue-ble-stale-bond-recovery')),
        findsOneWidget,
      );
      expect(
        find.textContaining(
          'removes only the bulb you select',
          findRichText: true,
        ),
        findsOneWidget,
      );

      await tester.tap(find.byKey(const ValueKey('hue-ble-retry')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(pairingParams, [isEmpty, isEmpty]);

      await tester.tap(
        find.byKey(const ValueKey('hue-ble-stale-bond-recovery')),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));

      expect(find.text('Choose the reset Hue bulb'), findsOneWidget);
      expect(find.text('Hue white lamp (LWA003)'), findsOneWidget);
      expect(find.text('Hue color lamp (LCA013)'), findsOneWidget);
      expect(pairingParams, [
        isEmpty,
        isEmpty,
        {'list_stale_bond_candidates': true},
      ]);
      expect(
        pairingSessionIds.last,
        startsWith('hue-stale-bond-test-recovery-candidates-'),
      );

      await tester.tap(
        find.byKey(
          const ValueKey(
            'hue-ble-stale-candidate-EB:01:B4:6B:01:34',
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(find.text('Confirm this bulb was reset'), findsOneWidget);
      expect(find.textContaining('Bluetooth ID …6B0134'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(pairingParams, hasLength(3));

      await tester.tap(
        find.byKey(const ValueKey('hue-ble-stale-bond-recovery')),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      await tester.tap(
        find.byKey(
          const ValueKey(
            'hue-ble-stale-candidate-EA:84:C2:50:A8:65',
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      await tester.tap(
        find.byKey(
          const ValueKey('hue-ble-confirm-stale-bond-recovery'),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));

      expect(pairingParams, [
        isEmpty,
        isEmpty,
        {'list_stale_bond_candidates': true},
        {'list_stale_bond_candidates': true},
        {
          'replace_stale_bonds': true,
          'candidate_address': 'EA:84:C2:50:A8:65',
        },
      ]);
      expect(pairingSessionIds[2], isNot(pairingSessionIds.last));

      final attempted = analyticsBackend.events
          .where((event) => event.name == 'hue_ble_pairing_attempted')
          .toList(growable: false);
      expect(attempted, hasLength(3));
      expect(attempted.last.properties, {
        'journey_id': 'hue-stale-bond-test',
        'source': 'test',
        'input_method': 'stale_bond_recovery',
        'attempt_number': 3,
      });
    },
  );

  testWidgets(
    'ignores a delayed terminal event from the scan while listing stale bonds',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(390, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final progress = StreamController<RhythmPairingProgress>.broadcast();
      final candidateResponse = Completer<Map<String, dynamic>?>();
      var candidateRequests = 0;
      addTearDown(progress.close);

      await tester.pumpWidget(
        MaterialApp(
          home: HueBleDeviceAddScreen(
            journeyId: 'hue-stale-race-test',
            progressEvents: progress.stream,
            pairingRequest: ({
              required hubType,
              required params,
              required receiveTimeout,
              required sessionId,
            }) {
              if (params['list_stale_bond_candidates'] == true) {
                candidateRequests += 1;
                return candidateResponse.future;
              }
              return Future.value({
                'status': 'failed',
                'error': 'No pairable Hue bulb found.',
              });
            },
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));

      await tester.tap(
        find.byKey(const ValueKey('hue-ble-stale-bond-recovery')),
      );
      await tester.pump();
      await tester.pump();
      expect(candidateRequests, 1);

      progress.add(
        const RhythmPairingProgress(
          hubType: 'hue_ble',
          sessionId: 'hue-stale-race-test',
          status: RhythmPairingStatus.complete,
          stage: RhythmPairingStage.complete,
          message: 'Late completion from the old scan',
          device: RhythmPairedDevice(
            deviceId: 'hue-ble-late',
            name: 'Late Hue bulb',
            deviceType: 'light',
          ),
        ),
      );
      await tester.pump();

      expect(
        find.byKey(const ValueKey('hue-ble-pairing-screen')),
        findsOneWidget,
      );
      expect(find.text('Late completion from the old scan'), findsNothing);

      candidateResponse.complete({
        'status': 'complete',
        'details': {
          'recovery_candidates': [
            {
              'candidate_address': 'EA:84:C2:50:A8:65',
              'name': 'Hue color lamp',
              'model': 'LCA013',
            },
          ],
        },
      });
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));

      expect(candidateRequests, 1);
      expect(find.text('Confirm this bulb was reset'), findsOneWidget);
      expect(find.textContaining('Bluetooth ID …50A865'), findsOneWidget);
    },
  );

  testWidgets('renders matching SSE progress while the request is active', (
    tester,
  ) async {
    final response = Completer<Map<String, dynamic>?>();
    final progress = StreamController<RhythmPairingProgress>.broadcast();
    addTearDown(progress.close);

    await tester.pumpWidget(
      MaterialApp(
        home: HueBleDeviceAddScreen(
          journeyId: 'hue-progress-test',
          progressEvents: progress.stream,
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) =>
              response.future,
        ),
      ),
    );
    await tester.pump();

    progress.add(
      const RhythmPairingProgress(
        hubType: 'hue_ble',
        status: RhythmPairingStatus.commissioning,
        stage: RhythmPairingStage.connecting,
        message: 'Progress without a session must be ignored.',
      ),
    );
    progress.add(
      const RhythmPairingProgress(
        hubType: 'hue_ble',
        sessionId: 'another-hue-session',
        status: RhythmPairingStatus.commissioning,
        stage: RhythmPairingStage.connecting,
        message: 'Progress from another session must be ignored.',
      ),
    );
    await tester.pump();

    expect(
      find.text('Progress without a session must be ignored.'),
      findsNothing,
    );
    expect(
      find.text('Progress from another session must be ignored.'),
      findsNothing,
    );

    progress.add(
      const RhythmPairingProgress(
        hubType: 'hue_ble',
        sessionId: 'hue-progress-test',
        status: RhythmPairingStatus.commissioning,
        stage: RhythmPairingStage.connecting,
        message: 'Connecting securely to Hue white lamp…',
      ),
    );
    await tester.pump();

    expect(find.text('Connecting securely to Hue white lamp…'), findsOneWidget);

    response.complete({'status': 'failed', 'error': 'Stopped for test.'});
    await tester.pump();
  });

  testWidgets('prevents leaving while the pairing request is active', (
    tester,
  ) async {
    final response = Completer<Map<String, dynamic>?>();

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () {
                Navigator.of(context).push<void>(
                  MaterialPageRoute(
                    builder: (_) => HueBleDeviceAddScreen(
                      journeyId: 'hue-pop-test',
                      pairingRequest: ({
                        required hubType,
                        required params,
                        required receiveTimeout,
                        required sessionId,
                      }) =>
                          response.future,
                    ),
                  ),
                );
              },
              child: const Text('Pair Hue'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Pair Hue'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(
      find.byKey(const ValueKey('hue-ble-pairing-screen')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<PopScope<Object?>>(
            find.byWidgetPredicate((widget) => widget is PopScope),
          )
          .canPop,
      isFalse,
    );

    await tester.binding.handlePopRoute();
    await tester.pump();

    expect(
      find.byKey(const ValueKey('hue-ble-pairing-screen')),
      findsOneWidget,
    );
    expect(
      find.textContaining('Pairing is still in progress'),
      findsWidgets,
    );

    response.complete({'status': 'failed', 'error': 'Stopped for test.'});
    await tester.pump();
    expect(find.text('Stopped for test.'), findsOneWidget);
    expect(
      tester
          .widget<PopScope<Object?>>(
            find.byWidgetPredicate((widget) => widget is PopScope),
          )
          .canPop,
      isTrue,
    );
  });

  testWidgets('returns the paired Hue light to its caller', (tester) async {
    HueBleDevicePairingResult? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async {
                result =
                    await Navigator.of(context).push<HueBleDevicePairingResult>(
                  MaterialPageRoute(
                    builder: (_) => HueBleDeviceAddScreen(
                      pairingRequest: ({
                        required hubType,
                        required params,
                        required receiveTimeout,
                        required sessionId,
                      }) async =>
                          {
                        'status': 'complete',
                        'devices': [
                          {
                            'device_id': '001788010c765ba7',
                            'name': 'Hue white lamp',
                            'device_type': 'light',
                            'manufacturer': 'Signify Netherlands B.V.',
                            'model': 'LWA003',
                          },
                          {
                            'device_id': '001788010fffffff',
                            'name': 'Hue color lamp',
                            'device_type': 'light',
                            'manufacturer': 'Signify Netherlands B.V.',
                            'model': 'LCA013',
                          },
                        ],
                        'warnings': ['One nearby bulb was out of range.'],
                      },
                    ),
                  ),
                );
              },
              child: const Text('Pair Hue'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Pair Hue'));
    await tester.pumpAndSettle();

    expect(
      result?.devices.map((device) => device.nativeDeviceId),
      ['001788010c765ba7', '001788010fffffff'],
    );
    expect(result?.devices.first.name, 'Hue white lamp');
    expect(result?.devices.last.model, 'LCA013');
    expect(result?.warnings, ['One nearby bulb was out of range.']);
  });

  testWidgets('accepts the legacy singular device response', (tester) async {
    HueBleDevicePairingResult? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result =
                  await Navigator.of(context).push<HueBleDevicePairingResult>(
                MaterialPageRoute(
                  builder: (_) => HueBleDeviceAddScreen(
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        {
                      'status': 'complete',
                      'device': {
                        'device_id': '001788010c765ba7',
                        'name': 'Legacy Hue response',
                        'device_type': 'light',
                      },
                    },
                  ),
                ),
              );
            },
            child: const Text('Pair Hue'),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Pair Hue'));
    await tester.pumpAndSettle();

    expect(result?.devices, hasLength(1));
    expect(result?.devices.single.nativeDeviceId, '001788010c765ba7');
    expect(result?.devices.single.name, 'Legacy Hue response');
  });

  testWidgets(
    'recovers a complete batch from terminal SSE without double popping',
    (tester) async {
      final response = Completer<Map<String, dynamic>?>();
      final progress = StreamController<RhythmPairingProgress>.broadcast();
      addTearDown(progress.close);
      HueBleDevicePairingResult? result;

      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () async {
                  result = await Navigator.of(context)
                      .push<HueBleDevicePairingResult>(
                    MaterialPageRoute(
                      builder: (_) => HueBleDeviceAddScreen(
                        journeyId: 'hue-terminal-batch',
                        progressEvents: progress.stream,
                        pairingRequest: ({
                          required hubType,
                          required params,
                          required receiveTimeout,
                          required sessionId,
                        }) =>
                            response.future,
                      ),
                    ),
                  );
                },
                child: const Text('Pair Hue'),
              ),
            ),
          ),
        ),
      );

      await tester.tap(find.text('Pair Hue'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      progress.add(
        const RhythmPairingProgress(
          hubType: 'hue_ble',
          sessionId: 'hue-terminal-batch',
          status: RhythmPairingStatus.complete,
          stage: RhythmPairingStage.complete,
          message: 'Added two Hue bulbs',
          devices: [
            RhythmPairedDevice(
              deviceId: '001788010c765ba7',
              name: 'Hue white lamp',
              deviceType: 'light',
            ),
            RhythmPairedDevice(
              deviceId: '001788010fffffff',
              name: 'Hue color lamp',
              deviceType: 'light',
            ),
          ],
          warnings: ['A third bulb could not be added.'],
        ),
      );
      await tester.pumpAndSettle();

      expect(result?.devices, hasLength(2));
      expect(result?.warnings, ['A third bulb could not be added.']);
      expect(find.text('Pair Hue'), findsOneWidget);

      response.complete({
        'status': 'complete',
        'device': {
          'device_id': 'late-http-device',
          'name': 'Late response',
          'device_type': 'light',
        },
      });
      await tester.pumpAndSettle();

      expect(result?.devices, hasLength(2));
      expect(find.text('Pair Hue'), findsOneWidget);
    },
  );
}
