import 'dart:async';
import 'dart:convert';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'package:flutter_blue_plus/flutter_blue_plus.dart';

class _Uuids {
  static final service = Guid('72797468-6d00-1000-8000-00805f9b34fb');
  static final wifiCommand = Guid('72797468-6d01-1000-8000-00805f9b34fb');
  static final status = Guid('72797468-6d02-1000-8000-00805f9b34fb');
  static final deviceInfo = Guid('72797468-6d03-1000-8000-00805f9b34fb');
  static final authRequest = Guid('72797468-6d04-1000-8000-00805f9b34fb');
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
  final ProvisioningStatusMessage? provisioningStatus;

  const BleDeviceInfo({
    required this.name,
    required this.version,
    this.mac,
    this.provisioningStatus,
  });
}

class BleProvisioningResult {
  final String ip;
  final String? ownerToken;
  final bool restartPending;

  const BleProvisioningResult({
    required this.ip,
    this.ownerToken,
    this.restartPending = false,
  });
}

class WifiFailedException implements Exception {
  final String message;

  const WifiFailedException(this.message);

  @override
  String toString() => 'WifiFailedException: $message';
}

class ProvisioningStatusMessage {
  final String status;
  final String? ip;
  final String? otaStage;
  final String? message;
  final String? ownerToken;
  final String? error;

  const ProvisioningStatusMessage({
    required this.status,
    this.ip,
    this.otaStage,
    this.message,
    this.ownerToken,
    this.error,
  });

  bool get isTerminal =>
      isConnected ||
      status == 'restarting' ||
      status == 'wifi_failed' ||
      status == 'failed';

  bool get hasIp => ip != null && ip!.trim().isNotEmpty;

  bool get isConnected =>
      status == 'connected' || (status == 'updating' && hasIp);

  bool get canResume =>
      isTerminal || ((status == 'updating' || status == 'restarting') && hasIp);
}

class BleProvisioningService {
  static const _provisioningTimeout = Duration(minutes: 35);
  static const _authTokenTimeout = Duration(seconds: 20);
  static const _statusPollInterval = Duration(milliseconds: 500);
  static const _bleOperationTimeout = Duration(seconds: 8);
  // Defensive guard for malformed progress states. Normal Wi-Fi handoff
  // completes immediately when the appliance status includes an IP.
  static const _staleProvisioningProgressTimeout = Duration(seconds: 12);

  StreamSubscription<List<ScanResult>>? _scanSubscription;
  StreamController<BleDevice>? _scanController;
  BluetoothDevice? _connectedDevice;
  BluetoothCharacteristic? _wifiCommandChar;
  BluetoothCharacteristic? _statusChar;
  BluetoothCharacteristic? _deviceInfoChar;
  BluetoothCharacteristic? _authRequestChar;

