import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/blocking_operation_overlay.dart';

void main() {
  testWidgets('blocking operation overlay prevents navigation until removed',
      (tester) async {
    BlockingOperationOverlayHandle? entry;
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

  testWidgets('blocking operation overlay vetoes system back on a pushed route',
      (tester) async {
    BlockingOperationOverlayHandle? entry;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () => Navigator.of(context).push<void>(
                MaterialPageRoute(
                  builder: (routeContext) => Scaffold(
                    body: TextButton(
                      onPressed: () {
                        entry = showBlockingOperationOverlay(
                          routeContext,
                          message: 'Saving room assignment…',
                        );
                      },
                      child: const Text('Start operation'),
                    ),
                  ),
                ),
              ),
              child: const Text('Open operation page'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Open operation page'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Start operation'));
    await tester.pump(const Duration(milliseconds: 300));

    await tester.binding.handlePopRoute();
    await tester.pump(const Duration(milliseconds: 300));

    expect(find.text('Start operation'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('blocking-operation-overlay')),
      findsOneWidget,
    );

    entry!.remove();
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Start operation'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('blocking-operation-overlay')),
      findsNothing,
    );
  });
}
