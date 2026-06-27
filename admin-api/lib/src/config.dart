class AdminApiConfig {
  const AdminApiConfig({
    required this.supabaseUrl,
    required this.supabaseAnonKey,
    required this.host,
    required this.port,
    required this.allowedOrigins,
    this.supabaseServiceRoleKey,
  });

  final Uri supabaseUrl;
  final String supabaseAnonKey;
  final String? supabaseServiceRoleKey;
  final String host;
  final int port;
  final Set<String> allowedOrigins;

  bool get hasServiceRoleKey =>
      supabaseServiceRoleKey != null && supabaseServiceRoleKey!.isNotEmpty;

  static AdminApiConfig fromEnvironment(Map<String, String> environment) {
    final url = _required(environment, 'SUPABASE_URL');
    final anonKey = _required(environment, 'SUPABASE_ANON_KEY');
    final serviceRoleKey = _optional(
          environment,
          'SUPABASE_SERVICE_ROLE_KEY',
        ) ??
        _optional(environment, 'SUPABASE_SERVICE_KEY');
    final host = _optional(environment, 'ADMIN_API_HOST') ?? '127.0.0.1';
    final port = int.tryParse(
          _optional(environment, 'ADMIN_API_PORT') ??
              _optional(environment, 'PORT') ??
              '',
        ) ??
        8787;
    final origins = (_optional(environment, 'ADMIN_API_ALLOWED_ORIGINS') ??
            'http://localhost:5173,http://127.0.0.1:5173')
        .split(',')
        .map((origin) => origin.trim())
        .where((origin) => origin.isNotEmpty)
        .toSet();

    final parsedUrl = Uri.parse(url);
    if (!parsedUrl.hasScheme || parsedUrl.host.isEmpty) {
      throw StateError('SUPABASE_URL must be a valid absolute URL.');
    }
    if (url.contains('your-project') || anonKey.contains('your-anon')) {
      throw StateError('Supabase environment values are still placeholders.');
    }

    return AdminApiConfig(
      supabaseUrl: parsedUrl,
      supabaseAnonKey: anonKey,
      supabaseServiceRoleKey: serviceRoleKey,
      host: host,
      port: port,
      allowedOrigins: origins,
    );
  }

  bool allowsOrigin(String? origin) {
    if (origin == null || origin.isEmpty) return false;
    return allowedOrigins.contains('*') || allowedOrigins.contains(origin);
  }

  static String _required(Map<String, String> environment, String key) {
    final value = _optional(environment, key);
    if (value == null) {
      throw StateError('$key environment variable is required.');
    }
    return value;
  }

  static String? _optional(Map<String, String> environment, String key) {
    final value = environment[key]?.trim();
    return value == null || value.isEmpty ? null : value;
  }
}
