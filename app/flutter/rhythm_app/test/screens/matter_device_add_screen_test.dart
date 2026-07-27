import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/matter_add_method.dart';
import 'package:rhythm_app/screens/hubs/matter_device_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

void main() {
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

  testWidgets('shows QR scanner button on Android', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;

    try {
      await tester.pumpWidget(
        const MaterialApp(
          home: MatterDeviceAddScreen(
            endpoint: HubEndpoint(host: '127.0.0.1', port: 54448),
            addMethod: MatterAddMethod.automatic,
          ),
        ),
      );

      expect(find.text('Scan QR Code'), findsOneWidget);
      expect(find.byIcon(Icons.qr_code_scanner_rounded), findsOneWidget);
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets('ignores duplicate transmit taps while request is in flight',
      (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.linux;
    final releaseResponse = Completer<void>();
    try {
      var pairRequestCount = 0;
      final receiveTimeouts = <Duration>[];
      final api = _FakeRhythmMatterApi(
        onPair: (receiveTimeout) async {
          pairRequestCount += 1;
          receiveTimeouts.add(receiveTimeout);
          await releaseResponse.future;
          return const RhythmMatterPairingResponse(
            httpStatus: 200,
            status: 'failed',
            error: 'test failure',
          );
        },
      );

      await tester.pumpWidget(
        MaterialApp(
          home: MatterDeviceAddScreen(
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
            addMethod: MatterAddMethod.onNetworkSetupCode,
            pairingApi: api,
          ),
        ),
      );

      await tester.enterText(find.byType(TextField), '34970112332');
      await tester.pump();
      final transmitButton = find.text('Add Device');
      await tester.ensureVisible(transmitButton);

      await tester.tap(transmitButton);
      await tester.tap(transmitButton);

      expect(pairRequestCount, 1);
      expect(receiveTimeouts.single, const Duration(minutes: 4));

      releaseResponse.complete();
      await tester.pump(const Duration(milliseconds: 100));
    } finally {
      if (!releaseResponse.isCompleted) {
        releaseResponse.complete();
      }
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets('starts pairing immediately for a scanned Matter payload',
      (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    var pairRequestCount = 0;
    try {
      final api = _FakeRhythmMatterApi(
        onPair: (receiveTimeout) async {
          pairRequestCount += 1;
          return const RhythmMatterPairingResponse(
            httpStatus: 200,
            status: 'failed',
            error: 'test failure',
          );
        },
      );

      await tester.pumpWidget(
        MaterialApp(
          home: MatterDeviceAddScreen(
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
            addMethod: MatterAddMethod.automatic,
            initialSetupPayload: 'MT:Y.K908OC16750648G00',
            analyticsSource: 'test_scanner',
            journeyId: 'matter-pair-test',
            pairingApi: api,
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 10));

      expect(pairRequestCount, 1);
      expect(find.textContaining('Pairing failed.'), findsOneWidget);
      expect(
        analyticsBackend.events.map((event) => event.name),
        containsAllInOrder([
          'matter_pairing_attempted',
          'matter_pairing_completed',
        ]),
      );
      final completed = analyticsBackend.events.singleWhere(
        (event) => event.name == 'matter_pairing_completed',
      );
      expect(completed.properties, {
        'journey_id': 'matter-pair-test',
        'source': 'test_scanner',
        'input_method': 'camera',
        'add_method': 'automatic',
        'attempt_number': 1,
        'outcome': 'failed',
        'failure_stage': 'commissioning',
      });
      expect(
        completed.properties.values,
        isNot(contains('MT:Y.K908OC16750648G00')),
      );
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets(
      'manual codes enter the same pairing path with manual attribution',
      (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    var pairRequestCount = 0;
    try {
      final api = _FakeRhythmMatterApi(
        onPair: (receiveTimeout) async {
          pairRequestCount += 1;
          return const RhythmMatterPairingResponse(
            httpStatus: 200,
            status: 'failed',
            error: 'test failure',
          );
        },
      );

      await tester.pumpWidget(
        MaterialApp(
          home: MatterDeviceAddScreen(
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
            addMethod: MatterAddMethod.automatic,
            initialSetupPayload: '3497-011-2332',
            initialInputMethod: 'manual_code',
            analyticsSource: 'test_manual',
            journeyId: 'matter-manual-test',
            pairingApi: api,
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 10));

      expect(pairRequestCount, 1);
      final attempted = analyticsBackend.events.singleWhere(
        (event) => event.name == 'matter_pairing_attempted',
      );
      expect(attempted.properties['input_method'], 'manual_code');
      expect(
        attempted.properties.values,
        isNot(contains('3497-011-2332')),
      );
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });
}

class _FakeRhythmMatterApi extends RhythmMatterApi {
  _FakeRhythmMatterApi({required this.onPair})
      : super(baseUrl: 'http://127.0.0.1');

  final Future<RhythmMatterPairingResponse> Function(Duration receiveTimeout)
      onPair;

  @override
  Future<RhythmMatterPairingResponse> pairDevice({
    required String setupPayload,
    String network = 'wifi',
    String rendezvous = 'auto',
    Duration receiveTimeout = const Duration(seconds: 45),
    String? sessionId,
  }) {
    return onPair(receiveTimeout);
  }
}
