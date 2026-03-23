/// Abstract interface for analytics tracking.
///
/// Implementations handle event logging and user property tracking
/// for various analytics providers (Firebase Analytics, PostHog, etc.).
abstract class AnalyticsBackend {
  /// Initialize the analytics backend.
  Future<void> initialize();

  /// Whether analytics is ready to use.
  bool get isInitialized;

  /// Log a custom event with optional parameters.
  Future<void> logEvent(String name, [Map<String, Object>? params]);

  /// Set a user property for segmentation.
  Future<void> setUserProperty(String name, String? value);

  /// Log a screen view event.
  Future<void> logScreenView(String screenName);

  /// Set the user ID for analytics tracking.
  Future<void> setUserId(String? userId);

  /// Dispose of resources.
  void dispose();
}
