/// Storage service for Hue SSE device registry and room mappings.
///
/// This is now a thin wrapper around SettingsService for backwards compatibility.
/// All data is stored in Hive via SettingsService.
library;

import 'settings_service.dart';

/// Storage keys for Hue SSE data.
class HueSseStorage {
  /// Save device registry state.
  ///
  /// The registry JSON includes:
  /// - Room mappings (Hue room ID -> Rhythm room ID)
  /// - Cached device list
  /// - Cached room list
  static Future<void> saveDeviceRegistry(Map<String, dynamic> registryJson) async {
    await SettingsService.instance.saveHueDeviceRegistry(registryJson);
  }

  /// Load device registry state.
  ///
  /// Returns null if no registry is saved.
  static Future<Map<String, dynamic>?> loadDeviceRegistry() async {
    return SettingsService.instance.getHueDeviceRegistry();
  }

  /// Clear device registry state.
  static Future<void> clearDeviceRegistry() async {
    await SettingsService.instance.clearHueDeviceRegistry();
  }

  /// Clear all SSE data (device registry and enabled flag).
  static Future<void> clearAll() async {
    await SettingsService.instance.clearHueDeviceRegistry();
    await SettingsService.instance.setHueSseEnabled(true); // Reset to default
  }

  /// Check if SSE is enabled.
  ///
  /// SSE is enabled by default when Hue is configured.
  static Future<bool> isSseEnabled() async {
    return SettingsService.instance.hueSseEnabled;
  }

  /// Set whether SSE is enabled.
  static Future<void> setSseEnabled(bool enabled) async {
    await SettingsService.instance.setHueSseEnabled(enabled);
  }
}
