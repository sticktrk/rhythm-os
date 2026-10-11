import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/data/local_data_source.dart';
import 'package:rhythm_app/services/settings_service.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late Directory directory;
  final data = LocalDataSource();
  final settings = SettingsService.instance;
  const channel = MethodChannel('plugins.flutter.io/path_provider');
  final preference = <String, Object>{
    'schema_version': 1,
    'enabled': true,
    'scope': {
      'account_id': 'owner',
      'home_id': 'home',
      'server_id': 'box',
      'node_id': 'room'
    }
  };

  setUpAll(() async {
    directory =
        await Directory.systemTemp.createTemp('rhythm-health-settings-');
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (_) async => directory.path);
    await data.initialize();
    await data.markMigrationComplete();
    await settings.initialize(localDataSource: data);
  });
  tearDownAll(() async {
    await data.close();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
    await directory.delete(recursive: true);
  });

  test(
      'legacy writers preserve separate consent; portable settings never contain it',
      () async {
    expect(settings.appleHealthPreference, isNull);
    await settings.setAppleHealthPreference(preference);
    await data.saveSettings(data.getSettings());
    expect(settings.appleHealthPreference, preference);
    expect(data.getSettings().runnerStateJson, isNull);
    expect(settings.buildCloudSettingsBundle().toString(),
        isNot(contains('apple_health')));
    await data.close();
    await data.initialize();
    expect(settings.appleHealthPreference, preference);
  });

  test('reset, old-version clearSettings and logout erase device consent',
      () async {
    await settings.setAppleHealthPreference(preference);
    await settings.resetToDefaults();
    expect(settings.appleHealthPreference, isNull);
    await settings.setAppleHealthPreference(preference);
    await data.clearSettings();
    expect(settings.appleHealthPreference, isNull);
    await settings.setAppleHealthPreference(preference);
    await settings.clearAll();
    expect(settings.appleHealthPreference, isNull);
  });
}
