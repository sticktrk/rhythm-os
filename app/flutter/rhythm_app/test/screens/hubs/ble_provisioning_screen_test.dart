import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/ble_provisioning_screen.dart';
import 'package:rhythm_app/services/account_cloud_sync_service.dart';
import 'package:rhythm_app/services/ble_provisioning_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('bleProvisioningSupportsPlatformForTesting', () {
    test('supports Android native provisioning', () {
      expect(
        bleProvisioningSupportsPlatformForTesting(
          platform: TargetPlatform.android,
        ),
        isTrue,
      );
    });

    test('does not advertise BLE provisioning on web', () {
      expect(
        bleProvisioningSupportsPlatformForTesting(
          platform: TargetPlatform.android,
          isWeb: true,
        ),
        isFalse,
      );
    });

    test('supports iOS and macOS but not desktop platforms without BLE setup',
        () {
      expect(
        bleProvisioningSupportsPlatformForTesting(
          platform: TargetPlatform.iOS,
        ),
        isTrue,
      );
      expect(
        bleProvisioningSupportsPlatformForTesting(
          platform: TargetPlatform.macOS,
        ),
        isTrue,
      );
      expect(
        bleProvisioningSupportsPlatformForTesting(
          platform: TargetPlatform.linux,
        ),
        isFalse,
      );
      expect(
        bleProvisioningSupportsPlatformForTesting(
          platform: TargetPlatform.windows,
        ),
        isFalse,
      );
    });
  });

  group('bleProvisioningShowsUpdateStatusForTesting', () {
    test('keeps update UI visible after Wi-Fi handoff', () {
      expect(
        bleProvisioningShowsUpdateStatusForTesting(
          const ProvisioningStatusMessage(
            status: 'updating',
            ip: '192.168.1.155',
            otaStage: 'checking',
            message: 'Checking for stable update',
          ),
        ),
        isTrue,
      );
    });

    test('uses normal provisioning UI for a connected status', () {
      expect(
        bleProvisioningShowsUpdateStatusForTesting(
          const ProvisioningStatusMessage(
            status: 'connected',
            ip: '192.168.1.155',
          ),
        ),
        isFalse,
      );
    });
  });

  group('bleProvisioningKnownHomeNameForTesting', () {
    test('matches a specific BLE device name to a saved server hub home', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Lake House',
        ownerId: 'user-1',
      );
      final hub = Hub.server(
        id: 'hub-1',
        homeId: home.id,
        name: 'rhythm-rpiz-A1B2',
        host: '192.168.1.20',
      );

      final homeName = bleProvisioningKnownHomeNameForTesting(
        deviceName: 'Rhythm RPIZ A1B2',
        homeEntries: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
      );

      expect(homeName, 'Lake House');
    });

    test('does not match generic Rhythm Box names', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Lake House',
        ownerId: 'user-1',
      );
      final hub = Hub.server(
        id: 'hub-1',
        homeId: home.id,
        name: 'Rhythm Box',
        host: '192.168.1.20',
      );

      final homeName = bleProvisioningKnownHomeNameForTesting(
        deviceName: 'Rhythm Box',
        homeEntries: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
      );

      expect(homeName, isNull);
    });

    test('normalizes punctuation and casing in specific device names', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Town House',
        ownerId: 'user-1',
      );
      final hub = Hub.server(
        id: 'hub-1',
        homeId: home.id,
        name: 'Rhythm-RPIZ-0a:b2',
        host: '192.168.1.20',
      );

      expect(
        bleProvisioningKnownHomeNameForTesting(
          deviceName: '  RHYTHM rpiz 0A B2  ',
          homeEntries: [
            AccountHomeServerHubs(home: home, serverHubs: [hub]),
          ],
        ),
        'Town House',
      );
    });

    test('ignores empty names and non-server hubs', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Town House',
        ownerId: 'user-1',
      );
      final hueHub = Hub.hue(
        id: 'hub-1',
        homeId: home.id,
        name: 'rhythm-rpiz-A1B2',
        bridgeIp: '192.168.1.20',
        appKey: 'key',
      );
      final entries = [
        AccountHomeServerHubs(home: home, serverHubs: [hueHub]),
      ];

      expect(
        bleProvisioningKnownHomeNameForTesting(
          deviceName: '',
          homeEntries: entries,
        ),
        isNull,
      );
      expect(
        bleProvisioningKnownHomeNameForTesting(
          deviceName: 'rhythm-rpiz-A1B2',
          homeEntries: entries,
        ),
        isNull,
      );
    });
  });

  group('bleProvisioningWaitForServerHealthForTesting', () {
    test('times out when health probes never complete', () async {
      var probes = 0;
      final elapsed = Stopwatch()..start();

      final healthy = await bleProvisioningWaitForServerHealthForTesting(
        healthCheck: () {
          probes += 1;
          return Completer<bool>().future;
        },
        timeout: const Duration(milliseconds: 70),
        pollInterval: const Duration(milliseconds: 5),
        probeTimeout: const Duration(milliseconds: 20),
      );
      elapsed.stop();

      expect(healthy, isFalse);
      expect(probes, greaterThan(0));
      expect(elapsed.elapsed, lessThan(const Duration(milliseconds: 250)));
    });

    test('keeps polling until a health probe succeeds', () async {
      var probes = 0;

      final healthy = await bleProvisioningWaitForServerHealthForTesting(
        healthCheck: () async {
          probes += 1;
          return probes == 3;
        },
        timeout: const Duration(milliseconds: 200),
        pollInterval: const Duration(milliseconds: 5),
        probeTimeout: const Duration(milliseconds: 20),
      );

      expect(healthy, isTrue);
      expect(probes, 3);
    });

    test('treats thrown probe errors as unhealthy and keeps polling', () async {
      var probes = 0;

      final healthy = await bleProvisioningWaitForServerHealthForTesting(
        healthCheck: () async {
          probes += 1;
          if (probes < 3) throw StateError('server is restarting');
          return true;
        },
        timeout: const Duration(milliseconds: 200),
        pollInterval: const Duration(milliseconds: 5),
        probeTimeout: const Duration(milliseconds: 20),
      );

      expect(healthy, isTrue);
      expect(probes, 3);
    });

    test('returns false without probing when the timeout is zero', () async {
      var probes = 0;

      final healthy = await bleProvisioningWaitForServerHealthForTesting(
        healthCheck: () async {
          probes += 1;
          return true;
        },
        timeout: Duration.zero,
        pollInterval: Duration.zero,
        probeTimeout: const Duration(milliseconds: 20),
      );

      expect(healthy, isFalse);
      expect(probes, 0);
    });
  });

  group('bleProvisioningWaitForServerRestartAndHealthForTesting', () {
    test('does not hang when the offline probe never completes', () async {
      var probes = 0;

      final healthy =
          await bleProvisioningWaitForServerRestartAndHealthForTesting(
        healthCheck: () {
          probes += 1;
          if (probes == 1) {
            return Completer<bool>().future;
          }
          return Future<bool>.value(true);
        },
        offlineTimeout: const Duration(milliseconds: 60),
        onlineTimeout: const Duration(milliseconds: 100),
        pollInterval: const Duration(milliseconds: 5),
        probeTimeout: const Duration(milliseconds: 20),
      );

      expect(healthy, isTrue);
      expect(probes, 2);
    });

    test('waits for the server to return after observing it offline', () async {
      var probes = 0;

      final healthy =
          await bleProvisioningWaitForServerRestartAndHealthForTesting(
        healthCheck: () async {
          probes += 1;
          return switch (probes) {
            1 => true,
            2 => false,
            3 => false,
            _ => true,
          };
        },
        offlineTimeout: const Duration(milliseconds: 100),
        onlineTimeout: const Duration(milliseconds: 100),
        pollInterval: const Duration(milliseconds: 5),
        probeTimeout: const Duration(milliseconds: 20),
      );

      expect(healthy, isTrue);
      expect(probes, 4);
    });

    test('accepts a continuously healthy server after the offline window',
        () async {
      var probes = 0;

      final healthy =
          await bleProvisioningWaitForServerRestartAndHealthForTesting(
        healthCheck: () async {
          probes += 1;
          return true;
        },
        offlineTimeout: const Duration(milliseconds: 35),
        onlineTimeout: const Duration(milliseconds: 100),
        pollInterval: const Duration(milliseconds: 5),
        probeTimeout: const Duration(milliseconds: 20),
      );

      expect(healthy, isTrue);
      expect(probes, greaterThan(1));
    });
  });

  group('bleProvisioningResolveVerifiedOwnerTokenForTesting', () {
    test('keeps a BLE-delivered token only after owner authentication',
        () async {
      var claims = 0;

      final token = await bleProvisioningResolveVerifiedOwnerTokenForTesting(
        candidates: const ['ble-owner-token'],
        verifyToken: (candidate) async => candidate == 'ble-owner-token',
        claimToken: () async {
          claims += 1;
          return 'claimed-owner-token';
        },
      );

      expect(token, 'ble-owner-token');
      expect(claims, 0);
    });

    test('reclaims and verifies when the BLE-delivered token is invalid',
        () async {
      final verified = <String>[];

      final token = await bleProvisioningResolveVerifiedOwnerTokenForTesting(
        candidates: const ['truncated-ble-token'],
        verifyToken: (candidate) async {
          verified.add(candidate);
          return candidate == 'claimed-owner-token';
        },
        claimToken: () async => 'claimed-owner-token',
      );

      expect(token, 'claimed-owner-token');
      expect(verified, ['truncated-ble-token', 'claimed-owner-token']);
    });

    test('rejects an unverified replacement instead of persisting it',
        () async {
      await expectLater(
        bleProvisioningResolveVerifiedOwnerTokenForTesting(
          candidates: const ['invalid-ble-token'],
          verifyToken: (_) async => false,
          claimToken: () async => 'invalid-replacement-token',
        ),
        throwsA(
          isA<StateError>().having(
            (error) => error.message,
            'message',
            contains('could not be authenticated'),
          ),
        ),
      );
    });
  });

  group('bleProvisioningAutoContinueForTesting', () {
    test('shows success feedback and returns to Connect Hub', () async {
      final events = <String>[];

      await bleProvisioningAutoContinueForTesting(
        showSuccess: () => events.add('success'),
        waitForSuccessFeedback: () async => events.add('waited'),
        isMounted: () => true,
        continueToConnectHub: () => events.add('continued'),
      );

      expect(events, ['success', 'waited', 'continued']);
    });

    test('does not navigate after the provisioning screen is disposed',
        () async {
      var continued = false;

      await bleProvisioningAutoContinueForTesting(
        showSuccess: () {},
        waitForSuccessFeedback: () async {},
        isMounted: () => false,
        continueToConnectHub: () => continued = true,
      );

      expect(continued, isFalse);
    });
  });

  testWidgets('unsupported desktop build shows the recoverable error UI',
      (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);

    await tester.pumpWidget(
      const MaterialApp(home: BleProvisioningScreen()),
    );
    await tester.pump();

    expect(find.text('Set Up New Box'), findsOneWidget);
    expect(find.text('Something went wrong'), findsOneWidget);
    expect(
      find.text(
        'Device setup is only available on the native mobile and desktop builds.',
      ),
      findsOneWidget,
    );
    expect(find.text('Try Again'), findsOneWidget);
    expect(find.text('Back'), findsOneWidget);
    expect(find.byIcon(Icons.error_outline_rounded), findsOneWidget);

    debugDefaultTargetPlatformOverride = null;
  });
}
