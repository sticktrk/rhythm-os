import 'package:flutter/services.dart';

/// Thin wrapper around the native `RhythmAccessoryPlugin` MethodChannel.
///
/// On iOS 18+, AccessorySetupKit replaces the manual BLE scan + permission
/// flow with a single OS-managed picker. On Android (or older iOS) these
/// methods are no-ops / return null.
class RhythmAccessoryService {
  static const _channel = MethodChannel('com.rhythm.accessory');

  /// Show the AccessorySetupKit picker.
  ///
  /// Returns the `bluetoothIdentifier` UUID string of the selected accessory,
  /// or `null` if the user cancelled or the picker is unavailable.
  Future<String?> showPicker() async {
    try {
      return await _channel.invokeMethod<String?>('showPicker');
    } on MissingPluginException {
      // Plugin not registered (Android, or iOS < 18)
      return null;
    }
  }

  /// Remove the currently paired accessory from AccessorySetupKit.
  ///
  /// Returns `true` if the accessory was removed successfully,
  /// `false` if no accessory was paired or the platform doesn't support it.
  Future<bool> removeAccessory() async {
    try {
      return await _channel.invokeMethod<bool>('removeAccessory') ?? false;
    } on MissingPluginException {
      return false;
    }
  }

}
