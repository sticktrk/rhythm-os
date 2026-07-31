import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/hue_bridge_light_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() async {
    final analyticsBackend = CapturingAnalyticsBackend();
    await analyticsBackend.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: analyticsBackend,
    );
    await AnalyticsService().initialize();
  });

  tearDown(BackendProvider.resetForTesting);

  testWidgets('searches a connected Hue Bridge by normalized serial', (
    tester,
  ) async {
    String? capturedHubType;
    Map<String, dynamic>? capturedParams;
    Duration? capturedTimeout;
    String? capturedSessionId;
    HueBridgeLightAddResult? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result =
                  await Navigator.of(context).push<HueBridgeLightAddResult>(
                MaterialPageRoute(
                  builder: (_) => HueBridgeLightAddScreen(
                    serial: 'e2-77-da',
                    journeyId: 'hue-bridge-test',
                    hubAddress: ' 192.0.2.10 ',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async {
                      capturedHubType = hubType;
                      capturedParams = params;
                      capturedTimeout = receiveTimeout;
                      capturedSessionId = sessionId;
                      return {
                        'status': 'complete',
                        'devices': [
                          {
                            'device_id': 'hue-light-1',
                            'name': 'Hue color lamp',
                            'device_type': 'light',
                          },
                        ],
                        'warnings': ['One Bridge candidate was skipped.'],
                      };
                    },
                  ),
                ),
              );
            },
            child: const Text('Search'),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Search'));
    await tester.pumpAndSettle();

    expect(capturedHubType, 'hue');
    expect(capturedParams, {
      'serial': 'E277DA',
      'hub_address': '192.0.2.10',
    });
    expect(capturedTimeout, const Duration(minutes: 2));
    expect(capturedSessionId, 'hue-bridge-test');
    expect(result?.addedCount, 1);
    expect(result?.devices.single.deviceId, 'hue-light-1');
    expect(result?.warnings, ['One Bridge candidate was skipped.']);
  });

  testWidgets('treats an empty device projection as unknown added count', (
    tester,
  ) async {
    HueBridgeLightAddResult? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result =
                  await Navigator.of(context).push<HueBridgeLightAddResult>(
                MaterialPageRoute(
                  builder: (_) => HueBridgeLightAddScreen(
                    serial: '27F706',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        {'status': 'complete', 'devices': <dynamic>[]},
                  ),
                ),
              );
            },
            child: const Text('Search'),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Search'));
    await tester.pumpAndSettle();

    expect(result?.addedCount, isNull);
  });

  testWidgets('accepts progress only from its exact Hue session', (
    tester,
  ) async {
    final response = Completer<Map<String, dynamic>?>();
    final progress = StreamController<RhythmPairingProgress>.broadcast();
    addTearDown(progress.close);

    await tester.pumpWidget(
      MaterialApp(
        home: HueBridgeLightAddScreen(
          serial: '27F706',
          journeyId: 'bridge-progress-test',
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
        hubType: 'hue',
        status: RhythmPairingStatus.searching,
        stage: RhythmPairingStage.searching,
        message: 'Missing session',
      ),
    );
    progress.add(
      const RhythmPairingProgress(
        hubType: 'hue',
        sessionId: 'another-session',
        status: RhythmPairingStatus.searching,
        stage: RhythmPairingStage.searching,
        message: 'Foreign session',
      ),
    );
    await tester.pump();
    expect(find.text('Missing session'), findsNothing);
    expect(find.text('Foreign session'), findsNothing);

    progress.add(
      const RhythmPairingProgress(
        hubType: 'hue',
        sessionId: 'bridge-progress-test',
        status: RhythmPairingStatus.searching,
        stage: RhythmPairingStage.searching,
        message: 'Hue Bridge is searching…',
      ),
    );
    await tester.pump();
    expect(find.text('Hue Bridge is searching…'), findsOneWidget);

    response.complete({'status': 'failed', 'error': 'Stopped for test.'});
    await tester.pump();
  });

  testWidgets(
    'recovers terminal SSE devices when the HTTP result arrives late',
    (tester) async {
      final response = Completer<Map<String, dynamic>?>();
      final progress = StreamController<RhythmPairingProgress>.broadcast();
      addTearDown(progress.close);
      HueBridgeLightAddResult? result;

      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => TextButton(
              onPressed: () async {
                result =
                    await Navigator.of(context).push<HueBridgeLightAddResult>(
                  MaterialPageRoute(
                    builder: (_) => HueBridgeLightAddScreen(
                      serial: 'E277DA',
                      journeyId: 'bridge-terminal-batch',
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
              child: const Text('Search'),
            ),
          ),
        ),
      );

      await tester.tap(find.text('Search'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      progress.add(
        const RhythmPairingProgress(
          hubType: 'hue',
          sessionId: 'bridge-terminal-batch',
          status: RhythmPairingStatus.complete,
          stage: RhythmPairingStage.complete,
          message: 'Hue Bridge added two lights',
          devices: [
            RhythmPairedDevice(
              deviceId: 'hue-v2-device-1',
              name: 'Hue bulb one',
              deviceType: 'light',
            ),
            RhythmPairedDevice(
              deviceId: 'hue-v2-device-2',
              name: 'Hue bulb two',
              deviceType: 'light',
            ),
          ],
        ),
      );
      await tester.pumpAndSettle();

      expect(result?.addedCount, 2);
      expect(
        result?.devices.map((device) => device.deviceId),
        ['hue-v2-device-1', 'hue-v2-device-2'],
      );
      expect(find.text('Search'), findsOneWidget);

      response.complete({
        'status': 'complete',
        'device': {
          'device_id': 'late-http-device',
          'name': 'Late result',
          'device_type': 'light',
        },
      });
      await tester.pumpAndSettle();
      expect(result?.addedCount, 2);
      expect(find.text('Search'), findsOneWidget);
    },
  );
}
