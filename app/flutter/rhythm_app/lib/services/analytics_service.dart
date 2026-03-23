import 'package:flutter/foundation.dart';

import '../backend/backend.dart';

/// Singleton service for analytics tracking.
///
/// Provides typed methods for all analytics events to ensure consistency
/// and enable compile-time checking of event names and parameters.
///
/// Delegates to the configured analytics backend (console, PostHog, etc.)
/// through the BackendProvider abstraction layer.
class AnalyticsService {
  static final AnalyticsService _instance = AnalyticsService._internal();
  factory AnalyticsService() => _instance;
  AnalyticsService._internal();

  bool _initialized = false;

  /// Whether analytics is ready to use.
  bool get isInitialized => _initialized;

  AnalyticsBackend? get _analytics {
    if (!BackendProvider.isInitialized) return null;
    return BackendProvider.instance.analytics;
  }

  /// Initialize the analytics service.
  ///
  /// Should be called after BackendProvider.initialize() succeeds.
  Future<void> initialize() async {
    if (_initialized) return;

    try {
      _initialized = BackendProvider.isInitialized &&
          BackendProvider.instance.analytics.isInitialized;
      if (_initialized) {
        debugPrint('AnalyticsService: initialized');
      }
    } catch (e) {
      debugPrint('AnalyticsService: initialization failed: $e');
    }
  }

  // ===========================================================================
  // Core Methods
  // ===========================================================================

  /// Log a custom event with optional parameters.
  Future<void> logEvent(String name, [Map<String, Object>? params]) async {
    if (!_initialized || _analytics == null) return;

    try {
      await _analytics!.logEvent(name, params);
    } catch (e) {
      debugPrint('Analytics error: $e');
    }
  }

  /// Set a user property for segmentation.
  Future<void> setUserProperty(String name, String? value) async {
    if (!_initialized || _analytics == null) return;

    try {
      await _analytics!.setUserProperty(name, value);
    } catch (e) {
      debugPrint('Analytics error: $e');
    }
  }

  /// Identify the current user for analytics attribution.
  ///
  /// Call this at every auth state transition (sign-in, session recovery,
  /// anonymous user creation) to link PostHog events to the Supabase user ID.
  Future<void> identifyUser(String userId) async {
    if (!_initialized || _analytics == null) return;

    try {
      await _analytics!.setUserId(userId);
    } catch (e) {
      debugPrint('Analytics error: $e');
    }
  }

  /// Reset user identity (e.g. on account deletion).
  ///
  /// Generates a new anonymous distinct_id in PostHog so subsequent
  /// events are not attributed to the deleted account.
  Future<void> resetUser() async {
    if (!_initialized || _analytics == null) return;

    try {
      await _analytics!.setUserId(null);
    } catch (e) {
      debugPrint('Analytics error: $e');
    }
  }

  /// Log a screen view event.
  Future<void> logScreenView(String screenName) async {
    if (!_initialized || _analytics == null) return;

    try {
      await _analytics!.logScreenView(screenName);
    } catch (e) {
      debugPrint('Analytics error: $e');
    }
  }

  // ===========================================================================
  // Onboarding Events
  // ===========================================================================

  /// Track when onboarding starts (welcome screen shown).
  Future<void> logOnboardingStarted() async {
    await logEvent('onboarding_started');
  }

  /// Track account choice on the account screen.
  Future<void> logOnboardingAccountChoice(String choice) async {
    await logEvent('onboarding_account_choice', {'choice': choice});
  }

  /// Track location method used during onboarding.
  Future<void> logOnboardingLocationMethod(String method) async {
    await logEvent('onboarding_location_method', {'method': method});
  }

  /// Track sleep schedule step.
  Future<void> logOnboardingSleepSchedule(String wakeTime, String sleepTime) async {
    await logEvent('onboarding_sleep_schedule', {
      'wake_time': wakeTime,
      'sleep_time': sleepTime,
    });
  }

  /// Track notifications step.
  Future<void> logOnboardingNotifications({required bool allowed}) async {
    await logEvent('onboarding_notifications', {'allowed': allowed ? 1 : 0});
  }

  /// Track onboarding completion.
  Future<void> logOnboardingCompleted() async {
    await logEvent('onboarding_completed');
  }

  // ===========================================================================
  // Hub Connection Events
  // ===========================================================================

  /// Track successful hub connection.
  Future<void> logHubConnected(String hubType) async {
    await logEvent('hub_connected', {'hub_type': hubType});
  }

  /// Track hub connection failure.
  Future<void> logHubConnectionFailed(String hubType, String error) async {
    await logEvent('hub_connection_failed', {
      'hub_type': hubType,
      'error': error.length > 100 ? error.substring(0, 100) : error,
    });
  }

  /// Track hub disconnection.
  Future<void> logHubDisconnected(String hubType) async {
    await logEvent('hub_disconnected', {'hub_type': hubType});
  }

