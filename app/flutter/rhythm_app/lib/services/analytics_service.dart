import 'package:flutter/foundation.dart';
import 'package:package_info_plus/package_info_plus.dart';

import '../backend/backend.dart';
import '../config/app_orientation_policy.dart';

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
  String? _pendingUserId;
  final List<({String name, Map<String, Object> properties})>
      _pendingStartupEvents = [];

  /// Whether analytics is ready to use.
  bool get isInitialized => _initialized;

  @visibleForTesting
  void resetForTesting() {
    _initialized = false;
    _pendingUserId = null;
    _pendingStartupEvents.clear();
  }

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
        final pendingUserId = _pendingUserId;
        _pendingUserId = null;
        if (pendingUserId != null) {
          await identifyUser(pendingUserId);
        }
        final pendingEvents = List.of(_pendingStartupEvents);
        _pendingStartupEvents.clear();
        for (final event in pendingEvents) {
          await logEvent(event.name, event.properties);
        }
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
    if (!_initialized || _analytics == null) {
      _pendingUserId = userId;
      return;
    }

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
    _pendingUserId = null;
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

  /// Record the first frame containing the All Rooms grid.
  Future<void> logStartupAllRoomsVisible({
    required int elapsedMs,
    required bool fromCache,
    required String roomCountBucket,
  }) async {
    await _logOrQueueStartupEvent('app_startup_all_rooms_visible', {
      'elapsed_ms': elapsedMs,
      'from_cache': fromCache ? 1 : 0,
      'room_count_bucket': roomCountBucket,
      'presentation_state': fromCache ? 'cached_read_only' : 'authoritative',
      'orientation_policy': appOrientationPolicyAnalyticsValue,
    });
  }

  /// Record when the authoritative All Rooms grid first accepts controls.
  Future<void> logStartupAllRoomsInteractive({
    required int elapsedMs,
    required bool showedCachedRooms,
    required String roomCountBucket,
  }) async {
    await _logOrQueueStartupEvent('app_startup_all_rooms_interactive', {
      'elapsed_ms': elapsedMs,
      'showed_cached_rooms': showedCachedRooms ? 1 : 0,
      'room_count_bucket': roomCountBucket,
      'presentation_state': 'authoritative_interactive',
      'orientation_policy': appOrientationPolicyAnalyticsValue,
    });
  }

  /// Stage durations and bounded dimensions supplied by AppStartupPerformance.
  Future<void> logControlReadiness(Map<String, Object> properties) async {
    final event = Map<String, Object>.of(properties);
    try {
      final info = await PackageInfo.fromPlatform();
      event['app_version'] = info.version;
      event['app_build'] = info.buildNumber;
    } catch (_) {
      // Diagnostics are optional on platforms without package metadata.
    }
    await _logOrQueueStartupEvent('app_control_readiness', event);
  }

  Future<void> _logOrQueueStartupEvent(
    String name,
    Map<String, Object> properties,
  ) async {
    if (!_initialized || _analytics == null) {
      // Resumes can recur while analytics is disabled or unavailable.
      if (_pendingStartupEvents.length >= 32) _pendingStartupEvents.removeAt(0);
      _pendingStartupEvents.add((name: name, properties: properties));
      return;
    }
    await logEvent(name, properties);
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
  Future<void> logOnboardingSleepSchedule(
    String wakeTime,
    String sleepTime,
  ) async {
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

  Future<void> logMatterSetupCodeRecoveryAttempted({
    required String source,
  }) async {
    await logEvent('matter_setup_code_recovery_attempted', {'source': source});
  }

  Future<void> logMatterSetupCodeRecoveryCompleted({
    required String source,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('matter_setup_code_recovery_completed', {
      'source': source,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  Future<void> logRemovedBulbArchiveCompleted({
    required String journeyId,
    required String hubType,
    required String outcome,
  }) async {
    await logEvent('removed_bulb_archive_completed', {
      'journey_id': journeyId,
      'hub_type': hubType,
      'source': 'device_detail',
      'outcome': outcome,
    });
  }

  Future<void> logRemovedBulbsOpened({
    required String hubType,
    required int count,
  }) async {
    await logEvent('removed_bulbs_opened', {
      'hub_type': hubType,
      'source': 'hub_detail',
      'count': count,
    });
  }

  Future<void> logRemovedBulbRetryCompleted({
    required String journeyId,
    required String hubType,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('removed_bulb_retry_completed', {
      'journey_id': journeyId,
      'hub_type': hubType,
      'source': 'removed_bulbs',
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  Future<void> logRemovedBulbPurgeCompleted({
    required String journeyId,
    required String hubType,
    required String outcome,
  }) async {
    await logEvent('removed_bulb_purge_completed', {
      'journey_id': journeyId,
      'hub_type': hubType,
      'source': 'removed_bulbs',
      'outcome': outcome,
    });
  }

  /// Track room sync from a hub.
  Future<void> logRoomSync(int roomCount, String hubType) async {
    await logEvent('room_sync', {'room_count': roomCount, 'hub_type': hubType});
  }

  Future<void> logHueAuthorityReviewOpened({
    required String journeyId,
    required String source,
    required String authorityScope,
    required int roomCount,
    required bool hadPriorReview,
  }) async {
    await logEvent('hue_authority_review_opened', {
      'journey_id': journeyId,
      'source': source,
      'authority_scope': authorityScope,
      'affected_room_count_bucket': _layoutCountBucket(roomCount),
      'had_prior_review': hadPriorReview ? 1 : 0,
    });
  }

  Future<void> logHueAuthorityReviewSubmitted({
    required String journeyId,
    required String source,
    required int roomCount,
    required int hueRoomCount,
    required int rhythmRoomCount,
    required bool bridgeTakeoverRequested,
    required String authorityScope,
  }) async {
    final choice = hueRoomCount == roomCount
        ? 'all_hue'
        : rhythmRoomCount == roomCount
            ? 'all_rhythm'
            : 'mixed';
    await logEvent('hue_room_control_choice_submitted', {
      'journey_id': journeyId,
      'source': source,
      'authority_scope': authorityScope,
      'choice': choice,
      'affected_room_count_bucket': _layoutCountBucket(roomCount),
      'conflict_count_bucket': 'unknown',
      'bridge_takeover_requested': bridgeTakeoverRequested ? 1 : 0,
    });
  }

  Future<void> logHueAuthorityReviewCompleted({
    required String journeyId,
    required String source,
    required String outcome,
    required bool bridgeTakeoverRequested,
    required String authorityScope,
    String? failureStage,
  }) async {
    await logEvent('hue_room_control_transition_completed', {
      'journey_id': journeyId,
      'source': source,
      'authority_scope': authorityScope,
      'outcome': outcome,
      'bridge_takeover_requested': bridgeTakeoverRequested ? 1 : 0,
      if (failureStage != null) 'failure_stage': failureStage,
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

  /// Track a global day/sleep mode change.
  Future<void> logGlobalModeChanged(
    String mode, {
    required String source,
  }) async {
    await logEvent('global_mode_changed', {'mode': mode, 'source': source});
  }

  /// Track a per-room mode change from the room card UI.
  Future<void> logRoomModeChanged({
    required String roomId,
    required String previousMode,
    required String nextMode,
  }) async {
    await logEvent('room_mode_changed', {
      'room_id': roomId,
      'previous_mode': previousMode,
      'next_mode': nextMode,
    });
  }

  /// Track a room brightness adjustment from the room card UI.
  Future<void> logRoomBrightnessAdjusted({
    required String roomId,
    required int brightness,
  }) async {
    await logEvent('room_brightness_adjusted', {
      'room_id': roomId,
      'brightness': brightness,
    });
  }

  /// Track the privacy-bounded expansion state of a room-card detail control.
  Future<void> logRoomCardDetailToggled({
    required String control,
    required String roomMode,
    required bool expanded,
  }) async {
    await logEvent('room_card_detail_toggled', {
      'control': control,
      'room_mode': roomMode,
      'expanded': expanded ? 1 : 0,
    });
  }

  /// Track the dedicated room-card settings affordance without identifiers.
  Future<void> logRoomCardSettingsOpened({required String nodeKind}) async {
    await logEvent('room_card_settings_opened', {'node_kind': nodeKind});
  }

  /// Track an explicit request to understand a physical delivery warning.
  ///
  /// This intentionally excludes room, bulb, hub, and endpoint identity.
  Future<void> logLightDeliveryWarningOpened({
    required String surface,
    required int affectedBulbCount,
    required bool hasUnresolvedTarget,
    required String failureKind,
  }) async {
    await logEvent('light_delivery_warning_opened', {
      'surface': surface,
      'affected_bulb_count': affectedBulbCount,
      'has_unresolved_target': hasUnresolvedTarget ? 1 : 0,
      'failure_kind': failureKind,
    });
  }

  /// Track resetting an individual room back to its adaptive curve.
  Future<void> logRoomResetToCurve({required String roomId}) async {
    await logEvent('room_reset_to_curve', {'room_id': roomId});
  }

  /// Track refreshing the rooms experience.
  Future<void> logRoomsRefreshed({required String source}) async {
    await logEvent('rooms_refreshed', {'source': source});
  }

  /// Track the authoritative outcome of an All Rooms batch action.
  Future<void> logGlobalRoomActionCompleted({
    required String journeyId,
    required String action,
    required int eligibleCount,
    required int attemptedCount,
    required int completedCount,
    required String outcome,
  }) async {
    await logEvent('global_room_action_completed', {
      'journey_id': journeyId,
      'action': action,
      'eligible_count': eligibleCount,
      'attempted_count': attemptedCount,
      'completed_count': completedCount,
      'outcome': outcome,
    });
  }

  /// Track a room-scoped scene catalog result without scene identifiers.
  Future<void> logMoodSceneCatalogLoaded({
    required int sceneCount,
    required int nativeSceneCount,
    required String outcome,
  }) async {
    await logEvent('mood_scene_catalog_loaded', {
      'scene_count': sceneCount,
      'native_scene_count': nativeSceneCount,
      'outcome': outcome,
    });
  }

  /// Track a scene apply outcome and its matching server journey.
  Future<void> logMoodSceneApplyCompleted({
    required String journeyId,
    required String sceneSource,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('mood_scene_apply_completed', {
      'journey_id': journeyId,
      'scene_source': sceneSource,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  /// Track opening the room Mood picker without exposing room or scene IDs.
  Future<void> logMoodPickerOpened({
    required String roomSource,
    required bool hasHueTab,
  }) async {
    await logEvent('mood_picker_opened', {
      'room_source': roomSource,
      'has_hue_tab': hasHueTab ? 1 : 0,
    });
  }

  /// Track movement between the low-cardinality Mood picker surfaces.
  Future<void> logMoodPickerTabChanged({
    required String roomSource,
    required String tab,
  }) async {
    await logEvent('mood_picker_tab_changed', {
      'room_source': roomSource,
      'tab': tab,
    });
  }

  /// Track scene application outcome by integration category, never identity.
  Future<void> logMoodSceneSelected({
    required String roomSource,
    required String sceneCategory,
    required bool success,
  }) async {
    await logEvent('mood_scene_selected', {
      'room_source': roomSource,
      'scene_category': sceneCategory,
      'success': success ? 1 : 0,
    });
  }

  /// Track entering room layout edit mode.
  Future<void> logRoomLayoutEditStarted({
    required int roomCount,
    required int pageCount,
  }) async {
    await logEvent('room_layout_edit_started', {
      'room_count': roomCount,
      'page_count': pageCount,
    });
  }

  /// Track leaving room layout edit mode.
  Future<void> logRoomLayoutEditCompleted({
    required int roomCount,
    required int pageCount,
  }) async {
    await logEvent('room_layout_edit_completed', {
      'room_count': roomCount,
      'page_count': pageCount,
    });
  }

  /// Track a room reorder or cross-page move.
  Future<void> logRoomLayoutChanged({
    required String action,
    required int fromPage,
    required int toPage,
    required int toIndex,
    required int roomCount,
    required int pageCount,
  }) async {
    await logEvent('room_layout_changed', {
      'action': action,
      'from_page': fromPage,
      'to_page': toPage,
      'to_index': toIndex,
      'room_count': roomCount,
      'page_count': pageCount,
    });
  }

  /// Track the terminal account-roaming outcome without layout contents or
  /// account/home/hub identifiers.
  Future<void> logRoomLayoutCloudSyncCompleted({
    required String direction,
    required String outcome,
    required int pageCount,
    required int roomCount,
  }) async {
    await logEvent('room_layout_cloud_sync_completed', {
      'direction': direction,
      'outcome': outcome,
      'page_count_bucket': _layoutCountBucket(pageCount),
      'room_count_bucket': _layoutCountBucket(roomCount),
    });
  }

  String _layoutCountBucket(int count) {
    if (count <= 0) return '0';
    if (count == 1) return '1';
    if (count <= 4) return '2_4';
    return '5_plus';
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
  Future<void> logMirrorToggle({
    required bool enabled,
    required String side,
  }) async {
    await logEvent('mirror_toggle', {'enabled': enabled ? 1 : 0, 'side': side});
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
  // Bridge Provisioning Events
  // ===========================================================================

  /// Track Rhythm bridge provisioning started.
  Future<void> logBridgeProvisioningStarted() async {
    await logEvent('bridge_provisioning_started');
  }

  /// Track Rhythm bridge provisioning completed.
  Future<void> logBridgeProvisioningCompleted() async {
    await logEvent('bridge_provisioning_completed');
  }

  /// Track Rhythm bridge provisioning failed.
  Future<void> logBridgeProvisioningFailed(String error) async {
    await logEvent('bridge_provisioning_failed', {
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

  /// Track account deletion.
  Future<void> logAccountDeleted() async {
    await logEvent('account_deleted');
  }

  /// Track opening the in-app feedback surface.
  Future<void> logFeedbackOpened() async {
    await logEvent('feedback_opened');
  }

  /// Track a classified support-report attempt without report contents.
  Future<void> logSupportReportAttempted({
    required String journeyId,
    required String reportKind,
    required String bundleScope,
  }) async {
    await logEvent('support_report_attempted', {
      'journey_id': journeyId,
      'report_kind': reportKind,
      'bundle_scope': bundleScope,
    });
  }

  /// Track the terminal app-observed support-report outcome.
  Future<void> logSupportReportCompleted({
    required String journeyId,
    required String reportKind,
    required String bundleScope,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('support_report_completed', {
      'journey_id': journeyId,
      'report_kind': reportKind,
      'bundle_scope': bundleScope,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  // ===========================================================================
  // Light Profile Events
  // ===========================================================================

  /// Track opening the light profile editor.
  Future<void> logLightProfileOpened(String profile) async {
    await logEvent('light_profile_opened', {'profile': profile});
  }

  /// Track successfully saving the light profile editor.
  Future<void> logLightProfileSaved(String profile) async {
    await logEvent('light_profile_saved', {'profile': profile});
  }

  /// Track a failed save attempt in the light profile editor.
  Future<void> logLightProfileSaveFailed(
    String profile, {
    required String stage,
  }) async {
    await logEvent('light_profile_save_failed', {
      'profile': profile,
      'stage': stage,
    });
  }

  /// Track resetting a profile back to defaults.
  Future<void> logLightProfileReset(String profile) async {
    await logEvent('light_profile_reset', {'profile': profile});
  }

  /// Track preview/apply actions in the time simulator.
  Future<void> logLightProfilePreviewAction({
    required String profile,
    required String action,
    double? offsetMinutes,
  }) async {
    await logEvent('light_profile_preview_action', {
      'profile': profile,
      'action': action,
      if (offsetMinutes != null) 'offset_minutes': offsetMinutes,
    });
  }

  /// Track opening the advanced color editor.
  Future<void> logLightProfileAdvancedColorEditorOpened(String profile) async {
    await logEvent('light_profile_advanced_color_editor_opened', {
      'profile': profile,
    });
  }

  /// Track changes to per-room profile defaults.
  Future<void> logLightProfileRoomDefaultChanged({
    required String profile,
    required bool cleared,
    String source = 'automations',
  }) async {
    await logEvent('light_profile_room_default_changed', {
      'profile': profile,
      'cleared': cleared ? 1 : 0,
      'source': source,
    });
  }

  /// Track room Schedule-tab entry without room identity or saved times.
  Future<void> logRoomScheduleOpened({required String source}) async {
    await logEvent('room_schedule_opened', {'source': source});
  }

  Future<void> logLightSchedulesOpened({required String source}) async {
    await logEvent('light_schedules_opened', {'source': source});
  }

  Future<void> logLightScheduleMutationAttempted({
    required String journeyId,
    required String mutation,
    required String inputMethod,
  }) async {
    await logEvent('light_schedule_mutation_attempted', {
      'journey_id': journeyId,
      'mutation': mutation,
      'input_method': inputMethod,
    });
  }

  Future<void> logLightScheduleMutationCompleted({
    required String journeyId,
    required String mutation,
    required String inputMethod,
    required String triggerKind,
    required String solarEvent,
    required String offsetDirection,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('light_schedule_mutation_completed', {
      'journey_id': journeyId,
      'mutation': mutation,
      'input_method': inputMethod,
      'trigger_kind': triggerKind,
      'solar_event': solarEvent,
      'offset_direction': offsetDirection,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  Future<void> logLightScheduleAssignmentCompleted({
    required String journeyId,
    required String assignmentKind,
    required String overrideScope,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('light_schedule_assignment_completed', {
      'journey_id': journeyId,
      'assignment_kind': assignmentKind,
      'override_scope': overrideScope,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  Future<void> logLightScheduleAssignmentAttempted({
    required String journeyId,
    required String assignmentKind,
    required String overrideScope,
  }) async {
    await logEvent('light_schedule_assignment_attempted', {
      'journey_id': journeyId,
      'assignment_kind': assignmentKind,
      'override_scope': overrideScope,
    });
  }

  Future<void> logRoomScheduleSaveAttempted({
    required String journeyId,
    required int attemptNumber,
    required String inputMethod,
    required String changeKind,
    required String source,
  }) async {
    await logEvent('room_schedule_save_attempted', {
      'journey_id': journeyId,
      'attempt_number': attemptNumber,
      'input_method': inputMethod,
      'change_kind': changeKind,
      'source': source,
    });
  }

  Future<void> logRoomScheduleSaveCompleted({
    required String journeyId,
    required int attemptNumber,
    required String inputMethod,
    required String changeKind,
    required String source,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('room_schedule_save_completed', {
      'journey_id': journeyId,
      'attempt_number': attemptNumber,
      'input_method': inputMethod,
      'change_kind': changeKind,
      'source': source,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  Future<void> logRoomScheduleTestCompleted({
    required String journeyId,
    required int attemptNumber,
    required String inputMethod,
    required String source,
    required String action,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('room_schedule_test_completed', {
      'journey_id': journeyId,
      'attempt_number': attemptNumber,
      'input_method': inputMethod,
      'source': source,
      'action': action,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  Future<void> logRoomScheduleInlinePresetChanged({
    required String journeyId,
    required int attemptNumber,
    required String inputMethod,
    required String mode,
    required String behavior,
  }) async {
    await logEvent('room_schedule_inline_preset_changed', {
      'journey_id': journeyId,
      'attempt_number': attemptNumber,
      'input_method': inputMethod,
      'mode': mode,
      'behavior': behavior,
    });
  }

  /// Track discovery of room-scoped Light settings without room identifiers.
  Future<void> logRoomLightSettingsOpened({
    required bool hasOverrides,
    required int overrideProfileCount,
    String scope = 'room',
  }) async {
    await logEvent('${scope}_light_settings_opened', {
      'has_overrides': hasOverrides ? 1 : 0,
      'override_profile_count': overrideProfileCount,
    });
  }

  /// Track the terminal outcome of a room profile save.
  Future<void> logRoomLightSettingsSaveCompleted({
    required String journeyId,
    required String profile,
    required String outcome,
    required int changedFieldCount,
    String? dayColorMode,
    String? dayBrightnessMode,
    String? failureStage,
    String scope = 'room',
  }) async {
    await logEvent('${scope}_light_settings_save_completed', {
      'journey_id': journeyId,
      'profile': profile,
      'outcome': outcome,
      'changed_field_count': changedFieldCount,
      if (dayColorMode != null) 'day_color_mode': dayColorMode,
      if (dayBrightnessMode != null) 'day_brightness_mode': dayBrightnessMode,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  /// Track the terminal outcome of returning a room to home Light settings.
  Future<void> logRoomLightSettingsResetCompleted({
    required String journeyId,
    required String profile,
    required String outcome,
    String? failureStage,
    String scope = 'room',
  }) async {
    await logEvent('${scope}_light_settings_reset_completed', {
      'journey_id': journeyId,
      'profile': profile,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  /// Track the terminal result of a user-requested physical bulb identify.
  Future<void> logBulbIdentifyCompleted({
    required String source,
    required String outcome,
  }) async {
    await logEvent('bulb_identify_completed', {
      'source': source,
      'outcome': outcome,
    });
  }

  /// Track entry into the privacy-bounded manual Matter bulb test journey.
  Future<void> logMatterBulbTesterStarted({
    required String journeyId,
    required String source,
    required int plannedTestCount,
  }) async {
    await logEvent('matter_bulb_tester_started', {
      'journey_id': journeyId,
      'source': source,
      'planned_test_count': plannedTestCount,
    });
  }

  /// Track the terminal app-observed outcome of saving a Matter bulb report.
  Future<void> logMatterBulbTesterSaveCompleted({
    required String journeyId,
    required String outcome,
    required int answeredTestCount,
    required int skippedXyTestCount,
    required String xyOutcome,
    required String serverOutcome,
    required String cloudOutcome,
  }) async {
    await logEvent('matter_bulb_tester_save_completed', {
      'journey_id': journeyId,
      'outcome': outcome,
      'answered_test_count': answeredTestCount,
      'skipped_xy_test_count': skippedXyTestCount,
      'xy_outcome': xyOutcome,
      'server_outcome': serverOutcome,
      'cloud_outcome': cloudOutcome,
    });
  }

  /// Track entry into the privacy-bounded Bulb Audition journey.
  Future<void> logBulbAuditionStarted({
    required String journeyId,
    required String source,
    required int plannedTestCount,
  }) async {
    await logEvent('bulb_audition_started', {
      'journey_id': journeyId,
      'source': source,
      'planned_scenario_count': plannedTestCount,
    });
  }

  /// Track one terminal scenario result without device or customer identity.
  Future<void> logBulbAuditionScenarioCompleted({
    required String journeyId,
    required String scenario,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('bulb_audition_scenario_completed', {
      'journey_id': journeyId,
      'scenario': scenario,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  /// Track the terminal app-observed save result for a schema-v3 audition.
  Future<void> logBulbAuditionSaveCompleted({
    required String journeyId,
    required String outcome,
    required int answeredTestCount,
    required String serverOutcome,
    required String cloudOutcome,
  }) async {
    await logEvent('bulb_audition_save_completed', {
      'journey_id': journeyId,
      'outcome': outcome,
      'answered_scenario_count': answeredTestCount,
      'server_outcome': serverOutcome,
      'cloud_outcome': cloudOutcome,
    });
  }

  /// Track the terminal app-observed result of assigning a device parent.
  Future<void> logDeviceRoomMoveCompleted({
    required String journeyId,
    required String source,
    required String destination,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('device_room_move_completed', {
      'journey_id': journeyId,
      'source': source,
      'destination': destination,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  /// Track the terminal app-observed result of saving button room targets.
  Future<void> logButtonControlTargetsSaveCompleted({
    required String journeyId,
    required String source,
    required String targetCountBucket,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('button_control_targets_save_completed', {
      'journey_id': journeyId,
      'source': source,
      'target_count_bucket': targetCountBucket,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  // ===========================================================================
  // Hub Recovery Events
  // ===========================================================================

  /// Track actions taken from the hub recovery banner.
  Future<void> logHubRecoveryAction({
    required String action,
    required int disconnectedCount,
  }) async {
    await logEvent('hub_recovery_action', {
      'action': action,
      'disconnected_count': disconnectedCount,
    });
  }

  // ===========================================================================
  // Device Pairing Events
  // ===========================================================================

  /// Track the room-scoped choice between scanning and selecting a canonical
  /// device that Rhythm already knows about.
  Future<void> logRoomDeviceAddMethodSelected({
    required String source,
    required String deviceType,
    required String method,
  }) async {
    await logEvent('room_device_add_method_selected', {
      'source': source,
      'device_type': deviceType,
      'method': method,
    });
  }

  /// Track a scanner classification without retaining its payload.
  Future<void> logDevicePairingCodeDetected({
    required String codeKind,
    required String outcome,
    String? journeyId,
    String? inputMethod,
    String? profileId,
  }) async {
    await logEvent('device_pairing_code_detected', {
      'code_kind': codeKind,
      'outcome': outcome,
      if (journeyId != null) 'journey_id': journeyId,
      if (inputMethod != null) 'input_method': inputMethod,
      if (profileId != null) 'profile_id': profileId,
    });
  }

  /// Track a Matter pairing request at the app-to-server boundary.
  Future<void> logMatterPairingAttempted({
    required String journeyId,
    required String source,
    required String inputMethod,
    required String addMethod,
    required int attemptNumber,
  }) async {
    await logEvent('matter_pairing_attempted', {
      'journey_id': journeyId,
      'source': source,
      'input_method': inputMethod,
      'add_method': addMethod,
      'attempt_number': attemptNumber,
    });
  }

  /// Track a terminal Matter pairing outcome without raw errors or device data.
  Future<void> logMatterPairingCompleted({
    required String journeyId,
    required String source,
    required String inputMethod,
    required String addMethod,
    required int attemptNumber,
    required String outcome,
    String? failureStage,
    String? recoveryAction,
  }) async {
    final boundedRecoveryAction = switch (recoveryAction) {
      'existing_connection_recovered' => 'existing_connection_recovered',
      'existing_node_recommissioned' => 'existing_node_recommissioned',
      'existing_node_recommission_failed' =>
        'existing_node_recommission_failed',
      _ => null,
    };
    await logEvent('matter_pairing_completed', {
      'journey_id': journeyId,
      'source': source,
      'input_method': inputMethod,
      'add_method': addMethod,
      'attempt_number': attemptNumber,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
      if (boundedRecoveryAction != null)
        'recovery_action': boundedRecoveryAction,
    });
  }

  /// Track direct Hue BLE pairing without retaining the bulb serial.
  Future<void> logHueBlePairingAttempted({
    required String journeyId,
    required String source,
    required String inputMethod,
    required int attemptNumber,
  }) async {
    await logEvent('hue_ble_pairing_attempted', {
      'journey_id': journeyId,
      'source': source,
      'input_method': inputMethod,
      'attempt_number': attemptNumber,
    });
  }

  /// Track a terminal direct Hue BLE pairing outcome without device data.
  Future<void> logHueBlePairingCompleted({
    required String journeyId,
    required String source,
    required String inputMethod,
    required int attemptNumber,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('hue_ble_pairing_completed', {
      'journey_id': journeyId,
      'source': source,
      'input_method': inputMethod,
      'attempt_number': attemptNumber,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  /// Track an explicit Hue Bridge button/switch search request.
  Future<void> logHueBridgeButtonPairingAttempted({
    required String journeyId,
    required String source,
    required int attemptNumber,
  }) async {
    await logEvent('hue_bridge_button_pairing_attempted', {
      'journey_id': journeyId,
      'source': source,
      'input_method': 'bridge_search',
      'attempt_number': attemptNumber,
    });
  }

  /// Track the terminal app-observed result of one Bridge accessory search.
  Future<void> logHueBridgeButtonPairingCompleted({
    required String journeyId,
    required String source,
    required int attemptNumber,
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('hue_bridge_button_pairing_completed', {
      'journey_id': journeyId,
      'source': source,
      'input_method': 'bridge_search',
      'attempt_number': attemptNumber,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  /// Track a terminal Hue Bridge device removal without retaining identity.
  Future<void> logHueBridgeDeviceRemovalCompleted({
    required String journeyId,
    required String deviceType,
    required String outcome,
    required bool force,
    String? failureStage,
  }) async {
    await logEvent('hue_bridge_device_removal_completed', {
      'journey_id': journeyId,
      'device_type': deviceType,
      'outcome': outcome,
      'force': force ? 1 : 0,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  /// Track one bounded phone-side Hue BLE observation without advertisement
  /// identity, signal strength, or raw plugin errors.
  Future<void> logHueBleNearbyDiscoveryCompleted({
    required String source,
    required String outcome,
    required int deviceCount,
  }) async {
    await logEvent('hue_ble_nearby_discovery_completed', {
      'source': source,
      'outcome': outcome,
      'device_count': deviceCount < 0
          ? 0
          : deviceCount > 10
              ? 10
              : deviceCount,
    });
  }

  /// Track the user's response to the privacy-safe nearby-bulb invitation.
  Future<void> logNearbyDeviceScanCompleted({
    required String source,
    required String outcome,
    required Map<String, int> familyCounts,
  }) async {
    await logEvent('device_pairing_nearby_scan_completed', {
      'source': source,
      'outcome': outcome,
      for (final entry in familyCounts.entries)
        '${entry.key}_count': entry.value < 0
            ? 0
            : entry.value > 10
                ? 10
                : entry.value,
    });
  }

  Future<void> logNearbyDeviceFamilySelected({
    required String source,
    required String family,
    required int deviceCount,
  }) async {
    await logEvent('device_pairing_nearby_family_selected', {
      'source': source,
      'family': family,
      'device_count': deviceCount < 0
          ? 0
          : deviceCount > 10
              ? 10
              : deviceCount,
    });
  }

  /// Staged Bluetooth-to-Wi-Fi onboarding attempt; [family] is the advertised
  /// profile id, never a serial or key.
  Future<void> logBleWifiPairingAttempted({
    required String journeyId,
    required String source,
    required String inputMethod,
    required String family,
    required int attemptNumber,
    String commissioner = 'server',
    bool resumed = false,
  }) async {
    await logEvent('ble_wifi_pairing_attempted', {
      'journey_id': journeyId,
      'source': source,
      'input_method': inputMethod,
      'family': family,
      'attempt_number': attemptNumber,
      'commissioner': commissioner == 'phone' ? 'phone' : 'server',
      'resumed': resumed,
    });
  }

  Future<void> logBleWifiPairingCompleted({
    required String journeyId,
    required String source,
    required String inputMethod,
    required String family,
    required int attemptNumber,
    String commissioner = 'server',
    required String outcome,
    String? failureStage,
  }) async {
    await logEvent('ble_wifi_pairing_completed', {
      'journey_id': journeyId,
      'source': source,
      'input_method': inputMethod,
      'family': family,
      'attempt_number': attemptNumber,
      'commissioner': commissioner == 'phone' ? 'phone' : 'server',
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  Future<void> logHueBleNearbyPromptAnswered({
    required String source,
    required String outcome,
  }) async {
    await logEvent('hue_ble_nearby_prompt_answered', {
      'source': source,
      'outcome': outcome,
    });
  }

  /// Track a local-BLE pairing attempt without retaining setup or identity.
  Future<void> logLocalBlePairingAttempted({
    required String journeyId,
    required String profileId,
    required String source,
    required String inputMethod,
    required int attemptNumber,
  }) async {
    await logEvent('local_ble_pairing_attempted', {
      'journey_id': journeyId,
      'profile_id': profileId,
      'source': source,
      'input_method': inputMethod,
      'attempt_number': attemptNumber,
    });
  }

  /// Track terminal local-BLE pairing without setup or device identity.
  Future<void> logLocalBlePairingCompleted({
    required String journeyId,
    required String profileId,
    required String source,
    required String inputMethod,
    required int attemptNumber,
    required String outcome,
    String? failureStage,
    String? deduplicationId,
  }) async {
    await logEvent('local_ble_pairing_completed', {
      if (deduplicationId != null) '\$insert_id': deduplicationId,
      'journey_id': journeyId,
      'profile_id': profileId,
      'source': source,
      'input_method': inputMethod,
      'attempt_number': attemptNumber,
      'outcome': outcome,
      if (failureStage != null) 'failure_stage': failureStage,
    });
  }

  // ===========================================================================
  // Device Review Events
  // ===========================================================================

  /// Track loading the device review queue.
  Future<void> logTriageViewed({
    required int entryCount,
    required int deviceCount,
    required int roomCount,
  }) async {
    await logEvent('device_review_loaded', {
      'entry_count': entryCount,
      'device_count': deviceCount,
      'room_count': roomCount,
    });
  }

  /// Track switching device review filters.
  Future<void> logTriageFilterChanged(String filter) async {
    await logEvent('device_review_filter_changed', {'filter': filter});
  }

  /// Track a device review resolution action.
  Future<void> logTriageResolution({
    required String kind,
    required String action,
    bool? hasTarget,
  }) async {
    await logEvent('device_review_resolution', {
      'kind': kind,
      'action': action,
      if (hasTarget != null) 'has_target': hasTarget ? 1 : 0,
    });
  }

  /// Track low-cardinality unreachable-device review boundaries. The random
  /// journey id is the only correlation key; device identity and network
  /// details are intentionally excluded.
  Future<void> logUnreachableDeviceAttention({
    required String journeyId,
    required String action,
    required String state,
  }) async {
    await logEvent('unreachable_device_attention', {
      'journey_id': journeyId,
      'action': action,
      'state': state,
      'source': 'add_review',
    });
  }

  // ===========================================================================
  // Power Usage Events
  // ===========================================================================

  /// Track viewing the power usage screen with calculated data.
  Future<void> logPowerUsageViewed({
    required int roomCount,
    required int lightCount,
  }) async {
    await logEvent('power_usage_viewed', {
      'room_count': roomCount,
      'light_count': lightCount,
    });
  }

  /// Track power-save toggles from the power usage screen.
  Future<void> logPowerSaveToggled({
    required bool enabled,
    required String source,
  }) async {
    await logEvent('power_save_toggled', {
      'enabled': enabled ? 1 : 0,
      'source': source,
    });
  }

  // ===========================================================================
  // Location Events
  // ===========================================================================

  /// Track successful manual location updates.
  Future<void> logLocationUpdated({required bool hasPlaceName}) async {
    await logEvent('location_updated', {
      'has_place_name': hasPlaceName ? 1 : 0,
    });
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
