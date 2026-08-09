import 'dart:convert';
import 'dart:typed_data';

import 'package:archive/archive.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/debug_bundle_submission_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  test('submission rows preserve feature classification with bug fallback', () {
    final feature = DebugBundleSubmission.fromRow({
      'id': 'submission-1',
      'reference_code': 'RHY-1234',
      'status': 'received',
      'report_kind': 'feature',
    });
    final legacy = DebugBundleSubmission.fromRow({
      'id': 'submission-2',
      'reference_code': 'RHY-5678',
      'status': 'received',
    });

    expect(feature.reportKind, SupportReportKind.feature);
    expect(feature.copyWith(status: 'reported').reportKind,
        SupportReportKind.feature);
    expect(legacy.reportKind, SupportReportKind.bug);
  });

  test('appends app log while preserving server tarball entries', () {
    final serverArchive = Archive()
      ..addFile(ArchiveFile.string('state.json', '{"ok":true}'));
    final serverTar = TarEncoder().encodeBytes(serverArchive);
    final serverGzip = GZipEncoder().encodeBytes(serverTar);

    final updated = DebugBundleSubmissionService.appendAppLogToBundleForTesting(
      bundle: RhythmDebugBundle(
        fileName: 'rhythm-debug-bundle-rpiz.tar.gz',
        bytes: Uint8List.fromList(serverGzip),
        contentType: 'application/gzip',
      ),
      appLogText: 'app log line\n',
    );

    final decoded = TarDecoder().decodeBytes(
      GZipDecoder().decodeBytes(updated.bytes),
      storeData: true,
    );

    expect(_text(decoded.findFile('state.json')), '{"ok":true}');
    expect(_text(decoded.findFile('app/app.log')), contains('app log line'));
    expect(_text(decoded.findFile('app/metadata.json')),
        contains('rhythm_app_log'));
  });

  test('keeps submission received when GitHub issue creation fails', () {
    const submission = DebugBundleSubmission(
      id: 'submission-1',
      referenceCode: 'RHY-1234',
      status: 'received',
    );

    final updated =
        DebugBundleSubmissionService.applyGitHubIssueResponseForTesting(
      submission,
      status: 500,
      data: {
        'error': 'GitHub issue creation failed (404): Not Found',
      },
    );

    expect(updated.status, 'received');
    expect(updated.githubIssueUrl, isNull);
    expect(updated.githubIssueNumber, isNull);
    expect(
      updated.githubIssueError,
      contains('Debug bundle uploaded, but failed to create the GitHub issue.'),
    );
    expect(updated.githubIssueError, contains('GitHub issue creation failed'));
  });

  test('applies created GitHub issue metadata', () {
    const submission = DebugBundleSubmission(
      id: 'submission-1',
      referenceCode: 'RHY-1234',
      status: 'received',
    );

    final updated =
        DebugBundleSubmissionService.applyGitHubIssueResponseForTesting(
      submission,
      status: 200,
      data: {
        'issue_created': true,
        'status': 'reported',
        'issue_url': 'https://github.com/sticktrk/cross/issues/42',
        'issue_number': 42,
      },
    );

    expect(updated.status, 'reported');
    expect(
      updated.githubIssueUrl,
      'https://github.com/sticktrk/cross/issues/42',
    );
    expect(updated.githubIssueNumber, 42);
    expect(updated.githubIssueError, isNull);
  });
}

String _text(ArchiveFile? file) {
  expect(file, isNotNull);
  return utf8.decode(file!.content);
}
