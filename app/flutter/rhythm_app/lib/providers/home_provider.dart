import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;
import 'package:uuid/uuid.dart';
import '../data/local_data_source.dart';
import '../repositories/home_repository.dart';
import '../services/account_cloud_sync_service.dart';
import '../services/auth_service.dart';
import '../services/hue/hue_service_locator.dart';
import '../services/settings_service.dart';

/// Resolved location with source indicator for debugging.
typedef ResolvedLocation = ({
  double latitude,
  double longitude,
  String timezone,
  String source, // 'home', 'hue', 'ha', 'default'
});

Hub? _preferredServerHub(Iterable<Hub> hubs) {
  final serverHubs = hubs.where((hub) => hub.type == HubType.server).toList();
  if (serverHubs.isEmpty) return null;

  final enabled = serverHubs.where((hub) => hub.enabled).toList();
  final candidates = enabled.isNotEmpty ? enabled : serverHubs;
  candidates.sort((left, right) {
    final leftRecency = left.lastConnected ?? left.updatedAt;
    final rightRecency = right.lastConnected ?? right.updatedAt;
    return rightRecency.compareTo(leftRecency);
  });
  return candidates.first;
}

@visibleForTesting
Hub? preferredServerHubForTesting(Iterable<Hub> hubs) =>
    _preferredServerHub(hubs);

@visibleForTesting
List<Hub> activateServerHubSnapshotForTesting({
  required Iterable<Hub> hubs,
  required Hub selectedHub,
  required DateTime now,
}) {
  var selectedSeen = false;
  final nextHubs = <Hub>[];

  for (final hub in hubs) {
    if (hub.type != HubType.server) {
      nextHubs.add(hub);
      continue;
    }

    if (hub.id == selectedHub.id) {
      selectedSeen = true;
      nextHubs.add(
        selectedHub.copyWith(
          enabled: true,
          lastConnected: now,
          updatedAt: now,
          pendingSync: true,
        ),
      );
      continue;
    }

    nextHubs.add(
      hub.enabled
          ? hub.copyWith(
              enabled: false,
              updatedAt: now,
              pendingSync: true,
            )
          : hub,
    );
  }

  if (!selectedSeen) {
    nextHubs.add(
      selectedHub.copyWith(
        enabled: true,
        lastConnected: now,
        updatedAt: now,
        pendingSync: true,
      ),
    );
  }

  return nextHubs;
}

String _homeScopedServerHubId({
  required String homeId,
  required String cloudHubId,
}) {
  return const Uuid().v5(
    Namespace.url.value,
    'rhythm:home-server-hub:$homeId:$cloudHubId',
  );
}

bool _cloudServerHubMatchesLocalHubForHome({
  required Hub localHub,
  required Hub cloudHub,
  required String homeId,
}) {
  if (_sameNonEmptyServerInstanceId(localHub, cloudHub)) return true;
  if (_differentNonEmptyServerInstanceIds(localHub, cloudHub)) return false;

  return localHub.id == cloudHub.id ||
      localHub.id ==
          _homeScopedServerHubId(
            homeId: homeId,
            cloudHubId: cloudHub.id,
          );
}

@visibleForTesting
Hub accountHomeServerHubForLocalStorageForTesting({
  required Hub cloudHub,
  required String homeId,
  required Iterable<Hub> existingHubs,
}) {
  final normalized = cloudHub.copyWith(homeId: homeId);
  if (cloudHub.type != HubType.server) return normalized;

  final scopedId = _homeScopedServerHubId(
    homeId: homeId,
    cloudHubId: cloudHub.id,
  );

  for (final hub in existingHubs) {
    if (hub.id == scopedId && hub.homeId == homeId) {
      return normalized.copyWith(id: scopedId);
    }
  }

  for (final hub in existingHubs) {
    if (hub.id == cloudHub.id && hub.homeId != homeId) {
      return normalized.copyWith(id: scopedId);
    }
  }

  return normalized;
}

AccountHomeServerHubs? _homeEntryForServerHubPairing({
  required Iterable<AccountHomeServerHubs> homes,
  required String host,
  required int port,
  required String? token,
  required String? serverInstanceId,
  String? hubName,
  DateTime? now,
}) {
  final timestamp = now ?? DateTime.now();
  for (final home in homes) {
    final nextHubs = <Hub>[];
    var matched = false;
    for (final hub in home.serverHubs) {
      if (_serverHubMatchesPairing(
        hub,
        host: host,
        port: port,
        token: token,
        serverInstanceId: serverInstanceId,
      )) {
        matched = true;
        nextHubs.add(
          _serverHubForPairing(
            hub: hub,
            host: host,
            port: port,
            token: token,
            serverInstanceId: serverInstanceId,
            hubName: hubName,
            now: timestamp,
          ),
        );
      } else {
        nextHubs.add(hub);
      }
    }

    if (matched) {
      return AccountHomeServerHubs(
        home: home.home,
        serverHubs: List.unmodifiable(nextHubs),
      );
    }
  }
  return null;
}

