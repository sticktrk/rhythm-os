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

    test('treats restart status as terminal after device-driven update',
        () async {
      final status =
          await BleProvisioningService.waitForTerminalProvisioningStatus(
        enableNotifications: () async {},
        writePayload: () async {},
        statusUpdates: Stream<ProvisioningStatusMessage>.value(
          const ProvisioningStatusMessage(
            status: 'restarting',
            ip: '192.168.1.154',
            otaStage: 'restarting',
            message: 'Updated, restarting',
          ),
        ),
        readStatus: () async =>
            const ProvisioningStatusMessage(status: 'updating'),
        timeout: const Duration(seconds: 1),
        pollInterval: const Duration(milliseconds: 50),
      );

      expect(status.status, 'restarting');
      expect(status.ip, '192.168.1.154');
      expect(status.message, 'Updated, restarting');
    });

    test('reports intermediate update statuses to caller', () async {
      final seen = <String>[];

      final status =
          await BleProvisioningService.waitForTerminalProvisioningStatus(
        enableNotifications: () async {},
        writePayload: () async {},
        statusUpdates: Stream<ProvisioningStatusMessage>.fromIterable(const [
          ProvisioningStatusMessage(
            status: 'updating',
            ip: '192.168.1.155',
            otaStage: 'downloading',
            message: 'Downloading update',
          ),
          ProvisioningStatusMessage(
            status: 'restarting',
            ip: '192.168.1.155',
            otaStage: 'restarting',
            message: 'Updated, restarting',
          ),
        ]),
        readStatus: () async =>
            const ProvisioningStatusMessage(status: 'updating'),
        onStatus: (update) {
          final message = update.message;
          if (message != null) seen.add(message);
        },
        timeout: const Duration(seconds: 1),
        pollInterval: const Duration(milliseconds: 50),
      );

      expect(status.status, 'restarting');
      expect(seen, contains('Downloading update'));
      expect(seen, contains('Updated, restarting'));
    });

    test('treats up-to-date update status as terminal', () async {
      final status =
          await BleProvisioningService.waitForTerminalProvisioningStatus(
        enableNotifications: () async {},
        writePayload: () async {},
        statusUpdates: Stream<ProvisioningStatusMessage>.value(
          const ProvisioningStatusMessage(
            status: 'updating',
            ip: '192.168.1.157',
            otaStage: 'up_to_date',
            message: 'Device is already on the latest stable update',
          ),
        ),
        readStatus: () async =>
            const ProvisioningStatusMessage(status: 'updating'),
        timeout: const Duration(seconds: 1),
        pollInterval: const Duration(milliseconds: 50),
      );

      expect(status.isConnected, isTrue);
      expect(status.isTerminal, isTrue);
      expect(status.ip, '192.168.1.157');
    });

    test('treats BLE disconnect after update progress as restart pending',
        () async {
      final seen = <String>[];

      final status =
          await BleProvisioningService.waitForTerminalProvisioningStatus(
        enableNotifications: () async {},
        writePayload: () async {},
        statusUpdates: Stream<ProvisioningStatusMessage>.value(
          const ProvisioningStatusMessage(
            status: 'updating',
            ip: '192.168.1.156',
            otaStage: 'installing',
            message: 'Installing update',
          ),
        ),
        readStatus: () async {
          await Future<void>.delayed(const Duration(milliseconds: 1));
          throw PlatformException(
            code: 'device_disconnected',
            message: 'device is disconnected',
          );
        },
        onStatus: (update) {
          final message = update.message;
          if (message != null) seen.add(message);
        },
        timeout: const Duration(seconds: 1),
        pollInterval: const Duration(milliseconds: 50),
      );

      expect(status.status, 'restarting');
      expect(status.ip, '192.168.1.156');
      expect(status.otaStage, 'restarting');
      expect(seen, contains('Installing update'));
    });

    test('does not hide BLE disconnect before update progress', () async {
      final updates = StreamController<ProvisioningStatusMessage>();
      addTearDown(updates.close);

      await expectLater(
        BleProvisioningService.waitForTerminalProvisioningStatus(
          enableNotifications: () async {},
          writePayload: () async {},
          statusUpdates: updates.stream,
          readStatus: () async {
            throw PlatformException(
              code: 'device_disconnected',
              message: 'device is disconnected',
            );
          },
          timeout: const Duration(seconds: 1),
          pollInterval: const Duration(milliseconds: 50),
        ),
        throwsA(isA<PlatformException>()),
      );
    });

    test('can wait for auth token status', () async {
      var wrotePayload = false;

      final status =
          await BleProvisioningService.waitForMatchingProvisioningStatus(
        enableNotifications: () async {},
        writePayload: () async {
          wrotePayload = true;
        },
        statusUpdates: Stream<ProvisioningStatusMessage>.value(
          const ProvisioningStatusMessage(
            status: 'auth_token',
            ownerToken: 'rhythm_owner_test',
          ),
        ),
        readStatus: () async =>
            const ProvisioningStatusMessage(status: 'waiting'),
        isMatch: (update) => update.status == 'auth_token',
        timeoutMessage: 'Timed out waiting for owner token',
        timeout: const Duration(seconds: 1),
        pollInterval: const Duration(milliseconds: 50),
      );

      expect(wrotePayload, isTrue);
      expect(status.status, 'auth_token');
      expect(status.ownerToken, 'rhythm_owner_test');
    });
  });
}
