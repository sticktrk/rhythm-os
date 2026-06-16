import 'dart:convert';
import 'dart:typed_data';

import 'package:archive/archive.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/debug_bundle_submission_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
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
}

String _text(ArchiveFile? file) {
  expect(file, isNotNull);
  return utf8.decode(file!.content);
}
