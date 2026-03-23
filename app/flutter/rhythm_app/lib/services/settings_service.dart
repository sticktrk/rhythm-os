import 'dart:convert';
import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../data/local_data_source.dart';

/// Singleton service for device-specific settings.
///
/// This service:
/// - Initializes before backend (for onboardingComplete check)
/// - Performs one-time migration from SharedPreferences to Hive
/// - Provides getters/setters for all device-specific settings
///
/// Location, sleep schedule, and timezone are stored in the Home model
/// (via HomeProvider) and sync to cloud. This service only handles
/// device-local settings.
class SettingsService {
  static SettingsService? _instance;
  static SettingsService get instance => _instance ??= SettingsService._();

  SettingsService._();

  LocalDataSource? _localDataSource;
  AppSettings _settings = AppSettings.defaults();
  bool _initialized = false;

  /// Whether the service has been initialized.
  bool get isInitialized => _initialized;

  /// Get the current settings.
  AppSettings get settings => _settings;

  // ============================================================
  // Initialization
  // ============================================================

  /// Initialize the settings service.
  ///
  /// This must be called before accessing settings. It will:
  /// 1. Initialize LocalDataSource if needed
  /// 2. Perform one-time migration from SharedPreferences
  /// 3. Load settings from Hive
  Future<void> initialize({LocalDataSource? localDataSource}) async {
    if (_initialized) return;

    debugPrint('SettingsService: Initializing...');

    // Use provided data source or create new one
    _localDataSource = localDataSource ?? LocalDataSource();
    if (!_localDataSource!.isInitialized) {
      await _localDataSource!.initialize();
    }

    // Migrate from SharedPreferences if needed
    if (!_localDataSource!.isMigrationComplete()) {
      await _migrateFromSharedPreferences();
    }

    // Load settings
    _settings = _localDataSource!.getSettings();
    _initialized = true;

    debugPrint('SettingsService: Initialized (onboardingComplete=${_settings.onboardingComplete})');
  }

  /// Migrate settings from SharedPreferences to Hive.
  ///
  /// This is a one-time operation that preserves all existing settings.
  Future<void> _migrateFromSharedPreferences() async {
    debugPrint('SettingsService: Migrating from SharedPreferences...');

    try {
      final prefs = await SharedPreferences.getInstance();

      // Read all SP values
      final use24HourFormat = prefs.getBool('use24HourFormat') ?? false;
      final onboardingComplete = prefs.getBool('onboardingComplete') ?? false;
      final notificationsEnabled = prefs.getBool('notificationsEnabled') ?? false;
      final hueSseEnabled = prefs.getBool('hue_sse_enabled') ?? true;

      // Read JSON data
      final hueDeviceRegistryJson = prefs.getString('hue_sse_device_registry');
      final curveConfigJson = prefs.getString('rhythm_curve_config');
      final runnerStateJson = prefs.getString('rhythm_runner_state');

      // Create settings object
      _settings = AppSettings(
        use24HourFormat: use24HourFormat,
        onboardingComplete: onboardingComplete,
        notificationsEnabled: notificationsEnabled,
        hueSseEnabled: hueSseEnabled,
        hueDeviceRegistryJson: hueDeviceRegistryJson,
        curveConfigJson: curveConfigJson,
        runnerStateJson: runnerStateJson,
      );

      // Save to Hive
      await _localDataSource!.saveSettings(_settings);
      await _localDataSource!.markMigrationComplete();

      debugPrint('SettingsService: Migration complete');
    } catch (e) {
      debugPrint('SettingsService: Migration failed: $e');
      // Continue with defaults - don't block app startup
    }
  }

  // ============================================================
  // Boolean Settings
  // ============================================================

  /// Whether to use 24-hour time format.
  bool get use24HourFormat => _settings.use24HourFormat;

  /// Set 24-hour time format preference.
  Future<void> setUse24HourFormat(bool value) async {
    _settings = _settings.copyWith(use24HourFormat: value);
    await _save();
  }

  /// Toggle 24-hour time format.
  Future<void> toggleTimeFormat() async {
    await setUse24HourFormat(!_settings.use24HourFormat);
  }

  /// Whether onboarding has been completed.
  bool get onboardingComplete => _settings.onboardingComplete;

  /// Mark onboarding as complete.
  Future<void> setOnboardingComplete(bool value) async {
    _settings = _settings.copyWith(onboardingComplete: value);
    await _save();
  }

  /// Whether push notifications are enabled.
  bool get notificationsEnabled => _settings.notificationsEnabled;

  /// Set notifications preference.
  Future<void> setNotificationsEnabled(bool value) async {
    _settings = _settings.copyWith(notificationsEnabled: value);
    await _save();
  }

  /// Whether Hue SSE is enabled.
  bool get hueSseEnabled => _settings.hueSseEnabled;

  /// Set Hue SSE preference.
  Future<void> setHueSseEnabled(bool value) async {
    _settings = _settings.copyWith(hueSseEnabled: value);
    await _save();
  }

  // ============================================================
  // Electricity Rate
  // ============================================================

  /// Electricity rate in currency per kWh (e.g. 0.12 for $0.12/kWh).
  double? get electricityRate => _settings.electricityRate;

  /// Set electricity rate.
  Future<void> setElectricityRate(double? value) async {
    _settings = _settings.copyWith(electricityRate: value);
    await _save();
  }

  // ============================================================
  // JSON Data Storage
  // ============================================================

  /// Get Hue device registry JSON.
  String? get hueDeviceRegistryJson => _settings.hueDeviceRegistryJson;

