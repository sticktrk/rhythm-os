import 'package:flutter/foundation.dart';

import 'analytics_backend.dart';

/// Console-based analytics backend for development and fallback.
///
/// Logs analytics events to the debug console instead of sending
/// them to a remote service. Can be replaced with PostHog, Mixpanel,
/// or another analytics service in the future.
class ConsoleAnalyticsBackend implements AnalyticsBackend {
  final bool enabled;
  bool _initialized = false;

  ConsoleAnalyticsBackend({this.enabled = true});

  @override
  bool get isInitialized => _initialized;

  @override
  Future<void> initialize() async {
    _initialized = true;
    if (enabled) {
      debugPrint('ConsoleAnalyticsBackend: Initialized');
    }
  }

  @override
  Future<void> logEvent(String name, [Map<String, Object>? params]) async {
    if (!_initialized || !enabled) return;

    final paramsStr = params != null ? ' $params' : '';
    debugPrint('Analytics: $name$paramsStr');
  }

  @override
  Future<void> setUserProperty(String name, String? value) async {
    if (!_initialized || !enabled) return;

    debugPrint('Analytics: user property $name = $value');
  }

  @override
  Future<void> logScreenView(String screenName) async {
    if (!_initialized || !enabled) return;

    debugPrint('Analytics: screen_view $screenName');
  }

  @override
  Future<void> setUserId(String? userId) async {
    if (!_initialized) return;

    if (enabled) {
      debugPrint('Analytics: user_id = $userId');
    }
  }

  @override
  void dispose() {
    _initialized = false;
  }
}
