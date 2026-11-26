import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/push_registration_service.dart';
import 'package:rhythm_app/services/settings_service.dart';

class _Settings extends Fake implements SettingsService {
  @override
  bool notificationsEnabled = false;
  @override
  bool pushPermissionConsidered = false;
  @override
  String? registeredPushToken;
  @override
  Future<void> setNotificationsEnabled(bool value) async =>
      notificationsEnabled = value;
  @override
  Future<void> markPushPermissionConsidered() async =>
      pushPermissionConsidered = true;
  @override
  Future<void> saveRegisteredPushToken(String? value) async =>
      registeredPushToken = value;
}

class _Client {
  _Client get auth => this;
  ({bool isAnonymous, String id})? currentUser =
      (isAnonymous: false, id: 'fixture');
  final calls = <List<Object>>[];
  Future<void> rpc(String name, {required Map<String, dynamic> params}) async {
    calls.add([name, params]);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('lighting.rhythm.app/push');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  late _Settings settings;
  late _Client client;
  late PushRegistrationService service;
  late List<String> native;
  Map<String, String>? token;
  Map<String, String>? coldTap;
  late StreamController<Object?> accounts;

  setUp(() {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    native = [];
    token = {
      'token': 'abc123',
      'environment': 'development',
      'app_id': 'lighting.rhythm.app'
    };
    coldTap = null;
    settings = _Settings();
    client = _Client();
    accounts = StreamController<Object?>.broadcast();
    messenger.setMockMethodCallHandler(channel, (call) async {
      native.add(call.method);
      return switch (call.method) {
        'takeInitialNotification' => coldTap,
        'requestAuthorization' => true,
        'getToken' => token,
        _ => null
      };
    });
    service = PushRegistrationService.testing(
        settings: settings, client: () => client, channel: channel);
  });
  tearDown(() async {
    await accounts.close();
    messenger.setMockMethodCallHandler(channel, null);
    channel.setMethodCallHandler(null);
    debugDefaultTargetPlatformOverride = null;
  });

  test(
      'requests permission on the first assistant visit and unregisters before logout',
      () async {
    service.start(accountChanges: accounts.stream);
    await Future<void>.delayed(Duration.zero);
    expect(native, ['takeInitialNotification']);
    await service.requestForAssistant();
    await service.requestForAssistant();
    expect(native.where((m) => m == 'requestAuthorization'), hasLength(1));
    expect(client.calls, [
      [
        'register_push_device',
        {
          'p_platform': 'ios',
          'p_environment': 'development',
          'p_token': 'abc123',
          'p_app_id': 'lighting.rhythm.app'
        }
      ]
    ]);
    await service.beforeSignOut();
    expect(client.calls.last, [
      'unregister_push_device',
      {'p_token': 'abc123'}
    ]);
    expect(native.last, 'stopRegistration');
    expect(settings.registeredPushToken, isNull);
  });

  test('an anonymous user and a missing APNs token create no device rows',
      () async {
    client.currentUser = (isAnonymous: true, id: 'anonymous');
    await service.requestForAssistant();
    expect(native, isEmpty);
    expect(settings.pushPermissionConsidered, isFalse);
    client.currentUser = (isAnonymous: false, id: 'owner');
    token = null;
    await service.requestForAssistant();
    expect(client.calls, isEmpty);
    await service.beforeSignOut();
    expect(client.calls, isEmpty);
  });

  test('a token arriving after permission binds to the signed-in resident',
      () async {
    token = null;
    service.start(accountChanges: accounts.stream);
    await service.requestForAssistant();
    final changed = const StandardMethodCodec().encodeMethodCall(
        const MethodCall('tokenChanged', {
      'token': '999a',
      'environment': 'production',
      'app_id': 'lighting.rhythm.app'
    }));
    final done = Completer<void>();
    await messenger.handlePlatformMessage(
        channel.name, changed, (_) => done.complete());
    await done.future;
    expect(settings.registeredPushToken, '999a');
    expect(client.calls.single.last, {
      'p_platform': 'ios',
      'p_token': '999a',
      'p_environment': 'production',
      'p_app_id': 'lighting.rhythm.app',
    });
    await service.beforeSignOut();
    token = {
      'token': 'abcd',
      'environment': 'production',
      'app_id': 'lighting.rhythm.app'
    };
    await service.restoreAfterFailedSignOut();
    expect(settings.registeredPushToken, 'abcd');
  });

  test('restores registration without prompting and drains a cold tap once',
      () async {
    settings.notificationsEnabled = true;
    settings.registeredPushToken = 'retired';
    coldTap = {'hub_id': 'selected-hub', 'kind': 'plan'};
    service.start(accountChanges: accounts.stream);
    await Future<void>.delayed(Duration.zero);
    expect(service.takePendingAttentionHubId(), 'selected-hub');
    expect(service.takePendingAttentionHubId(), isNull);
    expect(native, isNot(contains('requestAuthorization')));
    expect(client.calls.last, [
      'unregister_push_device',
      {'p_token': 'retired'}
    ]);
    expect(settings.registeredPushToken, 'abc123');
    await service.setEnabled(false);
    await service.requestForAssistant();
    expect(settings.notificationsEnabled, isFalse);
    expect(native.where((m) => m == 'requestAuthorization'), isEmpty);
  });
}