bool _serverHubMatchesPairing(
  Hub hub, {
  required String host,
  required int port,
  required String? token,
  required String? serverInstanceId,
}) {
  if (hub.type != HubType.server) return false;

  final cleanServerInstanceId = _cleanOptionalString(serverInstanceId);
  final hubServerInstanceId = _cleanOptionalString(hub.serverInstanceId);
  if (cleanServerInstanceId != null && hubServerInstanceId != null) {
    return cleanServerInstanceId == hubServerInstanceId;
  }

  final cleanToken = _cleanOptionalString(token);
  final hubToken = _cleanOptionalString(hub.token);
  final endpointMatches =
      hub.endpoint.host == host && hub.endpoint.port == port;
  if (endpointMatches) {
    if (cleanToken != null && hubToken != null && cleanToken != hubToken) {
      return false;
    }
    return true;
  }

  return cleanToken != null && hubToken != null && cleanToken == hubToken;
}

Hub _serverHubForPairing({
  required Hub hub,
  required String host,
  required int port,
  required String? token,
  required String? serverInstanceId,
  required String? hubName,
  required DateTime now,
}) {
  final cleanToken = _cleanOptionalString(token);
  final cleanServerInstanceId = _cleanOptionalString(serverInstanceId);
  final cleanHubName = _cleanOptionalString(hubName);
  return hub.copyWith(
    name: hub.name.trim().isNotEmpty ? hub.name : cleanHubName,
    endpoint: HubEndpoint(host: host, port: port),
    token: cleanToken,
    serverInstanceId: cleanServerInstanceId,
    updatedAt: now,
    pendingSync: true,
  );
}

@visibleForTesting
AccountHomeServerHubs? serverHomeEntryForPairingForTesting({
  required Iterable<AccountHomeServerHubs> homes,
  required String host,
  required int port,
  required String? token,
  required String? serverInstanceId,
  String? hubName,
  DateTime? now,
}) =>
    _homeEntryForServerHubPairing(
      homes: homes,
      host: host,
      port: port,
      token: token,
      serverInstanceId: serverInstanceId,
      hubName: hubName,
      now: now,
    );

Home? _selectedHomeFrom(
  Iterable<Home> homes, {
  required String? selectedHomeId,
}) {
  final homeList = homes.toList(growable: false);
  if (homeList.isEmpty) return null;
  if (selectedHomeId == null || selectedHomeId.isEmpty) {
    return homeList.first;
  }
  for (final home in homeList) {
    if (home.id == selectedHomeId) return home;
  }
  return homeList.first;
}

@visibleForTesting
Home? selectedHomeFromForTesting(
  Iterable<Home> homes, {
  required String? selectedHomeId,
}) =>
    _selectedHomeFrom(homes, selectedHomeId: selectedHomeId);

/// Provider for Home and Hub state management.
///
/// Provides reactive state for UI to observe:
/// - Current home and list of homes
/// - Hubs for the current home
///
/// Uses HomeRepository for local data operations.
class HomeProvider extends ChangeNotifier {
  // Dependencies
  late final LocalDataSource _localDataSource;
  late final HomeRepository _repository;

  // State
  List<Home> _homes = [];
  Home? _currentHome;
  List<Hub> _currentHomeHubs = [];
  bool _isLoading = true;
  String? _error;

  // Subscriptions
  StreamSubscription<List<Home>>? _homesSubscription;
  StreamSubscription<List<Hub>>? _hubsSubscription;
  Timer? _accountCloudSyncDebounce;
  final Set<String> _pendingCloudRemoteEndpointClears = <String>{};

  // Initialization tracking
  Completer<void>? _initCompleter;

  // Getters
  List<Home> get homes => _homes;
  Home? get currentHome => _currentHome;
  List<Hub> get currentHomeHubs => _currentHomeHubs;
  List<Hub> get currentHomeServerHubs => getHubsByType(HubType.server);
  Hub? get activeServerHub => _preferredServerHub(currentHomeHubs);
  List<AccountHomeServerHubs> get homeServerHubSnapshots {
    if (_isLoading) return const [];

    return [
      for (final home in _homes)
        AccountHomeServerHubs(
          home: home,
          serverHubs: _repository
              .getHubsForHome(home.id)
              .where((hub) => hub.type == HubType.server)
              .toList(growable: false),
        ),
    ];
  }

  bool get isLoading => _isLoading;
  String? get error => _error;
  HomeRepository get repository => _repository;

  /// Whether the provider has been initialized.
  bool get isInitialized => !_isLoading && _error == null;

  /// Whether there are any homes.
  bool get hasHomes => _homes.isNotEmpty;

  /// Get the current user ID from AuthService.
  String? get currentUserId => AuthService().currentUserId;

  HomeProvider();

