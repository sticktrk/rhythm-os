import 'dart:convert';

/// Only these infrastructure log source families leave the staff service.
const _sources = {
  'wifi.log': 'wifi',
  'cloudflared.log': 'remote_tunnel',
  'rhythm-server.err.log': 'server_errors',
  'rhythm-server.log': 'server',
  'rhythm-http.log': 'http',
  'rhythm-periodic.log': 'periodic',
  'rhythm-sse.log': 'events',
  'rhythm-matter.log': 'matter',
  'rhythm-matter-verbose.log': 'matter',
  'bluetooth.log': 'bluetooth'
};

String staffLogFamily(String id) {
  for (final e in _sources.entries) {
    if (id == e.key || id.startsWith('${e.key}.')) return e.value;
  }
  return 'other';
}

// Accept an explicit time zone and normalize precision. Never copy source text
// or make a local wall-clock time look like a UTC instant.
final _iso = RegExp(
    r'^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:?\d{2})$');
final _ansi = RegExp(r'\x1b\[[0-9;]*m');
final _atStart = RegExp(
    r'^\[?(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:?\d{2}))(?=\s|\]|$)');
DateTime? staffDiagnosticTime(Object? value) {
  if (value is! String || !_iso.hasMatch(value)) return null;
  final year = int.parse(value.substring(0, 4)),
      month = int.parse(value.substring(5, 7)),
      day = int.parse(value.substring(8, 10));
  final localDate = DateTime.utc(year, month, day);
  if (localDate.year != year ||
      localDate.month != month ||
      localDate.day != day ||
      int.parse(value.substring(11, 13)) > 23 ||
      int.parse(value.substring(14, 16)) > 59 ||
      int.parse(value.substring(17, 19)) > 59) {
    return null;
  }
  final offset = RegExp(r'[+-](\d{2}):?(\d{2})$').firstMatch(value);
  if (offset != null &&
      (int.parse(offset.group(1)!) > 23 || int.parse(offset.group(2)!) > 59)) {
    return null;
  }
  final parsed = DateTime.tryParse(value)?.toUtc();
  if (parsed == null || parsed.year < 2000 || parsed.year > 2100) return null;
  return DateTime.fromMillisecondsSinceEpoch(parsed.millisecondsSinceEpoch,
      isUtc: true);
}

final _householdEvent = RegExp(
    r'\b(?:motion|occupancy|presence)\b|motion_(?:sensor|node|detected)|light_command');
final _timing = RegExp(
    r'\bselected_state nodes_scope=(controls|all|none) duration_us=(\d{1,10})\b');
final _critical = RegExp(
    r'\b(panic(?:ked)?|fatal|segmentation fault|assertion failed|stack overflow)\b');
final _error = RegExp(r'\berror\b');
final _warning = RegExp(r'\bwarn(?:ing)?\b');

String? _category(String line) {
  if (_householdEvent.hasMatch(line)) return null;
  if (line.contains('wifi: no wireless interface appeared')) {
    return 'wifi_interface_unavailable';
  }
  if (line.contains('wifi: no ip yet') ||
      line.contains('wifi: udhcpc failed to start')) {
    return 'wifi_address_unavailable';
  }
  if (line.contains('wifi: no configured network') ||
      line.contains('wifi: wpa_supplicant failed to start')) {
    return 'wifi_configuration_unavailable';
  }
  if (line.contains('wifi: connected with ip') ||
      line.contains("wifi: connected to '")) {
    return 'network_restored';
  }
  if (RegExp(
          r'network (?:is )?(?:unreachable|down)|link (?:is )?down|wi-?fi.*(?:disconnected|disassociated)|tunnel.*(?:disconnect|closed|down)|failed to serve (?:tunnel|quic)|connection reset by peer')
      .hasMatch(line)) {
    return 'network_offline';
  }
  if (RegExp(
          r'registered tunnel connection|tunnel.*(?:reconnected|connection established)|link (?:is )?up|wi-?fi.*(?:reconnected|connection established)')
      .hasMatch(line)) {
    return 'network_restored';
  }
  if (RegExp('auth.*(fail|denied)|unauthorized|forbidden').hasMatch(line)) {
    return 'authentication';
  }
  if (RegExp(r'\btimeout\b|timed out').hasMatch(line)) return 'timeout';
  if (RegExp(r'\breboot(?:ing|ed)?\b|server (?:started|restart(?:ed|ing)?)')
      .hasMatch(line)) {
    return 'restart';
  }
  if (RegExp('integration.*(unavailable|offline)').hasMatch(line)) {
    return 'integration_unavailable';
  }
  if (_critical.hasMatch(line) || _error.hasMatch(line)) return 'device_error';
  return null;
}

