import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/matter_add_method.dart';
import 'package:rhythm_app/screens/hubs/matter_device_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/phone_matter_commissioner.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';
import '../helpers/ui_evidence_fonts.dart';

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

  testWidgets('a Box failure offers the phone only when the handoff is supported',
      (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    try {
      for (final available in [false, true]) {
        analyticsBackend.events.clear();
        final phone = _FakePhoneMatterCommissioner(
          onCommission: () async => const RhythmMatterPairingResponse(
              httpStatus: 200, status: 'failed', error: 'phone failed'),
        );
        final api = _FakeRhythmMatterApi(
          onPair: (_) async => const RhythmMatterPairingResponse(
              httpStatus: 200, status: 'failed', error: 'Box BLE timed out'),
        );
        await tester.pumpWidget(
          MaterialApp(
            home: MatterDeviceAddScreen(
              // A fresh State per iteration; same-position widgets reuse it.
              key: ValueKey('box-first-$available'),
              endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
              addMethod: MatterAddMethod.automatic,
              phoneCommissioningAvailable: available,
              initialSetupPayload: 'MT:Y.K908OC16750648G00',
              journeyId: 'matter-box-first-test',
              pairingApi: api,
              phoneCommissioner: phone,
            ),
          ),
        );
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 10));

        // The Box is always the first attempt, regardless of phone support.
        expect(api.sessionIds, hasLength(1));
        expect(api.lastRendezvous, 'auto');
        expect(phone.sessionIds, isEmpty);
        expect(api.resultQueries, isEmpty);
        expect(find.text('Try from Rhythm Box'), findsNothing);
        expect(find.text('Try from phone'),
            available ? findsOneWidget : findsNothing);
        if (!available) continue;

        await tester.ensureVisible(find.text('Try from phone'));
        await tester.tap(find.text('Try from phone'));
        await tester.pump(const Duration(milliseconds: 10));

        expect(phone.sessionIds, hasLength(1));
        expect(api.sessionIds, hasLength(1));
        expect(phone.sessionIds.single, isNot(api.sessionIds.single));
        expect(phone.setupPayloads.single, 'MT:Y.K908OC16750648G00');
        // A phone attempt can switch back to the Box, never the reverse twice.
        expect(find.text('Try from Rhythm Box'), findsOneWidget);
        expect(find.text('Try from phone'), findsNothing);
        final attempts = analyticsBackend.events
            .where((event) => event.name == 'matter_pairing_attempted')
            .map((event) => event.properties['add_method'])
            .toList();
        expect(attempts, ['automatic', 'phone_commissioning']);
      }
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets('phone failure offers an explicit server fallback',
      (tester) async {
    final screenshotPath =
        Platform.environment['RHYTHM_PHONE_MATTER_SCREENSHOT'];
    if (screenshotPath != null) {
      await tester.runAsync(loadUiEvidenceFonts);
      await tester.binding.setSurfaceSize(const Size(420, 920));
      addTearDown(() => tester.binding.setSurfaceSize(null));
    }
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    var phoneRequestCount = 0;
    var serverRequestCount = 0;
    try {
      final phone = _FakePhoneMatterCommissioner(
        onCommission: () async {
          phoneRequestCount += 1;
          throw const PhoneMatterCommissioningException(
            stage: 'handoff',
            message: 'The phone could not reach the Rhythm Box.',
          );
        },
      );
      final api = _FakeRhythmMatterApi(
        onResult: () => const RhythmMatterPairingResponse(
            httpStatus: 200, status: 'failed', error: 'Handoff timed out'),
        onPair: (_) async {
          serverRequestCount += 1;
          return const RhythmMatterPairingResponse(
            httpStatus: 200,
            status: 'failed',
            error: 'server BLE timed out',
          );
        },
      );

      await tester.pumpWidget(
        MaterialApp(
          debugShowCheckedModeBanner: false,
          home: MatterDeviceAddScreen(
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
            addMethod: MatterAddMethod.phoneCommissioning,
            initialSetupPayload: 'MT:Y.K908OC16750648G00',
            analyticsSource: 'test_phone',
            journeyId: 'matter-phone-test',
            pairingApi: api,
            phoneCommissioner: phone,
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 10));

      expect(phoneRequestCount, 1);
      expect(serverRequestCount, 0);
      expect(find.text('Try from Rhythm Box'), findsOneWidget);
      if (screenshotPath != null) {
        await tester.pump(const Duration(seconds: 1));
        await expectLater(
            find.byType(Overlay), matchesGoldenFile(screenshotPath));
      }
      final firstAttempt = analyticsBackend.events.firstWhere(
        (event) => event.name == 'matter_pairing_attempted',
      );
      final firstCompletion = analyticsBackend.events.firstWhere(
        (event) => event.name == 'matter_pairing_completed',
      );
      expect(firstAttempt.properties['add_method'], 'phone_commissioning');
      expect(firstCompletion.properties['failure_stage'], 'handoff');
      expect(
        firstCompletion.properties.values,
        isNot(contains('MT:Y.K908OC16750648G00')),
      );

      await tester.ensureVisible(find.text('Try from Rhythm Box'));
      await tester.tap(find.text('Try from Rhythm Box'));
      await tester.pump(const Duration(milliseconds: 10));

      expect(phoneRequestCount, 1);
      expect(serverRequestCount, 1);
      expect(api.lastRendezvous, 'auto');
      expect(api.resultQueries, phone.sessionIds);
      expect(api.sessionIds.single, isNot(phone.sessionIds.single));
      expect(api.lastSetupPayload, phone.setupPayloads.single);
      final attempts = analyticsBackend.events
          .where((event) => event.name == 'matter_pairing_attempted')
          .toList();
      expect(attempts, hasLength(2));
      expect(attempts.last.properties['add_method'], 'automatic');
      expect(attempts.map((event) => event.properties['journey_id']).toSet(),
          {'matter-phone-test'});
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets(
      'native failed receipt retains recovery and skips redundant reconciliation',
      (tester) async {
    final screenshot = Platform.environment['RHYTHM_NATIVE_RECEIPT_SCREENSHOT'];
    if (screenshot != null) {
      await tester.binding.setSurfaceSize(const Size(420, 920));
      addTearDown(() => tester.binding.setSurfaceSize(null));
    }
    final fixture = jsonDecode(File(
      '../../../tools/app/testdata/phone_matter_contract.json',
    ).readAsStringSync()) as Map<String, dynamic>;
    const channel = MethodChannel('phone-matter-test');
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel,
        (call) async {
      calls.add(call);
      return {'http_status': 200, 'body': fixture['failed_receipt']};
    });
    addTearDown(() => tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null));
    const commissioner = PhoneMatterCommissioner(channel: channel);
    final receipt = await commissioner.commission(
        baseUrl: 'http://127.0.0.1',
        originalSetupPayload: 'MT:ORIGINAL-OWNER-CODE',
        sessionId: 'test-receipt');
    expect(receipt.status, 'failed');
    expect(receipt.recoveryAction, 'existing_node_recommission_failed');
    expect(receipt.warnings, ['The original setup code remains available.']);
    calls.clear();
    final api = _FakeRhythmMatterApi(onPair: (_) async => receipt);
    await tester.pumpWidget(MaterialApp(
        debugShowCheckedModeBanner: false,
        home: MatterDeviceAddScreen(
          endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
          initialSetupPayload: 'MT:Y.K908OC16750648G00',
          addMethod: MatterAddMethod.phoneCommissioning,
          phoneCommissioner: commissioner,
          pairingApi: api,
        )));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    expect(find.textContaining('Pairing failed.'), findsOneWidget);
    expect(find.textContaining('Phone commissioning did not complete.'),
        findsNothing);
    expect(find.textContaining('Please reset the bulb before trying again.'),
        findsOneWidget);
    final event = analyticsBackend.events
        .singleWhere((event) => event.name == 'matter_pairing_completed');
    expect(event.properties['failure_stage'], 'commissioning');
    expect(event.properties['recovery_action'],
        'existing_node_recommission_failed');
    if (screenshot != null) {
      await tester.pump(const Duration(seconds: 1));
      await expectLater(find.byType(Overlay), matchesGoldenFile(screenshot));
    }
    await tester.ensureVisible(find.text('Try from Rhythm Box'));
    await tester.tap(find.text('Try from Rhythm Box'));
    await tester.pump(const Duration(milliseconds: 50));
    expect(api.resultQueries, isEmpty);
    expect(api.sessionIds, hasLength(1));
    expect(api.sessionIds.single,
        isNot((calls.single.arguments as Map)['session_id']));
  });

  for (final state in ['pending', 'unavailable']) {
    testWidgets('does not start another attempt while prior receipt is $state',
        (tester) async {
      final phone = _FakePhoneMatterCommissioner(
        onCommission: () async => throw const PhoneMatterCommissioningException(
            stage: 'handoff', message: 'Response lost'),
      );
      final api = _FakeRhythmMatterApi(
        onResult: () => state == 'pending'
            ? const RhythmMatterPairingResponse(
                httpStatus: 200, status: 'pending')
            : null,
        onPair: (_) async => throw StateError('Must not recommission'),
      );
      await tester.pumpWidget(MaterialApp(
          home: MatterDeviceAddScreen(
        endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
        addMethod: MatterAddMethod.phoneCommissioning,
        initialSetupPayload: 'MT:Y.K908OC16750648G00',
        pairingApi: api,
        phoneCommissioner: phone,
      )));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 10));
      for (var retry = 0; retry < 2; retry++) {
        await tester.ensureVisible(find.text('Try from Rhythm Box'));
        await tester.tap(find.text('Try from Rhythm Box'));
        await tester.pump(const Duration(milliseconds: 10));
      }
      expect(api.sessionIds, isEmpty);
      expect(phone.sessionIds, hasLength(1));
      expect(api.resultQueries,
          [phone.sessionIds.single, phone.sessionIds.single]);
      expect(
          find.textContaining(
              state == 'pending' ? 'still finishing' : 'could not be checked'),
          findsOneWidget);
    });
  }

  testWidgets('recovers a completed phone result without recommissioning',
      (tester) async {
    MatterDevicePairingResult? result;
    final phone = _FakePhoneMatterCommissioner(
      onCommission: () async => throw const PhoneMatterCommissioningException(
          stage: 'handoff', message: 'Response lost'),
    );
    final api = _FakeRhythmMatterApi(
      onResult: () => const RhythmMatterPairingResponse(
          httpStatus: 200,
          status: 'complete',
          device: {
            'device_id': 'matter-42',
            'name': 'Test bulb',
            'device_type': 'light'
          },
          warnings: [
            'Recovery code was not saved.'
          ]),
      onPair: (_) async => throw StateError('Must not recommission'),
    );
    await tester.pumpWidget(MaterialApp(
        home: Builder(
      builder: (context) => TextButton(
          onPressed: () async {
            result = await Navigator.of(context)
                .push<MatterDevicePairingResult>(MaterialPageRoute(
                    builder: (_) => MatterDeviceAddScreen(
                          endpoint:
                              const HubEndpoint(host: '127.0.0.1', port: 0),
                          addMethod: MatterAddMethod.phoneCommissioning,
                          initialSetupPayload: 'MT:Y.K908OC16750648G00',
                          pairingApi: api,
                          phoneCommissioner: phone,
                        )));
          },
          child: const Text('PAIR')),
    )));
    await tester.tap(find.text('PAIR'));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    await tester.ensureVisible(find.text('Try from Rhythm Box'));
    await tester.tap(find.text('Try from Rhythm Box'));
    await tester.pumpAndSettle();
    expect(result?.nativeDeviceId, 'matter-42');
    expect(result?.warnings, ['Recovery code was not saved.']);
    expect(api.sessionIds, isEmpty);
    expect(phone.sessionIds, hasLength(1));
    final completion = analyticsBackend.events
        .lastWhere((event) => event.name == 'matter_pairing_completed');
    expect(completion.properties['add_method'], 'phone_commissioning');
    expect(completion.properties['outcome'], 'succeeded');
  });

  testWidgets('a missing prior receipt allows a fresh phone retry',
      (tester) async {
    final phone = _FakePhoneMatterCommissioner(
      onCommission: () async => throw const PhoneMatterCommissioningException(
          stage: 'cancelled', message: 'Cancelled'),
    );
    final api = _FakeRhythmMatterApi(
      onPair: (_) async => throw StateError('Must use phone'),
    );
    await tester.pumpWidget(MaterialApp(
        home: MatterDeviceAddScreen(
      endpoint: const HubEndpoint(host: '127.0.0.1', port: 0),
      addMethod: MatterAddMethod.phoneCommissioning,
      initialSetupPayload: 'MT:Y.K908OC16750648G00',
      pairingApi: api,
      phoneCommissioner: phone,
    )));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));
    await tester.tap(find.text('Try Again'));
    await tester.pump(const Duration(seconds: 1));
    await tester.ensureVisible(find.text('Add Device'));
    await tester.tap(find.text('Add Device'));
    await tester.pump(const Duration(milliseconds: 10));
    expect(phone.sessionIds, hasLength(2));
    expect(phone.sessionIds[0], isNot(phone.sessionIds[1]));
    expect(api.resultQueries, [phone.sessionIds.first]);
    expect(phone.setupPayloads.toSet(), {'MT:Y.K908OC16750648G00'});
  });
}