  /// Initialize the provider.
  ///
  /// Must be called before using the provider. This initializes:
  /// - Local data source (Hive)
  /// - Home repository
  Future<void> initialize() async {
    if (_initCompleter != null) {
      return _initCompleter!.future;
    }

    _initCompleter = Completer<void>();

    try {
      debugPrint('HomeProvider: Initializing...');

      // Initialize local data source
      _localDataSource = LocalDataSource();
      await _localDataSource.initialize();

      // Initialize repository
      _repository = HomeRepository(
        localDataSource: _localDataSource,
      );

      // Load initial data
      _loadHomes();

      // On web, there's no onboarding flow to create a Home.
      // Ensure a default one exists so hub pairing works.
      if (kIsWeb && _homes.isEmpty) {
        debugPrint(
            'HomeProvider: Web platform with no homes, creating default');
        final home = await _repository.createHome(
          name: 'My Home',
          ownerId: 'web-local',
        );
        _loadHomes();
        _setCurrentHome(home);
      }

      // On web, auto-detect if we're served by a rhythm-server / HA addon.
      // Use Uri.base so the health check goes through ingress when applicable.
      if (kIsWeb &&
          getFirstHubOfType(HubType.server) == null &&
          _currentHome != null) {
        try {
          final base = Uri.base;
          final baseUrl = base.toString();
          final ok = await sdk.RhythmConfigApi(
            baseUrl: baseUrl.endsWith('/') ? baseUrl : '$baseUrl/',
          ).healthCheck();
          if (ok) {
            debugPrint(
                'HomeProvider: Server detected at ${base.host}:${base.port}, auto-pairing');
            await addServerHub(
                name: 'RhythmServer', host: base.host, port: base.port);
          }
        } catch (_) {
          // Not served by a rhythm-server — user can pair manually
        }
      }

      // Start watching for changes
      _startWatching();

      // Demo-mode hooks: auto-seed/cleanup the demo home + server hub so the
      // App Store demo user reaches the same surfaces as a real user without
      // any manual setup.
      HueServiceLocator.onDemoEnabled(_seedDemoEnvironment);
      HueServiceLocator.onDemoDisabled(_clearDemoEnvironment);
      if (HueServiceLocator.isDemoMode) {
        // Already in demo mode (e.g. hot restart) — seed now.
        unawaited(_seedDemoEnvironment());
      }

      _isLoading = false;
      _error = null;
      notifyListeners();

      debugPrint('HomeProvider: Initialized with ${_homes.length} homes');
      _initCompleter!.complete();
    } catch (e, stackTrace) {
      debugPrint('HomeProvider: Initialization failed: $e');
      debugPrint('$stackTrace');
      _error = e.toString();
      _isLoading = false;
      notifyListeners();
      _initCompleter!.completeError(e, stackTrace);
    }
  }

  /// Wait for initialization to complete.
  Future<void> ensureInitialized() async {
    if (_initCompleter == null) {
      await initialize();
    } else {
      await _initCompleter!.future;
    }
  }

  void _loadHomes() {
    _homes = _repository.getAllHomes();

    // Set current home to first home if not set
    if (_currentHome == null && _homes.isNotEmpty) {
      _setCurrentHome(
        _selectedHomeFrom(
          _homes,
          selectedHomeId: SettingsService.instance.selectedHomeId,
        ),
      );
    } else if (_currentHome != null) {
      // Refresh current home from local data
      final refreshedHome = _repository.getHome(_currentHome!.id);
      if (refreshedHome != null) {
        _currentHome = refreshedHome;
        _loadCurrentHomeHubs();
      } else {
        // Current home was deleted
        _setCurrentHome(_homes.isNotEmpty ? _homes.first : null);
      }
    }
  }

  void _loadCurrentHomeHubs() {
    if (_currentHome != null) {
      _currentHomeHubs = _repository.getHubsForHome(_currentHome!.id);
    } else {
      _currentHomeHubs = [];
    }
  }

  void _setCurrentHome(Home? home) {
    _currentHome = home;
    _loadCurrentHomeHubs();
    unawaited(SettingsService.instance.setSelectedHomeId(home?.id));

    // Update hubs subscription
    _hubsSubscription?.cancel();
    if (home != null) {
      _hubsSubscription = _repository.watchHubsForHome(home.id).listen(
        (hubs) {
          _currentHomeHubs = hubs;
          _scheduleAccountCloudSync('hubs_changed');
          notifyListeners();
        },
        onError: (e) => debugPrint('HomeProvider: Hubs watch error: $e'),
      );
    }
  }

  void _startWatching() {
    _homesSubscription?.cancel();
    _homesSubscription = _repository.watchHomes().listen(
      (homes) {
        _homes = homes;
        // Update current home if it changed
        if (_currentHome != null) {
          final updated = homes.firstWhere(
            (h) => h.id == _currentHome!.id,
            orElse: () => homes.isNotEmpty ? homes.first : _currentHome!,
          );
          if (updated.id != _currentHome!.id ||
              !homes.any((h) => h.id == _currentHome!.id)) {
            _setCurrentHome(homes.isNotEmpty ? homes.first : null);
          } else {
            _currentHome = updated;
          }
        } else if (homes.isNotEmpty) {
          _setCurrentHome(homes.first);
        }
        _scheduleAccountCloudSync('homes_changed');
        notifyListeners();
      },
      onError: (e) => debugPrint('HomeProvider: Homes watch error: $e'),
    );
  }

