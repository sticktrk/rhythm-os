Map<String, dynamic>? jsonMap(dynamic value) {
  if (value is Map<String, dynamic>) return value;
  if (value is Map) return value.cast<String, dynamic>();
  return null;
}

num? jsonNum(
  dynamic value, {
  Iterable<String> preferredKeys = const <String>[],
}) {
  return _jsonNum(
    value,
    preferredKeys: _orderedKeys(preferredKeys),
    seen: <Object?>{},
  );
}

int? jsonInt(
  dynamic value, {
  Iterable<String> preferredKeys = const <String>[],
}) {
  return jsonNum(value, preferredKeys: preferredKeys)?.toInt();
}

/// Lenient bool parsing: accepts bool, 0/1 numbers, and "true"/"false"
/// strings. A malformed field must degrade to [fallback] rather than throw —
/// a throw here would drop the whole SSE event or poll cycle it arrived in.
bool jsonBool(dynamic value, {bool fallback = false}) {
  if (value is bool) return value;
  if (value is num) return value != 0;
  if (value is String) {
    final normalized = value.trim().toLowerCase();
    if (normalized == 'true' || normalized == '1') return true;
    if (normalized == 'false' || normalized == '0') return false;
  }
  return fallback;
}

double? jsonDouble(
  dynamic value, {
  Iterable<String> preferredKeys = const <String>[],
}) {
  return jsonNum(value, preferredKeys: preferredKeys)?.toDouble();
}

Map<String, dynamic> normalizeActiveProfile(Map<String, dynamic> json) {
  final rawActiveProfile = jsonMap(json['active_profile']);
  if (rawActiveProfile == null) {
    return Map<String, dynamic>.from(
      jsonMap(json['config']) ?? const <String, dynamic>{},
    );
  }

  final config = Map<String, dynamic>.from(
    jsonMap(rawActiveProfile['config']) ?? rawActiveProfile,
  );
  final effective =
      jsonMap(rawActiveProfile['effective']) ?? const <String, dynamic>{};
  final id = rawActiveProfile['id'] as String?;
  if (id != null && id.isNotEmpty && config['id'] == null) {
    config['id'] = id;
  }

  _overlayEffective(config, effective, 'fade_ms');
  _overlayEffective(config, effective, 'motion_timeout_secs');
  _overlayEffective(config, effective, 'rhythm_interval_secs');

  return config;
}

void _overlayEffective(
  Map<String, dynamic> config,
  Map<String, dynamic> effective,
  String key,
) {
  final configured = jsonNum(config[key], preferredKeys: <String>[key]);
  if (configured != null) return;
  final effectiveValue = jsonNum(effective[key], preferredKeys: <String>[key]);
  if (effectiveValue != null) {
    config[key] = effectiveValue;
  }
}

num? _jsonNum(
  dynamic value, {
  required List<String> preferredKeys,
  required Set<Object?> seen,
}) {
  if (value == null) return null;
  if (value is num) return value;
  if (value is String) return num.tryParse(value);
  if (value is! Map) return null;
  if (!seen.add(value)) return null;

  for (final key in preferredKeys) {
    final parsed = _jsonNum(
      value[key],
      preferredKeys: preferredKeys,
      seen: seen,
    );
    if (parsed != null) return parsed;
  }

  final candidateEntries = value.entries
      .where(
        (entry) =>
            entry.value != null &&
            !_jsonNumMetadataKeys.contains(entry.key.toString()),
      )
      .toList();
  if (candidateEntries.length == 1) {
    return _jsonNum(
      candidateEntries.single.value,
      preferredKeys: preferredKeys,
      seen: seen,
    );
  }

  return null;
}

List<String> _orderedKeys(Iterable<String> preferredKeys) {
  return <String>{
    ...preferredKeys,
    ..._defaultJsonNumKeys,
  }.toList(growable: false);
}

const Set<String> _jsonNumMetadataKeys = <String>{
  'auto',
  'enabled',
  'is_auto',
  'mode',
  'source',
  'unit',
  'units',
  'label',
  'name',
  'id',
  'type',
};

const List<String> _defaultJsonNumKeys = <String>[
  'value',
  'current',
  'raw',
  'amount',
  'count',
  'seconds',
  'secs',
  'ms',
  'hour',
  'brightness',
  'brightness_pct',
  'kelvin',
  'color_temp',
  'temperature',
  'time_offset',
  'brightness_offset',
  'remaining_secs',
  'motion_remaining',
  'timeout_secs',
  'motion_timeout',
  'port',
  'listen_port',
  'current_hour',
  'solar_position',
  'sun_position',
  'last_tick_epoch_ms',
  'effective_fade_ms',
  'effective_motion_timeout_secs',
  'fade_ms',
  'motion_timeout_secs',
  'rhythm_interval_secs',
];