  /// Track room sync from a hub.
  Future<void> logRoomSync(int roomCount, String hubType) async {
    await logEvent('room_sync', {
      'room_count': roomCount,
      'hub_type': hubType,
    });
  }

  // ===========================================================================
  // Lighting Control Events
  // ===========================================================================

  /// Track light toggle action.
  Future<void> logLightToggle({required bool turnedOn, String? roomId}) async {
    await logEvent('light_toggle', {
      'turned_on': turnedOn ? 1 : 0,
      if (roomId != null) 'room_id': roomId,
    });
  }

  /// Track config save.
  Future<void> logConfigSaved() async {
    await logEvent('config_saved');
  }

  // ===========================================================================
  // Curve Editing Events (Lower Priority)
  // ===========================================================================

  /// Track slider change (call on change end, not during drag).
  Future<void> logCurveSliderChange(String parameter, double value) async {
    await logEvent('curve_slider_change', {
      'parameter': parameter,
      'value': value,
    });
  }

  /// Track tune gesture in mobile designer.
  Future<void> logTuneGesture(String gesture) async {
    await logEvent('tune_gesture', {'gesture': gesture});
  }

  /// Track mirror toggle in slider controls.
  Future<void> logMirrorToggle({required bool enabled, required String side}) async {
    await logEvent('mirror_toggle', {
      'enabled': enabled ? 1 : 0,
      'side': side,
    });
  }

  // ===========================================================================
  // RhythmServer Settings Events
  // ===========================================================================

  /// Track OTA update check.
  Future<void> logOtaCheck(String currentVersion) async {
    await logEvent('ota_check', {'current_version': currentVersion});
  }

  /// Track OTA update started.
  Future<void> logOtaUpdateStarted(String fromVersion, String toVersion) async {
    await logEvent('ota_update_started', {
      'from_version': fromVersion,
      'to_version': toVersion,
    });
  }

  /// Track OTA update completed.
  Future<void> logOtaUpdateCompleted(String toVersion) async {
    await logEvent('ota_update_completed', {'to_version': toVersion});
  }

  /// Track OTA update failed.
  Future<void> logOtaUpdateFailed(String error) async {
    await logEvent('ota_update_failed', {
      'error': error.length > 100 ? error.substring(0, 100) : error,
    });
  }

  /// Track RhythmServer factory reset or removal.
  Future<void> logRhythmServerReset({required bool wasOnline}) async {
    await logEvent('rhythmserver_reset', {'was_online': wasOnline ? 1 : 0});
  }

  /// Track opening RhythmServer logs.
  Future<void> logRhythmServerLogsOpened() async {
    await logEvent('rhythmserver_logs_opened');
  }

  // ===========================================================================
  // RhythmServer Discovery Events
  // ===========================================================================

  /// Track mDNS scan completion.
  Future<void> logMdnsScanCompleted(int devicesFound) async {
    await logEvent('mdns_scan_completed', {'devices_found': devicesFound});
  }

  /// Track tap on a discovered RhythmServer device.
  Future<void> logRhythmServerDiscoveredConnect(String deviceIp) async {
    await logEvent('rhythmserver_discovered_connect', {'device_ip': deviceIp});
  }

  /// Track tap on RhythmServer setup CTA button.
  Future<void> logRhythmServerSetupTapped(String mode) async {
    await logEvent('rhythmserver_setup_tapped', {'mode': mode});
  }

  // ===========================================================================
  // ESP32 Provisioning Events
  // ===========================================================================

  /// Track ESP32 provisioning started.
  Future<void> logEsp32ProvisioningStarted() async {
    await logEvent('esp32_provisioning_started');
  }

  /// Track ESP32 provisioning completed.
  Future<void> logEsp32ProvisioningCompleted() async {
    await logEvent('esp32_provisioning_completed');
  }

  /// Track ESP32 provisioning failed.
  Future<void> logEsp32ProvisioningFailed(String error) async {
    await logEvent('esp32_provisioning_failed', {
      'error': error.length > 100 ? error.substring(0, 100) : error,
    });
  }

  // ===========================================================================
  // Account Events
  // ===========================================================================

  /// Track sign in.
  Future<void> logSignIn(String method) async {
    await logEvent('sign_in', {'method': method});
  }

  /// Track sign out.
  Future<void> logSignOut() async {
    await logEvent('sign_out');
  }

  // ===========================================================================
  // User Properties
  // ===========================================================================

  /// Update hub_type user property.
  Future<void> setHubType(String? hubType) async {
    await setUserProperty('hub_type', hubType);
  }

  /// Update account_status user property.
  Future<void> setAccountStatus(String status) async {
    await setUserProperty('account_status', status);
  }

  /// Update room_count user property.
  Future<void> setRoomCount(int count) async {
    await setUserProperty('room_count', count.toString());
  }
}