  // ============================================================
  // Home Operations
  // ============================================================

  /// Create a new home.
  Future<Home?> createHome({
    required String name,
    HomeLocation? location,
    SleepSchedule? sleepSchedule,
    CurveConfigDto? curveConfig,
    String? timezone,
  }) async {
    final userId = currentUserId;
    if (userId == null) {
      _error = 'User not signed in';
      notifyListeners();
      return null;
    }

    try {
      final home = await _repository.createHome(
        name: name,
        ownerId: userId,
        location: location,
        sleepSchedule: sleepSchedule,
        curveConfig: curveConfig,
        timezone: timezone,
      );

      _loadHomes();
      _setCurrentHome(home);
      notifyListeners();
      return home;
    } catch (e) {
      _error = 'Failed to create home: $e';
      notifyListeners();
      return null;
    }
  }

  /// Update the current home.
  Future<bool> updateCurrentHome(Home home) async {
    if (_currentHome == null || _currentHome!.id != home.id) {
      return false;
    }

    try {
      final updated = await _repository.updateHome(home);
      _currentHome = updated;
      _loadHomes();
      notifyListeners();
      return true;
    } catch (e) {
      _error = 'Failed to update home: $e';
      notifyListeners();
      return false;
    }
  }

  /// Delete a home.
  Future<bool> deleteHome(String id) async {
    final home = _repository.getHome(id);
    try {
      await _repository.deleteHome(id);
      if (home != null) {
        unawaited(
          AccountCloudSyncService.instance.deleteHome(
            homeId: home.id,
            reason: 'home_deleted',
          ),
        );
      }

      // If deleting current home, switch to another
      if (_currentHome?.id == id) {
        _loadHomes();
        _setCurrentHome(_homes.isNotEmpty ? _homes.first : null);
      } else {
        _loadHomes();
      }

      notifyListeners();
      return true;
    } catch (e) {
      _error = 'Failed to delete home: $e';
      notifyListeners();
      return false;
    }
  }

  /// Switch to a different home.
  void switchHome(String homeId) {
    final home = _repository.getHome(homeId);
    if (home != null) {
      _setCurrentHome(home);
      notifyListeners();
    }
  }

  Future<List<AccountHomeServerHubs>> loadAccountHomes() {
    return AccountCloudSyncService.instance.loadHomesAndServerHubs();
  }

  Future<AccountHomeServerHubs?> _existingHomeForServerHubPairing({
    required String host,
    required int port,
    required String? token,
    required String? serverInstanceId,
    required String hubName,
  }) async =>
      _homeEntryForServerHubPairing(
        homes: homeServerHubSnapshots,
        host: host,
        port: port,
        token: token,
        serverInstanceId: serverInstanceId,
        hubName: hubName,
      );

  /// Refresh the locally saved server endpoints from the signed-in account.
  Future<Hub> refreshServerHubEndpoints(Hub serverHub) async {
    if (serverHub.type != HubType.server) return serverHub;

    try {
      final snapshots = await loadAccountHomes();
      Hub? cloudHub;
      for (final snapshot in snapshots) {
        if (snapshot.home.id != serverHub.homeId) continue;
        for (final hub in snapshot.serverHubs) {
          if (_cloudServerHubMatchesLocalHubForHome(
            localHub: serverHub,
            cloudHub: hub,
            homeId: snapshot.home.id,
          )) {
            cloudHub = hub;
            break;
          }
        }
        if (cloudHub != null) break;
      }

      if (cloudHub == null) {
        return serverHub;
      }

      var updated = serverHub;

      if (!serverHub.pendingSync &&
          cloudHub.updatedAt.isAfter(serverHub.updatedAt) &&
          cloudHub.endpoint != serverHub.endpoint) {
        updated = updated.copyWith(endpoint: cloudHub.endpoint);
      }

      final cloudRemote = cloudHub.remoteEndpoint;
      final cloudCanUpdateRemoteEndpoint = !serverHub.pendingSync ||
          cloudHub.updatedAt.isAfter(serverHub.updatedAt);
      if (cloudCanUpdateRemoteEndpoint &&
          cloudRemote != serverHub.remoteEndpoint) {
        updated = updated.copyWith(
          remoteEndpoint: cloudRemote,
          clearRemoteEndpoint: cloudRemote == null,
        );
      }

      if (updated.endpoint == serverHub.endpoint &&
          updated.remoteEndpoint == serverHub.remoteEndpoint) {
        return serverHub;
      }

      final saved = await updateHub(updated);
      if (!saved) return serverHub;
      debugPrint(
        'HomeProvider: refreshed server endpoints for ${serverHub.id}',
      );
      return updated;
    } catch (error) {
      debugPrint(
        'HomeProvider: unable to refresh server endpoints for '
        '${serverHub.id}: $error',
      );
      return serverHub;
    }
  }

