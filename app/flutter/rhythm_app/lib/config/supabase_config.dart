/// Supabase configuration for Rhythm Lighting.
///
/// These values should be set based on your Supabase project.
/// You can find these in your Supabase dashboard under Settings > API.
///
/// For production, consider using environment variables or a secure
/// configuration management solution.
class SupabaseConfig {
  /// Supabase project URL (e.g., 'https://your-project.supabase.co')
  static const String url = String.fromEnvironment(
    'SUPABASE_URL',
    defaultValue: 'https://YOUR_PROJECT.supabase.co',
  );

  /// Supabase anonymous/public key
  /// This key is safe to use in client-side code as it only allows
  /// operations permitted by your Row Level Security policies.
  static const String anonKey = String.fromEnvironment(
    'SUPABASE_ANON_KEY',
    defaultValue: 'YOUR_ANON_KEY',
  );

  /// Check if configuration is valid (not using placeholder values).
  static bool get isConfigured =>
      !url.contains('YOUR_PROJECT') && !anonKey.contains('YOUR_ANON_KEY');
}
