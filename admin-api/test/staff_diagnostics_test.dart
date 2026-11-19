import 'package:rhythm_admin_api/src/staff_diagnostics.dart';
import 'package:test/test.dart';

void main() {
  test(
      'local wall-clock times and unrelated log lines do not establish an outage time',
      () {
    final lines = [
      {
        'line_number': 10,
        'text': '2026-11-18T23:30:00-05:00 network is unreachable'
      },
      {
        'line_number': 11,
        'text':
            '{"time":"2026-11-19T05:05:02.987654Z","message":"Registered tunnel connection","token":"private_canary"}'
      },
      {'line_number': 12, 'text': '2026-11-19 05:03:00 WiFi disconnected'},
      {
        'line_number': 13,
        'text':
            '2026-11-19T05:12:00Z motion detected; battery error in a private room'
      },
    ];
    final all = staffLogDiagnostics(lines);
    expect(all['undated_diagnostics'], 1);
    expect(all['diagnostics'], [
      {
        'line': 10,
        'category': 'network_offline',
        'observed_at': '2026-11-19T04:30:00.000Z'
      },
      {
        'line': 11,
        'category': 'network_restored',
        'observed_at': '2026-11-19T05:05:02.987Z'
      },
      {'line': 12, 'category': 'network_offline'},
    ]);
    final limited = staffLogDiagnostics(lines,
        since: DateTime.utc(2026, 11, 19, 5),
        until: DateTime.utc(2026, 11, 19, 6));
    expect(limited['diagnostics'], [all['diagnostics'][1]]);
    expect(staffLogFamily('cloudflared.log.3'), 'remote_tunnel');
    expect(staffLogFamily('cloudflared.log-private_canary'), 'other');
  });
}