  Future<Hub?> enterHome(AccountHomeServerHubs snapshot) async {
    try {
      await _repository.updateHome(snapshot.home);

      final allExistingHubs = _repository.getAllHubs();
      final existingHubs = _repository.getHubsForHome(snapshot.home.id);
      for (final hub in snapshot.serverHubs) {
        final localHub = accountHomeServerHubForLocalStorageForTesting(
          cloudHub: hub,
          homeId: snapshot.home.id,
          existingHubs: allExistingHubs,
        );
        final merged = mergeCloudServerHubForLocalStorageForTesting(
          cloudHub: localHub,
          existingHubs: existingHubs,
        );
        await _repository.updateHub(merged);
      }

      _loadHomes();
      _setCurrentHome(_repository.getHome(snapshot.home.id));
      notifyListeners();

      final selectedHub = _preferredServerHub(
        _repository.getHubsForHome(snapshot.home.id),
      );
      if (selectedHub == null) return null;
      return activateServerHub(selectedHub);
    } catch (e) {
      _error = 'Failed to enter home: $e';
      notifyListeners();
      return null;
    }
  }

  /// Update location for the current home.
  Future<bool> updateCurrentHomeLocation(HomeLocation location) async {
    if (_currentHome == null) return false;

    try {
      final updated =
          await _repository.updateHomeLocation(_currentHome!.id, location);
      if (updated != null) {
        _currentHome = updated;
        _loadHomes();
        notifyListeners();
        return true;
      }
      return false;
    } catch (e) {
      _error = 'Failed to update location: $e';
      notifyListeners();
      return false;
    }
  }

  /// Resolve best available location with priority: Home > Hue > HA > defaults.
  Future<ResolvedLocation> resolveLocation({
    HueConfig? hueConfig,
    HaWebSocketProvider? haWebSocket,
  }) async {
    // 1. Home location (always preferred)
    final home = currentHome;
    if (home?.location != null) {
      return (
        latitude: home!.location!.latitude,
        longitude: home.location!.longitude,
        timezone: home.timezone ?? 'America/New_York',
        source: 'home',
      );
    }

    // 2. Hue bridge geolocation
    if (hueConfig != null) {
      HueServiceLocator.realInstance.configure(hueConfig);
      final hueGeo = await HueServiceLocator.instance.fetchGeolocation();
      if (hueGeo != null) {
        return (
          latitude: hueGeo.$1,
          longitude: hueGeo.$2,
          timezone: home?.timezone ?? 'America/New_York',
          source: 'hue',
        );
      }
    }

    // 3. HA config
    if (haWebSocket != null && haWebSocket.isConnected) {
      try {
        final haConfig = await haWebSocket.getConfig();
        final lat = (haConfig['latitude'] as num?)?.toDouble();
        final lng = (haConfig['longitude'] as num?)?.toDouble();
        if (lat != null && lng != null) {
          return (
            latitude: lat,
            longitude: lng,
            timezone: haConfig['time_zone'] as String? ?? 'America/New_York',
            source: 'ha',
          );
        }
      } catch (e) {
        debugPrint('HomeProvider.resolveLocation: HA config fetch failed: $e');
      }
    }

    // 4. Defaults
    return (
      latitude: 35.0,
      longitude: -80.0,
      timezone: 'America/New_York',
      source: 'default',
    );
  }

  /// Update sleep schedule for the current home.
  Future<bool> updateCurrentHomeSleepSchedule(SleepSchedule schedule) async {
    if (_currentHome == null) return false;

    try {
      final updated =
          await _repository.updateHomeSleepSchedule(_currentHome!.id, schedule);
      if (updated != null) {
        _currentHome = updated;
        _loadHomes();
        notifyListeners();
        return true;
      }
      return false;
    } catch (e) {
      _error = 'Failed to update sleep schedule: $e';
      notifyListeners();
      return false;
    }
  }

  /// Update curve config for the current home.
  Future<bool> updateCurrentHomeCurveConfig(CurveConfigDto curveConfig) async {
    if (_currentHome == null) return false;

    try {
      final updated = await _repository.updateHomeCurveConfig(
          _currentHome!.id, curveConfig);
      if (updated != null) {
        _currentHome = updated;
        _loadHomes();
        notifyListeners();
        return true;
      }
      return false;
    } catch (e) {
      _error = 'Failed to update curve config: $e';
      notifyListeners();
      return false;
    }
  }

  // ============================================================
  // Hub Operations
  // ============================================================

  /// Add a Home Assistant hub to the current home.
  Future<Hub?> addHomeAssistantHub({
    required String name,
    required String host,
    int port = 8123,
    bool useSsl = false,
    required String token,
  }) async {
    if (_currentHome == null) {
      _error = 'No home selected';
      notifyListeners();
      return null;
    }

    try {
      final hub = await _repository.createHomeAssistantHub(
        homeId: _currentHome!.id,
        name: name,
        host: host,
        port: port,
        useSsl: useSsl,
        token: token,
      );

      _loadCurrentHomeHubs();
      _scheduleAccountCloudSync('server_hub_added');
      notifyListeners();
      return hub;
    } catch (e) {
      _error = 'Failed to add Home Assistant hub: $e';
      notifyListeners();
      return null;
    }
  }

