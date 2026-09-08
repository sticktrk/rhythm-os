enum MatterAddMethod {
  automatic,
  phoneCommissioning,
  onNetworkSetupCode,
  bleWifiCommissioning,
}

extension MatterAddMethodCopy on MatterAddMethod {
  String get actionLabel => switch (this) {
        MatterAddMethod.automatic => 'Add Device',
        MatterAddMethod.phoneCommissioning => 'Add Device',
        MatterAddMethod.onNetworkSetupCode => 'Add Device',
        MatterAddMethod.bleWifiCommissioning => 'Add Device',
      };

  String get description => switch (this) {
        MatterAddMethod.automatic =>
          'Send a setup code or QR payload to the server and let it choose the add path.',
        MatterAddMethod.phoneCommissioning =>
          'Keep the phone near the device while it handles Bluetooth and network setup, then Rhythm finishes on the server.',
        MatterAddMethod.onNetworkSetupCode =>
          'The device is already in another Matter app. Add Rhythm with the new setup code from that app.',
        MatterAddMethod.bleWifiCommissioning =>
          'The device is not on Wi-Fi yet. Use BLE plus the server appliance\'s stored Wi-Fi credentials.',
      };

  bool get requiresWifiCommissioningPreflight =>
      this == MatterAddMethod.bleWifiCommissioning;

  bool get usesPhoneCommissioner => this == MatterAddMethod.phoneCommissioning;

  String get rendezvous => switch (this) {
        MatterAddMethod.automatic => 'auto',
        MatterAddMethod.phoneCommissioning => 'phone',
        MatterAddMethod.onNetworkSetupCode => 'on_network',
        MatterAddMethod.bleWifiCommissioning => 'ble',
      };
}
