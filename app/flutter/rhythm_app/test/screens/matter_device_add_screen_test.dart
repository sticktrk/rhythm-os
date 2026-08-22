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
            recoveryAction: 'existing_node_recommission_failed',
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
        'recovery_action': 'existing_node_recommission_failed',
      });
      expect(
        completed.properties.values,
        isNot(contains('MT:Y.K908OC16750648G00')),
      );
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets('returns non-fatal recovery warnings after successful pairing',
      (tester) async {
    MatterDevicePairingResult? pairingResult;
    final api = _FakeRhythmMatterApi(
      onPair: (_) async => const RhythmMatterPairingResponse(
        httpStatus: 200,
        status: 'complete',
        device: {
          'device_id': 'matter-42',
          'name': 'Desk bulb',
          'device_type': 'light',
        },
        warnings: [
          'The light was paired, but its Matter setup code was not saved.',
        ],
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              pairingResult = await Navigator.of(context).push(
                PageRouteBuilder<MatterDevicePairingResult>(
                  transitionDuration: Duration.zero,
                  reverseTransitionDuration: Duration.zero,
                  pageBuilder: (_, __, ___) => MatterDeviceAddScreen(
                    endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
                    addMethod: MatterAddMethod.automatic,
                    initialSetupPayload: 'MT:Y.K908OC16750648G00',
                    pairingApi: api,
                  ),
                ),
              );
            },
            child: const Text('PAIR'),
          ),
        ),
      ),
    );

    await tester.tap(find.text('PAIR'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(pairingResult, isNotNull);
    expect(pairingResult!.nativeDeviceId, 'matter-42');
    expect(pairingResult!.warnings, [
      'The light was paired, but its Matter setup code was not saved.',
    ]);
  });

  testWidgets('hides technical appliance details after Matter pairing fails',
      (tester) async {
    final api = _FakeRhythmMatterApi(
      onPair: (_) async => const RhythmMatterPairingResponse(
        httpStatus: 200,
        status: 'failed',
        error: 'BlueZ/CHIP lost the BLE connection: '
            'src/platform/Linux/bluez/BluezConnection.cpp:109: '
            'CHIP Error 0x0000040F',
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: MatterDeviceAddScreen(
          endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
          addMethod: MatterAddMethod.automatic,
          initialSetupPayload: '3497-011-2332',
          pairingApi: api,
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(
      find.textContaining(
          'The device stopped responding before setup finished'),
      findsOneWidget,
    );
    expect(find.textContaining('BlueZ'), findsNothing);
    expect(find.textContaining('CHIP Error'), findsNothing);
    expect(find.textContaining('.cpp'), findsNothing);
  });

  testWidgets('explains on-network transport failures without BLE advice',
      (tester) async {
    final api = _FakeRhythmMatterApi(
      onPair: (_) async => const RhythmMatterPairingResponse(
        httpStatus: 200,
        status: 'failed',
        error: 'CHIP sidecar closed the socket without a response '
            '(chipd status: running)',
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: MatterDeviceAddScreen(
          endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
          addMethod: MatterAddMethod.onNetworkSetupCode,
          initialSetupPayload: '3497-011-2332',
          pairingApi: api,
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.textContaining('local Matter network'), findsOneWidget);
    expect(find.textContaining('Thread border router'), findsOneWidget);
    expect(find.textContaining('keep it near'), findsNothing);
    expect(find.textContaining('CHIP sidecar'), findsNothing);
    expect(find.textContaining('chipd'), findsNothing);
  });

  testWidgets('retry starts a new durable Matter pairing session',
      (tester) async {
    var pairRequestCount = 0;
    final api = _FakeRhythmMatterApi(
      onPair: (_) async {
        pairRequestCount += 1;
        return const RhythmMatterPairingResponse(
          httpStatus: 200,
          status: 'failed',
          error: 'The device stopped responding.',
        );
      },
    );

    await tester.pumpWidget(
      MaterialApp(
        home: MatterDeviceAddScreen(
          endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
          addMethod: MatterAddMethod.onNetworkSetupCode,
          initialSetupPayload: '3497-011-2332',
          pairingApi: api,
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(pairRequestCount, 1);
    await tester.tap(find.text('Try Again'));
    await tester.pump();
    await tester.ensureVisible(find.text('Add Device'));
    await tester.tap(find.text('Add Device'));
    await tester.pump(const Duration(milliseconds: 10));

    expect(pairRequestCount, 2);
    expect(api.sessionIds, hasLength(2));
    expect(api.sessionIds.first, isNot(api.sessionIds.last));
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
      expect(api.lastRendezvous, 'auto');
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

  testWidgets(
      'existing Matter devices use network discovery and bounded analytics',
      (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    try {
      final api = _FakeRhythmMatterApi(
        onPair: (_) async => const RhythmMatterPairingResponse(
          httpStatus: 200,
          status: 'failed',
          error: 'network discovery timed out',
        ),
      );

      await tester.pumpWidget(
        MaterialApp(
          home: MatterDeviceAddScreen(
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
            addMethod: MatterAddMethod.onNetworkSetupCode,
            analyticsSource: 'test_existing_matter',
            journeyId: 'matter-existing-test',
            pairingApi: api,
          ),
        ),
      );

      expect(find.text('Open Matter pairing mode'), findsOneWidget);
      expect(find.textContaining('Turn On Pairing Mode'), findsOneWidget);
      expect(find.textContaining('same home network'), findsOneWidget);

      await tester.enterText(find.byType(TextField), '3497-011-2332');
      tester.testTextInput.hide();
      await tester.ensureVisible(find.text('Add Device'));
      await tester.pump();
      await tester.tap(find.text('Add Device'));
      await tester.pump(const Duration(milliseconds: 10));

      expect(api.lastRendezvous, 'on_network');
      final attempted = analyticsBackend.events.singleWhere(
        (event) => event.name == 'matter_pairing_attempted',
      );
      expect(attempted.properties, {
        'journey_id': 'matter-existing-test',
        'source': 'test_existing_matter',
        'input_method': 'manual_code',
        'add_method': 'on_network_setup_code',
        'attempt_number': 1,
      });
      expect(attempted.properties.values, isNot(contains('3497-011-2332')));
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
  String? lastRendezvous;
  final sessionIds = <String?>[];

  @override
  Future<RhythmMatterPairingResponse> pairDevice({
    required String setupPayload,
    String network = 'wifi',
    String rendezvous = 'auto',
    Duration receiveTimeout = const Duration(seconds: 45),
    String? sessionId,
  }) {
    lastRendezvous = rendezvous;
    sessionIds.add(sessionId);
    return onPair(receiveTimeout);
  }
}