  /// Add a Philips Hue hub to the current home.
  Future<Hub?> addHueHub({
    required String name,
    required String bridgeIp,
    required String appKey,
  }) async {
    if (_currentHome == null) {
      _error = 'No home selected';
      notifyListeners();
      return null;
    }

    try {
      final hub = await _repository.createHueHub(
        homeId: _currentHome!.id,
        name: name,
        bridgeIp: bridgeIp,
        appKey: appKey,
      );

      _loadCurrentHomeHubs();
      notifyListeners();
      return hub;
    } catch (e) {
      _error = 'Failed to add Hue hub: $e';
      notifyListeners();
      return null;
    }
  }

  /// Add a server hub (rhythm-server, HA addon, or Rhythm bridge) to the current home.
  Future<Hub?> addServerHub({
    required String name,
    required String host,
    int port = 54448,
    String? token,
    String? serverInstanceId,
  }) async {
    if (_currentHome == null) {
      _error = 'No home selected';
      notifyListeners();
      return null;
    }

    try {
      final hub = await _repository.createServerHub(
        homeId: _currentHome!.id,
        name: name,
        host: host,
        port: port,
        token: token,
        serverInstanceId: serverInstanceId,
      );

      _loadCurrentHomeHubs();
      notifyListeners();
      return hub;
    } catch (e) {
      _error = 'Failed to add server hub: $e';
      notifyListeners();
      return null;
    }
  }

  /// Add a server hub into a brand-new Home and make that Home current.
  Future<Hub?> addServerHubInNewHome({
    required String homeName,
    required String hubName,
    required String host,
    int port = 54448,
    String? token,
    String? serverInstanceId,
    HomeLocation? location,
    String? timezone,
  }) async {
    final userId = currentUserId;
    if (userId == null) {
      _error = 'User not signed in';
      notifyListeners();
      return null;
    }

    Home? home;
    try {
      final existingHome = await _existingHomeForServerHubPairing(
        host: host,
        port: port,
        token: token,
        serverInstanceId: serverInstanceId,
        hubName: hubName,
      );
      if (existingHome != null) {
        debugPrint(
          'HomeProvider: reusing Home ${existingHome.home.id} for '
          'server hub ${serverInstanceId ?? '$host:$port'}',
        );
        return enterHome(existingHome);
      }

      home = await _repository.createHome(
        name: homeName,
        ownerId: userId,
        location: location,
        timezone: timezone,
      );
      _loadHomes();
      _setCurrentHome(home);

      final hub = await _repository.createServerHub(
        homeId: home.id,
        name: hubName,
        host: host,
        port: port,
        token: token,
        serverInstanceId: serverInstanceId,
      );
      final activatedHub = await activateServerHub(hub);
      final savedHub = activatedHub ?? hub;
      unawaited(
        AccountCloudSyncService.instance.syncHomeAndServerHubs(
          home: home,
          hubs: [savedHub],
          reason: 'server_hub_added',
        ),
      );
      return savedHub;
    } catch (e) {
      if (home != null) {
        try {
          await _repository.deleteHome(home.id);
          _loadHomes();
          _setCurrentHome(_homes.isNotEmpty ? _homes.first : null);
        } catch (cleanupError) {
          debugPrint(
            'HomeProvider: failed to clean up Home ${home.id}: $cleanupError',
          );
        }
      }
      _error = 'Failed to add server hub: $e';
      notifyListeners();
      return null;
    }
  }

  /// Update a hub.
  Future<bool> updateHub(
    Hub hub, {
    bool clearCloudRemoteEndpoint = false,
  }) async {
    if (clearCloudRemoteEndpoint) {
      _pendingCloudRemoteEndpointClears.add(hub.id);
    }
    try {
      await _repository.updateHub(hub);
      _loadCurrentHomeHubs();
      if (hub.type == HubType.server) {
        _scheduleAccountCloudSync('server_hub_updated');
      }
      notifyListeners();
      return true;
    } catch (e) {
      if (clearCloudRemoteEndpoint) {
        _pendingCloudRemoteEndpointClears.remove(hub.id);
      }
      _error = 'Failed to update hub: $e';
      notifyListeners();
      return false;
    }
  }

  /// Delete a hub.
  Future<bool> deleteHub(String hubId) async {
    final hub = _repository.getHub(hubId);
    try {
      await _repository.deleteHub(hubId);
      if (hub != null) {
        unawaited(
          AccountCloudSyncService.instance.deleteHub(
            hubId: hub.id,
            reason: 'hub_deleted',
          ),
        );
      }
      _loadCurrentHomeHubs();
      notifyListeners();
      return true;
    } catch (e) {
      _error = 'Failed to delete hub: $e';
      notifyListeners();
      return false;
    }
  }