class _FakePhoneMatterCommissioner extends PhoneMatterCommissioner {
  _FakePhoneMatterCommissioner({required this.onCommission});

  final Future<RhythmMatterPairingResponse> Function() onCommission;
  final sessionIds = <String>[];
  final setupPayloads = <String>[];

  @override
  Future<RhythmMatterPairingResponse> commission({
    required String baseUrl,
    required String originalSetupPayload,
    required String sessionId,
    String? authToken,
  }) {
    sessionIds.add(sessionId);
    setupPayloads.add(originalSetupPayload);
    return onCommission();
  }
}

class _FakeRhythmMatterApi extends RhythmMatterApi {
  _FakeRhythmMatterApi({required this.onPair, this.onResult})
      : super(baseUrl: 'http://127.0.0.1');

  final Future<RhythmMatterPairingResponse> Function(Duration receiveTimeout)
      onPair;
  final RhythmMatterPairingResponse? Function()? onResult;
  final resultQueries = <String>[];
  String? lastSetupPayload;
  String? lastRendezvous;

  @override
  Future<RhythmMatterPairingResponse?> getPairingResult(
      String sessionId) async {
    resultQueries.add(sessionId);
    return onResult != null
        ? onResult!()
        : const RhythmMatterPairingResponse(
            httpStatus: 404, status: 'not_found');
  }

  final sessionIds = <String?>[];

  @override
  Future<RhythmMatterPairingResponse> pairDevice({
    required String setupPayload,
    String network = 'wifi',
    String rendezvous = 'auto',
    Duration receiveTimeout = const Duration(seconds: 45),
    String? sessionId,
  }) {
    lastSetupPayload = setupPayload;
    lastRendezvous = rendezvous;
    sessionIds.add(sessionId);
    return onPair(receiveTimeout);
  }
}
