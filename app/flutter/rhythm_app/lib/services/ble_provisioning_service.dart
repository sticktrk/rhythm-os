import 'dart:async';

/// Bluetooth provisioning is currently disabled.
///
/// These stubs keep older code compiling while the BLE path is commented out
/// across Flutter and native targets.
class BleDevice {
  final String id;
  final String name;
  final int rssi;

  const BleDevice({
    required this.id,
    required this.name,
    required this.rssi,
  });
}

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

class WifiFailedException implements Exception {
  final String message;

  const WifiFailedException(this.message);

  @override
  String toString() => 'WifiFailedException: $message';
}

class BleProvisioningService {
  static const String _disabledMessage = 'Bluetooth provisioning is disabled';

  bool get isConnected => false;

  Future<bool> isBluetoothOn() async => false;

  Stream<BleDevice> scanForDevices() => Stream<BleDevice>.error(
        StateError(_disabledMessage),
      );

  void stopScan() {}

  Future<BleDeviceInfo> connect(BleDevice device) =>
      Future<BleDeviceInfo>.error(StateError(_disabledMessage));

  Future<BleDeviceInfo> connectById(String bluetoothId) =>
      Future<BleDeviceInfo>.error(StateError(_disabledMessage));

  Future<String> sendWifiCredentials(String ssid, String password) =>
      Future<String>.error(StateError(_disabledMessage));

  Future<void> disconnect() async {}

  void dispose() {}
}
