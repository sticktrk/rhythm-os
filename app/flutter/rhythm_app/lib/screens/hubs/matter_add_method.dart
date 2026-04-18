enum MatterAddMethod {
  automatic,
  onNetworkSetupCode,
  bleWifiCommissioning,
}

extension MatterAddMethodCopy on MatterAddMethod {
  String get actionLabel => switch (this) {
        MatterAddMethod.automatic => 'Add Matter Device',
        MatterAddMethod.onNetworkSetupCode => 'Add On-Network Matter Device',
        MatterAddMethod.bleWifiCommissioning => 'Commission New Matter Device',
      };

  String get description => switch (this) {
        MatterAddMethod.automatic =>
          'Send a setup code or QR payload to the server and let it choose the add path.',
        MatterAddMethod.onNetworkSetupCode =>
          'The device is already on IP/Wi-Fi. Add it with a setup code or QR payload.',
        MatterAddMethod.bleWifiCommissioning =>
          'The device is not on Wi-Fi yet. Use BLE plus the server appliance\'s stored Wi-Fi credentials.',
      };

  bool get requiresWifiCommissioningPreflight =>
      this == MatterAddMethod.bleWifiCommissioning;
}
