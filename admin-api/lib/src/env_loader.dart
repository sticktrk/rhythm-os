import 'dart:io';

Future<Map<String, String>> loadAdminApiEnvironment({
  Map<String, String>? environment,
}) async =>
    Map<String, String>.of(environment ?? Platform.environment);