  /// Get Hue device registry as Map.
  Map<String, dynamic>? getHueDeviceRegistry() {
    final json = _settings.hueDeviceRegistryJson;
    if (json == null) return null;
    try {
      return jsonDecode(json) as Map<String, dynamic>;
    } catch (e) {
      return null;
    }
  }

  /// Save Hue device registry.
  Future<void> saveHueDeviceRegistry(Map<String, dynamic> registry) async {
    _settings = _settings.copyWith(hueDeviceRegistryJson: jsonEncode(registry));
    await _save();
  }

  /// Clear Hue device registry.
  Future<void> clearHueDeviceRegistry() async {
    _settings = _settings.clearField(clearHueDeviceRegistryJson: true);
    await _save();
  }

  /// Get Hue grouped_light map as Map<roomId, groupedLightId>.
  Map<String, String>? getHueGroupedLightMap() {
    final json = _settings.hueGroupedLightMapJson;
    if (json == null) return null;
    try {
      final decoded = jsonDecode(json) as Map<String, dynamic>;
      return decoded.map((k, v) => MapEntry(k, v as String));
    } catch (e) {
      return null;
    }
  }

  /// Save Hue grouped_light map.
  Future<void> saveHueGroupedLightMap(Map<String, String> map) async {
    _settings = _settings.copyWith(hueGroupedLightMapJson: jsonEncode(map));
    await _save();
  }

  /// Clear Hue grouped_light map.
  Future<void> clearHueGroupedLightMap() async {
    _settings = _settings.clearField(clearHueGroupedLightMapJson: true);
    await _save();
  }

  /// Get curve config JSON.
  String? get curveConfigJson => _settings.curveConfigJson;

  /// Get curve config as RawConfig.
  RawConfig? getCurveConfig() {
    final json = _settings.curveConfigJson;
    if (json == null) return null;
    try {
      return RawConfig.fromJson(jsonDecode(json) as Map<String, dynamic>);
    } catch (e) {
      return null;
    }
  }

  /// Save curve config.
  Future<void> saveCurveConfig(RawConfig config) async {
    _settings = _settings.copyWith(curveConfigJson: jsonEncode(config.toJson()));
    await _save();
  }

  /// Clear curve config.
  Future<void> clearCurveConfig() async {
    _settings = _settings.clearField(clearCurveConfigJson: true);
    await _save();
  }

  /// Get runner state JSON.
  String? get runnerStateJson => _settings.runnerStateJson;

  /// Get runner state.
  RunnerStateDto? getRunnerState() {
    final json = _settings.runnerStateJson;
    if (json == null) return null;
    try {
      return runnerStateFromJson(json: json);
    } catch (e) {
      return null;
    }
  }

  /// Save runner state.
  Future<void> saveRunnerState(RunnerStateDto state) async {
    final json = runnerStateToJson(state: state);
    _settings = _settings.copyWith(runnerStateJson: json);
    await _save();
  }

  /// Clear runner state.
  Future<void> clearRunnerState() async {
    _settings = _settings.clearField(clearRunnerStateJson: true);
    await _save();
  }

  // ============================================================
  // Time Formatting Helper
  // ============================================================

  /// Format a time in the given format.
  String formatTime(int hour, int minute, {required bool use24h}) {
    if (use24h) {
      return '${hour.toString().padLeft(2, '0')}:${minute.toString().padLeft(2, '0')}';
    }
    final period = hour >= 12 ? 'PM' : 'AM';
    final displayHour = hour == 0 ? 12 : (hour > 12 ? hour - 12 : hour);
    return '$displayHour:${minute.toString().padLeft(2, '0')} $period';
  }

  // ============================================================
  // Utility Methods
  // ============================================================

  /// Save current settings to storage.
  Future<void> _save() async {
    if (_localDataSource != null) {
      await _localDataSource!.saveSettings(_settings);
    }
  }

  /// Reset all settings to defaults.
  Future<void> resetToDefaults() async {
    _settings = AppSettings.defaults();
    await _save();
  }

  /// Clear all settings (for sign out / account deletion).
  Future<void> clearAll() async {
    if (_localDataSource != null) {
      await _localDataSource!.clearSettings();
    }
    // Clear SharedPreferences to prevent re-migration of stale data
    try {
      final prefs = await SharedPreferences.getInstance();
      await prefs.clear();
    } catch (_) {}
    _settings = AppSettings.defaults();
    _onboardingPreferences = null;
    _initialized = false;
  }

  // ============================================================
  // Temporary Onboarding Preferences Storage
  // ============================================================

  /// Temporary storage for onboarding preferences.
  /// These are passed from onboarding to HomeProvider.onUserSignIn().
  OnboardingPreferencesData? _onboardingPreferences;

  /// Set onboarding preferences (called from auth_provider).
  void setOnboardingPreferences(OnboardingPreferencesData prefs) {
    _onboardingPreferences = prefs;
  }

  /// Get and clear onboarding preferences (called from home_provider).
  OnboardingPreferencesData? consumeOnboardingPreferences() {
    final prefs = _onboardingPreferences;
    _onboardingPreferences = null;
    return prefs;
  }
}

/// Temporary data structure for passing onboarding preferences.
class OnboardingPreferencesData {
  final double? latitude;
  final double? longitude;
  final String? cityName;
  final String? timezone;
  final int bedtimeHour;
  final int bedtimeMinute;
  final int wakeTimeHour;
  final int wakeTimeMinute;

  const OnboardingPreferencesData({
    this.latitude,
    this.longitude,
    this.cityName,
    this.timezone,
    this.bedtimeHour = 22,
    this.bedtimeMinute = 30,
    this.wakeTimeHour = 6,
    this.wakeTimeMinute = 30,
  });
}
