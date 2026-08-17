import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/hue_bridge_button_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';

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

  testWidgets('invokes native Bridge button search and returns only buttons', (
    tester,
  ) async {
    String? capturedHubType;
    Map<String, dynamic>? capturedParams;
    Duration? capturedTimeout;
    String? capturedSessionId;
    HueBridgeButtonAddResult? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result =
                  await Navigator.of(context).push<HueBridgeButtonAddResult>(
                MaterialPageRoute(
                  builder: (_) => HueBridgeButtonAddScreen(
                    hubAddress: ' 192.0.2.25 ',
                    journeyId: 'hue-button-test',
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
                            'name': 'Hue lamp',
                            'device_type': 'light',
                          },
                          {
                            'device_id': 'hue-button-1',
                            'name': 'Hue dimmer switch',
                            'device_type': 'button',
                          },
                        ],
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
    expect(find.text('Start Bridge Search'), findsOneWidget);
    expect(capturedParams, isNull, reason: 'search must require user intent');

    await tester.tap(find.byKey(const ValueKey('hue-bridge-button-search')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(capturedHubType, 'hue');
    expect(capturedParams, {
      'device_kind': 'button',
      'correlation_id': 'hue-button-test',
      'hub_address': '192.0.2.25',
    });
    expect(capturedTimeout, const Duration(minutes: 2));
    expect(capturedSessionId, 'hue-button-test-attempt-1');
    expect(result?.devices.single.deviceId, 'hue-button-1');
    expect(result?.devices.single.deviceType, 'button');
    expect(
      analyticsBackend.events.map((event) => event.name),
      containsAllInOrder([
        'hue_bridge_button_pairing_attempted',
        'hue_bridge_button_pairing_completed',
      ]),
    );
  });

  testWidgets('rejects a light-only result and retries with one journey id', (
    tester,
  ) async {
    final requests = <Map<String, dynamic>>[];
    var attempt = 0;

    await tester.pumpWidget(
      MaterialApp(
        home: HueBridgeButtonAddScreen(
          hubAddress: '192.0.2.25',
          journeyId: 'hue-button-retry',
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) async {
            attempt += 1;
            requests.add({
              'params': params,
              'session_id': sessionId,
            });
            if (attempt == 1) {
              return {
                'status': 'complete',
                'device': {
                  'device_id': 'hue-light-1',
                  'name': 'Hue lamp',
                  'device_type': 'light',
                },
              };
            }
            return {
              'status': 'failed',
              'error': 'No accessory found.',
            };
          },
        ),
      ),
    );

    await tester.tap(find.byKey(const ValueKey('hue-bridge-button-search')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('Try Again'), findsOneWidget);
    expect(
      find.textContaining('no button or switch was confirmed'),
      findsWidgets,
    );

    await tester.tap(find.byKey(const ValueKey('hue-bridge-button-search')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('No accessory found.'), findsWidgets);
    expect(requests, hasLength(2));
    expect(requests[0]['session_id'], 'hue-button-retry-attempt-1');
    expect(requests[1]['session_id'], 'hue-button-retry-attempt-2');
    expect(
      (requests[0]['params'] as Map)['correlation_id'],
      (requests[1]['params'] as Map)['correlation_id'],
    );
  });
}
