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
final _atStart = RegExp(
    r'^\[?(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:?\d{2}))(?=\s|\]|$)');
DateTime? staffDiagnosticTime(Object? value) {
  if (value is! String || !_iso.hasMatch(value)) return null;
  final parsed = DateTime.tryParse(value)?.toUtc();
  if (parsed == null || parsed.year < 2000 || parsed.year > 2100) return null;
  return DateTime.fromMillisecondsSinceEpoch(parsed.millisecondsSinceEpoch,
      isUtc: true);
}

String? _category(String line) {
  if (RegExp(
          r'\b(?:motion|occupancy|presence)\b|motion_(?:sensor|node|detected)|light_command')
      .hasMatch(line)) {
    return null;
  }
  if (RegExp(
          r'network (?:is )?(?:unreachable|down)|link (?:is )?down|wifi.*(?:disconnected|disassociated)|tunnel.*(?:disconnect|closed|down)|failed to serve (?:tunnel|quic)|connection reset by peer')
      .hasMatch(line)) {
    return 'network_offline';
  }
  if (RegExp(
          r'registered tunnel connection|tunnel.*(?:reconnected|connection established)|link (?:is )?up|wifi.*(?:reconnected|connection established)')
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
  if (RegExp(r'\berror\b').hasMatch(line)) return 'device_error';
  return null;
}

Map<String, dynamic> staffLogDiagnostics(List<dynamic> lines,
    {DateTime? since, DateTime? until}) {
  final diagnostics = <Map<String, dynamic>>[];
  var undated = 0;
  for (final line in lines.whereType<Map>()) {
    final text = line['text'];
    if (text is! String) continue;
    var message = text.trimLeft();
    var time = staffDiagnosticTime(_atStart.firstMatch(message)?.group(1));
    if (message.startsWith('{')) {
      try {
        final event = jsonDecode(message);
        if (event is! Map) continue;
        final fields = event['fields'];
        final msg = event['message'] ??
            event['msg'] ??
            (fields is Map ? fields['message'] : null);
        if (msg is! String) continue;
        message = msg;
        time = staffDiagnosticTime(
            event['timestamp'] ?? event['time'] ?? event['ts']);
      } on FormatException {
        continue;
      }
    }
    final category = _category(message.toLowerCase());
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
      if (time != null) 'observed_at': time.toIso8601String()
    });
  }
  return {
    'lines_reviewed': lines.length,
    'undated_diagnostics': undated,
    'coverage': 'recent_log_tail',
    'diagnostics': diagnostics
  };
}
