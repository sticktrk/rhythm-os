import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter_blue_plus/flutter_blue_plus.dart';
import 'package:permission_handler/permission_handler.dart';

import 'analytics_service.dart';

/// Device families Rhythm can add without a QR code, keyed by the service
/// UUID each family advertises while it is waiting to be set up.
enum NearbyBleFamily {
  hueBle(
    id: 'hue_ble',
    serviceUuid: '0000fe0f-0000-1000-8000-00805f9b34fb',
    label: 'Hue Bluetooth bulb',
    pluralLabel: 'Hue Bluetooth bulbs',
    hint: 'Factory-reset bulbs that advertise over Bluetooth.',
  ),
  monster(
    id: 'monster',
    serviceUuid: '0000fe28-0000-1000-8000-00805f9b34fb',
    label: 'Monster Neon Flow',
    pluralLabel: 'Monster Neon Flow strips',
    hint: 'A strip in setup mode. Rhythm joins it to Wi-Fi for you.',
  );

  const NearbyBleFamily({
    required this.id,
    required this.serviceUuid,
    required this.label,
    required this.pluralLabel,
    required this.hint,
  });

  final String id;
  final String serviceUuid;
  final String label;
  final String pluralLabel;
  final String hint;

  String countLabel(int count) => count == 1 ? label : '$count $pluralLabel';
}

enum NearbyBleDiscoveryOutcome {
  found,
  none,
  permissionDenied,
  bluetoothUnavailable,
  unsupported,
  failed,
}

class NearbyBleDiscoveryResult {
  const NearbyBleDiscoveryResult(
    this.outcome, {
    this.counts = const {},
  });

  final NearbyBleDiscoveryOutcome outcome;

  /// Bounded advertiser count per family. Identifiers never leave the scan.
  final Map<NearbyBleFamily, int> counts;

  bool get found => outcome == NearbyBleDiscoveryOutcome.found;

  List<NearbyBleFamily> get families => [
        for (final family in NearbyBleFamily.values)
          if ((counts[family] ?? 0) > 0) family,
      ];

  int countFor(NearbyBleFamily family) => counts[family] ?? 0;
}

typedef NearbyBleDiscoveryRequest = Future<NearbyBleDiscoveryResult> Function({
  required String source,
  required Set<NearbyBleFamily> families,
});

typedef NearbyBlePlatformScan = Future<NearbyBleDiscoveryResult> Function(
  Set<NearbyBleFamily> families,
  Duration timeout,
);

/// Classify one advertisement into a known family, or null when unknown.
@visibleForTesting
NearbyBleFamily? nearbyBleFamilyForAdvertisement({
  required Iterable<String> serviceUuids,
  required bool connectable,
  Set<NearbyBleFamily> families = const {},
}) {
  if (!connectable) return null;
  final normalized = serviceUuids.map((uuid) => uuid.trim().toLowerCase());
  for (final family in families.isEmpty
      ? NearbyBleFamily.values
      : NearbyBleFamily.values.where(families.contains)) {
    if (normalized.contains(family.serviceUuid)) return family;
  }
  return null;
}

int _boundedCount(int count) => count < 0
    ? 0
    : count > 10
        ? 10
        : count;

/// Runs one bounded phone-side Bluetooth observation across every family the
/// caller can onboard. Only per-family counts leave the scan boundary.
class NearbyBleDiscoveryService {
  NearbyBleDiscoveryService({
    @visibleForTesting NearbyBlePlatformScan? platformScan,
  }) : _platformScan = platformScan ?? _scanWithFlutterBluePlus;

  static final NearbyBleDiscoveryService instance = NearbyBleDiscoveryService();

  static const scanTimeout = Duration(seconds: 6);

  final NearbyBlePlatformScan _platformScan;
  Future<NearbyBleDiscoveryResult>? _inFlight;

  Future<NearbyBleDiscoveryResult> discover({
    required String source,
    required Set<NearbyBleFamily> families,
  }) {
    final existing = _inFlight;
    if (existing != null) return existing;
    if (families.isEmpty) {
      return Future.value(
        const NearbyBleDiscoveryResult(NearbyBleDiscoveryOutcome.none),
      );
    }

    late final Future<NearbyBleDiscoveryResult> request;
    request = Future.sync(() => _platformScan(families, scanTimeout))
        .onError(
      (error, stackTrace) => const NearbyBleDiscoveryResult(
        NearbyBleDiscoveryOutcome.failed,
      ),
    )
        .then((result) async {
      await AnalyticsService().logNearbyDeviceScanCompleted(
        source: source,
        outcome: analyticsOutcome(result.outcome),
        familyCounts: {
          for (final family in families)
            family.id: _boundedCount(result.countFor(family)),
        },
      );
      return result;
    }).whenComplete(() {
      if (identical(_inFlight, request)) _inFlight = null;
    });
    _inFlight = request;
    return request;
  }

