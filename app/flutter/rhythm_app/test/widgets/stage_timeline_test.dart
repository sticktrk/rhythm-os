import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/stage_timeline.dart';

import '../helpers/test_wrapper.dart';

void main() {
  const stages = [
    StageTimelineItem(label: 'Sending request', icon: Icons.outbox_outlined),
    StageTimelineItem(label: 'Searching', icon: Icons.radar_outlined),
    StageTimelineItem(label: 'Commissioning', icon: Icons.verified_user_outlined),
    StageTimelineItem(label: 'Finalizing', icon: Icons.check_circle_outline),
  ];

  group('StageTimeline', () {
    testWidgets('renders every stage label in order', (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const StageTimeline(
          stages: stages,
          activeIndex: 1,
        ),
      ));
      await tester.pump();

      expect(find.text('Sending request'), findsOneWidget);
      expect(find.text('Searching'), findsOneWidget);
      expect(find.text('Commissioning'), findsOneWidget);
      expect(find.text('Finalizing'), findsOneWidget);
    });

    testWidgets('shows the active sub-message under the active stage only',
        (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const StageTimeline(
          stages: stages,
          activeIndex: 2,
          activeMessage: 'Negotiating with device',
        ),
      ));
      await tester.pump();

      expect(find.text('Negotiating with device'), findsOneWidget);
    });

    testWidgets('hides the sub-message when activeMessage is empty',
        (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const StageTimeline(
          stages: stages,
          activeIndex: 2,
          activeMessage: '   ',
        ),
      ));
      await tester.pump();

      expect(find.textContaining('   '), findsNothing);
    });

    testWidgets('renders a download progress bar with percentage when supplied',
        (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const StageTimeline(
          stages: stages,
          activeIndex: 1,
          activePercent: 73,
          activeBytesLabel: '4.5 / 6.2 MB',
        ),
      ));
      await tester.pump();

      expect(find.text('73%'), findsOneWidget);
      expect(find.text('4.5 / 6.2 MB'), findsOneWidget);
    });

    testWidgets('hides the progress bar when activePercent is null',
        (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const StageTimeline(
          stages: stages,
          activeIndex: 1,
        ),
      ));
      await tester.pump();

      expect(find.textContaining('%'), findsNothing);
    });

    testWidgets('shows a checkmark for stages before activeIndex',
        (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const StageTimeline(
          stages: stages,
          activeIndex: 2,
        ),
      ));
      await tester.pump();

      // Two completed stages → two check icons.
      expect(find.byIcon(Icons.check_rounded), findsNWidgets(2));
    });

    testWidgets(
        'when failed=true, shows a red close icon at the active stage and no checks beyond it',
        (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const StageTimeline(
          stages: stages,
          activeIndex: 2,
          failed: true,
        ),
      ));
      await tester.pump();

      // Two stages before active are still complete.
      expect(find.byIcon(Icons.check_rounded), findsNWidgets(2));
      // The failed dot's close icon.
      expect(find.byIcon(Icons.close_rounded), findsOneWidget);
    });

    testWidgets('does not render any checks when activeIndex is 0',
        (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const StageTimeline(
          stages: stages,
          activeIndex: 0,
        ),
      ));
      await tester.pump();

      expect(find.byIcon(Icons.check_rounded), findsNothing);
    });

    testWidgets('renders all stages as complete when activeIndex == stages.length',
        (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const StageTimeline(
          stages: stages,
          activeIndex: 4,
        ),
      ));
      await tester.pump();

      expect(find.byIcon(Icons.check_rounded), findsNWidgets(4));
    });
  });
}
