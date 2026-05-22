import 'dart:async';
import 'dart:convert';
import 'package:flutter/foundation.dart';

import 'package:flutter_blue_plus/flutter_blue_plus.dart';

class _Uuids {
  static final service = Guid('72797468-6d00-1000-8000-00805f9b34fb');
  static final wifiCommand = Guid('72797468-6d01-1000-8000-00805f9b34fb');
  static final status = Guid('72797468-6d02-1000-8000-00805f9b34fb');
  static final deviceInfo = Guid('72797468-6d03-1000-8000-00805f9b34fb');
}

class BleDevice {
  final String id;
  final String name;
  final int rssi;

  final BluetoothDevice? _fbpDevice;

  const BleDevice({
    required this.id,
    required this.name,
    required this.rssi,
    BluetoothDevice? fbpDevice,
  }) : _fbpDevice = fbpDevice;
}

class BleDeviceInfo {
  final String name;
  final String version;
  final String? mac;

  const BleDeviceInfo({
    required this.name,
    required this.version,
    this.mac,
  });
}

class BleProvisioningResult {
  final String ip;
  final String? ownerToken;

  const BleProvisioningResult({
    required this.ip,
    this.ownerToken,
  });
}

class WifiFailedException implements Exception {
  final String message;

  const WifiFailedException(this.message);

  @override
  String toString() => 'WifiFailedException: $message';
}

@visibleForTesting
class ProvisioningStatusMessage {
  final String status;
  final String? ip;
  final String? ownerToken;
  final String? error;

  const ProvisioningStatusMessage({
    required this.status,
    this.ip,
    this.ownerToken,
    this.error,
  });

  bool get isTerminal =>
      status == 'connected' || status == 'wifi_failed' || status == 'failed';
}

class BleProvisioningService {
  static const _provisioningTimeout = Duration(seconds: 30);
  static const _statusPollInterval = Duration(milliseconds: 500);

  StreamSubscription<List<ScanResult>>? _scanSubscription;
  StreamController<BleDevice>? _scanController;
  BluetoothDevice? _connectedDevice;
  BluetoothCharacteristic? _wifiCommandChar;
  BluetoothCharacteristic? _statusChar;
  BluetoothCharacteristic? _deviceInfoChar;

  bool get isConnected =>
      _connectedDevice != null &&
      _wifiCommandChar != null &&
      _statusChar != null &&
      _deviceInfoChar != null;

  Future<BluetoothAdapterState> _waitForStableAdapterState({
    Duration timeout = const Duration(seconds: 4),
  }) async {
    final current = FlutterBluePlus.adapterStateNow;
    if (current == BluetoothAdapterState.on) {
      return current;
    }

    final initial = _isStableAdapterState(current)
        ? current
        : await FlutterBluePlus.adapterState
            .firstWhere(_isStableAdapterState)
            .timeout(timeout);

    if (defaultTargetPlatform != TargetPlatform.iOS ||
        initial == BluetoothAdapterState.on) {
      return initial;
    }

    // iOS can briefly report `off` while CoreBluetooth is still settling.
    // Give it a short window to update before treating the state as final.
    for (var attempt = 0; attempt < 4; attempt++) {
      await Future<void>.delayed(const Duration(milliseconds: 350));
      final retried = FlutterBluePlus.adapterStateNow;
      debugPrint('[BLE] adapter state retry ${attempt + 1}: $retried');
      if (retried == BluetoothAdapterState.on ||
          retried == BluetoothAdapterState.unauthorized ||
          retried == BluetoothAdapterState.unavailable) {
        return retried;
      }
    }

    return initial;
  }

  bool _isStableAdapterState(BluetoothAdapterState state) {
    return state != BluetoothAdapterState.unknown &&
        state != BluetoothAdapterState.turningOn &&
        state != BluetoothAdapterState.turningOff;
  }

  Future<bool> isBluetoothOn() async {
    try {
      return await _waitForStableAdapterState(
            timeout: const Duration(seconds: 5),
          ) ==
          BluetoothAdapterState.on;
    } on TimeoutException {
      return false;
    }
  }

  Stream<BleDevice> scanForDevices() {
    stopScan();
    _scanController = StreamController<BleDevice>();
    debugPrint('[BLE] startScan requested for legacy discovery');

    final seen = <String>{};
    _scanSubscription = FlutterBluePlus.scanResults.listen(
      (results) {
        for (final result in results) {
          final id = result.device.remoteId.str;
          final advName = result.advertisementData.advName.trim();
          final platformName = result.device.platformName.trim();
          final name = advName.isNotEmpty ? advName : platformName;
          final normalized = name.toLowerCase();
          final matchesRhythmName = normalized.startsWith('rhythm-') ||
              normalized.startsWith('rhythm box') ||
              normalized.startsWith('rhythmbox');
          if (!matchesRhythmName) continue;

          if (!seen.add(id)) continue;

          _scanController?.add(
            BleDevice(
              id: id,
              name: name,
              rssi: result.rssi,
              fbpDevice: result.device,
            ),
          );
        }
      },
      onError: (error, stackTrace) {
        _scanController?.addError(error, stackTrace);
      },
    );

    unawaited(_startLegacyScan());

    return _scanController!.stream;
  }

