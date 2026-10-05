import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/data/local_data_source.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/repositories/home_repository.dart';
import 'package:rhythm_app/services/hue/hue_service_locator.dart';
import 'package:rhythm_app/services/settings_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const pathProviderChannel = MethodChannel('plugins.flutter.io/path_provider');
  late Directory testStorage;
  late HomeRepository repository;

  setUpAll(() async {
    testStorage = await Directory.systemTemp.createTemp(
      'rhythm-demo-relaunch-test-',
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
    repository = HomeRepository(localDataSource: localDataSource);
  });

  tearDownAll(() async {
    await LocalDataSource().close();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(pathProviderChannel, null);
    await testStorage.delete(recursive: true);
  });

  setUp(() async {
    HueServiceLocator.setDemoMode(false);
    await LocalDataSource().clearHomesAndHubs();
    await LocalDataSource().clearSettings();
  });

  tearDown(() async {
    HueServiceLocator.setDemoMode(false);
    await LocalDataSource().clearHomesAndHubs();
    await LocalDataSource().clearSettings();
  });

  // What a Virtual Experience session leaves on disk: the seeded Demo Home
  // selected as current, holding the fake loopback server hub.
  Future<Home> persistDemoSession() async {
    final home = await repository.createHome(
      name: 'Demo Home',
      ownerId: 'demo-user',
    );
    await repository.createServerHub(
      homeId: home.id,
      name: 'Demo Server',
      host: '127.0.0.1',
      port: 54448,
    );
    await SettingsService.instance.setSelectedHomeId(home.id);
    return home;
  }

  // Providers register demo hooks on a static locator and never leave it, so
  // this is the only test that enables demo mode and it runs first.
  test('startup in demo mode keeps the demo Home', () async {
    final home = await persistDemoSession();
    HueServiceLocator.setDemoMode(true);

    final provider = HomeProvider();
    await provider.initialize();

    expect(provider.removedStaleDemoEnvironment, isFalse);
    expect(provider.currentHome?.id, home.id);
    expect(provider.activeServerHub?.endpoint.host, '127.0.0.1');
    expect(DirectHubAccess.allowed, isFalse);

    final hue = await provider.addHueHub(
      name: 'Demo Hue',
      bridgeIp: 'demo-hue.local',
      appKey: 'demo-key',
    );
    expect(hue, isNotNull);
    expect(
      await provider.updateHub(hue!.copyWith(token: 'replacement-demo-key')),
      isTrue,
    );
    expect(
        provider.getFirstHubOfType(HubType.hue)?.token, 'replacement-demo-key');
    expect(DirectHubAccess.allowed, isFalse);
  });

  test('relaunch outside demo mode drops the persisted demo Home', () async {
    await persistDemoSession();

    final provider = HomeProvider();
    await provider.initialize();

    expect(provider.removedStaleDemoEnvironment, isTrue);
    expect(provider.homes, isEmpty);
    expect(provider.currentHome, isNull);
    expect(provider.activeServerHub, isNull);
    expect(repository.getAllHomes(), isEmpty);
    expect(repository.getAllHubs(), isEmpty);
  });

  test('relaunch keeps a real Home next to the stale demo Home', () async {
    await persistDemoSession();
    final real = await repository.createHome(name: 'Cabin', ownerId: 'user-1');
    await repository.createServerHub(
      homeId: real.id,
      name: 'Rhythm Box',
      host: '192.168.5.10',
      port: 54448,
    );

    final provider = HomeProvider();
    await provider.initialize();

    expect(provider.removedStaleDemoEnvironment, isTrue);
    expect(provider.homes.map((home) => home.id), [real.id]);
    expect(provider.currentHome?.id, real.id);
    expect(provider.activeServerHub?.endpoint.host, '192.168.5.10');
  });

  test('startup without demo data reports nothing removed', () async {
    final provider = HomeProvider();
    await provider.initialize();

    expect(provider.removedStaleDemoEnvironment, isFalse);
    expect(provider.homes, isEmpty);
  });
}
