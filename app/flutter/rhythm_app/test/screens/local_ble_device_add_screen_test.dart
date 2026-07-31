import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/local_ble_device_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/device_pairing_code.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const qr = 'B:0A0B0C0D0E0F%G\$S:SYNTHETIC000001\$M:TESTMODEL001';
  late LocalBleSetup setup;
  late CapturingAnalyticsBackend analytics;

  setUp(() async {
    setup = parseLocalBleSetupCode(qr)!;
    analytics = CapturingAnalyticsBackend();
    await analytics.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: analytics,
    );
    await AnalyticsService().initialize();
  });
  tearDown(BackendProvider.resetForTesting);

  testWidgets('submits the canonical profile while retaining parsed fields',
      (tester) async {
    const canonicalProfileId = 'orein.oc02001.button.v2';
    String? capturedHubType;
    Map<String, dynamic>? capturedParams;
    Duration? capturedTimeout;
    RhythmPairedDevice? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    pairingProfileId: canonicalProfileId,
                    inputMethod: 'manual_code',
                    analyticsSource: 'test',
                    journeyId: 'journey-1',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async {
                      capturedHubType = hubType;
                      capturedParams = params;
                      capturedTimeout = receiveTimeout;
                      return {
                        'status': 'complete',
                        'device': {
                          'device_id': 'local-ble-button-1',
                          'name': 'Button',
                          'device_type': 'button',
                        },
                      };
                    },
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
    await tester.pumpAndSettle();

    expect(find.text('Put the device in pairing mode'), findsOneWidget);
    expect(find.text('Find Device'), findsOneWidget);
    expect(find.text(qr), findsNothing);
    expect(capturedParams, isNull);

    await tester.tap(find.text('Find Device'));
    await tester.pumpAndSettle();

    expect(capturedHubType, 'local_ble');
    expect(setup.profileId, RhythmDeviceProfileId.oreinOc02001Button);
    expect(capturedParams, {
      ...setup.pairingParams,
      'profile_id': canonicalProfileId,
    });
    expect(capturedParams.toString(), isNot(contains(qr)));
    expect(capturedTimeout, const Duration(seconds: 135));
    expect(result?.deviceId, 'local-ble-button-1');
    final pairingEvents = analytics.events.where(
      (event) => event.name.startsWith('local_ble_pairing_'),
    );
    expect(pairingEvents, hasLength(2));
    for (final event in pairingEvents) {
      expect(event.properties['journey_id'], 'journey-1');
      expect(event.properties['input_method'], 'manual_code');
      expect(
        event.properties['profile_id'],
        canonicalProfileId,
      );
      expect(event.properties.values, isNot(contains(qr)));
      expect(
        event.properties.values,
        isNot(contains(setup.setupFields['ble_identity'])),
      );
    }
  });

  testWidgets('blocks dismissal and reconciles a correlated terminal event',
      (tester) async {
    final request = Completer<Map<String, dynamic>?>();
    final progress = StreamController<RhythmPairingProgress>();
    addTearDown(progress.close);
    RhythmPairedDevice? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'camera',
                    journeyId: 'journey-2',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) =>
                        request.future,
                    progressEvents: progress.stream,
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
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    await tester.binding.handlePopRoute();
    await tester.pump();
    expect(
        find.byKey(const ValueKey('local-ble-pairing-screen')), findsOneWidget);
    expect(find.textContaining('Pairing is still in progress'), findsOneWidget);

    progress.add(
      const RhythmPairingProgress(
        hubType: 'local_ble',
        sessionId: 'journey-2',
        status: RhythmPairingStatus.complete,
        stage: RhythmPairingStage.complete,
        message: 'Complete',
        device: RhythmPairedDevice(
          deviceId: 'local-ble-button-2',
          name: 'Button',
          deviceType: 'button',
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(result?.deviceId, 'local-ble-button-2');
    request.complete(null);
    await tester.pump();
  });

  testWidgets('late SSE success wins after an early request exception',
      (tester) async {
    final progress = StreamController<RhythmPairingProgress>();
    addTearDown(progress.close);
    RhythmPairedDevice? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'camera',
                    journeyId: 'journey-3',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async {
                      throw StateError('sensitive transport detail');
                    },
                    progressEvents: progress.stream,
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
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    expect(
      find.textContaining('Waiting for final confirmation'),
      findsOneWidget,
    );
    expect(find.textContaining('sensitive transport detail'), findsNothing);
    final pairingButton = tester.widget<FilledButton>(
      find.byKey(const ValueKey('find-local-ble-device')),
    );
    expect(pairingButton.onPressed, isNull);

    await tester.binding.handlePopRoute();
    await tester.pump();
    expect(
      find.byKey(const ValueKey('local-ble-pairing-screen')),
      findsOneWidget,
    );

    progress.add(
      const RhythmPairingProgress(
        hubType: 'local_ble',
        sessionId: 'journey-3',
        status: RhythmPairingStatus.complete,
        stage: RhythmPairingStage.complete,
        message: 'Complete',
        device: RhythmPairedDevice(
          deviceId: 'local-ble-button-3',
          name: 'Button',
          deviceType: 'button',
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(result?.deviceId, 'local-ble-button-3');
    final pairingEvents = analytics.events
        .where((event) => event.name.startsWith('local_ble_pairing_'))
        .toList(growable: false);
    expect(pairingEvents, hasLength(2));
    expect(pairingEvents.last.properties['outcome'], 'succeeded');
    for (final event in pairingEvents) {
      expect(
        event.properties.values,
        isNot(contains('sensitive transport detail')),
      );
    }
  });

  testWidgets('late SSE success wins after an early gateway timeout',
      (tester) async {
    final progress = StreamController<RhythmPairingProgress>();
    addTearDown(progress.close);
    RhythmPairedDevice? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'camera',
                    journeyId: 'journey-504',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        {
                      'http_status': 504,
                      'error': 'sensitive gateway detail',
                    },
                    progressEvents: progress.stream,
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
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    expect(
      find.textContaining('Waiting for final confirmation'),
      findsOneWidget,
    );
    expect(find.textContaining('sensitive gateway detail'), findsNothing);
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('find-local-ble-device')),
          )
          .onPressed,
      isNull,
    );

    progress.add(
      const RhythmPairingProgress(
        hubType: 'local_ble',
        sessionId: 'journey-504',
        status: RhythmPairingStatus.complete,
        stage: RhythmPairingStage.complete,
        message: 'Complete',
        device: RhythmPairedDevice(
          deviceId: 'local-ble-button-504',
          name: 'Button',
          deviceType: 'button',
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(result?.deviceId, 'local-ble-button-504');
    final pairingEvents = analytics.events
        .where((event) => event.name.startsWith('local_ble_pairing_'))
        .toList(growable: false);
    expect(pairingEvents, hasLength(2));
    expect(pairingEvents.last.properties['outcome'], 'succeeded');
  });

  testWidgets('definitive HTTP conflict unlocks immediately', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () {
              Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'manual_code',
                    journeyId: 'journey-409',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        {
                      'http_status': 409,
                      'error': 'conflict detail',
                    },
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
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    expect(
      find.byKey(const ValueKey('local-ble-pairing-error')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('find-local-ble-device')),
          )
          .onPressed,
      isNotNull,
    );
    final completed = analytics.events.singleWhere(
      (event) => event.name == 'local_ble_pairing_completed',
    );
    expect(completed.properties['outcome'], 'failed');
    expect(completed.properties['failure_stage'], 'server_rejected');

    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
  });

  testWidgets('ambiguous response fails only at reconciliation deadline',
      (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () {
              Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'manual_code',
                    journeyId: 'journey-4',
                    pairingDeadline: const Duration(seconds: 1),
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        null,
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
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    await tester.pump(const Duration(milliseconds: 999));
    expect(
      find.byKey(const ValueKey('local-ble-pairing-error')),
      findsNothing,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('find-local-ble-device')),
          )
          .onPressed,
      isNull,
    );

    await tester.pump(const Duration(milliseconds: 1));
    expect(
      find.byKey(const ValueKey('local-ble-pairing-error')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('find-local-ble-device')),
          )
          .onPressed,
      isNotNull,
    );
    final completed = analytics.events.singleWhere(
      (event) => event.name == 'local_ble_pairing_completed',
    );
    expect(completed.properties['outcome'], 'failed');
    expect(
      completed.properties['failure_stage'],
      'reconciliation_deadline',
    );

    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
  });
}