  Future<void> _startLegacyScan() async {
    try {
      final adapterState = await _waitForStableAdapterState();
      debugPrint('[BLE] adapter state before scan: $adapterState');

      if (adapterState != BluetoothAdapterState.on) {
        throw StateError('Bluetooth adapter is not ready ($adapterState)');
      }

      // Match the older pre-ASK flow: do a plain scan and filter results in Dart
      // instead of relying on service UUID filtering.
      await FlutterBluePlus.startScan(
        timeout: const Duration(seconds: 15),
      );
    } catch (error, stackTrace) {
      debugPrint('[BLE] startScan failed: $error');
      _scanController?.addError(error, stackTrace);
    }
  }

  Future<void> waitForScanToFinish() {
    return _waitForScanToFinish();
  }

  void stopScan() {
    FlutterBluePlus.stopScan();
    unawaited(_scanSubscription?.cancel());
    _scanSubscription = null;
    unawaited(_scanController?.close());
    _scanController = null;
  }

  Future<BleDeviceInfo> connect(BleDevice device) async {
    final bluetoothDevice = device._fbpDevice;
    if (bluetoothDevice == null) {
      throw StateError('Invalid Bluetooth device handle');
    }

    await disconnect();
    stopScan();

    await bluetoothDevice.connect(timeout: const Duration(seconds: 10));
    _connectedDevice = bluetoothDevice;
    return _setupConnection(bluetoothDevice);
  }

  Future<BleDeviceInfo> connectById(String bluetoothId) async {
    await disconnect();
    stopScan();

    final bluetoothDevice = BluetoothDevice.fromId(bluetoothId);
    await bluetoothDevice.connect(timeout: const Duration(seconds: 10));
    _connectedDevice = bluetoothDevice;
    return _setupConnection(bluetoothDevice);
  }

  Future<BleDeviceInfo> _setupConnection(
      BluetoothDevice bluetoothDevice) async {
    final services = await bluetoothDevice.discoverServices();
    final service = services.firstWhere(
      (candidate) => candidate.serviceUuid == _Uuids.service,
      orElse: () =>
          throw StateError('Provisioning service not found on device'),
    );

    _wifiCommandChar = service.characteristics
        .where(
            (candidate) => candidate.characteristicUuid == _Uuids.wifiCommand)
        .firstOrNull;
    _statusChar = service.characteristics
        .where((candidate) => candidate.characteristicUuid == _Uuids.status)
        .firstOrNull;
    _deviceInfoChar = service.characteristics
        .where((candidate) => candidate.characteristicUuid == _Uuids.deviceInfo)
        .firstOrNull;

    if (_wifiCommandChar == null ||
        _statusChar == null ||
        _deviceInfoChar == null) {
      await disconnect();
      throw StateError(
          'Device is missing required provisioning characteristics');
    }

    final initialStatus = await _readStatus(_statusChar!);
    if (initialStatus.status != 'waiting' &&
        initialStatus.status != 'wifi_failed') {
      await disconnect();
      throw StateError(
        'Device is not ready for provisioning (status: ${initialStatus.status})',
      );
    }

    return _readDeviceInfo(_deviceInfoChar!);
  }

  Future<BleProvisioningResult> sendWifiCredentials(
    String ssid,
    String password,
  ) async {
    final wifiCommandChar = _wifiCommandChar;
    final statusChar = _statusChar;
    if (wifiCommandChar == null || statusChar == null) {
      throw StateError('Not connected to a provisioning device');
    }

    final payload = utf8.encode(
      json.encode({
        'ssid': ssid,
        'password': password,
      }),
    );

    final status = await waitForTerminalProvisioningStatus(
      enableNotifications: () => statusChar.setNotifyValue(true),
      writePayload: () => wifiCommandChar.write(
        payload,
        withoutResponse: false,
      ),
      statusUpdates: statusChar.onValueReceived.map(_parseStatus),
      readStatus: () => _readStatus(statusChar),
      timeout: _provisioningTimeout,
      pollInterval: _statusPollInterval,
    );

    switch (status.status) {
      case 'connected':
        final ip = status.ip;
        if (ip == null || ip.isEmpty) {
          throw StateError('Provisioning succeeded without an IP address');
        }
        final ownerToken = status.ownerToken?.trim();
        return BleProvisioningResult(
          ip: ip,
          ownerToken:
              ownerToken == null || ownerToken.isEmpty ? null : ownerToken,
        );
      case 'wifi_failed':
        throw WifiFailedException(status.error ?? 'Wi-Fi connection failed');
      case 'failed':
        throw StateError(status.error ?? 'Provisioning failed');
      default:
        throw StateError('Unexpected provisioning status: ${status.status}');
    }
  }

