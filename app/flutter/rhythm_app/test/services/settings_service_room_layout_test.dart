import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/data/local_data_source.dart';
import 'package:rhythm_app/providers/room_page_provider.dart';
import 'package:rhythm_app/services/cloud_backup_service.dart';
import 'package:rhythm_app/services/settings_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const pathProviderChannel = MethodChannel('plugins.flutter.io/path_provider');
  late Directory testStorage;

  setUpAll(() async {
    testStorage = await Directory.systemTemp.createTemp(
      'rhythm-room-layout-test-',
    );
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
      pathProviderChannel,
      (_) async => testStorage.path,
    );
    final localDataSource = LocalDataSource();
    await localDataSource.initialize();
    await localDataSource.markMigrationComplete();
    await SettingsService.instance.initialize(localDataSource: localDataSource);
  });

  tearDownAll(() async {
    await LocalDataSource().close();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(pathProviderChannel, null);
    await testStorage.delete(recursive: true);
  });

  setUp(() async {
    await LocalDataSource().clearSettings();
  });

  tearDown(() async {
    await LocalDataSource().clearSettings();
  });

  test('migrates an endpoint-scoped dirty marker before cloud restore',
      () async {
    const userId = 'user-1';
    const durableScope = 'server:server_instance:box-instance-1';
    const endpointScope = 'server:server:10.0.0.15:54448:plain';
    await SettingsService.instance.setRoomPageLayoutCloudDirty(
      userId: userId,
      scopeKey: endpointScope,
      dirty: true,
    );

    final dirty =
        await SettingsService.instance.migrateRoomPageLayoutCloudDirty(
      userId: userId,
      scopeKey: durableScope,
      scopeKeyAliases: const [endpointScope],
    );

    expect(dirty, isTrue);
    expect(
      SettingsService.instance.isRoomPageLayoutCloudDirty(
        userId: userId,
        scopeKey: durableScope,
      ),
      isTrue,
    );
    expect(
      SettingsService.instance.isRoomPageLayoutCloudDirty(
        userId: userId,
        scopeKey: endpointScope,
      ),
      isFalse,
    );
  });

  test('later partial transfers preserve existing room page assignments', () async {
    final target = Hub.server(
      id: 'ha', homeId: 'home', name: 'HA', host: 'ha.local',
      serverInstanceId: 'ha-instance',
    );
    final source = CloudBackupService.buildSnapshot(
      userId: 'user',
      serverHub: Hub.server(id: 'box', homeId: 'old-home', name: 'Box', host: 'box.local'),
      backupBundle: {'kind': 'backup_bundle'},
      configurationBundle: const {},
      appSettingsBundle: {
        'all_rooms_layouts': [{
          'hub_key': 'server:box.local:54448:plain',
          'pages': [['old-base', 'old-2'], ['old-1']],
        }],
      },
    );
    final settings = SettingsService.instance;
    final scope = RoomPageProvider.layoutScopeFor(home: null, hubs: [target]);
    await settings.saveRoomPageLayout(
      [['native-first'], ['native-second'], ['native-third']],
      scopeKey: scope,
    );
    final provider = RoomPageProvider()..initialize(scopeKey: scope);
    addTearDown(provider.dispose);
    final rooms = ['native-first', 'native-second', 'native-third', 'ha-base', 'ha-1', 'ha-2']
        .map((id) => RoomDto(
              id: id, name: id, source: RoomSourceDto.hue, deviceIds: const [],
              rhythmEnabled: true, disabled: false, lightsOn: true,
              timeOffsetMinutes: 0, brightnessOffset: 0,
            ))
        .toList();
    Future<void> transfer(Map<String, String> mappings) async {
      final bundle = CloudBackupService.remapLightingLayout(
        source: source, targetHub: target, roomMappings: mappings,
        targetPages: settings.getRoomPageLayout(scopeKey: scope)!,
      );
      expect(await settings.applyCloudSettingsBundle(
        bundle, roomLayoutScopeKey: scope,
        roomLayoutHubKey: RoomPageProvider.hubLayoutKey(target),
        roomLayoutHubKeyAliases: RoomPageProvider.hubLayoutKeyAliases(target),
      ), isTrue);
      provider.reloadLayout();
      provider.reconcileRooms(rooms);
    }

    await transfer({'old-base': 'ha-base', 'old-1': 'ha-1'});
    expect(provider.getPage('ha-1'), 1);
    await transfer({'old-2': 'ha-2'});
    expect(provider.getPage('ha-1'), 1);
    expect(provider.getPage('native-first'), 0);
    expect(provider.getPage('native-second'), 1);
    expect(provider.getPage('native-third'), 2);
    expect(provider.getPage('ha-base'), 0);
    expect(provider.getPage('ha-2'), 0);
    final saved = settings.getRoomPageLayout(scopeKey: scope)!;
    expect(saved[1], ['native-second', 'ha-1']);
    expect(saved.expand((page) => page).toSet(), rooms.map((room) => room.id).toSet());

    // Repeating a mapping moves rather than duplicates that destination room.
    await transfer({'old-1': 'ha-2'});
    expect(provider.getPage('ha-2'), 1);
    expect(provider.getPage('ha-1'), 1);
    final repeated = settings.getRoomPageLayout(scopeKey: scope)!;
    expect(repeated.expand((page) => page).where((id) => id == 'ha-2'), hasLength(1));
  });
}
