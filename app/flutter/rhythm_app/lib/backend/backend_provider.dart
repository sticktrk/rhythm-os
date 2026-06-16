import 'package:flutter/foundation.dart';

import 'backend_config.dart';
import 'auth/auth_backend.dart';
import 'auth/supabase_auth_backend.dart';
import 'auth/offline_auth_backend.dart';
import 'analytics/analytics_backend.dart';
import 'analytics/console_analytics_backend.dart';
import 'analytics/posthog_analytics_backend.dart';
import '../config/posthog_config.dart';

/// Factory and singleton for backend services.
///
/// Initialize once at app startup with [initialize], then access
/// backends through [instance].
class BackendProvider {
  static BackendProvider? _instance;

  /// Get the singleton instance.
  ///
  /// Throws if [initialize] hasn't been called.
  static BackendProvider get instance {
    if (_instance == null) {
      throw StateError(
        'BackendProvider not initialized. Call BackendProvider.initialize() first.',
      );
    }
    return _instance!;
  }

  /// Check if the backend has been initialized.
  static bool get isInitialized => _instance != null;

  final BackendConfig _config;
  final AuthBackend _auth;
  final AnalyticsBackend _analytics;

  BackendProvider._({
    required BackendConfig config,
    required AuthBackend auth,
    required AnalyticsBackend analytics,
  })  : _config = config,
        _auth = auth,
        _analytics = analytics;

  /// Initialize the backend provider with the given configuration.
  ///
  /// Must be called once before accessing [instance].
  static Future<void> initialize(BackendConfig config) async {
    if (_instance != null) {
      debugPrint('BackendProvider: Already initialized, disposing old instance');
      _instance!.dispose();
    }

    config.validate();

    AuthBackend auth;
    AnalyticsBackend analytics;

    // Use PostHog when configured, otherwise fall back to console logging
    analytics = PostHogAppConfig.isConfigured
        ? PostHogAnalyticsBackend(enableLogging: config.enableLogging)
        : ConsoleAnalyticsBackend(enabled: config.enableLogging);

    switch (config.type) {
      case BackendType.supabase:
        auth = SupabaseAuthBackend(
          supabaseUrl: config.supabaseUrl!,
          supabaseAnonKey: config.supabaseAnonKey!,
        );
      case BackendType.offline:
        auth = OfflineAuthBackend();
    }

    // Initialize backends
    await auth.initialize();
    await analytics.initialize();

    _instance = BackendProvider._(
      config: config,
      auth: auth,
      analytics: analytics,
    );

    debugPrint('BackendProvider: Initialized with ${config.type.name} backend');
  }

  /// Get the current configuration.
  BackendConfig get config => _config;

  /// Get the authentication backend.
  AuthBackend get auth => _auth;

  /// Get the analytics backend.
  AnalyticsBackend get analytics => _analytics;

  /// Dispose of all backend resources.
  void dispose() {
    _auth.dispose();
    _analytics.dispose();
    _instance = null;
    debugPrint('BackendProvider: Disposed');
  }
}
