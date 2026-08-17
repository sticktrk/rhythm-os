import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/data/local_data_source.dart';
import 'package:rhythm_app/services/settings_service.dart';

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
}
