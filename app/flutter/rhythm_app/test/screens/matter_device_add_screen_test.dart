import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/matter_add_method.dart';
import 'package:rhythm_app/screens/hubs/matter_device_add_screen.dart';
import 'package:rhythm_core/rhythm_core.dart';

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
}
