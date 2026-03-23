import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:uuid/uuid.dart';
import '../data/local_data_source.dart';

/// Repository for managing Home and Hub data.
///
/// Pure local CRUD over LocalDataSource (Hive).
class HomeRepository {
  final LocalDataSource _localDataSource;
  final Uuid _uuid;

  HomeRepository({
    required LocalDataSource localDataSource,
    Uuid? uuid,
  })  : _localDataSource = localDataSource,
        _uuid = uuid ?? const Uuid();

  // ============================================================
  // Homes
  // ============================================================

  /// Get all homes from local storage.
  List<Home> getAllHomes() {
    return _localDataSource.getAllHomes();
  }

  /// Get a home by ID from local storage.
  Home? getHome(String id) {
    return _localDataSource.getHome(id);
  }

  /// Watch all homes (reactive stream from local storage).
  Stream<List<Home>> watchHomes() {
    return _localDataSource.watchHomes();
  }

  /// Watch a specific home.
  Stream<Home?> watchHome(String id) {
    return _localDataSource.watchHome(id);
  }

  /// Create a new home.
  Future<Home> createHome({
    required String name,
    required String ownerId,
    HomeLocation? location,
    SleepSchedule? sleepSchedule,
    CurveConfigDto? curveConfig,
    String? timezone,
  }) async {
    final home = Home.create(
      id: _uuid.v4(),
      name: name,
      ownerId: ownerId,
      location: location,
      sleepSchedule: sleepSchedule,
      curveConfig: curveConfig,
      timezone: timezone,
    );

    await _localDataSource.saveHome(home);

    debugPrint('HomeRepository: Created home ${home.id}');
    return home;
  }

  /// Update an existing home.
  Future<Home> updateHome(Home home) async {
    await _localDataSource.saveHome(home);

    debugPrint('HomeRepository: Updated home ${home.id}');
    return home;
  }

  /// Delete a home and all its hubs.
  Future<void> deleteHome(String id) async {
    await _localDataSource.deleteHome(id);

    debugPrint('HomeRepository: Deleted home $id');
  }

  /// Update home location.
  Future<Home?> updateHomeLocation(String homeId, HomeLocation location) async {
    final home = _localDataSource.getHome(homeId);
    if (home == null) return null;

    return updateHome(home.copyWith(location: location));
  }

  /// Update home sleep schedule.
  Future<Home?> updateHomeSleepSchedule(String homeId, SleepSchedule schedule) async {
    final home = _localDataSource.getHome(homeId);
    if (home == null) return null;

    return updateHome(home.copyWith(sleepSchedule: schedule));
  }

  /// Update home curve config.
  Future<Home?> updateHomeCurveConfig(String homeId, CurveConfigDto curveConfig) async {
    final home = _localDataSource.getHome(homeId);
    if (home == null) return null;

    return updateHome(home.withCurveConfig(curveConfig));
  }

  // ============================================================
  // Hubs
  // ============================================================

  /// Get all hubs from local storage.
  List<Hub> getAllHubs() {
    return _localDataSource.getAllHubs();
  }

  /// Get all hubs for a specific home.
  List<Hub> getHubsForHome(String homeId) {
    return _localDataSource.getHubsForHome(homeId);
  }

  /// Get a hub by ID.
  Hub? getHub(String id) {
    return _localDataSource.getHub(id);
  }

  /// Watch all hubs.
  Stream<List<Hub>> watchHubs() {
    return _localDataSource.watchHubs();
  }

  /// Watch hubs for a specific home.
  Stream<List<Hub>> watchHubsForHome(String homeId) {
    return _localDataSource.watchHubsForHome(homeId);
  }

  /// Create a new Home Assistant hub.
  Future<Hub> createHomeAssistantHub({
    required String homeId,
    required String name,
    required String host,
    int port = 8123,
    bool useSsl = false,
    required String token,
  }) async {
    final hub = Hub.homeAssistant(
      id: _uuid.v4(),
      homeId: homeId,
      name: name,
      host: host,
      port: port,
      useSsl: useSsl,
      token: token,
    );

    await _localDataSource.saveHub(hub);

    debugPrint('HomeRepository: Created HA hub ${hub.id}');
    return hub;
  }

  /// Create a new Philips Hue hub.
  Future<Hub> createHueHub({
    required String homeId,
    required String name,
    required String bridgeIp,
    required String appKey,
  }) async {
    final hub = Hub.hue(
      id: _uuid.v4(),
      homeId: homeId,
      name: name,
      bridgeIp: bridgeIp,
      appKey: appKey,
    );

    await _localDataSource.saveHub(hub);

    debugPrint('HomeRepository: Created Hue hub ${hub.id}');
    return hub;
  }

  /// Create a new server hub (rhythm-server, HA addon, or ESP32).
  Future<Hub> createServerHub({
    required String homeId,
    required String name,
    required String host,
    int port = 54448,
  }) async {
    final hub = Hub.server(
      id: _uuid.v4(),
      homeId: homeId,
      name: name,
      host: host,
      port: port,
    );

    await _localDataSource.saveHub(hub);

    debugPrint('HomeRepository: Created server hub ${hub.id}');
    return hub;
  }

  /// Update an existing hub.
  Future<Hub> updateHub(Hub hub) async {
    await _localDataSource.saveHub(hub);

    debugPrint('HomeRepository: Updated hub ${hub.id}');
    return hub;
  }

  /// Delete a hub.
  Future<void> deleteHub(String id) async {
    await _localDataSource.deleteHub(id);

    debugPrint('HomeRepository: Deleted hub $id');
  }

  /// Update hub credentials (token/app key).
  Future<Hub?> updateHubCredentials(String hubId, String token) async {
    final hub = _localDataSource.getHub(hubId);
    if (hub == null) return null;

    return updateHub(hub.copyWith(token: token));
  }

  /// Mark a hub as connected.
  Future<Hub?> markHubConnected(String hubId) async {
    final hub = _localDataSource.getHub(hubId);
    if (hub == null) return null;

    return updateHub(hub.markConnected());
  }

  /// Enable or disable a hub.
  Future<Hub?> setHubEnabled(String hubId, bool enabled) async {
    final hub = _localDataSource.getHub(hubId);
    if (hub == null) return null;

    return updateHub(hub.copyWith(enabled: enabled));
  }

  // ============================================================
  // Convenience Methods
  // ============================================================

  /// Get the default home (first home or create one if none exists).
  Future<Home> getOrCreateDefaultHome(String userId) async {
    final homes = getAllHomes();
    if (homes.isNotEmpty) {
      return homes.first;
    }

    // Create default home
    return createHome(
      name: 'My Home',
      ownerId: userId,
    );
  }

  /// Get all hubs with their parent home info.
  List<({Hub hub, Home? home})> getAllHubsWithHomes() {
    final hubs = getAllHubs();
    return hubs.map((hub) => (hub: hub, home: getHome(hub.homeId))).toList();
  }

  /// Find a hub by endpoint (for detecting duplicates).
  Hub? findHubByEndpoint(String host, int port) {
    final hubs = getAllHubs();
    return hubs.cast<Hub?>().firstWhere(
      (hub) => hub!.endpoint.host == host && hub.endpoint.port == port,
      orElse: () => null,
    );
  }

  /// Check if a hub with the given endpoint already exists.
  bool hubExistsWithEndpoint(String host, int port) {
    return findHubByEndpoint(host, port) != null;
  }
}
