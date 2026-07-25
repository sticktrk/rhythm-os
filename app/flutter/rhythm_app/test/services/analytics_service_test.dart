import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/services/analytics_service.dart';

import '../helpers/capturing_analytics_backend.dart';

void main() {
  late CapturingAnalyticsBackend backend;

  setUp(() async {
    backend = CapturingAnalyticsBackend();
    await backend.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: backend,
    );
    await AnalyticsService().initialize();
  });

  tearDown(BackendProvider.resetForTesting);

  test('recent feature events use stable privacy-safe properties', () async {
    final analytics = AnalyticsService();

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

    expect(
      backend.events.map((event) => event.name),
      [
        'device_pairing_code_detected',
        'matter_pairing_completed',
        'mood_scene_apply_completed',
        'global_room_action_completed',
      ],
    );
    expect(
      backend.events[1].properties,
      containsPair('failure_stage', 'wifi_preflight'),
    );
    expect(
      backend.events[2].properties,
      containsPair('scene_source', 'native_hue'),
    );
    expect(
      backend.events[3].properties,
      containsPair('completed_count', 2),
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

  test('analytics remains a no-op when the backend is unavailable', () async {
    BackendProvider.resetForTesting();

    await expectLater(
      AnalyticsService().logMoodSceneApplyCompleted(
        journeyId: 'mood-scene-offline',
        sceneSource: 'rhythm',
        outcome: 'failed',
        failureStage: 'offline',
      ),
      completes,
    );
  });
}
