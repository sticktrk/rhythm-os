import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/matter_setup_code_dialog.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  const payload = 'MT:WIDGET-RECOVERY-SECRET';
  final secret = RhythmPairingRecoverySecret(
    payloadKind: RhythmPairingRecoveryPayloadKind.qrCode,
    setupPayload: payload,
    capturedAt: DateTime.utc(2026, 8, 11, 12),
  );

  testWidgets('masks the setup payload until the owner reveals it',
      (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () => showDialog<void>(
                context: context,
                builder: (_) => MatterSetupCodeDialog(secret: secret),
              ),
              child: const Text('OPEN'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('OPEN'));
    await tester.pumpAndSettle();

    expect(find.text(payload), findsNothing);
    expect(find.text('•••• •••• ••••'), findsOneWidget);
    expect(find.text('REVEAL'), findsOneWidget);

    await tester.tap(find.text('REVEAL'));
    await tester.pumpAndSettle();

    expect(find.text(payload), findsOneWidget);
    expect(find.text('HIDE'), findsOneWidget);

    await tester.tap(find.text('HIDE'));
    await tester.pumpAndSettle();
    expect(find.text(payload), findsNothing);
  });

  test('clears a copied setup payload after the private timeout', () async {
    var clipboardCleared = false;

    await clearMatterSetupClipboardIfUnchangedAfter(
      payload,
      Duration.zero,
      readText: () async => payload,
      clear: () async => clipboardCleared = true,
    );

    expect(clipboardCleared, isTrue);
  });

  test('does not erase newer clipboard contents', () async {
    var clipboardCleared = false;

    await clearMatterSetupClipboardIfUnchangedAfter(
      payload,
      Duration.zero,
      readText: () async => 'new clipboard value',
      clear: () async => clipboardCleared = true,
    );

    expect(clipboardCleared, isFalse);
  });
}
