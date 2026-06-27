import 'dart:io';

Future<Map<String, String>> loadAdminApiEnvironment() async {
  final loaded = <String, String>{};
  final candidates = <String>[
    'app/flutter/rhythm_app/.env',
    '../app/flutter/rhythm_app/.env',
    'admin-api/.env',
    '.env',
  ];

  for (final path in candidates) {
    final file = File(path);
    if (!await file.exists()) continue;
    loaded.addAll(await _parseEnvFile(file));
  }

  loaded.addAll(Platform.environment);
  return loaded;
}

Future<Map<String, String>> _parseEnvFile(File file) async {
  final values = <String, String>{};
  final lines = await file.readAsLines();
  for (final rawLine in lines) {
    var line = rawLine.trim();
    if (line.isEmpty || line.startsWith('#')) continue;
    if (line.startsWith('export ')) {
      line = line.substring('export '.length).trim();
    }
    final equals = line.indexOf('=');
    if (equals <= 0) continue;

    final key = line.substring(0, equals).trim();
    var value = line.substring(equals + 1).trim();
    if (key.isEmpty) continue;
    if ((value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'"))) {
      value = value.substring(1, value.length - 1);
    }
    values[key] = value;
  }
  return values;
}
