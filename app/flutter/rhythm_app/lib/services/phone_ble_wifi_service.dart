import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_blue_plus/flutter_blue_plus.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class BleWifiDiscoveredCandidate {
  const BleWifiDiscoveredCandidate({required this.dsn, required this.address});
  final String dsn;

  /// Opaque phone Bluetooth ID (an iOS UUID, not necessarily a MAC address).
  final String address;
  String get dsnSuffix =>
      dsn.substring(dsn.length > 4 ? dsn.length - 4 : 0).toUpperCase();
}

class PhoneBleWifiFailure implements Exception {
  const PhoneBleWifiFailure(this.message, {this.uncertain = false});
  final String message;
  final bool uncertain;
  @override
  String toString() => message;
}

/// Protocol-owned GATT boundary, separate from Flutter's platform transport.
abstract interface class PhoneWifiGatt {
  Future<List<int>> read(String service, String characteristic);
  Future<void> write(String service, String characteristic, List<int> value);
  Future<void> disconnect();
}

abstract interface class PhoneWifiTransport {
  Future<List<String>> scan(String service);
  Future<PhoneWifiGatt> connect(String address);
  Future<void> dispose();
}

abstract interface class PhoneBleWifiService {
  Future<List<BleWifiDiscoveredCandidate>> discover();
  Future<void> provision(BleWifiDiscoveredCandidate candidate,
      String setupToken, RhythmCommissioningWifi wifi);
  void validateWifi(RhythmCommissioningWifi wifi);
  Future<void> dispose();
}

/// Register protocol adapters here. Screens consume advertised capabilities and
/// this interface; they never select by vendor or encode protocol payloads.
abstract final class PhoneBleWifiServices {
  static PhoneBleWifiService? create(String? protocol,
          {PhoneBleWifiService? service}) =>
      AylaPhoneBleWifiService.supports(protocol)
          ? (service ?? AylaPhoneBleWifiService())
          : null;
}

/// Versioned Ayla protocol matching rhythm-monster/src/ble.rs. Manufacturer
/// routing comes from the advertised protocol, never the device's name.
class AylaPhoneBleWifiService implements PhoneBleWifiService {
  AylaPhoneBleWifiService({
    PhoneWifiTransport? transport,
    this.statusPollDelay = const Duration(milliseconds: 500),
    this.statusPollLimit = 120,
  }) : _transport = transport ?? FlutterPhoneWifiTransport();

  static const protocol = 'ayla_v1';
  static const idService = '0000fe28-0000-1000-8000-00805f9b34fb';
  static const dsnCharacteristic = '00000001-fe28-435b-991a-f1b21bb9bcd0';
  static const tokenService = 'fce3ec41-59b6-4873-ae36-fab25bd59adc';
  static const tokenCharacteristic = '7e9869ed-4db3-4520-88ea-1c21ef1ba834';
  static const wifiService = '1cf0fe66-3ecf-4d6e-a9fc-e287ab124b96';
  static const connectCharacteristic = '1f80af6a-2b71-4e35-94e5-00f854d8f16f';
  static const statusCharacteristic = '1f80af6c-2b71-4e35-94e5-00f854d8f16f';
  final PhoneWifiTransport _transport;
  final Duration statusPollDelay;
  final int statusPollLimit;
  bool _disposed = false;

  static bool supports(String? advertisedProtocol) =>
      advertisedProtocol == protocol &&
      !kIsWeb &&
      (defaultTargetPlatform == TargetPlatform.iOS ||
          defaultTargetPlatform == TargetPlatform.android);

  void _checkActive() {
    if (_disposed) {
      throw const PhoneBleWifiFailure('Phone setup was cancelled.');
    }
  }

  Future<String> _identity(PhoneWifiGatt gatt) async {
    final value = utf8
        .decode(await gatt.read(idService, dsnCharacteristic))
        .replaceFirst(RegExp(r'\x00+$'), '');
    if (!RegExp(r'^[a-zA-Z0-9]{8,32}$').hasMatch(value)) {
      throw const PhoneBleWifiFailure(
          'The device did not provide a valid serial number.');
    }
    return value;
  }