  bool get isConnected =>
      _connectedDevice != null &&
      _statusChar != null &&
      _deviceInfoChar != null &&
      (_wifiCommandChar != null || _authRequestChar != null);

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
    return _setupConnection(
      bluetoothDevice,
      requireProvisioningReady: true,
    );
  }

  Future<BleDeviceInfo> connectForAuth(BleDevice device) async {
    final bluetoothDevice = device._fbpDevice;
    if (bluetoothDevice == null) {
      throw StateError('Invalid Bluetooth device handle');
    }

    await disconnect();
    stopScan();

    await bluetoothDevice.connect(timeout: const Duration(seconds: 10));
    _connectedDevice = bluetoothDevice;
    return _setupConnection(
      bluetoothDevice,
      requireProvisioningReady: false,
    );
  }

  Future<BleDeviceInfo> connectById(String bluetoothId) async {
    await disconnect();
    stopScan();

    final bluetoothDevice = BluetoothDevice.fromId(bluetoothId);
    await bluetoothDevice.connect(timeout: const Duration(seconds: 10));
    _connectedDevice = bluetoothDevice;
    return _setupConnection(
      bluetoothDevice,
      requireProvisioningReady: true,
    );
  }

  Future<BleDeviceInfo> _setupConnection(
    BluetoothDevice bluetoothDevice, {
    required bool requireProvisioningReady,
  }) async {
    final services =
        await bluetoothDevice.discoverServices().timeout(_bleOperationTimeout);
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
    _authRequestChar = service.characteristics
        .where(
            (candidate) => candidate.characteristicUuid == _Uuids.authRequest)
        .firstOrNull;

    final missingProvisioningChars = requireProvisioningReady &&
        (_wifiCommandChar == null ||
            _statusChar == null ||
            _deviceInfoChar == null);
    final missingAuthChars = !requireProvisioningReady &&
        (_authRequestChar == null ||
            _statusChar == null ||
            _deviceInfoChar == null);
    if (missingProvisioningChars || missingAuthChars) {
      await disconnect();
      throw StateError(requireProvisioningReady
          ? 'Device is missing required provisioning characteristics'
          : 'Device is missing required auth characteristics');
    }

    final initialStatus = await _readStatus(_statusChar!);
    if (requireProvisioningReady &&
        initialStatus.status != 'waiting' &&
        initialStatus.status != 'wifi_failed' &&
        !initialStatus.canResume) {
      await disconnect();
      throw StateError(
        'Device is not ready for provisioning (status: ${initialStatus.status})',
      );
    }

    return _readDeviceInfo(
      _deviceInfoChar!,
      provisioningStatus: initialStatus,
    );
  }

  Future<String> requestOwnerToken({
    String label = 'Rhythm app',
  }) async {
    final authRequestChar = _authRequestChar;
    final statusChar = _statusChar;
    if (authRequestChar == null || statusChar == null) {
      throw StateError('Not connected to an auth-capable Rhythm device');
    }

    final trimmedLabel = label.trim();
    final payload = utf8.encode(
      json.encode({
        'label': trimmedLabel.isEmpty ? 'Rhythm app' : trimmedLabel,
      }),
    );

    final status = await waitForMatchingProvisioningStatus(
      enableNotifications: () => statusChar.setNotifyValue(true),
      writePayload: () => authRequestChar.write(
        payload,
        withoutResponse: false,
      ),
      statusUpdates: statusChar.onValueReceived.map(_parseStatus),
      readStatus: () => _readStatus(statusChar),
      isMatch: (update) => update.status == 'auth_token',
      timeoutMessage: 'Timed out waiting for owner token',
      timeout: _authTokenTimeout,
      pollInterval: _statusPollInterval,
      operationTimeout: _bleOperationTimeout,
    );

    final ownerToken = status.ownerToken?.trim();
    if (ownerToken == null || ownerToken.isEmpty) {
      throw StateError('Auth token response did not include an owner token');
    }
    return ownerToken;
  }

  Future<BleProvisioningResult> sendWifiCredentials(
    String ssid,
    String password, {
    void Function(ProvisioningStatusMessage status)? onStatus,
  }) async {
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
      onStatus: onStatus,
      timeout: _provisioningTimeout,
      pollInterval: _statusPollInterval,
      operationTimeout: _bleOperationTimeout,
    );

    return resultFromTerminalStatus(status);
  }

  Future<BleProvisioningResult> waitForProvisioningResult({
    void Function(ProvisioningStatusMessage status)? onStatus,
  }) async {
    final statusChar = _statusChar;
    if (statusChar == null) {
      throw StateError('Not connected to a provisioning device');
    }

    final status = await waitForTerminalProvisioningStatus(
      enableNotifications: () => statusChar.setNotifyValue(true),
      writePayload: () async {},
      statusUpdates: statusChar.onValueReceived.map(_parseStatus),
      readStatus: () => _readStatus(statusChar),
      onStatus: onStatus,
      timeout: _provisioningTimeout,
      pollInterval: _statusPollInterval,
      operationTimeout: _bleOperationTimeout,
    );

    return resultFromTerminalStatus(status);
  }

  static BleProvisioningResult resultFromTerminalStatus(
    ProvisioningStatusMessage status,
  ) {
    if (status.isConnected) {
      final ip = status.ip?.trim();
      if (ip == null || ip.isEmpty) {
        throw StateError('Provisioning succeeded without an IP address');
      }
      final ownerToken = status.ownerToken?.trim();
      return BleProvisioningResult(
        ip: ip,
        ownerToken:
            ownerToken == null || ownerToken.isEmpty ? null : ownerToken,
      );
    }

    switch (status.status) {
      case 'restarting':
        final ip = status.ip?.trim();
        if (ip == null || ip.isEmpty) {
          throw StateError(
              'Update restart status did not include an IP address');
        }
        return BleProvisioningResult(ip: ip, restartPending: true);
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
    void Function(ProvisioningStatusMessage status)? onStatus,
    Duration timeout = _provisioningTimeout,
    Duration pollInterval = _statusPollInterval,
    Duration operationTimeout = _bleOperationTimeout,
    Duration? staleProgressTimeout = _staleProvisioningProgressTimeout,
  }) async {
    return waitForMatchingProvisioningStatus(
      enableNotifications: enableNotifications,
      writePayload: writePayload,
      statusUpdates: statusUpdates,
      readStatus: readStatus,
      isMatch: (update) => update.isTerminal,
      timeoutMessage: 'Timed out waiting for Wi-Fi connection',
      onStatus: onStatus,
      timeout: timeout,
      pollInterval: pollInterval,
      operationTimeout: operationTimeout,
      recoverFromReadError: _recoverTerminalProvisioningReadError,
      staleStatusTimeout: staleProgressTimeout,
      recoverFromStaleStatus: _recoverStaleTerminalProvisioningStatus,
    );
  }

  @visibleForTesting
  static Future<ProvisioningStatusMessage> waitForMatchingProvisioningStatus({
    required Future<void> Function() enableNotifications,
    required Future<void> Function() writePayload,
    required Stream<ProvisioningStatusMessage> statusUpdates,
    required Future<ProvisioningStatusMessage> Function() readStatus,
    required bool Function(ProvisioningStatusMessage update) isMatch,
    required String timeoutMessage,
    void Function(ProvisioningStatusMessage status)? onStatus,
    Duration timeout = _provisioningTimeout,
    Duration pollInterval = _statusPollInterval,
    Duration operationTimeout = _bleOperationTimeout,
    ProvisioningStatusMessage? Function(
      Object error,
      ProvisioningStatusMessage? latestStatus,
    )? recoverFromReadError,
    Duration? staleStatusTimeout,
    ProvisioningStatusMessage? Function(
      ProvisioningStatusMessage latestStatus,
    )? recoverFromStaleStatus,
  }) async {
    final deadline = DateTime.now().add(timeout);
    Future<ProvisioningStatusMessage>? notificationStatus;
    ProvisioningStatusMessage? latestStatus;
    String? latestStatusKey;
    var latestStatusChangedAt = DateTime.now();

    void observeStatus(ProvisioningStatusMessage status) {
      final statusKey =
          '${status.status}|${status.ip}|${status.otaStage}|${status.message}|${status.error}';
      if (statusKey != latestStatusKey) {
        latestStatusKey = statusKey;
        latestStatusChangedAt = DateTime.now();
      }
      latestStatus = status;
      onStatus?.call(status);
    }

    ProvisioningStatusMessage? recoverStaleStatus(
      ProvisioningStatusMessage status,
    ) {
      if (staleStatusTimeout == null || recoverFromStaleStatus == null) {
        return null;
      }
      if (DateTime.now().difference(latestStatusChangedAt) <
          staleStatusTimeout) {
        return null;
      }
      return recoverFromStaleStatus(status);
    }

    try {
      await enableNotifications().timeout(operationTimeout);
      notificationStatus = statusUpdates
          .map((update) {
            observeStatus(update);
            return update;
          })
          .where(isMatch)
          .first
          .timeout(timeout);
    } catch (error) {
      debugPrint(
        '[BLE] status notifications unavailable; falling back to polling: '
        '$error',
      );
    }

    await writePayload().timeout(operationTimeout);

    final pollingStatus = _pollMatchingStatus(
      readStatus: readStatus,
      isMatch: isMatch,
      onStatus: observeStatus,
      deadline: deadline,
      pollInterval: pollInterval,
      operationTimeout: operationTimeout,
      recoverFromReadError: recoverFromReadError == null
          ? null
          : (error) => recoverFromReadError(error, latestStatus),
      recoverFromStaleStatus: recoverStaleStatus,
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
      throw TimeoutException(timeoutMessage);
    }
  }

  static Future<ProvisioningStatusMessage> _pollMatchingStatus({
    required Future<ProvisioningStatusMessage> Function() readStatus,
    required bool Function(ProvisioningStatusMessage update) isMatch,
    void Function(ProvisioningStatusMessage status)? onStatus,
    required DateTime deadline,
    required Duration pollInterval,
    required Duration operationTimeout,
    ProvisioningStatusMessage? Function(Object error)? recoverFromReadError,
    ProvisioningStatusMessage? Function(ProvisioningStatusMessage status)?
        recoverFromStaleStatus,
  }) async {
    while (DateTime.now().isBefore(deadline)) {
      final remainingBeforeRead = deadline.difference(DateTime.now());
      if (remainingBeforeRead <= Duration.zero) break;
      final readTimeout = remainingBeforeRead < operationTimeout
          ? remainingBeforeRead
          : operationTimeout;
      final ProvisioningStatusMessage status;
      try {
        status = await readStatus().timeout(readTimeout);
      } catch (error) {
        final recovered = recoverFromReadError?.call(error);
        if (recovered != null) {
          onStatus?.call(recovered);
          return recovered;
        }
        rethrow;
      }
      onStatus?.call(status);
      if (isMatch(status)) {
        return status;
      }
      final recovered = recoverFromStaleStatus?.call(status);
      if (recovered != null) {
        onStatus?.call(recovered);
        return recovered;
      }
      final remaining = deadline.difference(DateTime.now());
      if (remaining <= Duration.zero) break;
      await Future<void>.delayed(
        remaining < pollInterval ? remaining : pollInterval,
      );
    }
    throw TimeoutException('Timed out waiting for matching status');
  }

  static ProvisioningStatusMessage? _recoverStaleTerminalProvisioningStatus(
    ProvisioningStatusMessage latestStatus,
  ) {
    if (latestStatus.isTerminal || latestStatus.status != 'updating') {
      return null;
    }

    final ip = latestStatus.ip?.trim();
    if (ip == null || ip.isEmpty) {
      return null;
    }

    return ProvisioningStatusMessage(
      status: 'connected',
      ip: ip,
      message: 'Continuing setup over LAN',
    );
  }

  static ProvisioningStatusMessage? _recoverTerminalProvisioningReadError(
    Object error,
    ProvisioningStatusMessage? latestStatus,
  ) {
    if (!_isBleDisconnectedError(error) && error is! TimeoutException) {
      return null;
    }
    if (latestStatus == null) {
      return null;
    }

    final ip = latestStatus.ip?.trim();
    if (ip == null || ip.isEmpty) {
      return null;
    }
    if (latestStatus.isConnected) {
      return latestStatus;
    }
    if (latestStatus.status != 'restarting') {
      return null;
    }

    final message = latestStatus.message?.trim();
    return ProvisioningStatusMessage(
      status: 'restarting',
      ip: ip,
      otaStage: 'restarting',
      message: message == null || message.isEmpty
          ? 'Update is restarting the device'
          : message,
    );
  }

  static bool _isBleDisconnectedError(Object error) {
    if (error is PlatformException) {
      final code = error.code.toLowerCase();
      final message = error.message?.toLowerCase() ?? '';
      return code.contains('disconnect') ||
          message.contains('device is disconnected') ||
          message.contains('device disconnected');
    }

    final text = '$error'.toLowerCase();
    return text.contains('device is disconnected') ||
        text.contains('device disconnected');
  }

  Future<void> disconnect() async {
    final statusChar = _statusChar;
    _wifiCommandChar = null;
    _statusChar = null;
    _deviceInfoChar = null;
    _authRequestChar = null;

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
    BluetoothCharacteristic characteristic, {
    ProvisioningStatusMessage? provisioningStatus,
  }) async {
    final bytes = await characteristic.read().timeout(_bleOperationTimeout);
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
      provisioningStatus: provisioningStatus,
    );
  }

  Future<ProvisioningStatusMessage> _readStatus(
    BluetoothCharacteristic characteristic,
  ) async {
    final bytes = await characteristic.read().timeout(_bleOperationTimeout);
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
      otaStage: jsonMap['ota_stage'] as String?,
      message: jsonMap['message'] as String?,
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

    try {
      await FlutterBluePlus.isScanning
          .firstWhere((isScanning) => isScanning == false)
          .timeout(const Duration(seconds: 20));
    } on TimeoutException {
      stopScan();
    }
  }
}
