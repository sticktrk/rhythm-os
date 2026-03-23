import 'dart:async';
import 'dart:convert';
import 'package:flutter_blue_plus/flutter_blue_plus.dart';

/// GATT UUIDs matching the ESP32 BLE provisioning service.
class _Uuids {
  static final service = Guid('72797468-6d00-1000-8000-00805f9b34fb');
  static final wifiCommand = Guid('72797468-6d01-1000-8000-00805f9b34fb');
  static final status = Guid('72797468-6d02-1000-8000-00805f9b34fb');
  static final deviceInfo = Guid('72797468-6d03-1000-8000-00805f9b34fb');
}

/// A BLE device discovered during scanning.
class BleDevice {
  final String id;
  final String name;
  final int rssi;

  /// The underlying flutter_blue_plus device handle, used internally for
  /// connect/disconnect. Not exposed to the UI layer.
  final BluetoothDevice? _fbpDevice;

  const BleDevice({
    required this.id,
    required this.name,
    required this.rssi,
    BluetoothDevice? fbpDevice,
  }) : _fbpDevice = fbpDevice;
}

/// Device info read from the Device Info characteristic after connecting.
class BleDeviceInfo {
  final String name;
  final String version;
  final String mac;

  const BleDeviceInfo({
    required this.name,
    required this.version,
    required this.mac,
  });
}

/// Thrown when the ESP32 reports that WiFi credentials were rejected.
///
/// Distinct from generic [Exception] so the UI can distinguish "wrong password"
/// (recoverable — BLE still alive, user can re-enter) from "BLE disconnected"
/// or timeout (may need full reconnect).
class WifiFailedException implements Exception {
  final String message;
  const WifiFailedException(this.message);
  @override
  String toString() => 'WifiFailedException: $message';
}

/// Real BLE provisioning service for ESP32 WiFi setup using flutter_blue_plus.
class BleProvisioningService {
  StreamSubscription<List<ScanResult>>? _scanSubscription;
  StreamController<BleDevice>? _scanController;
  BluetoothDevice? _connectedDevice;
  BluetoothCharacteristic? _wifiCommandChar;
  BluetoothCharacteristic? _statusChar;

  /// Whether a BLE connection is still alive and ready to send credentials.
  bool get isConnected => _connectedDevice != null && _wifiCommandChar != null;

  /// Check whether the Bluetooth adapter is turned on.
  ///
  /// Waits up to 5 seconds for the adapter to reach the `on` state.
  /// On iOS, CoreBluetooth may briefly transition after a fresh permission
  /// grant, so [.first] can grab a stale/transitional value.
  Future<bool> isBluetoothOn() async {
    try {
      await FlutterBluePlus.adapterState
          .firstWhere((s) => s == BluetoothAdapterState.on)
          .timeout(const Duration(seconds: 5));
      return true;
    } on TimeoutException {
      return false;
    }
  }

  /// Scan for nearby Rhythm ESP32 devices.
  /// Filters by the "Rhythm-" name prefix advertised by the ESP32.
  /// Emits discovered devices as they are found, deduplicated by device ID.
  Stream<BleDevice> scanForDevices() {
    _scanController?.close();
    _scanSubscription?.cancel();
    _scanController = StreamController<BleDevice>();

    final seen = <String>{};

    // Subscribe to results BEFORE starting the scan so we don't miss early hits.
    _scanSubscription = FlutterBluePlus.scanResults.listen(
      (results) {
        for (final r in results) {
          final name = r.advertisementData.advName;
          if (!name.startsWith('Rhythm-')) continue;

          final id = r.device.remoteId.str;
          if (seen.contains(id)) continue;
          seen.add(id);

          _scanController?.add(BleDevice(
            id: id,
            name: name,
            rssi: r.rssi,
            fbpDevice: r.device,
          ));
        }
      },
      onError: (e) {
        _scanController?.addError(e);
      },
      onDone: () {
        _scanController?.close();
      },
    );

    // Don't filter by service UUID — esp-idf may advertise the UUID in a
    // byte order that doesn't match flutter_blue_plus's string parsing.
    // Instead filter by the "Rhythm-" name prefix in the callback.
    //
    // Pipe startScan errors to the stream controller so they surface in
    // the UI instead of becoming unhandled Future rejections.
    FlutterBluePlus.startScan(
      timeout: const Duration(seconds: 15),
    ).catchError((e) {
      _scanController?.addError(e);
      _scanController?.close();
    });

    return _scanController!.stream;
  }

  /// Stop an active scan.
  void stopScan() {
    FlutterBluePlus.stopScan();
    _scanSubscription?.cancel();
    _scanSubscription = null;
    _scanController?.close();
    _scanController = null;
  }

  /// Connect to a discovered BLE device, discover the provisioning service,
  /// and read the Device Info and Status characteristics.
  ///
  /// Throws if the device doesn't have the expected service/characteristics,
  /// or if the status is not "waiting" (device already provisioned or busy).
  Future<BleDeviceInfo> connect(BleDevice device) async {
    final fbp = device._fbpDevice;
    if (fbp == null) {
      throw Exception('Invalid device handle');
    }

    stopScan();

    await fbp.connect(timeout: const Duration(seconds: 10));
    _connectedDevice = fbp;

    return _setupConnection(fbp);
  }

