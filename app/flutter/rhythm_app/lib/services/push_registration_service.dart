import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../backend/backend.dart';
import 'settings_service.dart';

/// Push wakes the app. All queue reads and decisions still go to the selected hub.
class PushRegistrationService extends ChangeNotifier {
  PushRegistrationService._(this._settings, this._client, this._channel);

  static final instance = PushRegistrationService._(
    SettingsService.instance,
    () {
      if (!BackendProvider.isInitialized) return null;
      final auth = BackendProvider.instance.auth;
      return auth is SupabaseAuthBackend ? auth.client : null;
    },
    const MethodChannel('lighting.rhythm.app/push'),
  );

  @visibleForTesting
  PushRegistrationService.testing({
    required SettingsService settings,
    required dynamic Function() client,
    required MethodChannel channel,
  }) : this._(settings, client, channel);

  static bool get supported =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.iOS;
  final SettingsService _settings;
  final dynamic Function() _client;
  final MethodChannel _channel;
  final _attention = StreamController<String>.broadcast(sync: true);
  Stream<String> get attentionHubIds => _attention.stream;
  String? _pendingHub;
  bool _started = false;
  Future<void> _tail = Future.value();
  String? _leavingUser;
  bool get enabled => _settings.notificationsEnabled;

  String? takePendingAttentionHubId() {
    final hub = _pendingHub;
    _pendingHub = null;
    return hub;
  }

  String? get _user {
    final user = _client()?.auth.currentUser;
    return user == null || user.isAnonymous == true ? null : user.id as String?;
  }

  // Serialize account changes, token rotations and preference changes in tap order.
  // Cloud failures must leave local approval available.
  Future<void> _enqueue(Future<void> Function() work) {
    final next = _tail.then((_) => work()).catchError((Object _) {
      debugPrint('Assistant notification registration is unavailable');
    });
    return _tail = next;
  }

  void start({required Stream<Object?> accountChanges}) {
    if (_started || !supported) return;
    _started = true;
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'tokenChanged') {
        await _enqueue(() => _register(call.arguments));
      } else if (call.method == 'attention') {
        _receive(call.arguments);
      }
    });
    unawaited(_enqueue(() async {
      _receive(await _channel.invokeMethod('takeInitialNotification'));
      await _restore();
    }));
    accountChanges.listen((_) {
      if (_user != _leavingUser) _leavingUser = null;
      unawaited(_enqueue(_restore));
    });
  }

  void _receive(dynamic data) {
    if (data is! Map || !enabled || _user == null) return;
    final hub = data['hub_id'];
    if (hub is! String || hub.isEmpty) return;
    if (_attention.hasListener) {
      _attention.add(hub);
    } else {
      _pendingHub = hub;
    }
  }

  Future<void> _restore() async {
    final user = _user;
    if (!enabled || user == null || user == _leavingUser) return;
    await _channel.invokeMethod('restoreRegistration');
    await _register(await _channel.invokeMethod('getToken'));
  }

  Future<void> requestForAssistant() => _enqueue(() async {
        if (!supported || _user == null || _settings.pushPermissionConsidered) {
          return;
        }
        await _setEnabled(true);
      });

  Future<void> setEnabled(bool value) => _enqueue(() => _setEnabled(value));

  Future<void> _setEnabled(bool value) async {
    final user = _user;
    if (!supported || user == null || user == _leavingUser) return;
    await _settings.markPushPermissionConsidered();
    if (!value) {
      await _settings.setNotificationsEnabled(false);
      notifyListeners();
      await _unregister();
      return;
    }
    final granted =
        await _channel.invokeMethod<bool>('requestAuthorization') ?? false;
    if (user != _user || user == _leavingUser) return;
    await _settings.setNotificationsEnabled(granted);
    notifyListeners();
    if (granted) await _register(await _channel.invokeMethod('getToken'));
  }

  Future<void> _register(dynamic data) async {
    final user = _user;
    if (!enabled || user == null || user == _leavingUser || data is! Map) {
      return;
    }
    final token = data['token'];
    final environment = data['environment'];
    final appId = data['app_id'];
    if (token is! String ||
        token.isEmpty ||
        appId is! String ||
        appId.isEmpty ||
        !const {'development', 'production'}.contains(environment)) {
      return;
    }
    final previous = _settings.registeredPushToken;
    final client = _client();
    await client.rpc('register_push_device', params: {
      'p_platform': 'ios',
      'p_token': token,
      'p_environment': environment,
      'p_app_id': appId,
    }).timeout(const Duration(seconds: 8));
    if (_user != user) return;
    await _settings.saveRegisteredPushToken(token);
    if (previous != null && previous != token) {
      await client.rpc('unregister_push_device',
          params: {'p_token': previous}).timeout(const Duration(seconds: 5));
    }
  }

  Future<void> _unregister() async {
    final token = _settings.registeredPushToken;
    try {
      if (token != null && _user != null) {
        await _client().rpc('unregister_push_device',
            params: {'p_token': token}).timeout(const Duration(seconds: 5));
        await _settings.saveRegisteredPushToken(null);
      }
    } finally {
      _pendingHub = null;
      await _channel.invokeMethod('stopRegistration');
    }
  }

  Future<void> restoreAfterFailedSignOut() {
    if (!supported) return Future.value();
    _leavingUser = null;
    return _enqueue(_restore);
  }

  Future<void> beforeSignOut() {
    if (!supported) return Future.value();
    _leavingUser = _user;
    return _enqueue(_unregister);
  }
}
