import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:rhythm_app/services/cloud_backup_service.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

CloudBackupSnapshot _snapshot(
        {required bool appliance, String profile = 'old'}) =>
    CloudBackupService.buildSnapshot(
      userId: 'user',
      serverHub: Hub.server(
        id: appliance ? 'appliance' : 'addon',
        homeId: 'home',
        name: appliance ? 'Box' : 'Add-on',
        host: appliance ? 'box-host' : 'ha-host',
      ),
      backupBundle: appliance
          ? {'kind': 'backup_bundle', 'secret': 'synthetic-rollback-token'}
          : {'kind': 'rhythm_portable_snapshot', 'schema_version': 1},
      configurationBundle: {'kind': 'profile_bundle', 'profile': profile},
      appSettingsBundle: {
        'all_rooms_layouts': [
          {
            'hub_key': appliance ? 'appliance' : 'addon',
            'pages': [
              [profile]
            ]
          },
        ],
      },
      capturedAt: DateTime.utc(2026, 1, profile == 'old' ? 1 : 2),
    );

/// Fake the storage boundary; production Supabase query construction and the
/// persistence decisions run unchanged, including atomic PostgREST filters.
class _SnapshotStore {
  _SnapshotStore(this.row) {
    client = SupabaseClient(
      'http://localhost:54321',
      'test-key',
      accessToken: () async => 'test-token',
      httpClient: MockClient(_request),
    );
  }

  Map<String, dynamic>? row;
  Map<String, dynamic>? concurrentAppliance;
  final writes = <Map<String, dynamic>>[];
  late final SupabaseClient client;

  Future<http.Response> _request(http.Request request) async {
    http.Response json(Object value, {int status = 200}) => http.Response(
          jsonEncode(value),
          status,
          headers: {'content-type': 'application/json'},
          request: request,
        );
    expect(request.url.path, '/rest/v1/user_cloud_snapshots');
    if (request.method == 'GET') return json([if (row != null) row]);
    if (concurrentAppliance != null) {
      row = concurrentAppliance;
      concurrentAppliance = null;
    }
    final update = jsonDecode(request.body) as Map<String, dynamic>;
    writes.add(update);
    if (request.method == 'POST') {
      if (row != null) {
        return json({'code': '23505', 'message': 'duplicate key'}, status: 409);
      }
      row = update;
      return http.Response('', 201, request: request);
    }
    expect(request.method, 'PATCH');
    expect(request.url.queryParameters['user_id'], 'eq.user');
    final expectedKind = request.url.queryParameters['backup_bundle->>kind'];
    if (row == null ||
        (expectedKind != null &&
            expectedKind != 'eq.${(row!['backup_bundle'] as Map)['kind']}')) {
      return json([]);
    }
    row = {...row!, ...update};
    return json(request.headers['accept'] == 'application/vnd.pgrst.object+json'
        ? row!
        : [row]);
  }
}

void main() {
  test('portable capture creates then refreshes the cloud profile snapshot',
      () async {
    final store = _SnapshotStore(null);
    addTearDown(store.client.dispose);
    await CloudBackupService.persistPortableSnapshot(
      client: store.client,
      portable: _snapshot(appliance: false),
    );
    final latest = _snapshot(appliance: false, profile: 'current');
    final saved = await CloudBackupService.persistPortableSnapshot(
      client: store.client,
      portable: latest,
    );
    expect(saved.configurationBundle, latest.configurationBundle);
    expect(saved.capturedAt, latest.capturedAt);
    expect(saved.sourceHubId, 'addon');
    expect(saved.hasApplianceBackup, isFalse);
    expect(store.row!['configuration_bundle'], latest.configurationBundle);
  });

  for (final race in ['existing', 'insert', 'update']) {
    test('portable capture preserves appliance rollback during $race',
        () async {
      final appliance = _snapshot(appliance: true);
      final store = _SnapshotStore(switch (race) {
        'existing' => appliance.toUpsertJson(),
        'update' => _snapshot(appliance: false).toUpsertJson(),
        _ => null,
      });
      if (race != 'existing') {
        store.concurrentAppliance = appliance.toUpsertJson();
      }
      addTearDown(store.client.dispose);
      final saved = await CloudBackupService.persistPortableSnapshot(
        client: store.client,
        portable: _snapshot(appliance: false, profile: 'current'),
      );
      expect(saved.sourceHubId, 'appliance');
      expect(saved.sourceHubHost, 'box-host');
      expect(saved.backupBundle, appliance.backupBundle);
      expect(saved.configurationBundle, appliance.configurationBundle);
      expect(saved.capturedAt, appliance.capturedAt);
      expect(saved.appSettingsBundle['all_rooms_layouts'], hasLength(2));
      expect(store.writes.last.keys, ['app_settings_bundle']);
      expect(store.row!['backup_bundle'], appliance.backupBundle);
    });
  }
}