  /// Connect by Bluetooth UUID (from AccessorySetupKit picker on iOS).
  ///
  /// Uses [BluetoothDevice.fromId] to create a handle from the UUID string
  /// returned by the ASK picker, then runs the same post-connection setup.
  Future<BleDeviceInfo> connectById(String bluetoothId) async {
    stopScan();

    final fbp = BluetoothDevice.fromId(bluetoothId);
    await fbp.connect(timeout: const Duration(seconds: 10));
    _connectedDevice = fbp;

    return _setupConnection(fbp);
  }

  /// Shared post-connection setup: discover services, locate characteristics,
  /// verify the device is in "waiting" state, and read device info.
  Future<BleDeviceInfo> _setupConnection(BluetoothDevice fbp) async {
    final services = await fbp.discoverServices();
    final svc = services.firstWhere(
      (s) => s.serviceUuid == _Uuids.service,
      orElse: () => throw Exception('Provisioning service not found on device'),
    );

    // Locate characteristics
    BluetoothCharacteristic? statusChar;
    BluetoothCharacteristic? deviceInfoChar;

    for (final c in svc.characteristics) {
      if (c.characteristicUuid == _Uuids.wifiCommand) {
        _wifiCommandChar = c;
      } else if (c.characteristicUuid == _Uuids.status) {
        statusChar = c;
      } else if (c.characteristicUuid == _Uuids.deviceInfo) {
        deviceInfoChar = c;
      }
    }

    _statusChar = statusChar;

    if (_wifiCommandChar == null || statusChar == null || deviceInfoChar == null) {
      await disconnect();
      throw Exception('Device is missing required characteristics');
    }

    // Read status — accept "waiting" (fresh boot) or "wifi_failed" (previous
    // attempt failed, ESP32 looped back and is ready for new credentials).
    final statusBytes = await statusChar.read();
    final statusJson = json.decode(utf8.decode(statusBytes)) as Map<String, dynamic>;
    final statusValue = statusJson['status'] as String?;

    if (statusValue != 'waiting' && statusValue != 'wifi_failed') {
      await disconnect();
      throw Exception('Device is not ready for provisioning (status: $statusValue)');
    }

    // Read device info
    final infoBytes = await deviceInfoChar.read();
    final infoJson = json.decode(utf8.decode(infoBytes)) as Map<String, dynamic>;

    return BleDeviceInfo(
      name: infoJson['name'] as String? ?? 'RhythmBox',
      version: infoJson['version'] as String? ?? 'unknown',
      mac: infoJson['mac'] as String? ?? '',
    );
  }

  /// Send WiFi credentials and wait for the ESP32 to report its IP.
  ///
  /// The ESP32 keeps BLE alive while WiFi connects (BLE+WiFi coexistence on
  /// C6). Once connected, it sends a notification with `{"status":"connected",
  /// "ip":"..."}`. This method subscribes to that notification, writes the
  /// credentials, then waits up to 30 seconds for the IP.
  ///
  /// Returns the IP address string on success. Throws on failure or timeout.
  Future<String> sendWifiCredentials(String ssid, String password) async {
    if (_wifiCommandChar == null || _statusChar == null) {
      throw Exception('Not connected — call connect() first');
    }

    // Subscribe to status notifications before writing credentials
    await _statusChar!.setNotifyValue(true);

    final payload = json.encode({'ssid': ssid, 'password': password});

    await _wifiCommandChar!.write(
      utf8.encode(payload),
      withoutResponse: false,
    );

    // Wait for the WiFi result notification (up to 30s)
    try {
      final result = await _statusChar!.onValueReceived
          .where((bytes) {
            if (bytes.isEmpty) return false;
            try {
              final msg = json.decode(utf8.decode(bytes)) as Map<String, dynamic>;
              final status = msg['status'] as String?;
              // Accept terminal states only
              return status == 'connected' || status == 'wifi_failed';
            } catch (_) {
              return false;
            }
          })
          .first
          .timeout(const Duration(seconds: 30));

      final msg = json.decode(utf8.decode(result)) as Map<String, dynamic>;
      final status = msg['status'] as String?;

      if (status == 'connected') {
        final ip = msg['ip'] as String? ?? 'unknown';

        // Clean up — ESP32 will tear down BLE shortly after this
        _wifiCommandChar = null;
        _statusChar = null;
        _connectedDevice = null;

        return ip;
      } else {
        // WiFi failed — do NOT unsubscribe or tear down BLE.
        // The ESP32 loops back to waiting for new credentials on the same
        // connection. The next sendWifiCredentials() call re-subscribes
        // (idempotent on flutter_blue_plus).
        final error = msg['error'] as String? ?? 'Unknown error';
        throw WifiFailedException(error);
      }
    } on TimeoutException {
      // Timeout — best-effort unsubscribe, then throw generic exception
      try {
        await _statusChar?.setNotifyValue(false);
      } catch (_) {}
      throw Exception('Timed out waiting for WiFi connection (30s)');
    }
  }

  /// Disconnect from the current device if still connected.
  Future<void> disconnect() async {
    _wifiCommandChar = null;
    _statusChar = null;
    try {
      await _connectedDevice?.disconnect();
    } catch (_) {
      // Device may have already disconnected
    }
    _connectedDevice = null;
  }

  /// Release all resources.
  void dispose() {
    stopScan();
    disconnect();
  }
}
