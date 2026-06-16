/// Configuration for backend providers.
///
/// This configuration determines which backend implementation
/// to use for auth and analytics.
enum BackendType {
  /// Supabase backend.
  supabase,

  /// Offline backend (local-only, no cloud sync).
  offline,
}

/// Configuration for initializing the backend layer.
class BackendConfig {
  /// Which backend type to use.
  final BackendType type;

  /// Supabase URL (required for Supabase backend).
  final String? supabaseUrl;

  /// Supabase anonymous key (required for Supabase backend).
  final String? supabaseAnonKey;

  /// Whether to enable debug logging.
  final bool enableLogging;

  const BackendConfig({
    required this.type,
    this.supabaseUrl,
    this.supabaseAnonKey,
    this.enableLogging = false,
  });

  /// Create a Supabase configuration.
  factory BackendConfig.supabase({
    required String url,
    required String anonKey,
    bool enableLogging = false,
  }) {
    return BackendConfig(
      type: BackendType.supabase,
      supabaseUrl: url,
      supabaseAnonKey: anonKey,
      enableLogging: enableLogging,
    );
  }

  /// Create an offline configuration (local-only, no cloud sync).
  factory BackendConfig.offline({bool enableLogging = false}) {
    return BackendConfig(
      type: BackendType.offline,
      enableLogging: enableLogging,
    );
  }

  /// Validate that required configuration is present.
  void validate() {
    if (type == BackendType.supabase) {
      if (supabaseUrl == null || supabaseUrl!.isEmpty) {
        throw ArgumentError('Supabase URL is required for Supabase backend');
      }
      if (supabaseAnonKey == null || supabaseAnonKey!.isEmpty) {
        throw ArgumentError('Supabase anon key is required for Supabase backend');
      }
    }
  }
}
