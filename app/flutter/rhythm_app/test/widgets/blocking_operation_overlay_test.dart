import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/blocking_operation_overlay.dart';

void main() {
  testWidgets('blocking operation overlay prevents navigation until removed',
      (tester) async {
    OverlayEntry? entry;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () {
                entry = showBlockingOperationOverlay(
                  context,
                  message: 'Finishing setup…',
                );
              },
              child: const Text('Start'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Start'));
    await tester.pump();

    expect(
      find.byKey(const ValueKey('blocking-operation-overlay')),
      findsOneWidget,
    );
    expect(find.text('Finishing setup…'), findsOneWidget);
    expect(find.byType(ModalBarrier), findsWidgets);

    entry!.remove();
    await tester.pump();
    expect(
      find.byKey(const ValueKey('blocking-operation-overlay')),
      findsNothing,
    );
  });
}