  static String analyticsOutcome(NearbyBleDiscoveryOutcome outcome) =>
      switch (outcome) {
        NearbyBleDiscoveryOutcome.found => 'found',
        NearbyBleDiscoveryOutcome.none => 'none',
        NearbyBleDiscoveryOutcome.permissionDenied => 'permission_denied',
        NearbyBleDiscoveryOutcome.bluetoothUnavailable =>
          'bluetooth_unavailable',
        NearbyBleDiscoveryOutcome.unsupported => 'unsupported',
        NearbyBleDiscoveryOutcome.failed => 'failed',
      };

  static Future<NearbyBleDiscoveryResult> _scanWithFlutterBluePlus(
    Set<NearbyBleFamily> families,
    Duration timeout,
  ) async {
    if (kIsWeb || !(Platform.isAndroid || Platform.isIOS || Platform.isMacOS)) {
      return const NearbyBleDiscoveryResult(
        NearbyBleDiscoveryOutcome.unsupported,
      );
    }
    if (!await FlutterBluePlus.isSupported) {
      return const NearbyBleDiscoveryResult(
        NearbyBleDiscoveryOutcome.unsupported,
      );
    }
    if (Platform.isAndroid) {
      final statuses = await <Permission>[
        Permission.bluetoothScan,
        Permission.bluetoothConnect,
        Permission.locationWhenInUse,
      ].request();
      if (statuses.values.any((status) => !status.isGranted)) {
        return const NearbyBleDiscoveryResult(
          NearbyBleDiscoveryOutcome.permissionDenied,
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
        return const NearbyBleDiscoveryResult(
          NearbyBleDiscoveryOutcome.bluetoothUnavailable,
        );
      }
    }
    if (adapterState != BluetoothAdapterState.on) {
      return const NearbyBleDiscoveryResult(
        NearbyBleDiscoveryOutcome.bluetoothUnavailable,
      );
    }

    final seen = <NearbyBleFamily, Set<String>>{};
    StreamSubscription<List<ScanResult>>? subscription;
    try {
      subscription = FlutterBluePlus.onScanResults.listen((results) {
        for (final result in results) {
          final advertisement = result.advertisementData;
          final family = nearbyBleFamilyForAdvertisement(
            serviceUuids: advertisement.serviceUuids.map((uuid) => uuid.str),
            connectable: advertisement.connectable,
            families: families,
          );
          if (family == null) continue;
          seen
              .putIfAbsent(family, () => <String>{})
              .add(result.device.remoteId.str);
        }
      });

      // Let the full window elapse so every family gets a chance to show up
      // instead of stopping on the first match.
      await FlutterBluePlus.startScan(
        withServices: [for (final family in families) Guid(family.serviceUuid)],
        timeout: timeout,
      );
      await FlutterBluePlus.isScanning
          .firstWhere((isScanning) => !isScanning)
          .timeout(timeout + const Duration(seconds: 1),
              onTimeout: () => false);

      final counts = {
        for (final entry in seen.entries)
          entry.key: _boundedCount(entry.value.length),
      };
      return NearbyBleDiscoveryResult(
        counts.values.any((count) => count > 0)
            ? NearbyBleDiscoveryOutcome.found
            : NearbyBleDiscoveryOutcome.none,
        counts: counts,
      );
    } on FlutterBluePlusException catch (error) {
      if (error.code == FbpErrorCode.adapterIsOff.index) {
        return const NearbyBleDiscoveryResult(
          NearbyBleDiscoveryOutcome.bluetoothUnavailable,
        );
      }
      return const NearbyBleDiscoveryResult(NearbyBleDiscoveryOutcome.failed);
    } catch (_) {
      return const NearbyBleDiscoveryResult(NearbyBleDiscoveryOutcome.failed);
    } finally {
      await subscription?.cancel();
      if (FlutterBluePlus.isScanningNow) {
        try {
          await FlutterBluePlus.stopScan();
        } catch (_) {
          // Best effort; the timeout already bounds the scan.
        }
      }
    }
  }
}
