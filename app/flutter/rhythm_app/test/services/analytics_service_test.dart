import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/services/analytics_service.dart';

import '../helpers/capturing_analytics_backend.dart';

void main() {
  late CapturingAnalyticsBackend backend;
  late AnalyticsService analytics;

  setUp(() async {
    backend = CapturingAnalyticsBackend();
    await backend.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: backend,
    );
    analytics = AnalyticsService();
    analytics.resetForTesting();
    await analytics.initialize();
  });

  tearDown(() {
    analytics.resetForTesting();
    BackendProvider.resetForTesting();
  });

  test('recent feature events use stable privacy-safe properties', () async {
    await analytics.logRoomDeviceAddMethodSelected(
      source: 'room_settings_motion',
      deviceType: 'motion',
      method: 'existing',
    );
    await analytics.logDevicePairingCodeDetected(
      codeKind: 'homekit',
      outcome: 'guidance_shown',
    );
    await analytics.logMatterPairingCompleted(
      journeyId: 'matter-pair-123',
      source: 'add_review',
      inputMethod: 'camera',
      addMethod: 'automatic',
      attemptNumber: 1,
      outcome: 'failed',
      failureStage: 'wifi_preflight',
    );
    await analytics.logMoodSceneApplyCompleted(
      journeyId: 'mood-scene-123',
      sceneSource: 'native_hue',
      outcome: 'succeeded',
    );
    await analytics.logGlobalRoomActionCompleted(
      journeyId: 'global-room-123',
      action: 'soften',
      eligibleCount: 3,
      attemptedCount: 2,
      completedCount: 2,
      outcome: 'partial',
    );
    await analytics.logRoomCardDetailToggled(
      control: 'color',
      roomMode: 'on',
      expanded: true,
    );
    await analytics.logRoomLightSettingsOpened(
      hasOverrides: true,
      overrideProfileCount: 1,
    );
    await analytics.logRoomLightSettingsSaveCompleted(
      journeyId: 'room-light-settings-123',
      profile: 'rhythm',
      outcome: 'succeeded',
      changedFieldCount: 2,
    );
    await analytics.logRoomLightSettingsResetCompleted(
      journeyId: 'room-light-settings-456',
      profile: 'all',
      outcome: 'failed',
      failureStage: 'request',
    );
    await analytics.logBulbIdentifyCompleted(
      source: 'room_sheet_long_press',
      outcome: 'succeeded',
    );
    await analytics.logDeviceRoomMoveCompleted(
      journeyId: 'device-room-move-123',
      source: 'device_detail',
      destination: 'room',
      outcome: 'partial',
      failureStage: 'authoritative_refresh',
    );

    expect(
      backend.events.map((event) => event.name),
      [
        'room_device_add_method_selected',
        'device_pairing_code_detected',
        'matter_pairing_completed',
        'mood_scene_apply_completed',
        'global_room_action_completed',
        'room_card_detail_toggled',
        'room_light_settings_opened',
        'room_light_settings_save_completed',
        'room_light_settings_reset_completed',
        'bulb_identify_completed',
        'device_room_move_completed',
      ],
    );
    expect(backend.events.first.properties, {
      'source': 'room_settings_motion',
      'device_type': 'motion',
      'method': 'existing',
    });
    expect(
      backend.events[2].properties,
      containsPair('failure_stage', 'wifi_preflight'),
    );
    expect(
      backend.events[3].properties,
      containsPair('scene_source', 'native_hue'),
    );
    expect(
      backend.events[4].properties,
      containsPair('completed_count', 2),
    );
    expect(
      backend.events[5].properties,
      {
        'control': 'color',
        'room_mode': 'on',
        'expanded': 1,
      },
    );
    expect(
      backend.events[7].properties,
      containsPair('changed_field_count', 2),
    );
    expect(
      backend.events[8].properties,
      containsPair('failure_stage', 'request'),
    );
    expect(
      backend.events[9].properties,
      {
        'source': 'room_sheet_long_press',
        'outcome': 'succeeded',
      },
    );
    expect(
      backend.events[10].properties,
      containsPair('failure_stage', 'authoritative_refresh'),
    );

    final serialized = backend.events
        .map((event) => '${event.name}:${event.properties}')
        .join('\n');
    for (final forbidden in [
      'setup_payload',
      'qr_payload',
      'scene_id',
      'room_id',
      'device_id',
      'error',
    ]) {
      expect(serialized, isNot(contains(forbidden)));
    }
  });

  test('Mood picker analytics use privacy-bounded categories', () async {
    await analytics.logMoodPickerOpened(
      roomSource: 'matter',
      hasHueTab: true,
    );
    await analytics.logMoodPickerTabChanged(
      roomSource: 'matter',
      tab: 'hue',
    );
    await analytics.logMoodSceneSelected(
      roomSource: 'matter',
      sceneCategory: 'hue_palette',
      success: true,
    );

    expect(
      backend.events.map((event) => event.name),
      [
        'mood_picker_opened',
        'mood_picker_tab_changed',
        'mood_scene_selected',
      ],
    );
    expect(
      backend.events[0].properties,
      containsPair('has_hue_tab', 1),
    );
    expect(
      backend.events[1].properties,
      containsPair('tab', 'hue'),
    );
    expect(
      backend.events[2].properties,
      containsPair('scene_category', 'hue_palette'),
    );
    expect(
      backend.events[2].properties,
      containsPair('success', 1),
    );
    expect(
      backend.events.expand((event) => event.properties.keys),
      isNot(contains(anyOf('room_id', 'scene_id', 'scene_name'))),
    );
  });

  test('startup milestones queue until the analytics backend is ready',
      () async {
    analytics.resetForTesting();
    backend.dispose();

    await analytics.logStartupAllRoomsVisible(
      elapsedMs: 120,
      fromCache: true,
      roomCountBucket: '2_4',
    );
    await analytics.logStartupAllRoomsInteractive(
      elapsedMs: 340,
      showedCachedRooms: true,
      roomCountBucket: '2_4',
    );

    expect(backend.events, isEmpty);

    await backend.initialize();
    await analytics.initialize();

    expect(
      backend.events.map((event) => event.name),
      [
        'app_startup_all_rooms_visible',
        'app_startup_all_rooms_interactive',
      ],
    );
    expect(backend.events.first.properties, {
      'elapsed_ms': 120,
      'from_cache': 1,
      'room_count_bucket': '2_4',
      'presentation_state': 'cached_read_only',
    });
    final serialized = backend.events
        .map((event) => '${event.name}:${event.properties}')
        .join('\n');
    for (final forbidden in [
      'room_name',
      'home_name',
      'hub_id',
      'endpoint',
      'token',
    ]) {
      expect(serialized, isNot(contains(forbidden)));
    }
  });

  test('analytics remains a no-op when the backend is unavailable', () async {
    BackendProvider.resetForTesting();

    await expectLater(
      analytics.logMoodSceneApplyCompleted(
        journeyId: 'mood-scene-offline',
        sceneSource: 'rhythm',
        outcome: 'failed',
        failureStage: 'offline',
      ),
      completes,
    );
  });
}
