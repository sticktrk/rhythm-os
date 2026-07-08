import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/matter_add_method.dart';
import 'package:rhythm_app/screens/hubs/matter_device_add_screen.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
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
      final api = _FakeRhythmMatterApi(
        onPair: () async {
          pairRequestCount += 1;
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
      final transmitButton = find.text('Add Matter Device');
      await tester.ensureVisible(transmitButton);

      await tester.tap(transmitButton);
      await tester.tap(transmitButton);

      expect(pairRequestCount, 1);

      releaseResponse.complete();
      await tester.pump(const Duration(milliseconds: 100));
    } finally {
      if (!releaseResponse.isCompleted) {
        releaseResponse.complete();
      }
      debugDefaultTargetPlatformOverride = null;
    }
  });
}

class _FakeRhythmMatterApi extends RhythmMatterApi {
  _FakeRhythmMatterApi({required this.onPair})
      : super(baseUrl: 'http://127.0.0.1');

  final Future<RhythmMatterPairingResponse> Function() onPair;

  @override
  Future<RhythmMatterPairingResponse> pairDevice({
    required String setupPayload,
    String network = 'wifi',
    String rendezvous = 'auto',
    Duration receiveTimeout = const Duration(seconds: 45),
    String? sessionId,
  }) {
    return onPair();
  }
}
