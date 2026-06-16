import 'package:flutter/foundation.dart';
import 'package:posthog_flutter/posthog_flutter.dart';

import '../../config/posthog_config.dart';
import 'analytics_backend.dart';

/// PostHog analytics backend implementation.
///
/// Sends analytics events to PostHog for tracking user behavior,
/// feature usage, and app engagement metrics.
class PostHogAnalyticsBackend implements AnalyticsBackend {
  final bool enableLogging;
  bool _initialized = false;
  String? _userId;
  final Map<String, String> _lastProperties = {};

  PostHogAnalyticsBackend({this.enableLogging = false});

  @override
  bool get isInitialized => _initialized;

  @override
  Future<void> initialize() async {
    if (!PostHogAppConfig.isConfigured) {
      if (enableLogging) {
        debugPrint('PostHogAnalyticsBackend: No API key configured, skipping');
      }
      return;
    }

    final config = PostHogConfig(PostHogAppConfig.apiKey);
    config.host = PostHogAppConfig.host;
    config.debug = enableLogging;

    await Posthog().setup(config);
    _initialized = true;

    debugPrint('PostHogAnalyticsBackend: Initialized (host: ${PostHogAppConfig.host})');
  }

  @override
  Future<void> logEvent(String name, [Map<String, Object>? params]) async {
    if (!_initialized) return;

    await Posthog().capture(eventName: name, properties: params);

    if (enableLogging) {
      final paramsStr = params != null ? ' $params' : '';
      debugPrint('Analytics: $name$paramsStr');
    }
  }

  @override
  Future<void> setUserProperty(String name, String? value) async {
    if (!_initialized || _userId == null) return;

    // Only set property if value is non-null
    if (value != null) {
      // Skip if this property already has the same value
      if (_lastProperties[name] == value) return;

      await Posthog().identify(
        userId: _userId!,
        userProperties: {name: value},
      );
      _lastProperties[name] = value;
    }

    if (enableLogging) {
      debugPrint('Analytics: user property $name = $value');
    }
  }

  @override
  Future<void> logScreenView(String screenName) async {
    if (!_initialized) return;

    await Posthog().screen(screenName: screenName);

    if (enableLogging) {
      debugPrint('Analytics: screen_view $screenName');
    }
  }

  @override
  Future<void> setUserId(String? userId) async {
    if (!_initialized) return;

    // Skip if already identified as this user
    if (userId != null && userId == _userId) return;

    _userId = userId;
    _lastProperties.clear();
    if (userId != null) {
      await Posthog().identify(userId: userId);
    } else {
      await Posthog().reset();
    }

    if (enableLogging) {
      debugPrint('Analytics: user_id = $userId');
    }
  }

  @override
  void dispose() {
    _initialized = false;
    _userId = null;
    _lastProperties.clear();
  }
}
