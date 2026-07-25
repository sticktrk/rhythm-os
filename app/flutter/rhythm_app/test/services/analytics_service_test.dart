import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/analytics/analytics_backend.dart';
import 'package:rhythm_app/backend/auth/offline_auth_backend.dart';
import 'package:rhythm_app/backend/backend_provider.dart';
import 'package:rhythm_app/services/analytics_service.dart';

class _RecordingAnalyticsBackend implements AnalyticsBackend {
  bool _initialized = false;
  final events = <({String name, Map<String, Object>? params})>[];

  @override
  bool get isInitialized => _initialized;

  @override
  Future<void> initialize() async => _initialized = true;

  @override
  Future<void> logEvent(String name, [Map<String, Object>? params]) async {
    events.add((name: name, params: params));
  }

  @override
  Future<void> logScreenView(String screenName) async {}

  @override
  Future<void> setUserId(String? userId) async {}

  @override
  Future<void> setUserProperty(String name, String? value) async {}

  @override
  void dispose() => _initialized = false;
}

void main() {
  test('Mood picker analytics use privacy-bounded categories', () async {
    final backend = _RecordingAnalyticsBackend();
    await backend.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: backend,
    );
    final analytics = AnalyticsService();
    analytics.resetForTesting();
    await analytics.initialize();
    addTearDown(() {
      analytics.resetForTesting();
      BackendProvider.resetForTesting();
    });

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
      backend.events[0].params,
      containsPair('has_hue_tab', 1),
    );
    expect(
      backend.events[1].params,
      containsPair('tab', 'hue'),
    );
    expect(
      backend.events[2].params,
      containsPair('scene_category', 'hue_palette'),
    );
    expect(
      backend.events[2].params,
      containsPair('success', 1),
    );
    expect(
      backend.events.expand((event) => event.params?.keys ?? const []),
      isNot(contains(anyOf('room_id', 'scene_id', 'scene_name'))),
    );
  });
}
