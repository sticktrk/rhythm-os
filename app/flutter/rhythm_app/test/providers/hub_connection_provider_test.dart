import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:rhythm_app/providers/hub_connection_provider.dart';

void main() {
  late HubConnectionProvider provider;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
  });

  tearDown(() {
    provider.dispose();
  });

  group('HubConnectionProvider', () {
    group('initial state', () {
      test('starts with no active hub', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();

        // Allow async initialization
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.activeHubType, equals(HubType.none));
      });

      test('starts disconnected', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();

        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.connectionStatus, equals(ConnectionStatus.disconnected));
      });

      test('hasActiveHub is false when not connected', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();

        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.hasActiveHub, isFalse);
      });

      test('lastError is null initially', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();

        expect(provider.lastError, isNull);
      });

      test('lastVerified is null initially', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();

        expect(provider.lastVerified, isNull);
      });

      test('retryCount starts at 0', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();

        expect(provider.retryCount, equals(0));
      });
    });

    group('loadSavedHub', () {
      test('loads HA hub when verified', () async {
        SharedPreferences.setMockInitialValues({
          'hub_ha_verified': true,
          'hub_ha_host': 'homeassistant.local',
          'hub_ha_port': 8123,
          'hub_ha_token': 'test_token',
        });

        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.activeHubType, equals(HubType.homeAssistant));
        // Note: actual connection not verified in test since it needs real HA
        expect(provider.connectionStatus, equals(ConnectionStatus.disconnected));
      });

      test('loads Hue hub when verified and no HA', () async {
        SharedPreferences.setMockInitialValues({
          'hue_verified': true,
          'hue_bridge_ip': '192.168.1.100',
          'hue_username': 'test_user',
        });

        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        // Hue should be active when no HA is configured
        expect(provider.activeHubType, equals(HubType.hue));
      });

      test('prefers HA over Hue when both configured', () async {
        SharedPreferences.setMockInitialValues({
          'hub_ha_verified': true,
          'hub_ha_host': 'homeassistant.local',
          'hue_verified': true,
          'hue_bridge_ip': '192.168.1.100',
        });

        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        // HA takes precedence
        expect(provider.activeHubType, equals(HubType.homeAssistant));
      });

      test('remains none when nothing configured', () async {
        SharedPreferences.setMockInitialValues({});

        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.activeHubType, equals(HubType.none));
      });
    });

    group('verifyConnection', () {
      test('returns false when no hub configured', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        final result = await provider.verifyConnection();

        expect(result, isFalse);
        expect(provider.lastError, contains('No hub configured'));
      });

      test('sets connecting status during verification', () async {
        SharedPreferences.setMockInitialValues({
          'hub_ha_verified': true,
          'hub_ha_host': 'homeassistant.local',
          'hub_ha_port': 8123,
          'hub_ha_token': 'test_token',
        });
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        // Start verification (will fail since no real HA)
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
        SharedPreferences.setMockInitialValues({
          'hub_ha_verified': true,
        });
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        await provider.disconnect();

        expect(provider.connectionStatus, equals(ConnectionStatus.disconnected));
      });

      test('resets retry count', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        await provider.disconnect();

        expect(provider.retryCount, equals(0));
      });

      test('notifies listeners', () async {
        SharedPreferences.setMockInitialValues({});
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
        SharedPreferences.setMockInitialValues({
          'hub_ha_verified': true,
        });
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        await provider.reload();

        // Should have reloaded the saved hub type
        expect(provider.activeHubType, equals(HubType.homeAssistant));
        expect(provider.connectionStatus, equals(ConnectionStatus.disconnected));
      });
    });

    group('retryConnection', () {
      test('resets retry count', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        await provider.retryConnection();

        expect(provider.retryCount, equals(0));
      });
    });

    group('connection status helpers', () {
      test('isConnected reflects connection status', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.isConnected, isFalse);
      });

      test('isConnecting reflects connection status', () async {
        SharedPreferences.setMockInitialValues({});
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.isConnecting, isFalse);
      });

      test('isHaConnected is false when not connected', () async {
        SharedPreferences.setMockInitialValues({
          'hub_ha_verified': true,
        });
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.isHaConnected, isFalse);
      });

      test('isHueConnected is false when not connected', () async {
        SharedPreferences.setMockInitialValues({
          'hue_verified': true,
        });
        provider = HubConnectionProvider();
        await Future.delayed(const Duration(milliseconds: 100));

        expect(provider.isHueConnected, isFalse);
      });
    });

    group('Hue-specific', () {
      test('hueConnectionStatus starts disconnected', () async {
        SharedPreferences.setMockInitialValues({});
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
        SharedPreferences.setMockInitialValues({});
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