  @override
  Future<List<BleWifiDiscoveredCandidate>> discover() async {
    _checkActive();
    final addresses = await _transport.scan(idService);
    final candidates = <BleWifiDiscoveredCandidate>[];
    for (final address in addresses.take(4)) {
      _checkActive();
      PhoneWifiGatt? gatt;
      try {
        gatt = await _transport.connect(address);
        _checkActive();
        final dsn = await _identity(gatt);
        _checkActive();
        if (!candidates.any((candidate) => candidate.dsn == dsn)) {
          candidates
              .add(BleWifiDiscoveredCandidate(dsn: dsn, address: address));
        }
      } catch (_) {
        _checkActive();
        // A matching advertiser is only a candidate until GATT proves identity.
      } finally {
        await gatt?.disconnect();
      }
    }
    _checkActive();
    if (candidates.isEmpty) {
      throw const PhoneBleWifiFailure(
          'Your phone could not read a device in setup mode. Keep it near the blinking device and try again.');
    }
    return candidates;
  }

  /// One complete 105-byte write, not arbitrary MTU-sized protocol fragments.
  static Uint8List encodeWifi(RhythmCommissioningWifi wifi) {
    final ssid = utf8.encode(wifi.ssid);
    final password = utf8.encode(wifi.password);
    if (ssid.isEmpty ||
        ssid.length > 32 ||
        ssid.contains(0) ||
        password.contains(0) ||
        (password.isNotEmpty &&
            (password.length < 8 || password.length > 63))) {
      throw const PhoneBleWifiFailure(
          'Use a Wi-Fi name of 1–32 bytes and a password of 8–63 bytes, or leave it empty for an open network.');
    }
    final payload = Uint8List(105);
    payload.setRange(0, ssid.length, ssid);
    payload[32] = ssid.length;
    payload.setRange(39, 39 + password.length, password);
    payload[103] = password.length;
    payload[104] = password.isEmpty ? 0 : 3;
    return payload;
  }

  @override
  void validateWifi(RhythmCommissioningWifi wifi) {
    final payload = encodeWifi(wifi);
    payload.fillRange(0, payload.length, 0);
  }

  @override
  Future<void> provision(BleWifiDiscoveredCandidate candidate,
      String setupToken, RhythmCommissioningWifi wifi) async {
    _checkActive();
    final payload = encodeWifi(wifi);
    if (!RegExp(r'^[a-fA-F0-9]{32}$').hasMatch(setupToken)) {
      payload.fillRange(0, payload.length, 0);
      throw const PhoneBleWifiFailure(
          'The setup session is invalid. Start over.');
    }
    PhoneWifiGatt? gatt;
    var mayHaveWritten = false;
    try {
      gatt = await _transport.connect(candidate.address);
      _checkActive();
      if (await _identity(gatt) != candidate.dsn) {
        throw const PhoneBleWifiFailure(
            'The device identity changed. Start over from the scan.');
      }
      _checkActive();
      mayHaveWritten = true;
      await gatt.write(
          tokenService, tokenCharacteristic, utf8.encode(setupToken));
      _checkActive();
      await gatt.write(wifiService, connectCharacteristic, payload);
      _checkActive();
      final deadline = Stopwatch()..start();
      for (var i = 0;
          i < statusPollLimit && deadline.elapsed < const Duration(seconds: 90);
          i++) {
        _checkActive();
        final status = await gatt.read(wifiService, statusCharacteristic);
        _checkActive();
        if (status.length != 35 || status[32] > 32 || status[34] > 5) break;
        if (status[33] != 0 && status[33] != 20) {
          throw const PhoneBleWifiFailure(
              'The device could not join Wi-Fi. Check the network details.',
              uncertain: true);
        }
        if (status[34] == 5 && status[33] == 0) {
          if (!listEquals(
              status.sublist(0, status[32]), utf8.encode(wifi.ssid))) {
            break;
          }
          return;
        }
        await Future<void>.delayed(statusPollDelay);
      }
      throw const PhoneBleWifiFailure(
          'The device has not confirmed Wi-Fi setup.',
          uncertain: true);
    } on PhoneBleWifiFailure catch (error) {
      throw PhoneBleWifiFailure(error.message,
          uncertain: mayHaveWritten || error.uncertain);
    } catch (_) {
      throw PhoneBleWifiFailure(
          mayHaveWritten
              ? 'Bluetooth disconnected before Wi-Fi setup was confirmed.'
              : 'Your phone could not connect to the device. Keep it nearby and try again.',
          uncertain: mayHaveWritten);
    } finally {
      payload.fillRange(0, payload.length, 0);
      await gatt?.disconnect();
    }
  }

