import 'dart:convert';

import 'package:cryptography/cryptography.dart';

/// Canonical resource hash shared by staff and local guarded device writes.
Future<String> canonicalJsonSha256(Object? value) async {
  final digest =
      await Sha256().hash(utf8.encode(jsonEncode(_canonical(value))));
  return digest.bytes
      .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
      .join();
}

Object? _canonical(Object? value) {
  if (value is Map) {
    final keys = value.keys.map((key) => key.toString()).toList()..sort();
    return <String, Object?>{
      for (final key in keys) key: _canonical(value[key])
    };
  }
  if (value is List) return value.map(_canonical).toList(growable: false);
  return value;
}
