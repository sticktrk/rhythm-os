import 'dart:convert';
import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_core/runner/runner_state_json.dart' as runner_json;
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
  static const String roomPageLayoutScopePrefix = 'room_page_layout::';
  static const String _handledPasswordRecoveryLinksKey =
      'handled_password_recovery_links_v1';
  static const int _maxHandledPasswordRecoveryLinks = 20;

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

    debugPrint(
        'SettingsService: Initialized (onboardingComplete=${_settings.onboardingComplete})');
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
      final notificationsEnabled =
          prefs.getBool('notificationsEnabled') ?? false;
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

  Future<bool> hasHandledPasswordRecoveryLink(String fingerprint) async {
    if (fingerprint.isEmpty) return false;
    try {
      final prefs = await SharedPreferences.getInstance();
      final handled =
          prefs.getStringList(_handledPasswordRecoveryLinksKey) ?? const [];
      return handled.contains(fingerprint);
    } catch (_) {
      return false;
    }
  }

  Future<void> markPasswordRecoveryLinkHandled(String fingerprint) async {
    if (fingerprint.isEmpty) return;
    try {
      final prefs = await SharedPreferences.getInstance();
      final handled = List<String>.from(
        prefs.getStringList(_handledPasswordRecoveryLinksKey) ??
            const <String>[],
      )..remove(fingerprint);
      handled.add(fingerprint);
      final trimmed = handled.length > _maxHandledPasswordRecoveryLinks
          ? handled.sublist(handled.length - _maxHandledPasswordRecoveryLinks)
          : handled;
      await prefs.setStringList(_handledPasswordRecoveryLinksKey, trimmed);
    } catch (_) {}
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

  /// Get the Hue `grouped_light` map keyed by room ID.
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
    _settings =
        _settings.copyWith(curveConfigJson: jsonEncode(config.toJson()));
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
      return runner_json.runnerStateFromJson(json);
    } catch (e) {
      return null;
    }
  }

  /// Save runner state.
  Future<void> saveRunnerState(RunnerStateDto state) async {
    final json = runner_json.runnerStateToJson(state);
    _settings = _settings.copyWith(runnerStateJson: json);
    await _save();
  }

  /// Clear runner state.
  Future<void> clearRunnerState() async {
    _settings = _settings.clearField(clearRunnerStateJson: true);
    await _save();
  }

  // ============================================================
  // Room Page Layout
  // ============================================================

  /// Get room page layout as ordered lists of room IDs per page.
  List<List<String>>? getRoomPageLayout({String? scopeKey}) {
    final json = _roomPageLayoutJson(scopeKey: scopeKey);
    return _decodeRoomPageLayout(json);
  }

  /// Get the legacy unscoped room page layout.
  List<List<String>>? getLegacyRoomPageLayout() {
    return _decodeRoomPageLayout(_settings.roomPageAssignmentsJson);
  }

  /// Save room page layout.
  Future<void> saveRoomPageLayout(List<List<String>> pages,
      {String? scopeKey}) async {
    final json = jsonEncode(pages);
    if (scopeKey == null) {
      _settings = _settings.copyWith(roomPageAssignmentsJson: json);
      await _save();
      return;
    }

    await _localDataSource!.saveSettingsValue(
      _roomPageLayoutStorageKey(scopeKey),
      json,
    );
  }

  /// Clear room page layout.
  Future<void> clearRoomPageAssignments({String? scopeKey}) async {
    if (scopeKey == null) {
      _settings = _settings.clearField(clearRoomPageAssignmentsJson: true);
      await _save();
      return;
    }

    await _localDataSource!.deleteSettingsValue(
      _roomPageLayoutStorageKey(scopeKey),
    );
  }

  /// Copy the legacy global room page layout into a scoped slot.
  ///
  /// This is a one-time bridge from the previous single-layout storage model.
  Future<void> migrateLegacyRoomPageLayoutToScope(String scopeKey) async {
    final legacyJson = _settings.roomPageAssignmentsJson;
    if (legacyJson == null) return;

    final scopedKey = _roomPageLayoutStorageKey(scopeKey);
    final existing = _localDataSource!.getSettingsValue(scopedKey);
    if (existing is String && existing.isNotEmpty) {
      return;
    }

    await _localDataSource!.saveSettingsValue(scopedKey, legacyJson);
    _settings = _settings.clearField(clearRoomPageAssignmentsJson: true);
    await _save();
  }

  String _roomPageLayoutStorageKey(String scopeKey) {
    return '$roomPageLayoutScopePrefix$scopeKey';
  }

  /// Export app settings that should roam with a signed-in account.
  ///
  /// Keep this deliberately narrow for now. Device-local preferences stay
  /// local; the user-facing setting we sync is the All Rooms page layout.
  Map<String, dynamic> buildCloudSettingsBundle({
    String? roomLayoutScopeKey,
    String? roomLayoutHubKey,
  }) {
    final bundle = <String, dynamic>{
      'schema_version': 1,
    };

    final scopedPages = getRoomPageLayout(scopeKey: roomLayoutScopeKey);
    final legacyPages = getLegacyRoomPageLayout();
    final pages = scopedPages ?? legacyPages;
    if (pages != null) {
      bundle['all_rooms_layouts'] = <Map<String, dynamic>>[
        <String, dynamic>{
          if (roomLayoutHubKey != null) 'hub_key': roomLayoutHubKey,
          if (roomLayoutScopeKey != null)
            'source_scope_key': roomLayoutScopeKey,
          'pages': pages,
        },
      ];
    }

    return bundle;
  }

  /// Apply cloud-backed app settings to the current device.
  ///
  /// Restores the saved All Rooms page layout into the caller-provided current
  /// layout scope, so a backup captured on one local home/server ID can still
  /// apply to the equivalent server on this device.
  Future<bool> applyCloudSettingsBundle(
    Map<String, dynamic> bundle, {
    String? roomLayoutScopeKey,
    String? roomLayoutHubKey,
    bool overwrite = true,
  }) async {
    if (!overwrite && getRoomPageLayout(scopeKey: roomLayoutScopeKey) != null) {
      return false;
    }

    final layout = _findAllRoomsLayoutForHub(
      bundle,
      roomLayoutHubKey: roomLayoutHubKey,
    );
    if (layout == null) return false;

    final pages = _decodeRoomPageLayoutFromValue(layout['pages']);
    if (pages == null) return false;

    await saveRoomPageLayout(pages, scopeKey: roomLayoutScopeKey);
    return true;
  }

  Map<dynamic, dynamic>? _findAllRoomsLayoutForHub(
    Map<String, dynamic> bundle, {
    String? roomLayoutHubKey,
  }) {
    final layouts = bundle['all_rooms_layouts'];
    if (layouts is List) {
      for (final layout in layouts) {
        if (layout is! Map) continue;
        if (roomLayoutHubKey == null || layout['hub_key'] == roomLayoutHubKey) {
          return layout;
        }
      }
    }

    // Backward compatibility with early local snapshots before layouts were
    // keyed by hub.
    final legacyLayout = bundle['all_rooms_layout'];
    return legacyLayout is Map ? legacyLayout : null;
  }

  String? _roomPageLayoutJson({String? scopeKey}) {
    if (scopeKey == null) {
      return _settings.roomPageAssignmentsJson;
    }
    final value = _localDataSource!.getSettingsValue(
      _roomPageLayoutStorageKey(scopeKey),
    );
    if (value is String) return value;
    return _findHomeScopedRoomPageLayoutJson(scopeKey);
  }

  String? _findHomeScopedRoomPageLayoutJson(String scopeKey) {
    for (final key in _localDataSource!.getSettingsKeysWithPrefix(
      roomPageLayoutScopePrefix,
    )) {
      if (!key.endsWith(':$scopeKey')) continue;
      final value = _localDataSource!.getSettingsValue(key);
      if (value is String && value.isNotEmpty) {
        return value;
      }
    }
    return null;
  }

  List<List<String>>? _decodeRoomPageLayout(String? json) {
    if (json == null) return null;
    try {
      return _decodeRoomPageLayoutFromValue(jsonDecode(json));
    } catch (e) {
      return null;
    }
  }

  List<List<String>>? _decodeRoomPageLayoutFromValue(Object? value) {
    if (value is! List) return null;
    try {
      return value
          .map((page) => (page as List<dynamic>).cast<String>().toList())
          .toList();
    } catch (e) {
      return null;
    }
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
    _initialized = false;
  }
}
