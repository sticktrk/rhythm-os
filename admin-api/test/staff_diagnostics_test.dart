import 'package:rhythm_admin_api/src/staff_diagnostics.dart';
import 'package:test/test.dart';

void main() {
  test(
      'returns scoped timings only inside the explicit window and suppresses household events',
      () {
    final lines = [
      {
        'text':
            '\x1b[2m2026-11-18T06:00:00Z\x1b[0m selected_state nodes_scope=Controls duration_us=2500'
      },
      {
        'text':
            '2026-11-17T06:01:00Z selected_state nodes_scope=All duration_us=5000'
      },
      {
        'text':
            '2026-11-18T06:01:00Z motion private_canary selected_state nodes_scope=All duration_us=999'
      },
      {'text': '2026-11-18T06:02:00Z WARN timeout private_canary'},
    ];
    expect(staffLogDiagnostics(lines).containsKey('selected_state_timings'),
        isFalse);
    final tail = staffLogDiagnostics(lines,
        since: DateTime.utc(2026, 11, 18, 5),
        until: DateTime.utc(2026, 11, 18, 7),
        includeTimings: true);
    expect(tail['selected_state_timings'], [
      {
        'scope': 'controls',
        'duration_us': 2500,
        'observed_at': '2026-11-18T06:00:00.000Z'
      }
    ]);
    expect(tail['tail_reaches_since'], true);
    expect(tail['diagnostics'].single['severity'], 'warning');
    expect(tail.toString(), isNot(contains('private_canary')));
  });
  test(
      'classifies appliance Wi-Fi boot messages without exporting SSIDs or assigning a time',
      () {
    final result = staffLogDiagnostics([
      {
        'line_number': 1,
        'text': "wifi: no IP yet; state=SCANNING ssid='private_canary'"
      },
      {
        'line_number': 2,
        'text': "wifi: connected to 'private_canary' with IP 192.168.0.1"
      },
      {
        'line_number': 3,
        'text':
            '{"time":"2026-11-19T05:01:00Z","fields":{"event":"motion_node_control_suppressed","message":"integration offline"}}'
      }
    ]);
    expect(result['diagnostics'], [
      {'line': 1, 'category': 'wifi_address_unavailable'},
      {'line': 2, 'category': 'network_restored'}
    ]);
    expect(result['undated_diagnostics'], 2);
    expect(staffDiagnosticTime('2026-02-31T01:00:00Z'), isNull);
    expect(staffDiagnosticTime('2026-11-19T03:00:00+25:40'), isNull);
  });

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
