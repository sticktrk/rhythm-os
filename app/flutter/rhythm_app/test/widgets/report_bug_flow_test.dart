import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/debug_bundle_submission_service.dart';
import 'package:rhythm_app/widgets/report_bug_flow.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  group('report bug flow', () {
    testWidgets('prompt classifies a feature and promises a debug bundle',
        (tester) async {
      SupportReportRequest? result;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => TextButton(
              onPressed: () async {
                result = await showDialog<SupportReportRequest>(
                  context: context,
                  builder: (_) => const ReportPromptDialog(),
                );
              },
              child: const Text('Open report'),
            ),
          ),
        ),
      );

      await tester.tap(find.text('Open report'));
      await tester.pumpAndSettle();

      expect(find.text('Report an issue or idea'), findsOneWidget);
      expect(
        find.textContaining('A private debug bundle is included either way'),
        findsOneWidget,
      );
      expect(
        tester
            .widget<SegmentedButton<SupportReportKind>>(
              find.byKey(const Key('support-report-kind-selector')),
            )
            .selected,
        {SupportReportKind.bug},
      );
      expect(find.text('What went wrong? (optional)'), findsOneWidget);

      await tester.tap(find.text('Feature'));
      await tester.pump();
      expect(
        find.text('What would you like Rhythm to do? (optional)'),
        findsOneWidget,
      );

      await tester.enterText(
        find.byKey(const Key('support-report-summary')),
        'Add sunrise previews',
      );
      await tester.tap(find.text('Request feature'));
      await tester.pumpAndSettle();

      expect(result?.kind, SupportReportKind.feature);
      expect(result?.summary, 'Add sunrise previews');
    });

    test('fallback summary preserves user text and debug bundle failure', () {
      final summary = summaryWithDebugBundleFailureForTesting(
        summary: 'Lights are stuck',
        endpoint: 'http://192.168.5.123:54448/',
        detail: 'Failed to generate debug bundle (HTTP 500): log read failed',
      );

      expect(summary, startsWith('Lights are stuck'));
      expect(
        summary,
        contains('Server debug bundle download failed before upload.'),
      );
      expect(summary, contains('Endpoint: http://192.168.5.123:54448/'));
      expect(summary, contains('Failed to generate debug bundle'));
    });

    test('fallback summary works without user text', () {
      final summary = summaryWithDebugBundleFailureForTesting(
        summary: '  ',
        endpoint: 'https://server.rhythm.lighting:443/',
        detail: null,
      );

      expect(
        summary,
        startsWith('Server debug bundle download failed before upload.'),
      );
      expect(
          summary, contains('Endpoint: https://server.rhythm.lighting:443/'));
      expect(summary, isNot(contains('Error:')));
    });

    test('async acknowledgement says the app may close', () {
      final message = supportReportSubmittedMessageForTesting(
        kind: SupportReportKind.bug,
        collectingInBackground: true,
      );

      expect(message, contains('collecting the private debug bundle'));
      expect(message, contains('You can close the app'));
    });

    test('server-managed submission requires an advertised capability', () {
      expect(supportsServerManagedSupportReport(null), isFalse);
      expect(
        supportsServerManagedSupportReport(const RhythmCapabilities()),
        isFalse,
      );
      expect(
        supportsServerManagedSupportReport(const RhythmCapabilities(
          features: [RhythmFeature.asyncDebugBundleUpload],
        )),
        isTrue,
      );
    });
  });
}
