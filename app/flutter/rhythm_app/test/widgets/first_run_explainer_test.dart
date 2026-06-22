import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/first_run_explainer.dart';

const _points = [
  ExplainerPoint(
    icon: Icons.palette_rounded,
    title: 'Pick a scene or a color',
    body: 'Choose a saved scene or one fixed color.',
  ),
  ExplainerPoint(
    icon: Icons.lock_outline_rounded,
    title: 'It stays put',
    body: 'Your lights hold steady.',
  ),
];

void main() {
  late Set<String> seen;

  setUp(() {
    // Stub persistence so the test never touches Hive.
    seen = <String>{};
    FirstRunExplainer.seenReader = (id) => seen.contains(id);
    FirstRunExplainer.seenWriter = (id) async {
      seen.add(id);
    };
  });

  Widget host() {
    return MaterialApp(
      home: Scaffold(
        body: Builder(
          builder: (context) => Center(
            child: ElevatedButton(
              onPressed: () => FirstRunExplainer.maybeShow(
                context,
                id: 'demo',
                eyebrow: 'NEW',
                title: 'Meet Mood',
                subtitle: 'A calm light that stays how you set it.',
                icon: Icons.spa_rounded,
                points: _points,
                ctaLabel: 'Choose a Mood',
              ),
              child: const Text('open'),
            ),
          ),
        ),
      ),
    );
  }

  testWidgets('shows the explainer the first time and marks it seen',
      (tester) async {
    await tester.pumpWidget(host());

    await tester.tap(find.text('open'));
    await tester.pump(); // run maybeShow + open the sheet
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('Meet Mood'), findsOneWidget);
    expect(find.text('Pick a scene or a color'), findsOneWidget);
    expect(find.text('It stays put'), findsOneWidget);
    expect(find.text('Choose a Mood'), findsOneWidget);
    expect(seen.contains('demo'), isTrue);
  });

  testWidgets('does not show again once seen', (tester) async {
    seen.add('demo'); // pretend it was shown before

    await tester.pumpWidget(host());
    await tester.tap(find.text('open'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('Meet Mood'), findsNothing);
  });

  testWidgets('CTA dismisses the sheet', (tester) async {
    await tester.pumpWidget(host());

    await tester.tap(find.text('open'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 800)); // finish entrance
    expect(find.text('Meet Mood'), findsOneWidget);

    // The CTA can sit below the fold on the small test surface — bring it on
    // screen before tapping.
    await tester.ensureVisible(find.text('Choose a Mood'));
    await tester.pump();
    await tester.tap(find.text('Choose a Mood'));
    // Avoid pumpAndSettle — the ambient glow animation never settles.
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));

    expect(find.text('Meet Mood'), findsNothing);
  });
}
