import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/ble_provisioning_service.dart';

void main() {
  group('BleProvisioningService', () {
    test('polls status when notification setup fails', () async {
      var wrotePayload = false;
      var readCount = 0;

      final status =
          await BleProvisioningService.waitForTerminalProvisioningStatus(
        enableNotifications: () async {
          throw PlatformException(
            code: 'setNotifyValue',
            message:
                "primary service not found '72797468-6d00-1000-8000-00805f9b34fb'",
          );
        },
        writePayload: () async {
          wrotePayload = true;
        },
        statusUpdates: Stream<ProvisioningStatusMessage>.empty(),
        readStatus: () async {
          readCount += 1;
          if (readCount == 1) {
            return const ProvisioningStatusMessage(status: 'connecting');
          }
          return const ProvisioningStatusMessage(
            status: 'connected',
            ip: '192.168.1.152',
          );
        },
        timeout: const Duration(seconds: 1),
        pollInterval: const Duration(milliseconds: 1),
      );

      expect(wrotePayload, isTrue);
      expect(readCount, 2);
      expect(status.status, 'connected');
      expect(status.ip, '192.168.1.152');
    });

    test('uses notification terminal status when available', () async {
      var notificationsEnabled = false;
      var wrotePayload = false;

      final status =
          await BleProvisioningService.waitForTerminalProvisioningStatus(
        enableNotifications: () async {
          notificationsEnabled = true;
        },
        writePayload: () async {
          wrotePayload = true;
        },
        statusUpdates: Stream<ProvisioningStatusMessage>.value(
          const ProvisioningStatusMessage(
            status: 'connected',
            ip: '192.168.1.153',
          ),
        ),
        readStatus: () async =>
            const ProvisioningStatusMessage(status: 'connecting'),
        timeout: const Duration(seconds: 1),
        pollInterval: const Duration(milliseconds: 50),
      );

      expect(notificationsEnabled, isTrue);
      expect(wrotePayload, isTrue);
      expect(status.status, 'connected');
      expect(status.ip, '192.168.1.153');
    });
  });
}
