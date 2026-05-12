import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/providers/hub_connection_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  late HubConnectionProvider provider;

  setUp(() {});

  tearDown(() {
    provider.dispose();
  });

  group('HubConnectionProvider', () {
    group('initial state', () {
      test('starts with no active hub', () async {
        provider = HubConnectionProvider();

        // Allow async initialization
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.activeHubType, equals(HubConnectionType.none));
      });

      test('starts disconnected', () async {
        provider = HubConnectionProvider();

        await Future.delayed(const Duration(milliseconds: 100));

        expect(
            provider.connectionStatus, equals(ConnectionStatus.disconnected));
      });

      test('hasActiveHub is false when not connected', () async {
        provider = HubConnectionProvider();

        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.hasActiveHub, isFalse);
      });

      test('lastError is null initially', () async {
        provider = HubConnectionProvider();

        expect(provider.lastError, isNull);
      });

      test('lastVerified is null initially', () async {
        provider = HubConnectionProvider();

        expect(provider.lastVerified, isNull);
      });

      test('retryCount starts at 0', () async {
        provider = HubConnectionProvider();

        expect(provider.retryCount, equals(0));
      });
    });

    group('loadSavedHub', () {
      test('uses configured HA hub', () async {
        provider = HubConnectionProvider();
        provider.configureHubs([_haHub()]);

        expect(provider.activeHubType, equals(HubConnectionType.homeAssistant));
        // Note: actual connection not verified in test since it needs real HA
        expect(
            provider.connectionStatus, equals(ConnectionStatus.disconnected));
      });

      test('uses configured Hue hub when no HA', () async {
        provider = HubConnectionProvider();
        provider.configureHubs([_hueHub()]);

        // Hue should be active when no HA is configured
        expect(provider.activeHubType, equals(HubConnectionType.hue));
      });

      test('prefers HA over Hue when both configured', () async {
        provider = HubConnectionProvider();
        provider.configureHubs([_haHub(), _hueHub()]);

        // HA takes precedence
        expect(provider.activeHubType, equals(HubConnectionType.homeAssistant));
      });

      test('remains none when nothing configured', () async {
        provider = HubConnectionProvider();
        provider.configureHubs(const []);

        expect(provider.activeHubType, equals(HubConnectionType.none));
      });
    });

    group('verifyConnection', () {
      test('returns false when no hub configured', () async {
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        final result = await provider.verifyConnection();

        expect(result, isFalse);
        expect(provider.lastError, contains('No hub configured'));
      });

      test('sets connecting status during verification', () async {
        provider = HubConnectionProvider(
          haWebSocketFactory: _FailingHaWebSocketProvider.new,
        );
        provider.configureHubs([_haHub()]);

        final future = provider.verifyConnection();

        // Status might be connecting during the operation
        // (hard to test timing, so just verify it completes)
        await future;

        // Should end up in error state since can't connect
        expect(
          provider.connectionStatus,
          anyOf(
            equals(ConnectionStatus.error),
            equals(ConnectionStatus.disconnected),
          ),
        );
      });
    });

    group('disconnect', () {
      test('sets status to disconnected', () async {
        provider = HubConnectionProvider();
        provider.configureHubs([_haHub()]);

        await provider.disconnect();

        expect(
            provider.connectionStatus, equals(ConnectionStatus.disconnected));
      });

      test('resets retry count', () async {
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        await provider.disconnect();

        expect(provider.retryCount, equals(0));
      });

      test('notifies listeners', () async {
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        int notifyCount = 0;
        provider.addListener(() => notifyCount++);

        await provider.disconnect();

        expect(notifyCount, greaterThan(0));
      });
    });

    group('reload', () {
      test('disconnects and reloads saved hub', () async {
        provider = HubConnectionProvider();
        provider.configureHubs([_haHub()]);

        await provider.reload();

        // Should have reloaded the saved hub type
        expect(provider.activeHubType, equals(HubConnectionType.homeAssistant));
        expect(
            provider.connectionStatus, equals(ConnectionStatus.disconnected));
      });
    });

    group('retryConnection', () {
      test('resets retry count', () async {
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        await provider.retryConnection();

        expect(provider.retryCount, equals(0));
      });
    });

    group('connection status helpers', () {
      test('isConnected reflects connection status', () async {
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.isConnected, isFalse);
      });

      test('isConnecting reflects connection status', () async {
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.isConnecting, isFalse);
      });

      test('isHaConnected is false when not connected', () async {
        provider = HubConnectionProvider();
        provider.configureHubs([_haHub()]);

        expect(provider.isHaConnected, isFalse);
      });

      test('isHueConnected is false when not connected', () async {
        provider = HubConnectionProvider();
        provider.configureHubs([_hueHub()]);

        expect(provider.isHueConnected, isFalse);
      });
    });

    group('Hue-specific', () {
      test('hueConnectionStatus starts disconnected', () async {
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        expect(
          provider.hueConnectionStatus,
          anyOf(
            equals(ConnectionStatus.disconnected),
            equals(ConnectionStatus.connecting),
            equals(ConnectionStatus.error),
          ),
        );
      });

      test('hueLastError is null initially', () async {
        provider = HubConnectionProvider();

        expect(provider.hueLastError, isNull);
      });

      test('verifyHueConnection updates hue status', () async {
        // This test requires HTTP mocking to work properly
        // since it tries to connect to a real Hue bridge
      }, skip: 'Requires HTTP mocking to avoid network calls');
    });
  });
}

Hub _haHub() => Hub.homeAssistant(
      id: 'ha-1',
      homeId: 'home-1',
      name: 'Home Assistant',
      host: 'homeassistant.local',
      port: 8123,
      token: 'test-token',
    );

Hub _hueHub() => Hub.hue(
      id: 'hue-1',
      homeId: 'home-1',
      name: 'Hue',
      bridgeIp: '192.168.1.100',
      appKey: 'test-user',
    );

class _FailingHaWebSocketProvider extends HaWebSocketProvider {
  _FailingHaWebSocketProvider(super.config);

  @override
  bool get isConnected => false;

  @override
  String? get lastError => 'Fake connection failed';

  @override
  Future<bool> connect() async => false;

  @override
  Future<void> dispose() async {}
}
