import 'package:dio/dio.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _TestHomeProvider extends HomeProvider {
  _TestHomeProvider(this._hubs);

  final List<Hub> _hubs;

  @override
  List<Hub> get currentHomeHubs => _hubs;

  @override
  Hub? getFirstHubOfType(HubType type) {
    for (final hub in _hubs) {
      if (hub.type == type) return hub;
    }
    return null;
  }
}

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

  int hubCredentialsCalls = 0;
  String? lastHubType;
  String? lastAddress;
  Map<String, dynamic>? lastCredentials;

  @override
  Future<void> hubCredentials({
    required String hubType,
    required String address,
    required Map<String, dynamic> credentials,
  }) async {
    hubCredentialsCalls++;
    lastHubType = hubType;
    lastAddress = address;
    lastCredentials = credentials;
  }
}

class _FakeRhythmConnection extends RhythmConnection {
  _FakeRhythmConnection(this.fakeApi);

  final _FakeRhythmServerApi fakeApi;
  bool isConnected = true;
  int reconnectCalls = 0;

  @override
  bool get connected => isConnected;

  @override
  RhythmServerApi get api => fakeApi;

  @override
  Future<void> reconnect() async {
    reconnectCalls++;
  }
}

void main() {
  group('ServerSyncProvider.pushHubCredentials', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('pushes Hue credentials and reconnects immediately', () async {
      final homeProvider = _TestHomeProvider([
        Hub.hue(
          id: 'hue-1',
          homeId: 'home-1',
          name: 'Philips Hue',
          bridgeIp: '192.168.1.20',
          appKey: 'hue-user',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      await provider.pushHubCredentials(RoomSourceDto.hue);

      expect(api.hubCredentialsCalls, 1);
      expect(api.lastHubType, 'hue');
      expect(api.lastAddress, '192.168.1.20:443');
      expect(api.lastCredentials, {'username': 'hue-user'});
      expect(connection.reconnectCalls, 1);
    });

    test('pushes Home Assistant credentials with token payload', () async {
      final homeProvider = _TestHomeProvider([
        Hub.homeAssistant(
          id: 'ha-1',
          homeId: 'home-1',
          name: 'Home Assistant',
          host: 'ha.local',
          port: 8123,
          useSsl: false,
          token: 'ha-token',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      await provider.pushHubCredentials(RoomSourceDto.homeAssistant);

      expect(api.hubCredentialsCalls, 1);
      expect(api.lastHubType, 'homeassistant');
      expect(api.lastAddress, 'ha.local:8123');
      expect(api.lastCredentials, {'token': 'ha-token'});
      expect(connection.reconnectCalls, 1);
    });

    test('does nothing when the server is disconnected', () async {
      connection.isConnected = false;
      final homeProvider = _TestHomeProvider([
        Hub.hue(
          id: 'hue-1',
          homeId: 'home-1',
          name: 'Philips Hue',
          bridgeIp: '192.168.1.20',
          appKey: 'hue-user',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      await provider.pushHubCredentials(RoomSourceDto.hue);

      expect(api.hubCredentialsCalls, 0);
      expect(connection.reconnectCalls, 0);
    });
  });
}
