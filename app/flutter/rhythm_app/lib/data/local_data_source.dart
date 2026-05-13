import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:hive_flutter/hive_flutter.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// Local data source for offline-first storage using Hive.
///
/// Handles CRUD operations for Home and Hub models with local persistence.
/// This is the source of truth for UI.
///
/// This is a singleton to ensure consistent access across the app.
class LocalDataSource {
  static final LocalDataSource _instance = LocalDataSource._internal();
  factory LocalDataSource() => _instance;
  LocalDataSource._internal();

  static const String _homesBoxName = 'homes';
  static const String _hubsBoxName = 'hubs';
  static const String _settingsBoxName = 'settings';
  static const String _settingsKey = 'app_settings';

  Box<Home>? _homesBox;
  Box<Hub>? _hubsBox;
  Box<dynamic>? _settingsBox;

  bool _initialized = false;

  /// Whether the data source has been initialized.
  bool get isInitialized => _initialized;

  /// Initialize Hive and open boxes.
  Future<void> initialize() async {
    if (_initialized) return;

    // Initialize Hive for Flutter
    await Hive.initFlutter();

    // Register type adapters
    _registerAdapters();

    // Open boxes
    _homesBox = await Hive.openBox<Home>(_homesBoxName);
    _hubsBox = await Hive.openBox<Hub>(_hubsBoxName);
    _settingsBox = await Hive.openBox<dynamic>(_settingsBoxName);

    _initialized = true;
    debugPrint(
        'LocalDataSource: Initialized with ${_homesBox!.length} homes, ${_hubsBox!.length} hubs');
  }

  void _registerAdapters() {
    // Home-related adapters (type IDs 10-19)
    if (!Hive.isAdapterRegistered(10)) {
      Hive.registerAdapter(HomeLocationAdapter());
    }
    if (!Hive.isAdapterRegistered(11)) {
      Hive.registerAdapter(SleepScheduleAdapter());
    }
    if (!Hive.isAdapterRegistered(12)) {
      Hive.registerAdapter(HomeAdapter());
    }

    // Hub-related adapters (type IDs 20-29)
    if (!Hive.isAdapterRegistered(20)) {
      Hive.registerAdapter(HubTypeAdapter());
    }
    if (!Hive.isAdapterRegistered(21)) {
      Hive.registerAdapter(HubEndpointAdapter());
    }
    if (!Hive.isAdapterRegistered(22)) {
      Hive.registerAdapter(HubAdapter());
    }

    // AppSettings adapter (type ID 40)
    if (!Hive.isAdapterRegistered(40)) {
      Hive.registerAdapter(AppSettingsAdapter());
    }
  }

  // ============================================================
  // Homes CRUD
  // ============================================================

  /// Get all homes.
  List<Home> getAllHomes() {
    _ensureInitialized();
    return _homesBox!.values.toList();
  }

  /// Get a home by ID.
  Home? getHome(String id) {
    _ensureInitialized();
    return _homesBox!.get(id);
  }

  /// Save or update a home.
  Future<void> saveHome(Home home) async {
    _ensureInitialized();
    await _homesBox!.put(home.id, home);
  }

  /// Delete a home.
  Future<void> deleteHome(String id) async {
    _ensureInitialized();
    await _homesBox!.delete(id);
    // Also delete all hubs belonging to this home
    final hubsToDelete =
        _hubsBox!.values.where((h) => h.homeId == id).map((h) => h.id).toList();
    for (final hubId in hubsToDelete) {
      await _hubsBox!.delete(hubId);
    }
  }

  /// Watch all homes (stream of changes).
  Stream<List<Home>> watchHomes() {
    _ensureInitialized();
    return _homesBox!.watch().map((_) => getAllHomes());
  }

  /// Watch a specific home.
  Stream<Home?> watchHome(String id) {
    _ensureInitialized();
    return _homesBox!.watch(key: id).map((_) => getHome(id));
  }

  // ============================================================
  // Hubs CRUD
  // ============================================================

  /// Get all hubs.
  List<Hub> getAllHubs() {
    _ensureInitialized();
    return _hubsBox!.values.toList();
  }

  /// Get all hubs for a specific home.
  List<Hub> getHubsForHome(String homeId) {
    _ensureInitialized();
    return _hubsBox!.values.where((h) => h.homeId == homeId).toList();
  }

  /// Get a hub by ID.
  Hub? getHub(String id) {
    _ensureInitialized();
    return _hubsBox!.get(id);
  }

  /// Save or update a hub.
  Future<void> saveHub(Hub hub) async {
    _ensureInitialized();
    await _hubsBox!.put(hub.id, hub);
  }

  /// Delete a hub.
  Future<void> deleteHub(String id) async {
    _ensureInitialized();
    await _hubsBox!.delete(id);
  }

  /// Watch all hubs.
  Stream<List<Hub>> watchHubs() {
    _ensureInitialized();
    return _hubsBox!.watch().map((_) => getAllHubs());
  }

  /// Watch hubs for a specific home.
  Stream<List<Hub>> watchHubsForHome(String homeId) {
    _ensureInitialized();
    return _hubsBox!.watch().map((_) => getHubsForHome(homeId));
  }

  // ============================================================
  // Settings Operations
  // ============================================================

  /// Get app settings.
  AppSettings getSettings() {
    _ensureInitialized();
    final settings = _settingsBox!.get(_settingsKey);
    if (settings is AppSettings) {
      return settings;
    }
    return AppSettings.defaults();
  }

  /// Save app settings.
  Future<void> saveSettings(AppSettings settings) async {
    _ensureInitialized();
    await _settingsBox!.put(_settingsKey, settings);
  }

  /// Get an arbitrary value from the settings box.
  dynamic getSettingsValue(String key) {
    _ensureInitialized();
    return _settingsBox!.get(key);
  }

  /// Get settings keys matching a prefix.
  Iterable<String> getSettingsKeysWithPrefix(String prefix) {
    _ensureInitialized();
    return _settingsBox!.keys
        .whereType<String>()
        .where((key) => key.startsWith(prefix))
        .toList(growable: false);
  }

  /// Save an arbitrary value into the settings box.
  Future<void> saveSettingsValue(String key, dynamic value) async {
    _ensureInitialized();
    await _settingsBox!.put(key, value);
  }

  /// Delete an arbitrary value from the settings box.
  Future<void> deleteSettingsValue(String key) async {
    _ensureInitialized();
    await _settingsBox!.delete(key);
  }

  /// Check if migration from SharedPreferences has been completed.
  bool isMigrationComplete() {
    _ensureInitialized();
    return _settingsBox!.get('_migrated_from_sp') == true;
  }

  /// Mark migration from SharedPreferences as complete.
  Future<void> markMigrationComplete() async {
    _ensureInitialized();
    await _settingsBox!.put('_migrated_from_sp', true);
  }

  /// Clear settings.
  Future<void> clearSettings() async {
    _ensureInitialized();
    await _settingsBox!.clear();
  }

  // ============================================================
  // Utility Methods
  // ============================================================

  void _ensureInitialized() {
    if (!_initialized) {
      throw StateError(
          'LocalDataSource not initialized. Call initialize() first.');
    }
  }

  /// Clear all local data.
  Future<void> clearAll() async {
    _ensureInitialized();
    await _homesBox!.clear();
    await _hubsBox!.clear();
    await _settingsBox!.clear();
    debugPrint('LocalDataSource: All data cleared');
  }

  /// Close all boxes.
  Future<void> close() async {
    await _homesBox?.close();
    await _hubsBox?.close();
    await _settingsBox?.close();
    _initialized = false;
  }
}
