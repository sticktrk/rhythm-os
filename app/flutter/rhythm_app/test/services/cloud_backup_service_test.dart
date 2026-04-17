import 'package:dio/dio.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/cloud_backed_server_api.dart';
import 'package:rhythm_app/services/cloud_backup_service.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

  int configSetCalls = 0;
  int getSettingsCalls = 0;
  int triggerSyncCalls = 0;

  @override
  Future<bool> configSet(
    RhythmCurveConfig config, {
    String? id,
  }) async {
    configSetCalls++;
    return true;
  }

  @override
  Future<RhythmSettings?> getSettings() async {
    getSettingsCalls++;
    return const RhythmSettings(powerSave: false);
  }

  @override
  Future<Map<String, dynamic>?> triggerSync() async {
    triggerSyncCalls++;
    return <String, dynamic>{'queued': true};
  }
}

void main() {
  group('CloudBackupService.buildSnapshot', () {
    test('stamps source hub metadata on the single-user snapshot', () {
      final snapshot = CloudBackupService.buildSnapshot(
        userId: 'user-123',
        serverHub: Hub.server(
          id: 'server-hub-1',
          homeId: 'home-1',
          name: 'Bedroom Server',
          host: '192.168.1.50',
          port: 54448,
        ),
        home: Home.create(
          id: 'home-1',
          name: 'My Home',
          ownerId: 'user-123',
        ),
        configurationBundle: const <String, dynamic>{
          'kind': 'configuration_bundle',
        },
        backupBundle: const <String, dynamic>{
          'kind': 'backup_bundle',
        },
        capturedAt: DateTime.utc(2026, 4, 16),
      );

      final payload = snapshot.toUpsertJson();
      expect(payload['user_id'], 'user-123');
      expect(payload['source_hub_id'], 'server-hub-1');
      expect(payload['source_hub_name'], 'Bedroom Server');
      expect(payload['source_hub_host'], '192.168.1.50');
      expect(payload['source_hub_port'], 54448);
      expect(payload['home_id'], 'home-1');
      expect(payload['home_name'], 'My Home');
      expect(
        payload['configuration_bundle'],
        const <String, dynamic>{'kind': 'configuration_bundle'},
      );
      expect(
        payload['backup_bundle'],
        const <String, dynamic>{'kind': 'backup_bundle'},
      );
    });
  });

  group('CloudBackedServerApi', () {
    late _FakeRhythmServerApi delegate;
    late List<({Duration delay, String reason})> captures;
    late CloudBackedServerApi api;

    setUp(() {
      delegate = _FakeRhythmServerApi();
      captures = <({Duration delay, String reason})>[];
      api = CloudBackedServerApi(
        delegate: delegate,
        scheduleCloudCapture: ({delay = Duration.zero, reason = ''}) {
          captures.add((delay: delay, reason: reason));
        },
      );
    });

    test('schedules a capture after config writes', () async {
      await api.configSet(
        const RhythmCurveConfig(
          id: 'rhythm',
          name: 'Rhythm',
        ),
      );

      expect(delegate.configSetCalls, 1);
      expect(captures, hasLength(1));
      expect(captures.single.reason, 'config_set');
      expect(captures.single.delay, const Duration(seconds: 2));
    });

    test('does not schedule a capture for read-only calls', () async {
      final settings = await api.getSettings();

      expect(settings?.powerSave, isFalse);
      expect(delegate.getSettingsCalls, 1);
      expect(captures, isEmpty);
    });

    test('uses a longer debounce after sync operations', () async {
      await api.triggerSync();

      expect(delegate.triggerSyncCalls, 1);
      expect(captures, hasLength(1));
      expect(captures.single.reason, 'trigger_sync');
      expect(captures.single.delay, const Duration(seconds: 5));
    });
  });
}
