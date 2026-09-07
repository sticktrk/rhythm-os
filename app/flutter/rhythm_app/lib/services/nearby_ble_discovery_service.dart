import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter_blue_plus/flutter_blue_plus.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import 'analytics_service.dart';

/// How a nearby family is paired once the person picks it.
enum NearbyBleFamilyKind {
  /// Hue's built-in nearby scan: the appliance bonds every eligible bulb.
  hueBle,

  /// Vendor-neutral staged flow: the appliance finds the device over
  /// Bluetooth, joins it to Wi-Fi, and adopts its LAN credentials while the
  /// app brokers any cloud step through the family's named broker function.
  bleWifi,
}

/// A device family Rhythm can add without a QR code.
///
/// Apart from Hue's built-in scan, families are described entirely by the
/// appliance's advertised device profiles: the name people see, the Bluetooth
/// service UUIDs the phone scans for, and the cloud broker to call. The app
/// carries no vendor-specific knowledge.
class NearbyBleFamily {
  const NearbyBleFamily({
    required this.id,
    required this.kind,
    required this.hubType,
    required this.serviceUuids,
    required this.label,
    required this.hint,
    this.profileId,
    this.cloudBroker,
  });

  /// Stable analytics/UI key: the profile id, or `hue_ble` for the built-in.
  final String id;
  final NearbyBleFamilyKind kind;

  /// Hub type that owns pairing for this family.
  final String hubType;

  /// Lower-cased service UUIDs advertised while the device waits for setup.
  final List<String> serviceUuids;
  final String label;
  final String hint;
  final String? profileId;
  final String? cloudBroker;

  static const hueBle = NearbyBleFamily(
    id: 'hue_ble',
    kind: NearbyBleFamilyKind.hueBle,
    hubType: 'hue_ble',
    serviceUuids: ['0000fe0f-0000-1000-8000-00805f9b34fb'],
    label: 'Hue Bluetooth bulb',
    hint: 'Factory-reset bulbs that advertise over Bluetooth.',
  );

  /// Build a family from an advertised profile that supports nearby scanning.
  factory NearbyBleFamily.fromProfile({
    required String hubType,
    required RhythmDeviceProfile profile,
  }) {
    final name = profile.displayName.trim();
    return NearbyBleFamily(
      id: profile.id,
      kind: NearbyBleFamilyKind.bleWifi,
      hubType: hubType,
      serviceUuids: profile.nearbyServiceUuids,
      label: name.isEmpty ? 'Wi-Fi light' : name,
      hint: 'A device in setup mode. Rhythm joins it to Wi-Fi for you.',
      profileId: profile.id,
      cloudBroker: profile.cloudBroker,
    );
  }

  String countLabel(int count) => count == 1 ? label : '$label ($count nearby)';

  bool advertises(Iterable<String> normalizedServiceUuids) =>
      normalizedServiceUuids.any(serviceUuids.contains);

  @override
  bool operator ==(Object other) => other is NearbyBleFamily && other.id == id;

  @override
  int get hashCode => id.hashCode;

  @override
  String toString() => 'NearbyBleFamily($id)';
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
  const NearbyBleDiscoveryResult(this.outcome, {this.counts = const {}});

  final NearbyBleDiscoveryOutcome outcome;

  /// Bounded advertiser count per family. Identifiers never leave the scan.
  final Map<NearbyBleFamily, int> counts;

  bool get found => outcome == NearbyBleDiscoveryOutcome.found;

  /// Families with at least one advertiser, Hue first, then by label.
  List<NearbyBleFamily> get families {
    final found = [
      for (final entry in counts.entries)
        if (entry.value > 0) entry.key,
    ]..sort((a, b) {
        if (a.kind != b.kind) {
          return a.kind == NearbyBleFamilyKind.hueBle ? -1 : 1;
        }
        return a.label.toLowerCase().compareTo(b.label.toLowerCase());
      });
    return found;
  }

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

/// Classify one advertisement into one of [families], or null when unknown.
@visibleForTesting
NearbyBleFamily? nearbyBleFamilyForAdvertisement({
  required Iterable<String> serviceUuids,
  required bool connectable,
  required Iterable<NearbyBleFamily> families,
}) {
  if (!connectable) return null;
  final normalized =
      serviceUuids.map((uuid) => uuid.trim().toLowerCase()).toList();
  for (final family in families) {
    if (family.advertises(normalized)) return family;
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
      (error, stackTrace) =>
          const NearbyBleDiscoveryResult(NearbyBleDiscoveryOutcome.failed),
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
        withServices: [
          for (final family in families)
            for (final uuid in family.serviceUuids) Guid(uuid),
        ],
        timeout: timeout,
      );
      await FlutterBluePlus.isScanning
          .firstWhere((isScanning) => !isScanning)
          .timeout(
            timeout + const Duration(seconds: 1),
            onTimeout: () => false,
          );

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