  @visibleForTesting
  static Future<ProvisioningStatusMessage> waitForTerminalProvisioningStatus({
    required Future<void> Function() enableNotifications,
    required Future<void> Function() writePayload,
    required Stream<ProvisioningStatusMessage> statusUpdates,
    required Future<ProvisioningStatusMessage> Function() readStatus,
    Duration timeout = _provisioningTimeout,
    Duration pollInterval = _statusPollInterval,
  }) async {
    final deadline = DateTime.now().add(timeout);
    Future<ProvisioningStatusMessage>? notificationStatus;

    try {
      await enableNotifications();
      notificationStatus = statusUpdates
          .where((update) => update.isTerminal)
          .first
          .timeout(timeout);
    } catch (error) {
      debugPrint(
        '[BLE] status notifications unavailable; falling back to polling: '
        '$error',
      );
    }

    await writePayload();

    final pollingStatus = _pollTerminalStatus(
      readStatus: readStatus,
      deadline: deadline,
      pollInterval: pollInterval,
    );
    final notificationOrPolling = notificationStatus?.catchError((error) {
      debugPrint(
        '[BLE] status notification stream failed; waiting for polling: '
        '$error',
      );
      return pollingStatus;
    });

    try {
      return await (notificationOrPolling == null
          ? pollingStatus
          : Future.any([notificationOrPolling, pollingStatus]));
    } on TimeoutException {
      throw TimeoutException('Timed out waiting for Wi-Fi connection');
    }
  }

  static Future<ProvisioningStatusMessage> _pollTerminalStatus({
    required Future<ProvisioningStatusMessage> Function() readStatus,
    required DateTime deadline,
    required Duration pollInterval,
  }) async {
    while (DateTime.now().isBefore(deadline)) {
      final status = await readStatus();
      if (status.isTerminal) {
        return status;
      }
      final remaining = deadline.difference(DateTime.now());
      if (remaining <= Duration.zero) break;
      await Future<void>.delayed(
        remaining < pollInterval ? remaining : pollInterval,
      );
    }
    throw TimeoutException('Timed out waiting for Wi-Fi connection');
  }

  Future<void> disconnect() async {
    final statusChar = _statusChar;
    _wifiCommandChar = null;
    _statusChar = null;
    _deviceInfoChar = null;

    try {
      if (statusChar?.isNotifying == true) {
        await statusChar!.setNotifyValue(false);
      }
    } catch (_) {
      // Ignore teardown failures. The peripheral may already be gone.
    }

    try {
      await _connectedDevice?.disconnect();
    } catch (_) {
      // Ignore disconnect failures. The device may already be gone after success.
    }
    _connectedDevice = null;
  }

  void dispose() {
    stopScan();
    unawaited(disconnect());
  }

  Future<BleDeviceInfo> _readDeviceInfo(
    BluetoothCharacteristic characteristic,
  ) async {
    final bytes = await characteristic.read();
    final jsonMap = _decodeJson(bytes);

    final mac = (jsonMap['mac'] as String?)?.trim();
    return BleDeviceInfo(
      name: (jsonMap['name'] as String?)?.trim().isNotEmpty == true
          ? (jsonMap['name'] as String).trim()
          : 'Rhythm',
      version: (jsonMap['version'] as String?)?.trim().isNotEmpty == true
          ? (jsonMap['version'] as String).trim()
          : 'unknown',
      mac: mac == null || mac.isEmpty ? null : mac,
    );
  }

  Future<ProvisioningStatusMessage> _readStatus(
    BluetoothCharacteristic characteristic,
  ) async {
    final bytes = await characteristic.read();
    return _parseStatus(bytes);
  }

  ProvisioningStatusMessage _parseStatus(List<int> bytes) {
    final jsonMap = _decodeJson(bytes);
    final status = jsonMap['status'] as String?;
    if (status == null || status.isEmpty) {
      throw const FormatException('Provisioning status payload missing status');
    }

    return ProvisioningStatusMessage(
      status: status,
      ip: jsonMap['ip'] as String?,
      ownerToken: jsonMap['owner_token'] as String?,
      error: jsonMap['error'] as String?,
    );
  }

  Map<String, dynamic> _decodeJson(List<int> bytes) {
    final decoded = json.decode(utf8.decode(bytes));
    if (decoded is! Map<String, dynamic>) {
      throw const FormatException('Expected JSON object');
    }
    return decoded;
  }

  Future<void> _waitForScanToFinish() async {
    if (!FlutterBluePlus.isScanningNow) {
      try {
        await FlutterBluePlus.isScanning
            .firstWhere((isScanning) => isScanning == true)
            .timeout(const Duration(seconds: 2));
      } on TimeoutException {
        if (!FlutterBluePlus.isScanningNow) return;
      }
    }

    await FlutterBluePlus.isScanning
        .firstWhere((isScanning) => isScanning == false);
  }
}