  /// Mark a hub as connected.
  Future<void> markHubConnected(String hubId) async {
    final hub = _repository.getHub(hubId);
    await _repository.markHubConnected(hubId);
    _loadCurrentHomeHubs();
    if (hub?.type == HubType.server) {
      _scheduleAccountCloudSync('server_hub_connected');
    }
    notifyListeners();
  }

  /// Make one Rhythm Server hub active without deleting other saved servers.
  ///
  /// Keeping inactive server hub records preserves their owner tokens so
  /// switching back to another box does not strand the app behind a server-side
  /// hashed owner token.
  Future<Hub?> activateServerHub(Hub selectedHub) async {
    if (selectedHub.type != HubType.server) return null;

    try {
      final now = DateTime.now();
      final hubs = _repository.getHubsForHome(selectedHub.homeId);
      final nextHubs = activateServerHubSnapshotForTesting(
        hubs: hubs,
        selectedHub: selectedHub,
        now: now,
      );

      for (final hub in nextHubs) {
        if (hub.type != HubType.server) continue;
        await _repository.updateHub(hub);
      }

      final activatedMatches =
          nextHubs.where((hub) => hub.id == selectedHub.id);
      final activatedHub =
          activatedMatches.isEmpty ? null : activatedMatches.first;
      _loadCurrentHomeHubs();
      _scheduleAccountCloudSync('server_hub_activated');
      notifyListeners();
      return activatedHub;
    } catch (e) {
      _error = 'Failed to switch server hub: $e';
      notifyListeners();
      return null;
    }
  }

  /// Get hubs by type.
  List<Hub> getHubsByType(HubType type) {
    return currentHomeHubs.where((h) => h.type == type).toList();
  }

  /// Get the first hub of a specific type, or null if none exist.
  Hub? getFirstHubOfType(HubType type) {
    if (type == HubType.server) {
      return activeServerHub;
    }
    final hubs = getHubsByType(type);
    return hubs.isNotEmpty ? hubs.first : null;
  }

  // ============================================================
  // Demo seeding
  // ============================================================

  static const _demoHomeOwnerId = 'demo-user';
  static const _demoServerHost = 'demo.rhythm.local';

  /// Ensure the demo user has a Home + RhythmServer hub so they can reach
  /// surfaces like Matter pairing without manual setup.
  Future<void> _seedDemoEnvironment() async {
    if (_currentHome == null) {
      final home = await _repository.createHome(
        name: 'Demo Home',
        ownerId: _demoHomeOwnerId,
      );
      _loadHomes();
      _setCurrentHome(home);
    }
    if (getFirstHubOfType(HubType.server) == null) {
      await addServerHub(
        name: 'Demo Rhythm Server',
        host: _demoServerHost,
        port: 54448,
      );
    }
  }

  /// Remove demo-seeded data on sign-out so it doesn't leak into the next
  /// real session. Identifies demo data by the marker ownerId / hostname.
  Future<void> _clearDemoEnvironment() async {
    final demoHubs = _repository
        .getAllHubs()
        .where((h) => h.endpoint.host == _demoServerHost)
        .toList(growable: false);
    for (final hub in demoHubs) {
      await _repository.deleteHub(hub.id);
    }
    final demoHomes = _repository
        .getAllHomes()
        .where((h) => h.ownerId == _demoHomeOwnerId)
        .toList(growable: false);
    for (final home in demoHomes) {
      await _repository.deleteHome(home.id);
    }
    _loadHomes();
    _loadCurrentHomeHubs();
    notifyListeners();
  }

  // ============================================================
  // Auth & Lifecycle
  // ============================================================

  /// Called when user signs in.
  ///
  /// Loads homes from local storage and creates one if empty.
  Future<void> onUserSignIn() async {
    debugPrint('HomeProvider.onUserSignIn: Starting...');

    await ensureInitialized();

    // Load homes from local storage
    _loadHomes();
    final userId = currentUserId;
    if (userId != null) {
      await _removeHomesNotAccessibleToSignedInUser(userId);
      _loadHomes();
    }
    debugPrint('HomeProvider.onUserSignIn: Loaded ${_homes.length} homes');

    // Restore any cloud home that already has a paired server hub. A fresh
    // user with nothing to restore is intentionally left with no current Home:
    // a Home is now created when they pair a device (see `addServerHubInNewHome`
    // / AddHomeFlow, which also captures the location), so we no longer
    // fabricate an empty "My Home" placeholder here.
    if (_homes.isEmpty && userId != null) {
      await _restoreAccountHomeIfAvailable();
    }

    debugPrint(
        'HomeProvider.onUserSignIn: Complete. currentHome=${_currentHome?.id}');
    _scheduleAccountCloudSync('user_sign_in');
    notifyListeners();
  }

  Future<void> _removeHomesNotAccessibleToSignedInUser(String userId) async {
    final staleHomes = _repository
        .getAllHomes()
        .where(
          (home) => !accountHomeCanSyncForUserForTesting(
            home: home,
            userId: userId,
          ),
        )
        .toList(growable: false);
    if (staleHomes.isEmpty) return;

    for (final home in staleHomes) {
      await _repository.deleteHome(home.id);
    }
    if (staleHomes.any((home) => home.id == _currentHome?.id)) {
      _currentHome = null;
      _currentHomeHubs = [];
    }
    debugPrint(
      'HomeProvider.onUserSignIn: removed ${staleHomes.length} '
      'local Home(s) outside the signed-in account',
    );
  }

