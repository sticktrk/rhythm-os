import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/aidot_button_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const qr = 'B:1CD6BD2273F9%G\$S:L10599FAR002073\$M:A001462';

  late CapturingAnalyticsBackend analytics;
  setUp(() async {
    analytics = CapturingAnalyticsBackend();
    await analytics.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: analytics,
    );
    await AnalyticsService().initialize();
  });
  tearDown(BackendProvider.resetForTesting);

  testWidgets('waits for confirmation and submits the exact QR privately',
      (tester) async {
    String? capturedHubType;
    Map<String, dynamic>? capturedParams;
    RhythmPairedDevice? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => AidotButtonAddScreen(
                    setupPayload: qr,
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
                      return {
                        'status': 'complete',
                        'device': {
                          'device_id': 'aidot-ble-1cd6bd2273f9',
                          'name': 'Orein/AiDot Button',
                          'device_type': 'button',
                          'manufacturer': 'Orein/AiDot',
                          'model': 'OC02001-CR-B',
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

    expect(find.text('Put the button in pairing mode'), findsOneWidget);
    expect(find.text('Find Button'), findsOneWidget);
    expect(find.text(qr), findsNothing);
    expect(capturedParams, isNull);

    await tester.tap(find.text('Find Button'));
    await tester.pumpAndSettle();

    expect(capturedHubType, 'aidot_ble');
    expect(capturedParams, {'setup_payload': qr});
    expect(result?.deviceId, 'aidot-ble-1cd6bd2273f9');
    final pairingEvents = analytics.events.where(
      (event) => event.name.startsWith('aidot_button_pairing_'),
    );
    expect(pairingEvents, hasLength(2));
    expect(
      pairingEvents.expand((event) => event.properties.values),
      isNot(contains(qr)),
    );
  });
}
