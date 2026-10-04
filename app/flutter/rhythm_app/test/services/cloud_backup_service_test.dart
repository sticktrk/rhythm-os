import 'package:dio/dio.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/cloud_backed_server_api.dart';
import 'package:rhythm_app/services/cloud_backup_service.dart';
import 'package:rhythm_app/services/settings_service.dart';
import 'package:rhythm_app/providers/room_page_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

  int configSetCalls = 0;
  int getSettingsCalls = 0;
  int nodeMotionActivationSetCalls = 0;
  int triggerSyncCalls = 0;

  @override
  Future<bool> configSet(
    RhythmCurveConfig config, {
    String? id,
    bool apply = false,
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
  Future<RhythmRoomState?> nodeMotionActivationSet({
    required String nodeId,
    required bool enabled,
    required String requestId,
  }) async {
    nodeMotionActivationSetCalls++;
    return RhythmRoomState.fromJson({
      'node_id': nodeId,
      'state': 'active',
      'rhythm_enabled': true,
      'time_offset': 0.0,
      'brightness_offset': 0.0,
      'profile_settings': {'motion_activation_enabled': enabled},
    });
  }

  @override
  Future<Map<String, dynamic>?> triggerSync() async {
    triggerSyncCalls++;
    return <String, dynamic>{'queued': true};
  }
}

class _DeploymentBundleApi extends RhythmBundleApi {
  _DeploymentBundleApi(this.deployment) : super(baseUrl: 'http://test/');
  final RhythmDeploymentCapabilities deployment;
  int fullBackupCalls = 0;
  int profileCalls = 0;
  int lightingSettingsCalls = 0;
  bool failCapabilities = false;
  bool failProfiles = false;

  @override
  Future<RhythmDeploymentCapabilities> getDeploymentCapabilities() async {
    if (failCapabilities) throw StateError('offline');
    return deployment;
  }

  @override
  Future<Map<String, dynamic>> getBackupBundle({bool includeSecrets = false}) async {
    fullBackupCalls++;
    return {'kind': 'backup_bundle', 'includes_secrets': includeSecrets};
  }

  @override
  Future<Map<String, dynamic>> getConfigurationBundle() async {
    profileCalls++;
    if (failProfiles) throw StateError('profile unavailable');
    return {'kind': 'configuration_bundle'};
  }

  @override
  Future<Map<String, dynamic>> getLightingSettings() async {
    lightingSettingsCalls++;
    if (failProfiles) throw StateError('settings unavailable');
    return {
      'schema_version': 1,
      'kind': 'lighting_settings',
      'profile': {},
      'nodes': [],
    };
  }
}

void main() {
  group('deployment-aware cloud snapshots', () {
    test(
      'new HA captures portable room and light settings without appliance secrets',
      () async {
        final api = _DeploymentBundleApi(
          const RhythmDeploymentCapabilities(
            kind: 'home_assistant_addon',
            portableProfiles: true,
            portableSettings: true,
          ),
        );
        final bundles = await CloudBackupService.captureSupportedBundles(api);
        expect(api.fullBackupCalls, 0);
        expect(api.profileCalls, 0);
        expect(api.lightingSettingsCalls, 1);
        expect(bundles.configuration['kind'], 'lighting_settings');
        expect(bundles.backup['kind'], 'rhythm_portable_snapshot');
      },
    );

    test('HA captures portable profiles without ever requesting full backup', () async {
      final api = _DeploymentBundleApi(const RhythmDeploymentCapabilities(
        kind: 'home_assistant_addon', haDeviceManagement: true, portableProfiles: true,
      ));
      final bundles = await CloudBackupService.captureSupportedBundles(api);
      expect(api.fullBackupCalls, 0);
      expect(api.profileCalls, 1);
      expect(bundles.backup['kind'], 'rhythm_portable_snapshot');
      final snapshot = CloudBackupService.buildSnapshot(
        userId: 'user', serverHub: Hub.server(id: 'hub', homeId: 'home', name: 'Rhythm', host: 'host'),
        backupBundle: bundles.backup, configurationBundle: bundles.configuration,
      );
      expect(snapshot.hasApplianceBackup, isFalse);
      expect(CloudBackupSnapshot.fromRow(snapshot.toUpsertJson()).hasApplianceBackup, isFalse);
    });

    test('addon automatic capture preserves the appliance rollback snapshot and identity', () {
      final appliance = CloudBackupService.buildSnapshot(
        userId: 'user', serverHub: Hub.server(id: 'old', homeId: 'home', name: 'Old Box', host: 'old-host'),
        backupBundle: {'kind': 'backup_bundle', 'secret': 'rollback-token'},
        configurationBundle: {'kind': 'configuration_bundle', 'old': true},
        appSettingsBundle: {'all_rooms_layouts': [{'hub_key': 'old', 'pages': [['old-room']]}]},
      );
      final portable = CloudBackupService.buildSnapshot(
        userId: 'user', serverHub: Hub.server(id: 'addon', homeId: 'home', name: 'New Add-on', host: 'new-host'),
        backupBundle: {'kind': 'rhythm_portable_snapshot', 'schema_version': 1},
        configurationBundle: {'kind': 'configuration_bundle', 'new': true},
        appSettingsBundle: {'all_rooms_layouts': [{'hub_key': 'addon', 'pages': [['new-room']]}]},
      );
      final update = CloudBackupService.portableSnapshotSettingsUpdate(
        existing: appliance, portable: portable,
      );
      expect(update.keys, ['app_settings_bundle']);
      final stored = CloudBackupSnapshot.fromRow({...appliance.toUpsertJson(), ...update});
      expect(stored.sourceHubId, 'old');
      expect(stored.sourceHubHost, 'old-host');
      expect(stored.backupBundle, appliance.backupBundle);
      expect(stored.configurationBundle, appliance.configurationBundle);
      expect(stored.hasApplianceBackup, isTrue);
      expect(stored.appSettingsBundle['all_rooms_layouts'], hasLength(2));
      expect(stored.lightingRestoreSources, hasLength(2));
      final addonSource = stored.lightingRestoreSources.last;
      expect(addonSource.sourceHubId, 'addon');
      expect(addonSource.configurationBundle, portable.configurationBundle);
      expect(addonSource.hasApplianceBackup, isFalse);
      expect(addonSource.lightingRestorePayload, portable.configurationBundle);
      expect(stored.lightingRestorePayload, appliance.backupBundle);
    });

    test('legacy server keeps full secret-bearing appliance backup', () async {
      final api = _DeploymentBundleApi(const RhythmDeploymentCapabilities.legacy());
      final bundles = await CloudBackupService.captureSupportedBundles(api);
      expect(api.fullBackupCalls, 1);
      expect(bundles.backup['includes_secrets'], isTrue);
    });

    test('failed capability read cannot downgrade to legacy backup', () async {
      final api = _DeploymentBundleApi(const RhythmDeploymentCapabilities.legacy())
        ..failCapabilities = true;
      await expectLater(CloudBackupService.captureSupportedBundles(api), throwsStateError);
      expect(api.fullBackupCalls, 0);
      expect(api.profileCalls, 0);
    });

    test('portable profile failures never produce a successful empty snapshot', () async {
      final api = _DeploymentBundleApi(const RhythmDeploymentCapabilities(
        kind: 'home_assistant_addon', portableProfiles: true,
      ))..failProfiles = true;
      await expectLater(CloudBackupService.captureSupportedBundles(api), throwsStateError);
      expect(api.fullBackupCalls, 0);
    });
  });

  test(
    'migration remaps only reviewed source rooms without source aliases',
    () {
      final oldHub = Hub.server(
        id: 'old',
        homeId: 'home',
        name: 'Old',
        host: 'box.local',
      );
      final targetHub = Hub.server(
        id: 'ha',
        homeId: 'home',
        name: 'HA',
        host: 'ha.local',
      );
      final source = CloudBackupService.buildSnapshot(
        userId: 'user',
        serverHub: oldHub,
        configurationBundle: const {},
        backupBundle: {'kind': 'backup_bundle'},
        appSettingsBundle: {
          'all_rooms_layouts': [
            {
              'hub_key': 'other-home',
              'pages': [
                ['other-room'],
              ],
            },
            {
              'hub_key': 'server_instance:old',
              'hub_key_aliases': [RoomPageProvider.hubLayoutKey(oldHub)],
              'pages': [
                ['old-1', 'unmatched'],
                ['old-2', 'old-1'],
              ],
            },
          ],
        },
      );
      final remapped = CloudBackupService.remapLightingLayout(
        source: source,
        targetHub: targetHub,
        roomMappings: {'old-1': 'ha-1', 'old-2': 'ha-2'},
      );
      final layout = (remapped['all_rooms_layouts'] as List).single as Map;
      expect(layout['hub_key'], RoomPageProvider.hubLayoutKey(targetHub));
      expect(layout['hub_key_aliases'], isEmpty);
      expect(layout['pages'], [
        ['ha-1'],
        ['ha-2'],
      ]);
      expect(
        CloudBackupService.remapLightingLayout(
          source: source,
          targetHub: targetHub,
          roomMappings: {},
        ),
        isEmpty,
      );
    },
  );

  test('layout uploads preserve the independently saved add-on settings', () {
    final portable = {
      'source_hub_id': 'ha',
      'configuration_bundle': {'kind': 'lighting_settings'},
    };
    final merged = CloudBackupService.mergeAppSettingsBundles(
      {'portable_lighting_snapshot': portable},
      {
        'all_rooms_layouts': [
          {
            'hub_key': 'ha',
            'pages': [
              ['room'],
            ],
          },
        ],
      },
    );
    expect(merged['portable_lighting_snapshot'], portable);
  });

  test('migration keeps the durable source layout after its endpoint changes', () {
    final sourceHub = Hub.server(
      id: 'source', homeId: 'home', name: 'Box', host: 'new-address.local',
      serverInstanceId: 'box-instance',
    );
    final targetHub = Hub.server(id: 'ha', homeId: 'home', name: 'HA', host: 'ha.local');
    final captured = CloudBackupService.buildSnapshot(
      userId: 'user', serverHub: sourceHub,
      backupBundle: {'kind': 'backup_bundle'}, configurationBundle: const {},
      appSettingsBundle: {
        'all_rooms_layouts': [{
          'hub_key': RoomPageProvider.hubLayoutKey(sourceHub),
          'hub_key_aliases': ['server:old-address.local:54448:plain'],
          'pages': [['old-room']],
        }],
      },
    );
    final source = CloudBackupSnapshot.fromRow(captured.toUpsertJson());
    final result = CloudBackupService.remapLightingLayout(
      source: source, targetHub: targetHub,
      roomMappings: {'old-room': 'ha-room'},
    );
    expect((result['all_rooms_layouts'] as List).single['pages'], [['ha-room']]);
  });

  test('legacy snapshot resolves a unique layout after its endpoint changes', () {
    final originalLayout = {
      'hub_key': 'server_instance:box-instance',
      'hub_key_aliases': ['server:old-address.local:54448:plain'],
      'pages': [
        ['old-1', 'unreviewed'],
        ['old-2'],
      ],
    };
    final retained = CloudBackupService.appSettingsBundleForCapture(
      existing: {
        'all_rooms_layouts': [
          {
            'hub_key': 'server_instance:other',
            'pages': [['unrelated-room']],
          },
          originalLayout,
        ],
      },
      local: {
        'all_rooms_layouts': [
          {
            ...originalLayout,
            'hub_key_aliases': ['server:new-address.local:54448:plain'],
          },
        ],
      },
      hasUnsyncedLayoutEdit: false,
    );
    // A pre-upgrade capture stored the new endpoint but retained the old
    // layout, without the lighting_source_hub_keys added by newer clients.
    final source = CloudBackupSnapshot.fromRow({
      'source_hub_id': 'source',
      'source_hub_host': 'new-address.local',
      'source_hub_port': 54448,
      'app_settings_bundle': retained,
    });
    final target = Hub.server(
      id: 'ha', homeId: 'home', name: 'HA', host: 'ha.local',
      serverInstanceId: 'ha-instance',
    );
    final result = CloudBackupService.remapLightingLayout(
      source: source, targetHub: target,
      roomMappings: {'old-1': 'ha-1', 'old-2': 'ha-2'},
    );
    final layout = (result['all_rooms_layouts'] as List).single as Map;
    expect(layout['pages'], [['ha-1'], ['ha-2']]);
    expect(layout['hub_key'], RoomPageProvider.hubLayoutKey(target));
    expect(layout['hub_key_aliases'], RoomPageProvider.hubLayoutKeyAliases(target));
    expect((retained['all_rooms_layouts'] as List).last, originalLayout);
    expect(retained.containsKey('lighting_source_hub_keys'), isFalse);
  });

  test('legacy layout recovery rejects ambiguous reviewed room matches', () {
    final source = CloudBackupSnapshot.fromRow({
      'source_hub_id': 'source',
      'source_hub_host': 'new-address.local',
      'source_hub_port': 54448,
      'app_settings_bundle': {
        'all_rooms_layouts': [
          for (final hub in ['first', 'second'])
            {
              'hub_key': 'server_instance:$hub',
              'pages': [['old-room']],
            },
        ],
        'all_rooms_layout': {'pages': [['old-room']]},
      },
    });
    expect(
      CloudBackupService.remapLightingLayout(
        source: source,
        targetHub: Hub.server(id: 'ha', homeId: 'home', name: 'HA', host: 'ha.local'),
        roomMappings: {'old-room': 'ha-room'},
      ),
      isEmpty,
    );
  });

  test('recorded source identity does not fall back to another hub layout', () {
    final source = CloudBackupSnapshot.fromRow({
      'source_hub_id': 'source',
      'source_hub_host': 'box.local',
      'source_hub_port': 54448,
      'app_settings_bundle': {
        'lighting_source_hub_keys': {'source': 'server_instance:box'},
        'all_rooms_layouts': [
          {
            'hub_key': 'server_instance:other',
            'pages': [['old-room']],
          },
        ],
      },
    });
    expect(
      CloudBackupService.remapLightingLayout(
        source: source,
        targetHub: Hub.server(id: 'ha', homeId: 'home', name: 'HA', host: 'ha.local'),
        roomMappings: {'old-room': 'ha-room'},
      ),
      isEmpty,
    );
  });

  group('CloudBackupService.buildSnapshot', () {
    test('repeated portable capture refreshes profiles and source metadata',
        () {
      CloudBackupSnapshot portable(String profile, DateTime capturedAt) =>
          CloudBackupService.buildSnapshot(
            userId: 'user',
            serverHub: Hub.server(
              id: 'addon',
              homeId: 'home',
              name: 'Add-on',
              host: 'ha-host',
            ),
            backupBundle: {
              'kind': 'rhythm_portable_snapshot',
              'schema_version': 1
            },
            configurationBundle: {'kind': 'profile_bundle', 'profile': profile},
            appSettingsBundle: const {},
            capturedAt: capturedAt,
          );
      final old = portable('old', DateTime.utc(2026, 1, 1));
      final current = portable('current', DateTime.utc(2026, 1, 2));
      final update = CloudBackupService.portableSnapshotSettingsUpdate(
        existing: old,
        portable: current,
      );
      final stored =
          CloudBackupSnapshot.fromRow({...old.toUpsertJson(), ...update});
      expect(stored.configurationBundle, current.configurationBundle);
      expect(stored.capturedAt, current.capturedAt);
      expect(stored.hasApplianceBackup, isFalse);
    });

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
        appSettingsBundle: const <String, dynamic>{
          'schema_version': 1,
          'all_rooms_layouts': [
            {
              'hub_key': 'server:192.168.1.50',
              'pages': [
                ['room-1', 'room-2'],
              ],
            },
          ],
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
      expect(payload['configuration_bundle'], const <String, dynamic>{
        'kind': 'configuration_bundle',
      });
      expect(payload['backup_bundle'], const <String, dynamic>{
        'kind': 'backup_bundle',
      });
      expect(payload['app_settings_bundle'], const <String, dynamic>{
        'schema_version': 1,
        'lighting_source_hub_keys': {
          'server-hub-1': 'server:192.168.1.50:54448:plain',
        },
        'all_rooms_layouts': [
          {
            'hub_key': 'server:192.168.1.50',
            'pages': [
              ['room-1', 'room-2'],
            ],
          },
        ],
      });
    });

    test('layout-only sync replaces the same hub and preserves other hubs', () {
      final merged = CloudBackupService.mergeAppSettingsBundles(
        const <String, dynamic>{
          'schema_version': 1,
          'future_setting': true,
          'all_rooms_layouts': [
            {
              'hub_key': 'server:other:54448:plain',
              'pages': [
                ['other-room'],
              ],
            },
            {
              'hub_key': 'server:10.0.0.15:54448:plain',
              'pages': [
                ['old-room'],
              ],
            },
          ],
        },
        const <String, dynamic>{
          'schema_version': 1,
          'all_rooms_layouts': [
            {
              'hub_key': 'server_instance:box-instance-1',
              'hub_key_aliases': ['server:10.0.0.15:54448:plain'],
              'pages': [
                ['kitchen'],
                ['bedroom'],
              ],
            },
          ],
        },
      );

      expect(merged['future_setting'], isTrue);
      expect(merged['all_rooms_layouts'], [
        {
          'hub_key': 'server:other:54448:plain',
          'pages': [
            ['other-room'],
          ],
        },
        {
          'hub_key': 'server_instance:box-instance-1',
          'hub_key_aliases': ['server:10.0.0.15:54448:plain'],
          'pages': [
            ['kitchen'],
            ['bedroom'],
          ],
        },
      ]);
    });

    test('cloud layout aliases bridge durable and legacy endpoint identity',
        () {
      const layout = <String, dynamic>{
        'hub_key': 'server_instance:box-instance-1',
        'hub_key_aliases': ['server:10.0.0.15:54448:plain'],
        'pages': [
          ['kitchen'],
          ['bedroom'],
        ],
      };
      final selected = SettingsService.findCloudRoomLayoutForTesting(
        const <String, dynamic>{
          'schema_version': 1,
          'all_rooms_layouts': [layout],
        },
        roomLayoutHubKey: 'server:10.0.0.15:54448:plain',
      );

      expect(selected, layout);
    });
  });

  group('CloudBackupService.appSettingsBundleForCapture', () {
    Map<String, dynamic> bundleWith(List<List<String>> pages,
        {String hubKey = 'server_instance:abc'}) {
      return {
        'schema_version': 1,
        'all_rooms_layouts': [
          {'hub_key': hubKey, 'pages': pages},
        ],
      };
    }

    test('keeps the account layout when the local one is not a user edit', () {
      final existing = bundleWith([
        ['kitchen'],
        ['bedroom'],
      ]);
      final local = bundleWith([
        ['bedroom', 'kitchen'],
      ]);

      final chosen = CloudBackupService.appSettingsBundleForCapture(
        existing: existing,
        local: local,
        hasUnsyncedLayoutEdit: false,
      );

      expect(chosen, existing);
    });

    test('publishes an unsynced local edit over the account layout', () {
      final existing = bundleWith([
        ['kitchen'],
        ['bedroom'],
      ]);
      final local = bundleWith([
        ['bedroom', 'kitchen'],
      ]);

      final chosen = CloudBackupService.appSettingsBundleForCapture(
        existing: existing,
        local: local,
        hasUnsyncedLayoutEdit: true,
      );

      expect(chosen['all_rooms_layouts'], local['all_rooms_layouts']);
    });

    test('seeds a hub the account has no layout for yet', () {
      final existing = bundleWith([
        ['den'],
      ], hubKey: 'server_instance:other');
      final local = bundleWith([
        ['kitchen'],
        ['bedroom'],
      ]);

      final chosen = CloudBackupService.appSettingsBundleForCapture(
        existing: existing,
        local: local,
        hasUnsyncedLayoutEdit: false,
      );

      expect(chosen['all_rooms_layouts'], [
        ...existing['all_rooms_layouts'] as List,
        ...local['all_rooms_layouts'] as List,
      ]);
    });

    test('starts from the local bundle when the account has none', () {
      final local = bundleWith([
        ['kitchen'],
      ]);
      expect(
        CloudBackupService.appSettingsBundleForCapture(
          existing: null,
          local: local,
          hasUnsyncedLayoutEdit: false,
        ),
        local,
      );
    });
  });

  group('CloudBackedServerApi', () {
    late _FakeRhythmServerApi delegate;
    late CloudBackedServerApi api;

    setUp(() {
      delegate = _FakeRhythmServerApi();
      api = CloudBackedServerApi(
        delegate: delegate,
      );
    });

    test('does not auto-capture after config writes', () async {
      await api.configSet(
        const RhythmCurveConfig(
          id: 'rhythm',
          name: 'Rhythm',
        ),
      );

      expect(delegate.configSetCalls, 1);
    });

    test('does not schedule a capture for read-only calls', () async {
      final settings = await api.getSettings();

      expect(settings?.powerSave, isFalse);
      expect(delegate.getSettingsCalls, 1);
    });

    test('forwards authoritative motion activation writes', () async {
      final state = await api.nodeMotionActivationSet(
        nodeId: 'room-1',
        enabled: false,
        requestId: 'request-1',
      );

      expect(delegate.nodeMotionActivationSetCalls, 1);
      expect(state?.nodeId, 'room-1');
      expect(state?.profileSettings?.motionActivationEnabled, isFalse);
    });

    test('does not auto-capture after sync operations', () async {
      await api.triggerSync();

      expect(delegate.triggerSyncCalls, 1);
    });
  });
}