  Future<bool> _restoreAccountHomeIfAvailable() async {
    try {
      final snapshots = await loadAccountHomes();
      final restorableSnapshots =
          snapshots.where((entry) => entry.hasServerHub).toList();
      if (restorableSnapshots.isEmpty) return false;

      final snapshot = restorableSnapshots.first;
      await enterHome(snapshot);
      debugPrint(
        'HomeProvider.onUserSignIn: Restored account home '
        '${snapshot.home.id}',
      );
      return true;
    } catch (error) {
      debugPrint(
        'HomeProvider.onUserSignIn: Account home restore skipped: $error',
      );
      return false;
    }
  }

  /// Called when user signs out.
  Future<void> onUserSignOut() async {
    await ensureInitialized();
    _accountCloudSyncDebounce?.cancel();
    await _localDataSource.clearHomesAndHubs();
    await _hubsSubscription?.cancel();
    _hubsSubscription = null;
    _homes = [];
    _currentHome = null;
    _currentHomeHubs = [];
    _error = null;
    notifyListeners();
  }

  /// Clear any error state.
  void clearError() {
    _error = null;
    notifyListeners();
  }

  void _scheduleAccountCloudSync(String reason) {
    _accountCloudSyncDebounce?.cancel();
    _accountCloudSyncDebounce = Timer(const Duration(milliseconds: 500), () {
      final home = _currentHome;
      final hubs = List<Hub>.from(_currentHomeHubs);
      final clearRemoteEndpointHubIds =
          Set<String>.from(_pendingCloudRemoteEndpointClears);
      _pendingCloudRemoteEndpointClears.removeAll(clearRemoteEndpointHubIds);
      unawaited(
        AccountCloudSyncService.instance.syncHomeAndServerHubs(
          home: home,
          hubs: hubs,
          reason: reason,
          clearRemoteEndpointHubIds: clearRemoteEndpointHubIds,
        ),
      );
    });
  }

  @override
  void dispose() {
    _homesSubscription?.cancel();
    _hubsSubscription?.cancel();
    _accountCloudSyncDebounce?.cancel();
    super.dispose();
  }
}

@visibleForTesting
Hub mergeCloudServerHubForLocalStorageForTesting({
  required Hub cloudHub,
  required Iterable<Hub> existingHubs,
}) {
  Hub? existing;
  for (final hub in existingHubs) {
    if (hub.id == cloudHub.id) {
      existing = hub;
      break;
    }
  }
  existing ??= existingHubs.cast<Hub?>().firstWhere(
        (hub) =>
            hub?.type == cloudHub.type &&
            _sameNonEmptyServerInstanceId(hub!, cloudHub),
        orElse: () => null,
      );
  existing ??= existingHubs.cast<Hub?>().firstWhere(
        (hub) =>
            hub?.type == cloudHub.type &&
            !_differentNonEmptyServerInstanceIds(hub!, cloudHub) &&
            hub.endpoint.host == cloudHub.endpoint.host &&
            hub.endpoint.port == cloudHub.endpoint.port,
        orElse: () => null,
      );
  if (existing == null) return cloudHub;

  final existingToken = existing.token?.trim();
  final cloudRemote = cloudHub.remoteEndpoint;
  final keepExistingRemote =
      cloudRemote == null && existing.updatedAt.isAfter(cloudHub.updatedAt);
  return cloudHub.copyWith(
    token: existingToken != null && existingToken.isNotEmpty
        ? existingToken
        : cloudHub.token,
    lastConnected: cloudHub.lastConnected ?? existing.lastConnected,
    remoteEndpoint:
        cloudRemote ?? (keepExistingRemote ? existing.remoteEndpoint : null),
    clearRemoteEndpoint: cloudRemote == null && !keepExistingRemote,
    serverInstanceId: cloudHub.serverInstanceId ?? existing.serverInstanceId,
  );
}

bool _sameNonEmptyServerInstanceId(Hub left, Hub right) {
  final leftId = left.serverInstanceId?.trim();
  final rightId = right.serverInstanceId?.trim();
  return leftId != null &&
      leftId.isNotEmpty &&
      rightId != null &&
      rightId.isNotEmpty &&
      leftId == rightId;
}

bool _differentNonEmptyServerInstanceIds(Hub left, Hub right) {
  final leftId = left.serverInstanceId?.trim();
  final rightId = right.serverInstanceId?.trim();
  return leftId != null &&
      leftId.isNotEmpty &&
      rightId != null &&
      rightId.isNotEmpty &&
      leftId != rightId;
}

String? _cleanOptionalString(String? value) {
  final clean = value?.trim();
  return clean != null && clean.isNotEmpty ? clean : null;
}
