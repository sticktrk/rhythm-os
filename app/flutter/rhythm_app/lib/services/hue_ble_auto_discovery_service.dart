import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter_blue_plus/flutter_blue_plus.dart';
import 'package:permission_handler/permission_handler.dart';

import 'analytics_service.dart';

const _hueBleDiscoveryServiceUuid = '0000fe0f-0000-1000-8000-00805f9b34fb';

enum HueBleDiscoveryOutcome {
  found,
  none,
  permissionDenied,
  bluetoothUnavailable,
  unsupported,
  failed,
}

class HueBleDiscoveryResult {
  const HueBleDiscoveryResult(this.outcome, {this.deviceCount = 0});

  final HueBleDiscoveryOutcome outcome;
  final int deviceCount;

  bool get found => outcome == HueBleDiscoveryOutcome.found;
}

typedef HueBleDiscoveryRequest = Future<HueBleDiscoveryResult> Function({
  required String source,
});

typedef HueBlePlatformScan = Future<HueBleDiscoveryResult> Function(
  Duration timeout,
);

@visibleForTesting
bool isHueBleDiscoveryAdvertisement({
  required Iterable<String> serviceUuids,
  required bool connectable,
}) {
  if (!connectable) return false;
  return serviceUuids.any(
    (uuid) => uuid.trim().toLowerCase() == _hueBleDiscoveryServiceUuid,
  );
}

int _boundedDeviceCount(int count) => count < 0
    ? 0
    : count > 10
        ? 10
        : count;

/// Coordinates one bounded, phone-side Hue BLE observation at a time.
///
/// The result intentionally carries only a count. Bluetooth identifiers,
/// names, signal strength, and advertisement payloads never leave the scan
/// boundary or enter analytics.
class HueBleAutoDiscoveryService {
  HueBleAutoDiscoveryService({
    @visibleForTesting HueBlePlatformScan? platformScan,
  }) : _platformScan = platformScan ?? _scanWithFlutterBluePlus;

  static final HueBleAutoDiscoveryService instance =
      HueBleAutoDiscoveryService();

  static const scanTimeout = Duration(seconds: 6);

  final HueBlePlatformScan _platformScan;
  Future<HueBleDiscoveryResult>? _inFlight;

  Future<HueBleDiscoveryResult> discover({
    required String source,
  }) {
    final existing = _inFlight;
    if (existing != null) return existing;

    late final Future<HueBleDiscoveryResult> request;
    request = Future.sync(() => _platformScan(scanTimeout))
        .onError(
      (error, stackTrace) => const HueBleDiscoveryResult(
        HueBleDiscoveryOutcome.failed,
      ),
    )
        .then((result) async {
      await AnalyticsService().logHueBleNearbyDiscoveryCompleted(
        source: source,
        outcome: _analyticsOutcome(result.outcome),
        deviceCount: _boundedDeviceCount(result.deviceCount),
      );
      return result;
    }).whenComplete(() {
      if (identical(_inFlight, request)) _inFlight = null;
    });
    _inFlight = request;
    return request;
  }

  static String _analyticsOutcome(HueBleDiscoveryOutcome outcome) =>
      switch (outcome) {
        HueBleDiscoveryOutcome.found => 'found',
        HueBleDiscoveryOutcome.none => 'none',
        HueBleDiscoveryOutcome.permissionDenied => 'permission_denied',
        HueBleDiscoveryOutcome.bluetoothUnavailable => 'bluetooth_unavailable',
        HueBleDiscoveryOutcome.unsupported => 'unsupported',
        HueBleDiscoveryOutcome.failed => 'failed',
      };

  static Future<HueBleDiscoveryResult> _scanWithFlutterBluePlus(
    Duration timeout,
  ) async {
    if (kIsWeb || !(Platform.isAndroid || Platform.isIOS || Platform.isMacOS)) {
      return const HueBleDiscoveryResult(
        HueBleDiscoveryOutcome.unsupported,
      );
    }

    if (!await FlutterBluePlus.isSupported) {
      return const HueBleDiscoveryResult(
        HueBleDiscoveryOutcome.unsupported,
      );
    }

    if (Platform.isAndroid) {
      final statuses = await <Permission>[
        Permission.bluetoothScan,
        Permission.bluetoothConnect,
        Permission.locationWhenInUse,
      ].request();
      if (statuses.values.any((status) => !status.isGranted)) {
        return const HueBleDiscoveryResult(
          HueBleDiscoveryOutcome.permissionDenied,
        );
      }
    }

    var adapterState = FlutterBluePlus.adapterStateNow;
    if (adapterState == BluetoothAdapterState.unknown ||
        adapterState == BluetoothAdapterState.turningOn ||
        adapterState == BluetoothAdapterState.turningOff) {
      try {
        adapterState = await FlutterBluePlus.adapterState
            .firstWhere(
              (state) =>
                  state != BluetoothAdapterState.unknown &&
                  state != BluetoothAdapterState.turningOn &&
                  state != BluetoothAdapterState.turningOff,
            )
            .timeout(const Duration(seconds: 3));
      } on TimeoutException {
        return const HueBleDiscoveryResult(
          HueBleDiscoveryOutcome.bluetoothUnavailable,
        );
      }
    }
    if (adapterState != BluetoothAdapterState.on) {
      return const HueBleDiscoveryResult(
        HueBleDiscoveryOutcome.bluetoothUnavailable,
      );
    }

    final found = Completer<int>();
    final seen = <String>{};
    StreamSubscription<List<ScanResult>>? subscription;
    try {
      subscription = FlutterBluePlus.onScanResults.listen(
        (results) {
          for (final result in results) {
            final advertisement = result.advertisementData;
            if (!isHueBleDiscoveryAdvertisement(
              serviceUuids: advertisement.serviceUuids.map((uuid) => uuid.str),
              connectable: advertisement.connectable,
            )) {
              continue;
            }
            seen.add(result.device.remoteId.str);
          }
          if (seen.isNotEmpty && !found.isCompleted) {
            found.complete(_boundedDeviceCount(seen.length));
          }
        },
        onError: (Object error, StackTrace stackTrace) {
          if (!found.isCompleted) found.completeError(error, stackTrace);
        },
      );

      await FlutterBluePlus.startScan(
        withServices: [Guid(_hueBleDiscoveryServiceUuid)],
        timeout: timeout,
      );

      final count = await Future.any<int>([
        found.future,
        FlutterBluePlus.isScanning
            .firstWhere((isScanning) => !isScanning)
            .then((_) => 0),
      ]).timeout(timeout + const Duration(seconds: 1), onTimeout: () => 0);
      return HueBleDiscoveryResult(
        count > 0 ? HueBleDiscoveryOutcome.found : HueBleDiscoveryOutcome.none,
        deviceCount: count,
      );
    } on FlutterBluePlusException catch (error) {
      if (error.code == FbpErrorCode.adapterIsOff.index) {
        return const HueBleDiscoveryResult(
          HueBleDiscoveryOutcome.bluetoothUnavailable,
        );
      }
      return const HueBleDiscoveryResult(HueBleDiscoveryOutcome.failed);
    } catch (_) {
      return const HueBleDiscoveryResult(HueBleDiscoveryOutcome.failed);
    } finally {
      await subscription?.cancel();
      if (FlutterBluePlus.isScanningNow) {
        try {
          await FlutterBluePlus.stopScan();
        } catch (_) {
          // Cleanup failure must not turn a completed observation into a
          // product-flow failure. A later scan still owns its own stop/start.
        }
      }
    }
  }
}