  @override
  Future<void> dispose() async {
    _disposed = true;
    await _transport.dispose();
  }
}

class FlutterPhoneWifiTransport implements PhoneWifiTransport {
  bool _disposed = false;
  bool _ownsScan = false;
  BluetoothDevice? _device;

  void _checkActive() {
    if (_disposed) {
      throw const PhoneBleWifiFailure('Phone setup was cancelled.');
    }
  }

  @override
  Future<List<String>> scan(String service) async {
    _checkActive();
    // Plugin verbose logs contain raw GATT values. Disable before any secret I/O.
    await FlutterBluePlus.setLogLevel(LogLevel.none);
    final permissions = defaultTargetPlatform == TargetPlatform.android
        ? [
            Permission.bluetoothScan,
            Permission.bluetoothConnect,
            Permission.locationWhenInUse
          ]
        : [Permission.bluetooth];
    final statuses = await permissions.request();
    _checkActive();
    if (statuses.values.any((status) => !status.isGranted)) {
      throw const PhoneBleWifiFailure(
          'Allow Bluetooth access to set up this device from your phone.');
    }
    if (FlutterBluePlus.isScanningNow) {
      throw const PhoneBleWifiFailure(
          'Another Bluetooth scan is running. Try again in a moment.');
    }
    final addresses = <String>{};
    final subscription = FlutterBluePlus.onScanResults.listen((results) {
      for (final result in results) {
        if (result.advertisementData.connectable &&
            result.advertisementData.serviceUuids.contains(Guid(service))) {
          addresses.add(result.device.remoteId.str);
        }
      }
    });
    try {
      _ownsScan = true;
      await FlutterBluePlus.startScan(
          withServices: [Guid(service)], timeout: const Duration(seconds: 8));
      _checkActive();
      await FlutterBluePlus.isScanning
          .firstWhere((scanning) => !scanning)
          .timeout(const Duration(seconds: 10));
      _checkActive();
      return addresses.toList(growable: false);
    } catch (_) {
      throw const PhoneBleWifiFailure(
          'Your phone could not scan for devices. Check Bluetooth and try again.');
    } finally {
      await subscription.cancel();
      if (_ownsScan) {
        _ownsScan = false;
        await FlutterBluePlus.stopScan();
      }
    }
  }

  @override
  Future<PhoneWifiGatt> connect(String address) async {
    _checkActive();
    final device = BluetoothDevice.fromId(address);
    _device = device;
    try {
      await device.connect(timeout: const Duration(seconds: 12), mtu: null);
      _checkActive();
      final services = await device.discoverServices(timeout: 10);
      _checkActive();
      return _FlutterPhoneWifiGatt(device, services);
    } catch (_) {
      await _disconnect(device);
      rethrow;
    }
  }

  static Future<void> _disconnect(BluetoothDevice device) async {
    try {
      await device.disconnect(timeout: 5, queue: false);
    } catch (_) {/* no secret logging */}
  }

  @override
  Future<void> dispose() async {
    _disposed = true;
    if (_ownsScan) {
      _ownsScan = false;
      try {
        await FlutterBluePlus.stopScan();
      } catch (_) {/* already stopped */}
    }
    final device = _device;
    if (device != null) await _disconnect(device);
  }
}

class _FlutterPhoneWifiGatt implements PhoneWifiGatt {
  _FlutterPhoneWifiGatt(this.device, this.services);
  final BluetoothDevice device;
  final List<BluetoothService> services;

  BluetoothCharacteristic _characteristic(
      String service, String characteristic) {
    final matches = services
        .where((s) => s.uuid == Guid(service))
        .expand((s) => s.characteristics)
        .where((c) => c.uuid == Guid(characteristic))
        .toList();
    if (matches.length != 1) {
      throw const PhoneBleWifiFailure(
          'This device does not expose the expected setup service.');
    }
    return matches.single;
  }

  @override
  Future<List<int>> read(String service, String characteristic) =>
      _characteristic(service, characteristic).read(timeout: 10);

  @override
  Future<void> write(String service, String characteristic, List<int> value) =>
      _characteristic(service, characteristic).write(value,
          withoutResponse: false, allowLongWrite: true, timeout: 15);

  @override
  Future<void> disconnect() => FlutterPhoneWifiTransport._disconnect(device);
}
