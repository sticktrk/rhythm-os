import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/app_log_service.dart';

void main() {
  tearDown(() {
    AppLogService.instance.resetForTesting();
  });

  test('captures redacted log snapshots', () async {
    AppLogService.instance.record(
      'Remote call Authorization: Bearer abc123 owner_token=secret',
    );

    final snapshot = await AppLogService.instance.snapshotText();

    expect(snapshot, contains('Rhythm app log'));
    expect(snapshot, contains('Bearer [REDACTED]'));
    expect(snapshot, contains('owner_token=[REDACTED]'));
    expect(snapshot, isNot(contains('abc123')));
    expect(snapshot, isNot(contains('secret')));
  });

  test('caps stored log text to the support bundle budget', () async {
    // Budget sized so the bundled app.log spans the whole support
    // interaction (issue #123 lost the removal attempt one minute before
    // the report), while staying well under the transfer path's limits.
    for (var i = 0; i < 1000; i++) {
      AppLogService.instance.record(
        '${i.toString().padLeft(4, '0')} ${'x' * 1000}',
      );
    }

    final snapshot = await AppLogService.instance.snapshotText();

    expect(snapshot.length, lessThanOrEqualTo(513000));
    expect(snapshot, contains('[INFO] 0999 '));
    expect(snapshot, isNot(contains('[INFO] 0000 ')));
  });
}