Map<String, dynamic> staffLogDiagnostics(List<dynamic> lines,
    {DateTime? since, DateTime? until, bool includeTimings = false}) {
  final diagnostics = <Map<String, dynamic>>[];
  final timings = <Map<String, dynamic>>[];
  var tailReachesSince = false;
  var undated = 0;
  for (final line in lines.whereType<Map>()) {
    final text = line['text'];
    if (text is! String) continue;
    var message = text.replaceAll(_ansi, '').trimLeft();
    var time = staffDiagnosticTime(_atStart.firstMatch(message)?.group(1));
    String? level;
    if (message.startsWith('{')) {
      try {
        final event = jsonDecode(message);
        if (event is! Map) continue;
        final fields = event['fields'];
        if ([
          event['event'],
          event['target'],
          if (fields is Map) fields['event']
        ]
            .whereType<String>()
            .any((v) => _householdEvent.hasMatch(v.toLowerCase()))) {
          continue;
        }
        final msg = event['message'] ??
            event['msg'] ??
            (fields is Map ? fields['message'] : null);
        if (msg is! String) continue;
        if (event['level'] is String) {
          level = (event['level'] as String).toLowerCase();
        }
        message = msg;
        time = staffDiagnosticTime(
            event['timestamp'] ?? event['time'] ?? event['ts']);
      } on FormatException {
        continue;
      }
    }
    final lower = message.toLowerCase();
    if (_householdEvent.hasMatch(lower)) continue;
    if (time != null && since != null && !time.isAfter(since)) {
      tailReachesSince = true;
    }
    if (includeTimings &&
        time != null &&
        since != null &&
        until != null &&
        !time.isBefore(since) &&
        time.isBefore(until)) {
      final sample = _timing.firstMatch(lower);
      if (sample != null) {
        final micros = int.parse(sample[2]!);
        if (micros <= 3600000000) {
          timings.add({
            'observed_at': time.toIso8601String(),
            'scope': sample[1],
            'duration_us': micros
          });
        }
      }
    }
    final severity = _critical.hasMatch(lower)
        ? 'critical'
        : level == 'error' || _error.hasMatch(lower)
            ? 'error'
            : level == 'warn' || _warning.hasMatch(lower)
                ? 'warning'
                : null;
    final category =
        _category(lower) ?? (severity == null ? null : 'device_error');
    if (category == null) continue;
    if (time == null) {
      undated++;
      if (since != null || until != null) continue;
    } else if ((since != null && time.isBefore(since)) ||
        (until != null && !time.isBefore(until))) {
      continue;
    }
    diagnostics.add({
      'line': line['line_number'] is int ? line['line_number'] : null,
      'category': category,
      if (severity != null) 'severity': severity,
      if (time != null) 'observed_at': time.toIso8601String()
    });
  }
  return {
    'lines_reviewed': lines.length,
    'undated_diagnostics': undated,
    'coverage': 'recent_log_tail',
    if (since != null) 'tail_reaches_since': tailReachesSince,
    if (includeTimings) 'selected_state_timings': timings,
    'diagnostics': diagnostics
  };
}
