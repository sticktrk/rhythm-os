import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/ble_provisioning_screen.dart';
import 'package:rhythm_app/services/account_cloud_sync_service.dart';
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
  });
}
