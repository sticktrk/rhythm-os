import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/mobile_enrollment_dialog.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  testWidgets('invalid or expired codes never call enrollment', (tester) async {
    var calls = 0;
    await tester.pumpWidget(MaterialApp(home: MobileEnrollmentDialog(
      exchange: (_) async {
        calls++;
        return const RhythmOwnerClaim(tokenId: 'id', token: 'phone-token');
      },
    )));
    await tester.enterText(
        find.byKey(const ValueKey('mobile-enrollment-code')), 'bad-code');
    await tester.tap(find.byKey(const ValueKey('mobile-enrollment-connect')));
    await tester.pump();
    expect(calls, 0);
    expect(find.text('Paste the complete mobile connection code.'),
        findsOneWidget);
    await tester.enterText(
        find.byKey(const ValueKey('mobile-enrollment-code')),
        jsonEncode({
          'format': 'rhythm-mobile-enrollment',
          'version': 1,
          'code': 'expired',
          'server_instance_id': 'installation',
          'expires_at_epoch_ms': 1,
        }));
    await tester.tap(find.byKey(const ValueKey('mobile-enrollment-connect')));
    await tester.pump();
    expect(calls, 0);
    expect(find.text('Create a fresh connection code in Home Assistant.'),
        findsOneWidget);
  });

  testWidgets('connection errors redact credentials and allow a fresh code',
      (tester) async {
    await tester.pumpWidget(MaterialApp(
        home: MobileEnrollmentDialog(
      exchange: (_) async =>
          throw StateError('secret-approval-must-not-be-displayed'),
    )));
    await tester.enterText(
        find.byKey(const ValueKey('mobile-enrollment-code')),
        jsonEncode({
          'format': 'rhythm-mobile-enrollment',
          'version': 1,
          'code': 'valid',
          'server_instance_id': 'installation',
          'expires_at_epoch_ms': DateTime.now()
              .add(const Duration(minutes: 5))
              .millisecondsSinceEpoch,
        }));
    await tester.tap(find.byKey(const ValueKey('mobile-enrollment-connect')));
    await tester.pump();
    expect(find.textContaining('secret-approval'), findsNothing);
    expect(find.textContaining('Create a new connection code'), findsOneWidget);
    expect(
        tester
            .widget<FilledButton>(
                find.byKey(const ValueKey('mobile-enrollment-connect')))
            .onPressed,
        isNotNull);
  });
}
