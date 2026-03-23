/// PostHog configuration for Rhythm Lighting analytics.
///
/// Configure via dart-define environment variables:
/// ```
/// flutter run --dart-define=POSTHOG_API_KEY=phc_xxx
/// flutter run --dart-define=POSTHOG_HOST=https://us.i.posthog.com
/// ```
class PostHogAppConfig {
  /// PostHog project API key (starts with 'phc_')
  static const String apiKey = String.fromEnvironment('POSTHOG_API_KEY');

  /// PostHog host URL
  static const String host = String.fromEnvironment(
    'POSTHOG_HOST',
    defaultValue: 'https://us.i.posthog.com',
  );

  /// Check if PostHog is configured with an API key.
  static bool get isConfigured => apiKey.isNotEmpty;
}
